//! Comprehensive tests for job result storage and retrieval functionality.

mod test_utils;

use hammerwork::{
    job::{ResultConfig, ResultStorage},
    worker::JobResult,
};
use serde_json::json;

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    async fn test_job_result_storage_and_retrieval() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create a job with result storage enabled
        let job = Job::new("test_queue".to_string(), json!({"task": "process_data"}))
            .with_result_storage(ResultStorage::Database)
            .with_result_ttl(Duration::from_secs(3600)); // 1 hour TTL

        let job_id = queue.enqueue(job).await.unwrap();

        // Store some result data
        let result_data = json!({
            "processed_items": 42,
            "total_time_ms": 1500,
            "status": "completed",
            "output_file": "/tmp/result.json"
        });

        queue
            .store_job_result(job_id, result_data.clone(), None)
            .await
            .unwrap();

        // Retrieve the result
        let retrieved_result = queue.get_job_result(job_id).await.unwrap();
        assert!(retrieved_result.is_some());
        assert_eq!(retrieved_result.unwrap(), result_data);

        // Test deletion
        queue.delete_job_result(job_id).await.unwrap();
        let deleted_result = queue.get_job_result(job_id).await.unwrap();
        assert!(deleted_result.is_none());
    }

    #[tokio::test]
    async fn test_result_expiration() {
        let queue = test_utils::setup_postgres_queue().await;

        let job = Job::new("test_queue".to_string(), json!({"task": "short_lived"}));
        let job_id = queue.enqueue(job).await.unwrap();

        let result_data = json!({"status": "completed"});
        let expires_at = chrono::Utc::now() + chrono::Duration::seconds(1);

        queue
            .store_job_result(job_id, result_data, Some(expires_at))
            .await
            .unwrap();

        // Should be retrievable immediately
        let result = queue.get_job_result(job_id).await.unwrap();
        assert!(result.is_some());

        // Wait for expiration
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Should be expired now
        let expired_result = queue.get_job_result(job_id).await.unwrap();
        assert!(expired_result.is_none());
    }

    #[tokio::test]
    async fn test_cleanup_expired_results() {
        let queue = test_utils::setup_postgres_queue().await;

        // Clear any existing expired results to ensure clean test state
        let _ = queue.cleanup_expired_results().await;

        // Create multiple jobs with different expiration times
        let mut job_ids = Vec::new();
        let test_queue = format!("test_cleanup_{}", chrono::Utc::now().timestamp_millis());
        for i in 0..5 {
            let job = Job::new(test_queue.clone(), json!({"task": i}));
            let job_id = queue.enqueue(job).await.unwrap();
            job_ids.push(job_id);

            let result_data = json!({"task_id": i});
            let expires_at = if i < 3 {
                // First 3 expire immediately
                Some(chrono::Utc::now() - chrono::Duration::seconds(1))
            } else {
                // Last 2 expire in the future
                Some(chrono::Utc::now() + chrono::Duration::hours(1))
            };

            queue
                .store_job_result(job_id, result_data, expires_at)
                .await
                .unwrap();
        }

        // Clean up expired results
        let cleaned_count = queue.cleanup_expired_results().await.unwrap();
        assert_eq!(cleaned_count, 3);

        // Verify only non-expired results remain
        for (i, job_id) in job_ids.iter().enumerate() {
            let result = queue.get_job_result(*job_id).await.unwrap();
            if i < 3 {
                assert!(result.is_none(), "Job {} result should be expired", i);
            } else {
                assert!(result.is_some(), "Job {} result should still exist", i);
            }
        }
    }

    #[tokio::test]
    async fn test_worker_automatic_result_storage() {
        let queue = test_utils::setup_postgres_queue().await;
        let unique_queue = format!("auto_result_test_{}", uuid::Uuid::new_v4());

        // Create a handler that returns result data
        let handler: JobHandlerWithResult = Arc::new(|job| {
            Box::pin(async move {
                let task_id: u64 = job.payload["task_id"].as_u64().unwrap();
                let result_data = json!({
                    "task_id": task_id,
                    "processed_at": chrono::Utc::now(),
                    "result": format!("Processed task {}", task_id)
                });

                Ok(JobResult::with_data(result_data))
            })
        });

        // Create worker with result handler
        let worker = Worker::new_with_result_handler(queue.clone(), unique_queue.clone(), handler);

        // Create a job with result storage enabled
        let job = Job::new(unique_queue, json!({"task_id": 123}))
            .with_result_storage(ResultStorage::Database)
            .with_result_ttl(Duration::from_secs(3600));

        let job_id = queue.enqueue(job).await.unwrap();

        // Create and start worker pool to process the job
        let mut worker_pool = WorkerPool::new();
        worker_pool.add_worker(worker);

        // Start the worker pool for a short time to process the job
        let worker_handle = tokio::spawn(async move { worker_pool.start().await });

        // Wait for the job to be processed (check status periodically)
        let mut attempts = 0;
        let max_attempts = 40; // Wait up to 20 seconds
        let mut job_completed = false;

        while attempts < max_attempts {
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Check if job is completed
            if let Some(job) = queue.get_job(job_id).await.unwrap() {
                if job.status == JobStatus::Completed {
                    job_completed = true;
                    break;
                }
            }
            attempts += 1;
        }

        // Ensure job was completed
        assert!(job_completed, "Job should have completed within timeout");

        // Verify result was automatically stored
        let stored_result = queue.get_job_result(job_id).await.unwrap();
        assert!(
            stored_result.is_some(),
            "Job result should be stored after completion"
        );

        let result = stored_result.unwrap();
        assert_eq!(result["task_id"], 123);
        assert!(result["result"].as_str().unwrap().contains("123"));

        // Stop the worker pool
        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_worker_legacy_handler_compatibility() {
        let queue = test_utils::setup_postgres_queue().await;
        let unique_queue = format!("legacy_test_{}", uuid::Uuid::new_v4());

        // Create a legacy handler (returns ())
        let legacy_handler: JobHandler = Arc::new(|_job| {
            Box::pin(async move {
                // Just complete successfully without result data
                Ok(())
            })
        });

        // Create worker with legacy handler
        let worker = Worker::new(queue.clone(), unique_queue.clone(), legacy_handler);

        // Create a job (even with result storage enabled)
        let job = Job::new(unique_queue, json!({"task": "legacy"}))
            .with_result_storage(ResultStorage::Database);

        let job_id = queue.enqueue(job).await.unwrap();

        // Process the job using worker pool
        let mut worker_pool = WorkerPool::new();
        worker_pool.add_worker(worker);

        let worker_handle = tokio::spawn(async move { worker_pool.start().await });

        // Wait for the job to be processed (check status periodically)
        let mut attempts = 0;
        let max_attempts = 20; // Wait up to 10 seconds
        while attempts < max_attempts {
            tokio::time::sleep(Duration::from_millis(500)).await;

            // Check if job is completed
            if let Some(job) = queue.get_job(job_id).await.unwrap() {
                if job.status == JobStatus::Completed {
                    break;
                }
            }
            attempts += 1;
        }

        // Verify no result was stored (legacy handler returns no data)
        let stored_result = queue.get_job_result(job_id).await.unwrap();
        assert!(
            stored_result.is_none(),
            "Legacy handler should not store result data"
        );

        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_result_storage_none_configuration() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create a handler that returns result data
        let handler: JobHandlerWithResult = Arc::new(|_job| {
            Box::pin(async move {
                let result_data = json!({"status": "completed"});
                Ok(JobResult::with_data(result_data))
            })
        });

        let worker =
            Worker::new_with_result_handler(queue.clone(), "test_queue".to_string(), handler);

        // Create a job with result storage disabled
        let job = Job::new("test_queue".to_string(), json!({"task": "no_storage"}))
            .with_result_storage(ResultStorage::None);

        let job_id = queue.enqueue(job).await.unwrap();

        // Process the job using worker pool
        let mut worker_pool = WorkerPool::new();
        worker_pool.add_worker(worker);

        let worker_handle = tokio::spawn(async move { worker_pool.start().await });

        // Wait for job processing
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify no result was stored despite handler returning data
        let stored_result = queue.get_job_result(job_id).await.unwrap();
        assert!(stored_result.is_none());

        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_result_config_builder_methods() {
        // Test the job builder methods for result configuration
        let job1 =
            Job::new("test".to_string(), json!({})).with_result_storage(ResultStorage::Database);
        assert_eq!(job1.result_config.storage, ResultStorage::Database);

        let job2 =
            Job::new("test".to_string(), json!({})).with_result_ttl(Duration::from_secs(7200));
        assert_eq!(job2.result_config.ttl, Some(Duration::from_secs(7200)));

        let config = ResultConfig {
            storage: ResultStorage::Database,
            ttl: Some(Duration::from_secs(3600)),
            max_size_bytes: Some(1024 * 1024), // 1MB
        };
        let job3 = Job::new("test".to_string(), json!({})).with_result_config(config.clone());
        assert_eq!(job3.result_config.storage, config.storage);
        assert_eq!(job3.result_config.ttl, config.ttl);
        assert_eq!(job3.result_config.max_size_bytes, config.max_size_bytes);
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    async fn test_mysql_result_storage() {
        let queue = test_utils::setup_mysql_queue().await;

        // Test basic result storage with MySQL
        let job = Job::new("test_queue".to_string(), json!({"task": "mysql_test"}))
            .with_result_storage(ResultStorage::Database);

        let job_id = queue.enqueue(job).await.unwrap();

        let result_data = json!({
            "database": "mysql",
            "processed_items": 100,
            "status": "success"
        });

        queue
            .store_job_result(job_id, result_data.clone(), None)
            .await
            .unwrap();

        let retrieved_result = queue.get_job_result(job_id).await.unwrap();
        assert!(retrieved_result.is_some());
        assert_eq!(retrieved_result.unwrap(), result_data);
    }

    #[tokio::test]
    async fn test_mysql_worker_integration() {
        let queue = test_utils::setup_mysql_queue().await;

        // Clear any existing test data
        let _ = queue.cleanup_expired_results().await;

        let handler: JobHandlerWithResult = Arc::new(|job| {
            Box::pin(async move {
                let result_data = json!({
                    "mysql_worker": true,
                    "payload": job.payload
                });
                Ok(JobResult::with_data(result_data))
            })
        });

        let test_queue = format!("mysql_worker_{}", chrono::Utc::now().timestamp_millis());
        let worker = Worker::new_with_result_handler(queue.clone(), test_queue.clone(), handler);

        let job = Job::new(test_queue.clone(), json!({"test": "mysql"}))
            .with_result_storage(ResultStorage::Database);

        let job_id = queue.enqueue(job).await.unwrap();

        // Process job using worker pool
        let mut worker_pool = WorkerPool::new();
        worker_pool.add_worker(worker);

        let worker_handle = tokio::spawn(async move { worker_pool.start().await });

        // Wait for job processing with longer timeout
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Check job status first
        let job = queue.get_job(job_id).await.unwrap();
        println!("Job status: {:?}", job.map(|j| j.status));

        // Verify result storage
        let result = queue.get_job_result(job_id).await.unwrap();
        println!("Retrieved result: {:?}", result);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["mysql_worker"], true);

        worker_handle.abort();
    }
}

// Tests that work regardless of database backend
#[tokio::test]
async fn test_result_types() {
    // Test JobResult creation methods
    let success_result = JobResult::success();
    assert!(success_result.data.is_none());

    let data_result = JobResult::with_data(json!({"key": "value"}));
    assert!(data_result.data.is_some());
    assert_eq!(data_result.data.unwrap()["key"], "value");

    // Test default
    let default_result = JobResult::default();
    assert!(default_result.data.is_none());
}

#[tokio::test]
async fn test_result_config_defaults() {
    let config = ResultConfig::default();
    assert_eq!(config.storage, ResultStorage::None);
    assert!(config.ttl.is_none());
    assert!(config.max_size_bytes.is_none());
}
