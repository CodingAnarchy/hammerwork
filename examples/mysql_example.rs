use hammerwork::{
    job::Job,
    queue::JobQueue,
    worker::{Worker, WorkerPool},
    Result,
};
use serde_json::json;
use sqlx::{MySql, Pool};
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // Connect to MySQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "mysql://localhost/hammerwork_test".to_string());

    let pool = Pool::<MySql>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    #[cfg(feature = "mysql")]
    {
        use hammerwork::queue::DatabaseQueue;
        queue.create_tables().await?;
    }

    // Create a job handler for image processing
    let image_handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!(
                "Processing image job: {} with payload: {}",
                job.id, job.payload
            );

            let image_url = job
                .payload
                .get("image_url")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            info!("Processing image: {}", image_url);

            // Simulate image processing work
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

            info!("Image processing completed for job {}", job.id);
            Ok(()) as Result<()>
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create a job handler for email sending
    let email_handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!(
                "Processing email job: {} with payload: {}",
                job.id, job.payload
            );

            let recipient = job
                .payload
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            info!("Sending email to: {}", recipient);

            // Simulate email sending
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            info!("Email sent successfully for job {}", job.id);
            Ok(()) as Result<()>
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create workers for different queues
    let image_worker = Worker::new(queue.clone(), "image_processing".to_string(), image_handler)
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(2);

    let email_worker = Worker::new(queue.clone(), "email_queue".to_string(), email_handler)
        .with_poll_interval(tokio::time::Duration::from_millis(500))
        .with_max_retries(3);

    let mut pool = WorkerPool::new();
    pool.add_worker(image_worker);
    pool.add_worker(email_worker);

    // Enqueue some test jobs
    #[cfg(feature = "mysql")]
    {
        use hammerwork::queue::DatabaseQueue;

        let image_job = Job::new(
            "image_processing".to_string(),
            json!({
                "image_url": "https://example.com/photo.jpg",
                "resize": "800x600",
                "format": "webp"
            }),
        );

        let email_job = Job::new(
            "email_queue".to_string(),
            json!({
                "to": "customer@example.com",
                "subject": "Your image has been processed",
                "template": "image_ready"
            }),
        );

        // Schedule a delayed job
        let delayed_job = Job::with_delay(
            "email_queue".to_string(),
            json!({
                "to": "admin@example.com",
                "subject": "Daily report",
                "template": "daily_summary"
            }),
            chrono::Duration::minutes(5),
        );

        queue.enqueue(image_job).await?;
        queue.enqueue(email_job).await?;
        queue.enqueue(delayed_job).await?;

        info!("Enqueued test jobs including one delayed job");
    }

    // Start the worker pool
    pool.start().await?;

    Ok(())
}
