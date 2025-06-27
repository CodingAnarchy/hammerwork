use hammerwork::{
    HammerworkError, Job, JobPriority, JobQueue,
    batch::{JobBatch, PartialFailureMode},
    queue::DatabaseQueue,
    stats::{InMemoryStatsCollector, StatisticsCollector},
    worker::{JobHandler, Worker, WorkerPool},
};
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // Connect to PostgreSQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost/hammerwork_test".to_string());

    let pool = Pool::<Postgres>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    // Run `cargo hammerwork migrate` to set up the database schema before running this example

    // Create statistics collector
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

    // Demo: Worker with batch processing enabled
    info!("=== Worker Batch Processing Demo ===");
    batch_worker_demo(&queue, &stats_collector).await?;

    info!("Worker batch processing demo completed successfully!");
    Ok(())
}

async fn batch_worker_demo(
    queue: &Arc<JobQueue<Postgres>>,
    stats_collector: &Arc<InMemoryStatsCollector>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    info!("Setting up worker with batch processing enabled...");

    // Create a job handler that demonstrates batch processing features
    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            let job_type = job
                .payload
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let batch_info = if job.batch_id.is_some() {
                format!(" (batch: {})", job.batch_id.unwrap())
            } else {
                String::new()
            };

            match job_type {
                "data_processing" => {
                    let file_name = job
                        .payload
                        .get("file")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    info!("🔄 Processing data file: {}{}", file_name, batch_info);

                    // Simulate processing time based on file size
                    let size = job
                        .payload
                        .get("size_mb")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1);
                    let processing_time = Duration::from_millis(size * 10); // 10ms per MB
                    tokio::time::sleep(processing_time).await;

                    // Simulate occasional processing failures
                    if file_name.contains("corrupted") {
                        return Err(HammerworkError::Processing(format!(
                            "File {} is corrupted and cannot be processed",
                            file_name
                        )));
                    }

                    info!("✅ Completed processing: {}", file_name);
                }
                "image_resize" => {
                    let image_id = job
                        .payload
                        .get("image_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let dimensions = job
                        .payload
                        .get("target_size")
                        .and_then(|v| v.as_str())
                        .unwrap_or("1024x768");

                    info!(
                        "🖼️ Resizing image {} to {}{}",
                        image_id, dimensions, batch_info
                    );

                    // Simulate image processing
                    tokio::time::sleep(Duration::from_millis(100)).await;

                    // Simulate memory issues for large images
                    if dimensions.contains("4K") {
                        return Err(HammerworkError::Processing(format!(
                            "Insufficient memory to resize image {} to {}",
                            image_id, dimensions
                        )));
                    }

                    info!("✅ Image resized: {}", image_id);
                }
                "email_send" => {
                    let recipient = job
                        .payload
                        .get("to")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let subject = job
                        .payload
                        .get("subject")
                        .and_then(|v| v.as_str())
                        .unwrap_or("No subject");

                    info!(
                        "📧 Sending email to {} with subject '{}'{}",
                        recipient, subject, batch_info
                    );

                    // Simulate email sending
                    tokio::time::sleep(Duration::from_millis(50)).await;

                    // Simulate email delivery failures
                    if recipient.contains("bounced") {
                        return Err(HammerworkError::Processing(format!(
                            "Email delivery failed to {}: Recipient address bounced",
                            recipient
                        )));
                    }

                    info!("✅ Email sent to: {}", recipient);
                }
                _ => {
                    error!("❌ Unknown job type: {}", job_type);
                    return Err(HammerworkError::Processing(format!(
                        "Unknown job type: {}",
                        job_type
                    )));
                }
            }

            Ok(())
        })
    });

    // Create worker with batch processing enabled
    let worker = Worker::new(queue.clone(), "batch_processing_queue".to_string(), handler)
        .with_batch_processing_enabled(true) // Enable batch processing features
        .with_poll_interval(Duration::from_millis(100))
        .with_max_retries(3)
        .with_stats_collector(stats_collector.clone())
        .with_priority_weights(
            hammerwork::PriorityWeights::new()
                .with_weight(JobPriority::Critical, 50)
                .with_weight(JobPriority::High, 20)
                .with_weight(JobPriority::Normal, 10),
        );

    // Get initial batch statistics
    let initial_stats = worker.get_batch_stats();
    info!(
        "Initial batch stats - Jobs processed: {}, Batches completed: {}",
        initial_stats.jobs_processed, initial_stats.batches_completed
    );

    // Create multiple batches to demonstrate batch processing
    let batches = create_demo_batches();
    let mut batch_ids = Vec::new();

    for batch in batches {
        info!(
            "Enqueueing batch: {} with {} jobs",
            batch.name,
            batch.job_count()
        );
        let batch_id = queue.enqueue_batch(batch).await?;
        batch_ids.push(batch_id);
    }

    // Start the worker pool
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(worker);

    info!(
        "Starting worker pool to process {} batches...",
        batch_ids.len()
    );

    // Run the worker pool for a limited time
    let worker_handle = tokio::spawn(async move {
        if let Err(e) = worker_pool.start().await {
            error!("Worker pool error: {}", e);
        }
    });

    // Monitor batch progress
    let mut completed_batches = 0;
    for _ in 0..60 {
        // Monitor for up to 60 seconds
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut all_complete = true;
        for &batch_id in &batch_ids {
            let batch_result = queue.get_batch_status(batch_id).await?;

            if batch_result.pending_jobs > 0 {
                all_complete = false;
            } else if batch_result.pending_jobs == 0 && batch_result.total_jobs > 0 {
                // This batch just completed
                if completed_batches < batch_ids.len() {
                    completed_batches += 1;
                    let success_rate = batch_result.success_rate();
                    info!(
                        "Batch {} completed! Success rate: {:.1}% ({}/{} jobs successful)",
                        batch_id,
                        success_rate * 100.0,
                        batch_result.completed_jobs,
                        batch_result.total_jobs
                    );
                }
            }
        }

        if all_complete {
            info!("🎉 All batches completed!");
            break;
        }
    }

    // Stop the worker
    worker_handle.abort();

    // Display final batch processing statistics
    info!("\n=== Final Batch Processing Statistics ===");

    for &batch_id in &batch_ids {
        let batch_result = queue.get_batch_status(batch_id).await?;
        info!(
            "Batch {}: {} total, {} completed, {} failed, {:.1}% success rate",
            batch_id,
            batch_result.total_jobs,
            batch_result.completed_jobs,
            batch_result.failed_jobs,
            batch_result.success_rate() * 100.0
        );
    }

    // Display overall statistics
    let overall_stats = stats_collector
        .get_system_statistics(Duration::from_secs(300))
        .await?;
    info!("\nOverall Statistics:");
    info!("  Total processed: {}", overall_stats.total_processed);
    info!("  Completed: {}", overall_stats.completed);
    info!("  Failed: {}", overall_stats.failed);
    info!(
        "  Success rate: {:.1}%",
        (1.0 - overall_stats.error_rate) * 100.0
    );

    if overall_stats.avg_processing_time_ms > 0.0 {
        info!(
            "  Average processing time: {:.1}ms",
            overall_stats.avg_processing_time_ms
        );
    }

    // Clean up batches
    for batch_id in batch_ids {
        queue.delete_batch(batch_id).await?;
    }

    info!("Batch processing demo completed and cleaned up successfully!\n");

    Ok(())
}

/// Create sample batches for demonstration
fn create_demo_batches() -> Vec<JobBatch> {
    vec![
        // Batch 1: Data processing jobs
        JobBatch::new("data_processing_batch")
            .with_jobs(vec![
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "data_processing",
                        "file": "dataset_001.csv",
                        "size_mb": 50
                    }),
                )
                .as_high_priority(),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "data_processing",
                        "file": "dataset_002.csv",
                        "size_mb": 30
                    }),
                ),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "data_processing",
                        "file": "corrupted_data.csv",  // This will fail
                        "size_mb": 25
                    }),
                ),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "data_processing",
                        "file": "dataset_003.csv",
                        "size_mb": 40
                    }),
                ),
            ])
            .with_partial_failure_handling(PartialFailureMode::ContinueOnError)
            .with_metadata("department", "data_science")
            .with_metadata("priority", "high"),
        // Batch 2: Image processing jobs
        JobBatch::new("image_processing_batch")
            .with_jobs(vec![
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "image_resize",
                        "image_id": "IMG_001.jpg",
                        "target_size": "1920x1080"
                    }),
                )
                .as_critical(),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "image_resize",
                        "image_id": "IMG_002.jpg",
                        "target_size": "1024x768"
                    }),
                ),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "image_resize",
                        "image_id": "IMG_003.jpg",
                        "target_size": "4K"  // This will fail due to memory
                    }),
                ),
            ])
            .with_partial_failure_handling(PartialFailureMode::CollectErrors)
            .with_metadata("campaign", "product_images")
            .with_metadata("quality", "high"),
        // Batch 3: Email notifications
        JobBatch::new("email_notification_batch")
            .with_jobs(vec![
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "email_send",
                        "to": "user1@example.com",
                        "subject": "Welcome to our service!"
                    }),
                )
                .as_high_priority(),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "email_send",
                        "to": "user2@example.com",
                        "subject": "Your order has shipped"
                    }),
                ),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "email_send",
                        "to": "bounced@example.com",  // This will fail
                        "subject": "Monthly newsletter"
                    }),
                ),
                Job::new(
                    "batch_processing_queue".to_string(),
                    json!({
                        "type": "email_send",
                        "to": "user3@example.com",
                        "subject": "Password reset confirmation"
                    }),
                )
                .as_critical(),
            ])
            .with_partial_failure_handling(PartialFailureMode::ContinueOnError)
            .with_metadata("campaign_id", "Q4_2024")
            .with_metadata("email_type", "transactional"),
    ]
}
