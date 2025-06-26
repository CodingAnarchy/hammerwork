use hammerwork::{
    job::Job,
    queue::JobQueue,
    stats::{InMemoryStatsCollector, StatisticsCollector},
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

    // Create statistics collector for monitoring job processing
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default()) as Arc<dyn StatisticsCollector>;

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

            // Simulate occasional failures for demonstration
            if image_url.contains("corrupt") {
                return Err(hammerwork::HammerworkError::Worker {
                    message: "Corrupted image file".to_string(),
                });
            }

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

    // Create workers for different queues with statistics collection
    let image_worker = Worker::new(queue.clone(), "image_processing".to_string(), image_handler)
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(2)
        .with_stats_collector(Arc::clone(&stats_collector));

    let email_worker = Worker::new(queue.clone(), "email_queue".to_string(), email_handler)
        .with_poll_interval(tokio::time::Duration::from_millis(500))
        .with_max_retries(3)
        .with_stats_collector(Arc::clone(&stats_collector));

    let mut worker_pool = WorkerPool::new()
        .with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(image_worker);
    worker_pool.add_worker(email_worker);

    // Enqueue some test jobs
    #[cfg(feature = "mysql")]
    {
        use hammerwork::queue::DatabaseQueue;

        // Create various test jobs including some that will fail
        let successful_image_job = Job::new(
            "image_processing".to_string(),
            json!({
                "image_url": "https://example.com/photo.jpg",
                "resize": "800x600",
                "format": "webp"
            }),
        );

        let failing_image_job = Job::new(
            "image_processing".to_string(),
            json!({
                "image_url": "https://example.com/corrupt_photo.jpg",
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
            chrono::Duration::minutes(1), // Reduced delay for demo
        );

        let job1_id = queue.enqueue(successful_image_job).await?;
        let job2_id = queue.enqueue(failing_image_job).await?;
        let job3_id = queue.enqueue(email_job).await?;
        let job4_id = queue.enqueue(delayed_job).await?;

        info!("Enqueued test jobs: {}, {}, {}, {}", job1_id, job2_id, job3_id, job4_id);

        // Let jobs process for a bit
        tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

        // Demonstrate statistics collection
        info!("=== Job Processing Statistics ===");
        let system_stats = stats_collector.get_system_statistics(std::time::Duration::from_secs(300)).await?;
        info!("System Stats - Total: {}, Completed: {}, Failed: {}, Dead: {}, Error Rate: {:.2}%",
            system_stats.total_processed,
            system_stats.completed,
            system_stats.failed,
            system_stats.dead,
            system_stats.error_rate * 100.0
        );

        // Get statistics for each queue
        let image_stats = stats_collector.get_queue_statistics("image_processing", std::time::Duration::from_secs(300)).await?;
        info!("Image Processing Stats - Total: {}, Completed: {}, Failed: {}, Avg Time: {:.2}ms",
            image_stats.total_processed,
            image_stats.completed,
            image_stats.failed,
            image_stats.avg_processing_time_ms
        );

        let email_stats = stats_collector.get_queue_statistics("email_queue", std::time::Duration::from_secs(300)).await?;
        info!("Email Queue Stats - Total: {}, Completed: {}, Failed: {}, Avg Time: {:.2}ms",
            email_stats.total_processed,
            email_stats.completed,
            email_stats.failed,
            email_stats.avg_processing_time_ms
        );

        // Demonstrate dead job management
        info!("=== Dead Job Management ===");
        let dead_jobs = queue.get_dead_jobs(Some(10), None).await?;
        info!("Found {} dead jobs", dead_jobs.len());
        
        for dead_job in &dead_jobs {
            info!("Dead Job {} in queue '{}': {:?}", 
                dead_job.id, 
                dead_job.queue_name, 
                dead_job.error_message
            );
        }

        // Get dead job summary
        let dead_summary = queue.get_dead_job_summary().await?;
        info!("Dead Job Summary - Total: {}, By Queue: {:?}",
            dead_summary.total_dead_jobs,
            dead_summary.dead_jobs_by_queue
        );

        // Show queue statistics from database
        let all_queue_stats = queue.get_all_queue_stats().await?;
        for queue_stat in all_queue_stats {
            info!("Queue '{}' - Pending: {}, Running: {}, Dead: {}, Completed: {}",
                queue_stat.queue_name,
                queue_stat.pending_count,
                queue_stat.running_count,
                queue_stat.dead_count,
                queue_stat.completed_count
            );
        }

        // Demonstrate error frequency analysis
        let error_frequencies = queue.get_error_frequencies(
            Some("image_processing"), 
            chrono::Utc::now() - chrono::Duration::hours(1)
        ).await?;
        if !error_frequencies.is_empty() {
            info!("Error patterns for image_processing queue: {:?}", error_frequencies);
        }
    }

    info!("MySQL example completed successfully with statistics and dead job management.");
    info!("In a real application, you would start the worker pool to run indefinitely:");
    info!("worker_pool.start().await?;");

    Ok(())
}
