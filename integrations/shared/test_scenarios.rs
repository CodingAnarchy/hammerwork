use hammerwork::queue::DatabaseQueue;
use hammerwork::stats::{InMemoryStatsCollector, StatisticsCollector};
use hammerwork::{HammerworkError, Job, JobQueue, JobStatus, Result, Worker, WorkerPool};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{error, info};

/// Shared test scenarios for both PostgreSQL and MySQL integration tests
#[derive(Clone)]
pub struct TestScenarios<DB>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    pub queue: Arc<JobQueue<DB>>,
}

impl<DB> TestScenarios<DB>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    pub fn new(queue: Arc<JobQueue<DB>>) -> Self {
        Self { queue }
    }

    /// Test basic job lifecycle: create, enqueue, dequeue, complete
    pub async fn test_basic_job_lifecycle(&self) -> Result<()> {
        info!("🧪 Testing basic job lifecycle");

        // Create a test job
        let job = Job::new(
            "test_basic".to_string(),
            json!({
                "message": "Basic lifecycle test",
                "timestamp": chrono::Utc::now().timestamp()
            }),
        );
        let job_id = job.id;

        // Enqueue the job
        self.queue.enqueue(job).await?;
        info!("✅ Job enqueued: {}", job_id);

        // Dequeue the job
        let dequeued_job = self.queue.dequeue("test_basic").await?;
        assert!(dequeued_job.is_some(), "Job should be dequeued");
        let dequeued_job = dequeued_job.unwrap();
        assert_eq!(dequeued_job.id, job_id);
        info!("✅ Job dequeued: {}", job_id);

        // Complete the job
        self.queue.complete_job(job_id).await?;
        info!("✅ Job completed: {}", job_id);

        // Verify job status
        let completed_job = self.queue.get_job(job_id).await?;
        assert!(completed_job.is_some());
        let completed_job = completed_job.unwrap();
        assert_eq!(completed_job.status, hammerwork::JobStatus::Completed);
        info!("✅ Job status verified as completed");

        // Cleanup
        self.queue.delete_job(job_id).await?;
        info!("✅ Basic job lifecycle test completed");

        Ok(())
    }

    /// Test delayed job scheduling
    pub async fn test_delayed_jobs(&self) -> Result<()> {
        info!("🧪 Testing delayed job scheduling");

        let delay = chrono::Duration::seconds(2);
        let job = Job::with_delay(
            "test_delayed".to_string(),
            json!({
                "message": "Delayed job test",
                "delay_seconds": 2
            }),
            delay,
        );
        let job_id = job.id;

        // Enqueue the delayed job
        self.queue.enqueue(job).await?;
        info!("✅ Delayed job enqueued: {}", job_id);

        // Try to dequeue immediately - should get nothing
        let immediate_result = self.queue.dequeue("test_delayed").await?;
        assert!(
            immediate_result.is_none(),
            "Delayed job should not be available immediately"
        );
        info!("✅ Delayed job correctly not available immediately");

        // Wait for the delay period
        sleep(Duration::from_secs(3)).await;

        // Now try to dequeue - should get the job
        let delayed_result = self.queue.dequeue("test_delayed").await?;
        assert!(
            delayed_result.is_some(),
            "Delayed job should be available after delay"
        );
        let delayed_job = delayed_result.unwrap();
        assert_eq!(delayed_job.id, job_id);
        info!("✅ Delayed job available after delay period");

        // Cleanup
        self.queue.complete_job(job_id).await?;
        self.queue.delete_job(job_id).await?;
        info!("✅ Delayed job test completed");

        Ok(())
    }

    /// Test job retry mechanism
    pub async fn test_job_retries(&self) -> Result<()> {
        info!("🧪 Testing job retry mechanism");

        let job = Job::new(
            "test_retry".to_string(),
            json!({
                "message": "Retry test job",
                "should_fail": true
            }),
        )
        .with_max_attempts(3);
        let job_id = job.id;

        // Enqueue the job
        self.queue.enqueue(job).await?;
        info!("✅ Retry test job enqueued: {}", job_id);

        // Dequeue and fail the job
        let dequeued_job = self.queue.dequeue("test_retry").await?;
        assert!(dequeued_job.is_some());
        let dequeued_job = dequeued_job.unwrap();
        assert_eq!(dequeued_job.attempts, 1);

        // Fail the job (simulate failure)
        self.queue
            .fail_job(job_id, "Simulated failure for retry test")
            .await?;
        info!("✅ Job failed with error message");

        // Verify job status
        let failed_job = self.queue.get_job(job_id).await?;
        assert!(failed_job.is_some());
        let failed_job = failed_job.unwrap();
        assert_eq!(failed_job.status, hammerwork::JobStatus::Failed);
        assert!(failed_job.error_message.is_some());
        info!("✅ Job status verified as failed with error message");

        // Cleanup
        self.queue.delete_job(job_id).await?;
        info!("✅ Job retry test completed");

        Ok(())
    }

    /// Test worker pool functionality
    pub async fn test_worker_pool(&self) -> Result<()> {
        info!("🧪 Testing worker pool functionality");

        // Create a simple job handler
        let job_counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = job_counter.clone();

        let handler = Arc::new(move |job: Job| {
            let counter = counter_clone.clone();
            Box::pin(async move {
                info!("Processing job: {} with payload: {}", job.id, job.payload);
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                // Simulate some work
                sleep(Duration::from_millis(100)).await;

                Ok(()) as Result<()>
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        });

        // Create workers
        let worker1 = Worker::new(
            self.queue.clone(),
            "worker_test".to_string(),
            handler.clone(),
        )
        .with_poll_interval(Duration::from_millis(100))
        .with_max_retries(2);

        let worker2 = Worker::new(
            self.queue.clone(),
            "worker_test".to_string(),
            handler.clone(),
        )
        .with_poll_interval(Duration::from_millis(100))
        .with_max_retries(2);

        // Enqueue test jobs
        for i in 0..5 {
            let job = Job::new(
                "worker_test".to_string(),
                json!({
                    "job_number": i,
                    "message": format!("Worker test job {}", i)
                }),
            );
            self.queue.enqueue(job).await?;
        }
        info!("✅ Enqueued 5 test jobs for worker pool");

        // Start worker pool with timeout
        let mut pool = WorkerPool::new();
        pool.add_worker(worker1);
        pool.add_worker(worker2);

        // Run workers for a limited time
        let pool_task = tokio::spawn(async move { pool.start().await });

        // Wait for jobs to be processed
        sleep(Duration::from_secs(2)).await;

        // Check that jobs were processed
        let processed_count = job_counter.load(std::sync::atomic::Ordering::SeqCst);
        info!("✅ Worker pool processed {} jobs", processed_count);
        assert!(
            processed_count > 0,
            "Worker pool should have processed at least one job"
        );

        // Stop the worker pool
        pool_task.abort();
        info!("✅ Worker pool test completed");

        Ok(())
    }

    /// Test concurrent job processing
    pub async fn test_concurrent_processing(&self) -> Result<()> {
        info!("🧪 Testing concurrent job processing");

        // Enqueue multiple jobs
        let mut job_ids = Vec::new();
        for i in 0..10 {
            let job = Job::new(
                "concurrent_test".to_string(),
                json!({
                    "index": i,
                    "message": format!("Concurrent job {}", i)
                }),
            );
            job_ids.push(job.id);
            self.queue.enqueue(job).await?;
        }
        info!("✅ Enqueued 10 jobs for concurrent processing test");

        // Process jobs concurrently
        let mut handles = Vec::new();
        for _ in 0..3 {
            let queue_clone = self.queue.clone();
            let handle = tokio::spawn(async move {
                if let Ok(Some(job)) = queue_clone.dequeue("concurrent_test").await {
                    // Simulate processing
                    sleep(Duration::from_millis(50)).await;
                    let _ = queue_clone.complete_job(job.id).await;
                    return Some(job.id);
                }
                None
            });
            handles.push(handle);
        }

        // Wait for concurrent processing
        let mut processed_jobs = 0;
        for handle in handles {
            if let Ok(Some(_job_id)) = handle.await {
                processed_jobs += 1;
            }
        }

        info!("✅ Concurrently processed {} jobs", processed_jobs);
        assert!(
            processed_jobs > 0,
            "Should have processed at least one job concurrently"
        );

        // Cleanup remaining jobs
        for job_id in job_ids {
            let _ = self.queue.delete_job(job_id).await;
        }
        info!("✅ Concurrent processing test completed");

        Ok(())
    }

    /// Test error handling and edge cases
    pub async fn test_error_handling(&self) -> Result<()> {
        info!("🧪 Testing error handling and edge cases");

        // Test dequeue from empty queue
        let empty_result = self.queue.dequeue("nonexistent_queue").await?;
        assert!(empty_result.is_none(), "Empty queue should return None");
        info!("✅ Empty queue correctly returns None");

        // Test get non-existent job
        let fake_id = uuid::Uuid::new_v4();
        let nonexistent_job = self.queue.get_job(fake_id).await?;
        assert!(
            nonexistent_job.is_none(),
            "Non-existent job should return None"
        );
        info!("✅ Non-existent job correctly returns None");

        // Test delete non-existent job (should not error)
        let delete_result = self.queue.delete_job(fake_id).await;
        assert!(
            delete_result.is_ok(),
            "Deleting non-existent job should not error"
        );
        info!("✅ Deleting non-existent job handled gracefully");

        info!("✅ Error handling test completed");

        Ok(())
    }

    /// Test dead job management functionality
    pub async fn test_dead_job_management(&self) -> Result<()> {
        info!("🧪 Testing dead job management");

        // Create a job that will fail multiple times
        let job = Job::new(
            "dead_job_test".to_string(),
            json!({
                "message": "This job will become dead",
                "should_fail": true
            }),
        )
        .with_max_attempts(2);
        let job_id = job.id;

        // Enqueue the job
        self.queue.enqueue(job).await?;
        info!("✅ Dead job test job enqueued: {}", job_id);

        // Dequeue and fail the job multiple times to exhaust retries
        for attempt in 1..=2 {
            let dequeued_job = self.queue.dequeue("dead_job_test").await?;
            assert!(dequeued_job.is_some());
            let dequeued_job = dequeued_job.unwrap();
            assert_eq!(dequeued_job.attempts, attempt);

            if attempt == 2 {
                // On final attempt, mark as dead
                self.queue
                    .mark_job_dead(job_id, "Exhausted all retries")
                    .await?;
                info!("✅ Job marked as dead after {} attempts", attempt);
            } else {
                // Fail the job for retry
                self.queue
                    .fail_job(job_id, &format!("Simulated failure attempt {}", attempt))
                    .await?;

                // Retry the job
                let retry_at = chrono::Utc::now() + chrono::Duration::seconds(1);
                self.queue.retry_job(job_id, retry_at).await?;
                info!("✅ Job failed and scheduled for retry, attempt {}", attempt);

                // Wait for retry
                sleep(Duration::from_secs(2)).await;
            }
        }

        // Verify job is dead
        let dead_job = self.queue.get_job(job_id).await?;
        assert!(dead_job.is_some());
        let dead_job = dead_job.unwrap();
        assert_eq!(dead_job.status, JobStatus::Dead);
        assert!(dead_job.failed_at.is_some());
        info!("✅ Job status verified as dead");

        // Test dead job retrieval
        let dead_jobs = self.queue.get_dead_jobs(Some(10), None).await?;
        assert!(!dead_jobs.is_empty());
        let found_dead_job = dead_jobs.iter().find(|j| j.id == job_id);
        assert!(found_dead_job.is_some());
        info!("✅ Dead job found in dead jobs list");

        // Test dead job summary
        let dead_summary = self.queue.get_dead_job_summary().await?;
        assert!(dead_summary.total_dead_jobs > 0);
        assert!(
            dead_summary
                .dead_jobs_by_queue
                .contains_key("dead_job_test")
        );
        info!("✅ Dead job summary contains expected data");

        // Test retry dead job
        self.queue.retry_dead_job(job_id).await?;
        let retried_job = self.queue.get_job(job_id).await?;
        assert!(retried_job.is_some());
        let retried_job = retried_job.unwrap();
        assert_eq!(retried_job.status, JobStatus::Pending);
        assert_eq!(retried_job.attempts, 0);
        assert!(retried_job.failed_at.is_none());
        info!("✅ Dead job successfully retried");

        // Cleanup
        self.queue.delete_job(job_id).await?;
        info!("✅ Dead job management test completed");

        Ok(())
    }

    /// Test statistics collection functionality
    pub async fn test_statistics_collection(&self) -> Result<()> {
        info!("🧪 Testing statistics collection");

        // Create statistics collector
        let stats_collector =
            Arc::new(InMemoryStatsCollector::new_default()) as Arc<dyn StatisticsCollector>;

        // Create a job handler that records statistics
        let stats_clone = Arc::clone(&stats_collector);
        let handler = Arc::new(move |job: Job| {
            let stats = stats_clone.clone();
            Box::pin(async move {
                info!("Processing statistics test job: {}", job.id);

                // Simulate some work
                sleep(Duration::from_millis(100)).await;

                // Fail some jobs for testing
                if job
                    .payload
                    .get("should_fail")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return Err(HammerworkError::Worker {
                        message: "Simulated failure for statistics test".to_string(),
                    });
                }

                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        });

        // Create worker with stats collector
        let worker = Worker::new(self.queue.clone(), "stats_test".to_string(), handler)
            .with_poll_interval(Duration::from_millis(100))
            .with_max_retries(2)
            .with_stats_collector(Arc::clone(&stats_collector));

        // Enqueue test jobs (mix of successful and failing)
        let mut job_ids = Vec::new();
        for i in 0..5 {
            let job = Job::new(
                "stats_test".to_string(),
                json!({
                    "index": i,
                    "should_fail": i % 3 == 0  // Fail every 3rd job
                }),
            );
            job_ids.push(job.id);
            self.queue.enqueue(job).await?;
        }
        info!("✅ Enqueued 5 jobs for statistics test");

        // Start worker pool with timeout
        let mut pool = WorkerPool::new().with_stats_collector(Arc::clone(&stats_collector));
        pool.add_worker(worker);

        let pool_task = tokio::spawn(async move { pool.start().await });

        // Wait for jobs to be processed
        sleep(Duration::from_secs(3)).await;

        // Stop the worker pool
        pool_task.abort();

        // Check statistics
        let system_stats = stats_collector
            .get_system_statistics(Duration::from_secs(300))
            .await?;
        info!(
            "System stats - Total: {}, Completed: {}, Failed: {}",
            system_stats.total_processed, system_stats.completed, system_stats.failed
        );

        assert!(
            system_stats.total_processed > 0,
            "Should have processed some jobs"
        );

        let queue_stats = stats_collector
            .get_queue_statistics("stats_test", Duration::from_secs(300))
            .await?;
        assert!(
            queue_stats.total_processed > 0,
            "Queue should have processed some jobs"
        );
        info!("✅ Queue-specific statistics collected successfully");

        // Test all queue statistics
        let all_stats = stats_collector
            .get_all_statistics(Duration::from_secs(300))
            .await?;
        assert!(
            !all_stats.is_empty(),
            "Should have statistics for at least one queue"
        );
        let stats_test_queue = all_stats.iter().find(|s| s.queue_name == "stats_test");
        assert!(
            stats_test_queue.is_some(),
            "Should have statistics for stats_test queue"
        );
        info!("✅ All queue statistics collected successfully");

        // Cleanup
        for job_id in job_ids {
            let _ = self.queue.delete_job(job_id).await;
        }
        info!("✅ Statistics collection test completed");

        Ok(())
    }

    /// Test database queue statistics functionality  
    pub async fn test_database_queue_stats(&self) -> Result<()> {
        info!("🧪 Testing database queue statistics");

        // Create test jobs in multiple queues
        let mut job_ids = Vec::new();

        // Queue 1: email_queue
        for i in 0..3 {
            let job = Job::new("email_queue".to_string(), json!({ "email_id": i }));
            job_ids.push(job.id);
            self.queue.enqueue(job).await?;
        }

        // Queue 2: notification_queue
        for i in 0..2 {
            let job = Job::new(
                "notification_queue".to_string(),
                json!({ "notification_id": i }),
            );
            job_ids.push(job.id);
            self.queue.enqueue(job).await?;
        }

        info!("✅ Enqueued jobs in multiple queues");

        // Complete some jobs to generate statistics
        if let Some(job) = self.queue.dequeue("email_queue").await? {
            self.queue.complete_job(job.id).await?;
        }
        if let Some(job) = self.queue.dequeue("notification_queue").await? {
            self.queue.complete_job(job.id).await?;
        }

        // Test queue-specific statistics
        let email_stats = self.queue.get_queue_stats("email_queue").await?;
        assert_eq!(email_stats.queue_name, "email_queue");
        assert!(email_stats.pending_count > 0 || email_stats.completed_count > 0);
        info!(
            "✅ Email queue statistics: pending={}, completed={}",
            email_stats.pending_count, email_stats.completed_count
        );

        // Test all queue statistics
        let all_queue_stats = self.queue.get_all_queue_stats().await?;
        assert!(
            all_queue_stats.len() >= 2,
            "Should have stats for at least 2 queues"
        );

        let email_found = all_queue_stats
            .iter()
            .any(|s| s.queue_name == "email_queue");
        let notification_found = all_queue_stats
            .iter()
            .any(|s| s.queue_name == "notification_queue");
        assert!(
            email_found && notification_found,
            "Should have stats for both queues"
        );
        info!("✅ All queue statistics retrieved successfully");

        // Test job counts by status
        let status_counts = self.queue.get_job_counts_by_status("email_queue").await?;
        assert!(!status_counts.is_empty(), "Should have status counts");
        info!("✅ Job status counts: {:?}", status_counts);

        // Cleanup
        for job_id in job_ids {
            let _ = self.queue.delete_job(job_id).await;
        }
        info!("✅ Database queue statistics test completed");

        Ok(())
    }

    /// Run all test scenarios
    pub async fn run_all_tests(&self) -> Result<()> {
        info!("🚀 Starting comprehensive integration tests");

        let mut passed = 0;
        let mut failed = 0;

        // Test basic job lifecycle
        info!("🧪 Running test: Basic Job Lifecycle");
        match timeout(Duration::from_secs(30), self.test_basic_job_lifecycle()).await {
            Ok(Ok(())) => {
                info!("✅ Basic Job Lifecycle - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Basic Job Lifecycle - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Basic Job Lifecycle - TIMEOUT");
                failed += 1;
            }
        }

        // Test delayed jobs
        info!("🧪 Running test: Delayed Jobs");
        match timeout(Duration::from_secs(30), self.test_delayed_jobs()).await {
            Ok(Ok(())) => {
                info!("✅ Delayed Jobs - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Delayed Jobs - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Delayed Jobs - TIMEOUT");
                failed += 1;
            }
        }

        // Test job retries
        info!("🧪 Running test: Job Retries");
        match timeout(Duration::from_secs(30), self.test_job_retries()).await {
            Ok(Ok(())) => {
                info!("✅ Job Retries - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Job Retries - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Job Retries - TIMEOUT");
                failed += 1;
            }
        }

        // Test worker pool
        info!("🧪 Running test: Worker Pool");
        match timeout(Duration::from_secs(30), self.test_worker_pool()).await {
            Ok(Ok(())) => {
                info!("✅ Worker Pool - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Worker Pool - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Worker Pool - TIMEOUT");
                failed += 1;
            }
        }

        // Test concurrent processing
        info!("🧪 Running test: Concurrent Processing");
        match timeout(Duration::from_secs(30), self.test_concurrent_processing()).await {
            Ok(Ok(())) => {
                info!("✅ Concurrent Processing - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Concurrent Processing - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Concurrent Processing - TIMEOUT");
                failed += 1;
            }
        }

        // Test error handling
        info!("🧪 Running test: Error Handling");
        match timeout(Duration::from_secs(30), self.test_error_handling()).await {
            Ok(Ok(())) => {
                info!("✅ Error Handling - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Error Handling - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Error Handling - TIMEOUT");
                failed += 1;
            }
        }

        // Test dead job management
        info!("🧪 Running test: Dead Job Management");
        match timeout(Duration::from_secs(45), self.test_dead_job_management()).await {
            Ok(Ok(())) => {
                info!("✅ Dead Job Management - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Dead Job Management - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Dead Job Management - TIMEOUT");
                failed += 1;
            }
        }

        // Test statistics collection
        info!("🧪 Running test: Statistics Collection");
        match timeout(Duration::from_secs(45), self.test_statistics_collection()).await {
            Ok(Ok(())) => {
                info!("✅ Statistics Collection - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Statistics Collection - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Statistics Collection - TIMEOUT");
                failed += 1;
            }
        }

        // Test database queue statistics
        info!("🧪 Running test: Database Queue Statistics");
        match timeout(Duration::from_secs(30), self.test_database_queue_stats()).await {
            Ok(Ok(())) => {
                info!("✅ Database Queue Statistics - PASSED");
                passed += 1;
            }
            Ok(Err(e)) => {
                error!("❌ Database Queue Statistics - FAILED: {}", e);
                failed += 1;
            }
            Err(_) => {
                error!("❌ Database Queue Statistics - TIMEOUT");
                failed += 1;
            }
        }

        info!(
            "🏁 Integration tests completed: {} passed, {} failed",
            passed, failed
        );

        if failed > 0 {
            return Err(HammerworkError::Worker {
                message: format!("{} tests failed", failed),
            });
        }

        Ok(())
    }
}
