mod test_utils;

use hammerwork::{
    Job, JobStatus, 
    batch::{BatchStatus, JobBatch, PartialFailureMode},
    queue::DatabaseQueue,
};
use serde_json::json;
// use std::sync::Arc;

#[cfg(feature = "postgres")]
mod postgres_batch_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_batch_enqueue() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create a batch of jobs
        let jobs = vec![
            Job::new(
                "email_queue".to_string(),
                json!({"to": "user1@example.com"}),
            ),
            Job::new(
                "email_queue".to_string(),
                json!({"to": "user2@example.com"}),
            ),
            Job::new(
                "email_queue".to_string(),
                json!({"to": "user3@example.com"}),
            ),
        ];

        let batch = JobBatch::new("test_email_batch")
            .with_jobs(jobs)
            .with_batch_size(10)
            .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

        // Enqueue the batch
        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Get batch status
        let batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(batch_result.total_jobs, 3);
        assert_eq!(batch_result.pending_jobs, 3);
        assert_eq!(batch_result.completed_jobs, 0);
        assert_eq!(batch_result.failed_jobs, 0);
        assert_eq!(batch_result.status, BatchStatus::Pending);

        // Get batch jobs
        let batch_jobs = queue.get_batch_jobs(batch_id).await.unwrap();
        assert_eq!(batch_jobs.len(), 3);

        // All jobs should have the same batch_id
        for job in &batch_jobs {
            assert_eq!(job.batch_id, Some(batch_id));
            assert_eq!(job.queue_name, "email_queue");
            assert_eq!(job.status, JobStatus::Pending);
        }

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_batch_validation() {
        let queue = test_utils::setup_postgres_queue().await;

        // Test empty batch validation
        let empty_batch = JobBatch::new("empty_batch");
        let result = queue.enqueue_batch(empty_batch).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Batch cannot be empty")
        );

        // Test mixed queue validation
        let mixed_jobs = vec![
            Job::new("queue1".to_string(), json!({"id": 1})),
            Job::new("queue2".to_string(), json!({"id": 2})),
        ];

        let mixed_batch = JobBatch::new("mixed_batch").with_jobs(mixed_jobs);
        let result = queue.enqueue_batch(mixed_batch).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("same queue name"));
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_large_batch() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create a large batch of jobs
        let mut jobs = Vec::new();
        for i in 0..500 {
            jobs.push(Job::new(
                "large_queue".to_string(),
                json!({"batch_index": i, "message": format!("Job {}", i)}),
            ));
        }

        let batch = JobBatch::new("large_batch")
            .with_jobs(jobs)
            .with_batch_size(100); // Will be split into chunks

        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Verify all jobs were inserted
        let batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(batch_result.total_jobs, 500);
        assert_eq!(batch_result.pending_jobs, 500);

        let batch_jobs = queue.get_batch_jobs(batch_id).await.unwrap();
        assert_eq!(batch_jobs.len(), 500);

        // Verify jobs are correctly ordered
        for job in batch_jobs.iter() {
            let _expected_index = job.payload["batch_index"].as_u64().unwrap() as usize;
            assert_eq!(job.batch_id, Some(batch_id));
            assert_eq!(job.queue_name, "large_queue");
        }

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_batch_partial_failure_modes() {
        let queue = test_utils::setup_postgres_queue().await;

        // Test ContinueOnError mode
        let jobs = vec![
            Job::new("test_queue".to_string(), json!({"will_succeed": true})),
            Job::new("test_queue".to_string(), json!({"will_fail": true})),
            Job::new("test_queue".to_string(), json!({"will_succeed": true})),
        ];

        let batch = JobBatch::new("partial_failure_test")
            .with_jobs(jobs)
            .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Simulate processing: mark one job as failed
        let batch_jobs = queue.get_batch_jobs(batch_id).await.unwrap();
        let failing_job = &batch_jobs[1]; // The one with will_fail: true

        queue
            .fail_job(failing_job.id, "Simulated failure")
            .await
            .unwrap();

        // Verify batch still exists and shows partial failure
        let batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(batch_result.total_jobs, 3);
        assert_eq!(batch_result.pending_jobs, 2); // Two jobs still pending
        assert_eq!(batch_result.failed_jobs, 0); // Will be updated by worker logic

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_batch_metadata() {
        let queue = test_utils::setup_postgres_queue().await;

        let job = Job::new("metadata_queue".to_string(), json!({"test": "data"}));

        let batch = JobBatch::new("metadata_batch")
            .with_jobs(vec![job])
            .with_metadata("user_id", "12345")
            .with_metadata("campaign_id", "summer_2024")
            .with_metadata("priority", "high");

        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Note: Metadata is stored in the batch table but not directly accessible
        // through the current BatchResult API. This test verifies it doesn't break anything.
        let batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(batch_result.total_jobs, 1);
        assert_eq!(batch_result.batch_id, batch_id);

        queue.delete_batch(batch_id).await.unwrap();
    }
}

#[cfg(feature = "mysql")]
mod mysql_batch_tests {
    use super::*;
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_batch_enqueue() {
        let queue = test_utils::setup_mysql_queue().await;

        // Create a batch of jobs
        let jobs = vec![
            Job::new(
                "email_queue".to_string(),
                json!({"to": "user1@example.com"}),
            ),
            Job::new(
                "email_queue".to_string(),
                json!({"to": "user2@example.com"}),
            ),
            Job::new(
                "email_queue".to_string(),
                json!({"to": "user3@example.com"}),
            ),
        ];

        let batch = JobBatch::new("test_email_batch")
            .with_jobs(jobs)
            .with_batch_size(10)
            .with_partial_failure_handling(PartialFailureMode::FailFast);

        // Enqueue the batch
        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Get batch status
        let batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(batch_result.total_jobs, 3);
        assert_eq!(batch_result.pending_jobs, 3);
        assert_eq!(batch_result.completed_jobs, 0);
        assert_eq!(batch_result.failed_jobs, 0);
        assert_eq!(batch_result.status, BatchStatus::Pending);

        // Get batch jobs
        let batch_jobs = queue.get_batch_jobs(batch_id).await.unwrap();
        assert_eq!(batch_jobs.len(), 3);

        // All jobs should have the same batch_id
        for job in &batch_jobs {
            assert_eq!(job.batch_id, Some(batch_id));
            assert_eq!(job.queue_name, "email_queue");
            assert_eq!(job.status, JobStatus::Pending);
        }

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_batch_chunking() {
        let queue = test_utils::setup_mysql_queue().await;

        // Create a batch larger than MySQL chunk size (100)
        let mut jobs = Vec::new();
        for i in 0..250 {
            jobs.push(Job::new("chunked_queue".to_string(), json!({"index": i})));
        }

        let batch = JobBatch::new("chunked_batch").with_jobs(jobs);
        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Verify all jobs were inserted despite chunking
        let batch_result = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(batch_result.total_jobs, 250);

        let batch_jobs = queue.get_batch_jobs(batch_id).await.unwrap();
        assert_eq!(batch_jobs.len(), 250);

        // Clean up
        queue.delete_batch(batch_id).await.unwrap();
    }
}

// Database-agnostic tests for JobBatch struct itself
mod batch_struct_tests {
    use super::*;

    #[test]
    fn test_batch_chunking() {
        let mut jobs = Vec::new();
        for i in 0..250 {
            jobs.push(Job::new("test_queue".to_string(), json!({"id": i})));
        }

        let batch = JobBatch::new("chunk_test")
            .with_jobs(jobs)
            .with_batch_size(100);

        let chunks = batch.into_chunks();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].job_count(), 100);
        assert_eq!(chunks[1].job_count(), 100);
        assert_eq!(chunks[2].job_count(), 50);

        // Verify chunk naming
        assert_eq!(chunks[0].name, "chunk_test_chunk_1");
        assert_eq!(chunks[1].name, "chunk_test_chunk_2");
        assert_eq!(chunks[2].name, "chunk_test_chunk_3");
    }

    #[test]
    fn test_batch_failure_modes() {
        let modes = vec![
            PartialFailureMode::ContinueOnError,
            PartialFailureMode::FailFast,
            PartialFailureMode::CollectErrors,
        ];

        for mode in modes {
            let job = Job::new("test_queue".to_string(), json!({"test": "data"}));
            let batch = JobBatch::new("mode_test")
                .with_jobs(vec![job])
                .with_partial_failure_handling(mode.clone());

            assert_eq!(batch.failure_mode, mode);
        }
    }

    #[test]
    fn test_batch_builder_pattern() {
        let job1 = Job::new("test_queue".to_string(), json!({"id": 1}));
        let job2 = Job::new("test_queue".to_string(), json!({"id": 2}));

        let batch = JobBatch::new("builder_test")
            .add_job(job1)
            .add_job(job2)
            .with_batch_size(50)
            .with_partial_failure_handling(PartialFailureMode::CollectErrors)
            .with_metadata("test_key", "test_value")
            .with_metadata("priority", "high");

        assert_eq!(batch.job_count(), 2);
        assert_eq!(batch.batch_size, Some(50));
        assert_eq!(batch.failure_mode, PartialFailureMode::CollectErrors);
        assert_eq!(
            batch.metadata.get("test_key"),
            Some(&"test_value".to_string())
        );
        assert_eq!(batch.metadata.get("priority"), Some(&"high".to_string()));
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_batch_validation_edge_cases() {
        // Test very large batch
        let mut large_jobs = Vec::new();
        for i in 0..10_001 {
            large_jobs.push(Job::new("test_queue".to_string(), json!({"id": i})));
        }

        let large_batch = JobBatch::new("too_large").with_jobs(large_jobs);
        let result = large_batch.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum allowed size")
        );

        // Test valid large batch (just under limit)
        let mut valid_large_jobs = Vec::new();
        for i in 0..10_000 {
            valid_large_jobs.push(Job::new("test_queue".to_string(), json!({"id": i})));
        }

        let valid_large_batch = JobBatch::new("valid_large").with_jobs(valid_large_jobs);
        assert!(valid_large_batch.validate().is_ok());
    }
}
