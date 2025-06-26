use hammerwork::{
    job::Job,
    queue::JobQueue,
    worker::{Worker, WorkerPool},
    Result,
};
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // Connect to PostgreSQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost/hammerwork_test".to_string());

    let pool = Pool::<Postgres>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;
        queue.create_tables().await?;
    }

    // Create a job handler
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Processing job: {} with payload: {}", job.id, job.payload);

            // Simulate some work
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            // For demo purposes, fail jobs with "fail" in the payload
            if job.payload.get("action").and_then(|v| v.as_str()) == Some("fail") {
                return Err(hammerwork::HammerworkError::Worker {
                    message: "Simulated failure".to_string(),
                });
            }

            info!("Job {} completed successfully", job.id);
            Ok(()) as Result<()>
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create and start workers
    let worker1 = Worker::new(queue.clone(), "email".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(3);

    let worker2 = Worker::new(queue.clone(), "notifications".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_secs(2))
        .with_max_retries(2);

    let mut pool = WorkerPool::new();
    pool.add_worker(worker1);
    pool.add_worker(worker2);

    // Enqueue some test jobs
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;

        let job1 = Job::new(
            "email".to_string(),
            json!({"to": "user@example.com", "subject": "Hello"}),
        );
        let job2 = Job::new(
            "notifications".to_string(),
            json!({"message": "Welcome!", "user_id": 123}),
        );
        let job3 = Job::new("email".to_string(), json!({"action": "fail"})); // This will fail

        queue.enqueue(job1).await?;
        queue.enqueue(job2).await?;
        queue.enqueue(job3).await?;

        info!("Enqueued test jobs");
    }

    // Start the worker pool (this will run indefinitely)
    // In a real application, you'd want to handle shutdown signals
    pool.start().await?;

    Ok(())
}
