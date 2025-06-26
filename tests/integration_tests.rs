use hammerwork::{Job, JobQueue, Worker, Result};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;
    use sqlx::{Pool, Postgres};
    use hammerwork::queue::DatabaseQueue;

    async fn setup_postgres_pool() -> Pool<Postgres> {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgres://postgres:password@localhost/hammerwork_test".to_string());
        
        Pool::<Postgres>::connect(&database_url)
            .await
            .expect("Failed to connect to Postgres")
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_full_workflow() {
        let pool = setup_postgres_pool().await;
        let queue = Arc::new(JobQueue::new(pool));
        
        // Setup tables
        queue.create_tables().await.unwrap();
        
        // Create and enqueue a job
        let job = Job::new("test_queue".to_string(), json!({"message": "Hello, World!"}));
        let job_id = job.id;
        
        queue.enqueue(job).await.unwrap();
        
        // Dequeue the job
        let dequeued_job = queue.dequeue("test_queue").await.unwrap();
        assert!(dequeued_job.is_some());
        let dequeued_job = dequeued_job.unwrap();
        assert_eq!(dequeued_job.id, job_id);
        assert_eq!(dequeued_job.payload, json!({"message": "Hello, World!"}));
        
        // Complete the job
        queue.complete_job(job_id).await.unwrap();
        
        // Verify job is completed
        let completed_job = queue.get_job(job_id).await.unwrap();
        assert!(completed_job.is_some());
        let completed_job = completed_job.unwrap();
        assert_eq!(completed_job.status, hammerwork::JobStatus::Completed);
        
        // Clean up
        queue.delete_job(job_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_retry_workflow() {
        let pool = setup_postgres_pool().await;
        let queue = Arc::new(JobQueue::new(pool));
        
        queue.create_tables().await.unwrap();
        
        // Create and enqueue a job
        let job = Job::new("retry_queue".to_string(), json!({"will_fail": true}));
        let job_id = job.id;
        
        queue.enqueue(job).await.unwrap();
        
        // Dequeue and fail the job
        let dequeued_job = queue.dequeue("retry_queue").await.unwrap().unwrap();
        queue.fail_job(dequeued_job.id, "Intentional failure").await.unwrap();
        
        // Verify job failed
        let failed_job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(failed_job.status, hammerwork::JobStatus::Failed);
        assert_eq!(failed_job.error_message, Some("Intentional failure".to_string()));
        
        // Clean up
        queue.delete_job(job_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection  
    async fn test_postgres_delayed_job() {
        let pool = setup_postgres_pool().await;
        let queue = Arc::new(JobQueue::new(pool));
        
        queue.create_tables().await.unwrap();
        
        // Create a delayed job (1 second in the future)
        let delay = chrono::Duration::seconds(1);
        let job = Job::with_delay("delayed_queue".to_string(), json!({"delayed": true}), delay);
        let job_id = job.id;
        
        queue.enqueue(job).await.unwrap();
        
        // Try to dequeue immediately - should get nothing
        let immediate_dequeue = queue.dequeue("delayed_queue").await.unwrap();
        assert!(immediate_dequeue.is_none());
        
        // Wait for the delay and try again
        sleep(Duration::from_secs(2)).await;
        let delayed_dequeue = queue.dequeue("delayed_queue").await.unwrap();
        assert!(delayed_dequeue.is_some());
        
        let dequeued_job = delayed_dequeue.unwrap();
        assert_eq!(dequeued_job.id, job_id);
        
        // Clean up
        queue.complete_job(job_id).await.unwrap();
        queue.delete_job(job_id).await.unwrap();
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;
    use sqlx::{Pool, MySql};
    use hammerwork::queue::DatabaseQueue;

    async fn setup_mysql_pool() -> Pool<MySql> {
        let database_url = std::env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:password@localhost/hammerwork_test".to_string());
        
        Pool::<MySql>::connect(&database_url)
            .await
            .expect("Failed to connect to MySQL")
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_full_workflow() {
        let pool = setup_mysql_pool().await;
        let queue = Arc::new(JobQueue::new(pool));
        
        // Setup tables
        queue.create_tables().await.unwrap();
        
        // Create and enqueue a job
        let job = Job::new("test_queue".to_string(), json!({"message": "Hello, MySQL!"}));
        let job_id = job.id;
        
        queue.enqueue(job).await.unwrap();
        
        // Dequeue the job
        let dequeued_job = queue.dequeue("test_queue").await.unwrap();
        assert!(dequeued_job.is_some());
        let dequeued_job = dequeued_job.unwrap();
        assert_eq!(dequeued_job.id, job_id);
        assert_eq!(dequeued_job.payload, json!({"message": "Hello, MySQL!"}));
        
        // Complete the job
        queue.complete_job(job_id).await.unwrap();
        
        // Verify job is completed
        let completed_job = queue.get_job(job_id).await.unwrap();
        assert!(completed_job.is_some());
        let completed_job = completed_job.unwrap();
        assert_eq!(completed_job.status, hammerwork::JobStatus::Completed);
        
        // Clean up
        queue.delete_job(job_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_concurrent_dequeue() {
        let pool = setup_mysql_pool().await;
        let queue = Arc::new(JobQueue::new(pool));
        
        queue.create_tables().await.unwrap();
        
        // Create multiple jobs
        for i in 0..5 {
            let job = Job::new("concurrent_queue".to_string(), json!({"index": i}));
            queue.enqueue(job).await.unwrap();
        }
        
        // Try to dequeue concurrently
        let mut handles = Vec::new();
        for _ in 0..3 {
            let queue_clone = queue.clone();
            let handle = tokio::spawn(async move {
                queue_clone.dequeue("concurrent_queue").await
            });
            handles.push(handle);
        }
        
        let mut successful_dequeues = 0;
        for handle in handles {
            let result = handle.await.unwrap().unwrap();
            if result.is_some() {
                successful_dequeues += 1;
                // Complete the job to clean up
                queue.complete_job(result.unwrap().id).await.unwrap();
            }
        }
        
        // Should have dequeued some jobs without conflicts
        assert!(successful_dequeues > 0);
        assert!(successful_dequeues <= 3);
    }
}

// Common tests that don't require database features
#[tokio::test]
async fn test_job_creation_patterns() {
    // Test basic job creation
    let job1 = Job::new("queue1".to_string(), json!({"type": "email"}));
    assert_eq!(job1.queue_name, "queue1");
    assert_eq!(job1.max_attempts, 3);
    
    // Test delayed job creation
    let delay = chrono::Duration::minutes(30);
    let job2 = Job::with_delay("queue2".to_string(), json!({"type": "notification"}), delay);
    assert!(job2.scheduled_at > job2.created_at);
    
    // Test builder pattern
    let job3 = Job::new("queue3".to_string(), json!({"type": "report"}))
        .with_max_attempts(5);
    assert_eq!(job3.max_attempts, 5);
    
    // Test chaining with delay
    let job4 = Job::with_delay("queue4".to_string(), json!({"type": "cleanup"}), delay)
        .with_max_attempts(10);
    assert_eq!(job4.max_attempts, 10);
    assert!(job4.scheduled_at > job4.created_at);
}

#[tokio::test]
async fn test_job_status_transitions() {
    let mut job = Job::new("status_test".to_string(), json!({"data": "test"}));
    
    // Initial state
    assert_eq!(job.status, hammerwork::JobStatus::Pending);
    assert_eq!(job.attempts, 0);
    
    // Simulate processing
    job.status = hammerwork::JobStatus::Running;
    job.attempts = 1;
    job.started_at = Some(chrono::Utc::now());
    
    assert_eq!(job.status, hammerwork::JobStatus::Running);
    assert_eq!(job.attempts, 1);
    assert!(job.started_at.is_some());
    
    // Simulate completion
    job.status = hammerwork::JobStatus::Completed;
    job.completed_at = Some(chrono::Utc::now());
    
    assert_eq!(job.status, hammerwork::JobStatus::Completed);
    assert!(job.completed_at.is_some());
}

#[tokio::test]
async fn test_error_types() {
    use hammerwork::HammerworkError;
    
    // Test error display
    let worker_error = HammerworkError::Worker {
        message: "Worker crashed".to_string(),
    };
    assert_eq!(worker_error.to_string(), "Worker error: Worker crashed");
    
    let queue_error = HammerworkError::Queue {
        message: "Connection failed".to_string(),
    };
    assert_eq!(queue_error.to_string(), "Queue error: Connection failed");
    
    // Test error conversion from JSON
    let json_error = serde_json::from_str::<serde_json::Value>("invalid json");
    assert!(json_error.is_err());
    
    let hammerwork_error: HammerworkError = json_error.unwrap_err().into();
    assert!(matches!(hammerwork_error, HammerworkError::Serialization(_)));
}