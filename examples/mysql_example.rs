use hammerwork::{
    Result,
    job::Job,
    queue::JobQueue,
    stats::{InMemoryStatsCollector, StatisticsCollector},
    worker::{Worker, WorkerPool},
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
    // Before running this example, make sure to run the migration command to create the necessary tables:
    // cargo hammerwork migrate --database-url mysql://localhost/hammerwork_test

    // Create statistics collector for monitoring job processing
    let stats_collector =
        Arc::new(InMemoryStatsCollector::new_default()) as Arc<dyn StatisticsCollector>;

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

    // Create workers for different queues with statistics collection and timeout configuration
    let image_worker = Worker::new(queue.clone(), "image_processing".to_string(), image_handler)
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(2)
        .with_default_timeout(tokio::time::Duration::from_secs(120)) // 2-minute timeout for image processing
        .with_stats_collector(Arc::clone(&stats_collector));

    let email_worker = Worker::new(queue.clone(), "email_queue".to_string(), email_handler)
        .with_poll_interval(tokio::time::Duration::from_millis(500))
        .with_max_retries(3)
        .with_default_timeout(tokio::time::Duration::from_secs(30)) // 30-second timeout for emails
        .with_stats_collector(Arc::clone(&stats_collector));

    let mut worker_pool = WorkerPool::new().with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(image_worker);
    worker_pool.add_worker(email_worker);

    // Enqueue some test jobs
    #[cfg(feature = "mysql")]
    {
        use hammerwork::queue::DatabaseQueue;

        // Create various test jobs with timeout configurations
        let successful_image_job = Job::new(
            "image_processing".to_string(),
            json!({
                "image_url": "https://example.com/photo.jpg",
                "resize": "800x600",
                "format": "webp"
            }),
        )
        .with_timeout(std::time::Duration::from_secs(60)); // Custom 1-minute timeout for this job

        let failing_image_job = Job::new(
            "image_processing".to_string(),
            json!({
                "image_url": "https://example.com/corrupt_photo.jpg",
                "resize": "800x600",
                "format": "webp"
            }),
        ); // Uses worker default timeout (2 minutes)

        let email_job = Job::new(
            "email_queue".to_string(),
            json!({
                "to": "customer@example.com",
                "subject": "Your image has been processed",
                "template": "image_ready"
            }),
        )
        .with_timeout(std::time::Duration::from_secs(10)); // Quick timeout for email sending

        // High-priority email with longer timeout and more retries
        let priority_email_job = Job::new(
            "email_queue".to_string(),
            json!({
                "to": "vip@example.com",
                "subject": "VIP Image Processing Complete",
                "template": "vip_notification",
                "priority": "high"
            }),
        )
        .with_timeout(std::time::Duration::from_secs(45)) // Longer timeout for VIP
        .with_max_attempts(5); // More retry attempts for important emails

        // Large image processing job with extended timeout
        let large_image_job = Job::new(
            "image_processing".to_string(),
            json!({
                "image_url": "https://example.com/huge_photo.raw",
                "resize": "4K",
                "format": "png",
                "effects": ["sharpen", "color_correct", "denoise"]
            }),
        )
        .with_timeout(std::time::Duration::from_secs(600)) // 10-minute timeout for large processing
        .with_max_attempts(2); // Fewer retries for expensive operations

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
        let job4_id = queue.enqueue(priority_email_job).await?;
        let job5_id = queue.enqueue(large_image_job).await?;
        let job6_id = queue.enqueue(delayed_job).await?;

        info!("Enqueued test jobs with various timeout configurations:");
        info!("  {} - image processing with 60s timeout", job1_id);
        info!(
            "  {} - failing image job (uses worker default 2min timeout)",
            job2_id
        );
        info!("  {} - email with 10s timeout", job3_id);
        info!("  {} - VIP email with 45s timeout", job4_id);
        info!("  {} - large image with 10min timeout", job5_id);
        info!("  {} - delayed email (1min delay)", job6_id);

        // Let jobs process for a bit
        tokio::time::sleep(tokio::time::Duration::from_secs(8)).await;

        // Demonstrate statistics collection with timeout tracking
        info!("=== Job Processing Statistics (Including Timeouts) ===");
        let system_stats = stats_collector
            .get_system_statistics(std::time::Duration::from_secs(300))
            .await?;
        info!(
            "System Stats - Total: {}, Completed: {}, Failed: {}, Dead: {}, Timed Out: {}, Error Rate: {:.2}%",
            system_stats.total_processed,
            system_stats.completed,
            system_stats.failed,
            system_stats.dead,
            system_stats.timed_out,
            system_stats.error_rate * 100.0
        );

        // Get statistics for each queue
        let image_stats = stats_collector
            .get_queue_statistics("image_processing", std::time::Duration::from_secs(300))
            .await?;
        info!(
            "Image Processing Stats - Total: {}, Completed: {}, Failed: {}, Timed Out: {}, Avg Time: {:.2}ms",
            image_stats.total_processed,
            image_stats.completed,
            image_stats.failed,
            image_stats.timed_out,
            image_stats.avg_processing_time_ms
        );

        let email_stats = stats_collector
            .get_queue_statistics("email_queue", std::time::Duration::from_secs(300))
            .await?;
        info!(
            "Email Queue Stats - Total: {}, Completed: {}, Failed: {}, Timed Out: {}, Avg Time: {:.2}ms",
            email_stats.total_processed,
            email_stats.completed,
            email_stats.failed,
            email_stats.timed_out,
            email_stats.avg_processing_time_ms
        );

        // Demonstrate dead job management
        info!("=== Dead Job Management ===");
        let dead_jobs = queue.get_dead_jobs(Some(10), None).await?;
        info!("Found {} dead jobs", dead_jobs.len());

        for dead_job in &dead_jobs {
            info!(
                "Dead Job {} in queue '{}': {:?}",
                dead_job.id, dead_job.queue_name, dead_job.error_message
            );
        }

        // Get dead job summary
        let dead_summary = queue.get_dead_job_summary().await?;
        info!(
            "Dead Job Summary - Total: {}, By Queue: {:?}",
            dead_summary.total_dead_jobs, dead_summary.dead_jobs_by_queue
        );

        // Show queue statistics from database (including timeout counts)
        let all_queue_stats = queue.get_all_queue_stats().await?;
        for queue_stat in all_queue_stats {
            info!(
                "Queue '{}' - Pending: {}, Running: {}, Dead: {}, Timed Out: {}, Completed: {}",
                queue_stat.queue_name,
                queue_stat.pending_count,
                queue_stat.running_count,
                queue_stat.dead_count,
                queue_stat.timed_out_count,
                queue_stat.completed_count
            );
        }

        // Demonstrate error frequency analysis
        let error_frequencies = queue
            .get_error_frequencies(
                Some("image_processing"),
                chrono::Utc::now() - chrono::Duration::hours(1),
            )
            .await?;
        if !error_frequencies.is_empty() {
            info!(
                "Error patterns for image_processing queue: {:?}",
                error_frequencies
            );
        }
    }

    info!("MySQL example completed successfully!");
    info!("Demonstrated features:");
    info!("  ✓ Job timeout configuration (per-job and worker defaults)");
    info!("  ✓ Various timeout scenarios (10s email, 60s image, 10min large jobs)");
    info!("  ✓ Timeout statistics tracking and monitoring");
    info!("  ✓ Dead job management and error analysis");
    info!("  ✓ Comprehensive queue statistics with timeout counts");
    info!("");
    info!("Timeout Features Demonstrated:");
    info!("  • Worker default timeouts (30s for email, 2min for image processing)");
    info!("  • Job-specific timeouts (10s, 45s, 60s, 600s examples)");
    info!("  • Priority-based timeout configuration (VIP vs standard jobs)");
    info!("  • Timeout event tracking in statistics");
    info!("  • Database timeout count tracking");
    info!("");
    info!("In a real application, you would start the worker pool to run indefinitely:");
    info!("worker_pool.start().await?;");

    Ok(())
}
