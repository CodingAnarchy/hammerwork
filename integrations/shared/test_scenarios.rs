use hammerwork::queue::DatabaseQueue;
use hammerwork::{HammerworkError, Job, JobQueue, Result, Worker, WorkerPool};
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
