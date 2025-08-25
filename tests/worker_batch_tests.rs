mod test_utils;

use hammerwork::{
    BatchProcessingStats, Job, JobBatch, PartialFailureMode, Worker, WorkerPool,
    queue::test::TestQueue,
};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

#[cfg(feature = "postgres")]
mod postgres_worker_batch_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_worker_batch_processing_enabled() {
        let queue = test_utils::setup_postgres_queue().await;
        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Create job handler that simulates different processing times
        let handler: JobHandler = Arc::new(|job: Job| {
            Box::pin(async move {
                let delay_ms = job
                    .payload
                    .get("delay_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10);

                tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                // Simulate occasional failures
                if job
                    .payload
                    .get("should_fail")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return Err(hammerwork::HammerworkError::Processing(
                        "Simulated failure".to_string(),
                    ));
                }

                Ok(())
            })
        });

        // Create worker with batch processing enabled
        let worker = Worker::new(queue.clone(), "batch_test_queue".to_string(), handler)
            .with_batch_processing_enabled(true)
            .with_stats_collector(stats_collector.clone())
            .with_poll_interval(Duration::from_millis(50));

        // Verify initial batch stats
        let initial_stats = worker.get_batch_stats();
        assert_eq!(initial_stats.jobs_processed, 0);
        assert_eq!(initial_stats.jobs_completed, 0);
        assert_eq!(initial_stats.batches_completed, 0);

        // Create a batch of jobs
        let batch_jobs = vec![
            Job::new(
                "batch_test_queue".to_string(),
                json!({"task": "process_1", "delay_ms": 20}),
            )
            .as_high_priority(),
            Job::new(
                "batch_test_queue".to_string(),
                json!({"task": "process_2", "delay_ms": 15}),
            ),
            Job::new(
                "batch_test_queue".to_string(),
                json!({"task": "process_3", "delay_ms": 10, "should_fail": true}),
            ),
            Job::new(
                "batch_test_queue".to_string(),
                json!({"task": "process_4", "delay_ms": 25}),
            )
            .as_critical(),
        ];

        let batch = JobBatch::new("worker_test_batch")
            .with_jobs(batch_jobs)
            .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

        // Enqueue the batch
        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Start worker pool
        let mut worker_pool = WorkerPool::new();
        worker_pool.add_worker(worker);

        // Run worker for a short time to process the batch
        let worker_task = tokio::spawn(async move {
            let _ = timeout(Duration::from_secs(5), worker_pool.start()).await;
        });

        // Wait for jobs to be processed
        let mut attempts = 0;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;

            let batch_result = queue.get_batch_status(batch_id).await.unwrap();
            if batch_result.pending_jobs == 0 || attempts > 50 {
                break;
            }
            attempts += 1;
        }

        // Stop the worker
        worker_task.abort();

        // Verify batch was processed
        let final_batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(final_batch_result.total_jobs, 4);
        assert_eq!(final_batch_result.pending_jobs, 0);
        assert_eq!(final_batch_result.completed_jobs, 3); // One job should fail
        assert_eq!(final_batch_result.failed_jobs, 1);

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_batch_statistics_tracking() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create simple handler
        let handler: JobHandler = Arc::new(|_job: Job| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        });

        // Create worker with batch processing enabled
        let worker = Worker::new(queue.clone(), "stats_test_queue".to_string(), handler)
            .with_batch_processing_enabled(true)
            .with_poll_interval(Duration::from_millis(50));

        // Create a small batch
        let batch_jobs = vec![
            Job::new("stats_test_queue".to_string(), json!({"id": 1})),
            Job::new("stats_test_queue".to_string(), json!({"id": 2})),
        ];

        let batch = JobBatch::new("stats_test_batch")
            .with_jobs(batch_jobs)
            .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Start worker pool
        let mut worker_pool = WorkerPool::new();
        worker_pool.add_worker(worker);

        let worker_task = tokio::spawn(async move {
            let _ = timeout(Duration::from_secs(3), worker_pool.start()).await;
        });

        // Wait for processing
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Stop worker
        worker_task.abort();

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }

    #[test]
    fn test_batch_processing_stats_struct() {
        let mut stats = BatchProcessingStats::default();

        // Test initial state
        assert_eq!(stats.jobs_processed, 0);
        assert_eq!(stats.success_rate(), 0.0);
        assert_eq!(stats.batch_success_rate(), 0.0);

        // Test statistics updates
        stats.jobs_processed = 10;
        stats.jobs_completed = 8;
        stats.jobs_failed = 2;
        stats.total_processing_time_ms = 1000;

        stats.update_average_processing_time();

        assert_eq!(stats.success_rate(), 0.8);
        assert_eq!(stats.average_processing_time_ms, 125.0); // 1000ms / 8 jobs

        // Test batch statistics
        stats.batches_completed = 5;
        stats.batches_successful = 4;

        assert_eq!(stats.batch_success_rate(), 0.8);
    }
}

#[test]
fn test_worker_batch_processing_config() {
    // Test that batch processing configuration works
    // This is a compile-time test since we can't create actual workers without database

    use hammerwork::worker::BatchProcessingStats;

    // Test that BatchProcessingStats can be created and manipulated
    let stats = BatchProcessingStats {
        jobs_processed: 100,
        jobs_completed: 95,
        jobs_failed: 5,
        batches_completed: 10,
        batches_successful: 9,
        total_processing_time_ms: 5000,
        average_processing_time_ms: 52.6,
        last_processed_job: Some(chrono::Utc::now()),
    };

    assert_eq!(stats.success_rate(), 0.95);
    assert_eq!(stats.batch_success_rate(), 0.9);

    // Test default
    let default_stats = BatchProcessingStats::default();
    assert_eq!(default_stats.jobs_processed, 0);
    assert!(default_stats.last_processed_job.is_none());
}
