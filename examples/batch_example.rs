use hammerwork::{
    batch::{JobBatch, PartialFailureMode, BatchStatus},
    job::Job,
    queue::{DatabaseQueue, JobQueue},
    stats::{InMemoryStatsCollector, StatisticsCollector},
    worker::{Worker, WorkerPool},
    HammerworkError, JobPriority, Result,
};
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tracing::{info, warn, error};
use uuid::Uuid;

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
        queue.create_tables().await?;
    }

    // Create statistics collector
    let stats_collector =
        Arc::new(InMemoryStatsCollector::new_default()) as Arc<dyn StatisticsCollector>;

    // Demo 1: Basic batch processing
    info!("=== Demo 1: Basic Batch Processing ===");
    basic_batch_demo(&queue).await?;

    // Demo 2: Large batch with chunking
    info!("=== Demo 2: Large Batch Processing ===");
    large_batch_demo(&queue).await?;

    // Demo 3: Batch processing with workers
    info!("=== Demo 3: Batch Processing with Workers ===");
    batch_worker_demo(&queue, &stats_collector).await?;

    // Demo 4: Different failure modes
    info!("=== Demo 4: Partial Failure Handling ===");
    failure_modes_demo(&queue).await?;

    info!("All batch demos completed successfully!");
    Ok(())
}

/// Demonstrates basic batch creation and enqueueing
async fn basic_batch_demo(queue: &Arc<JobQueue<Postgres>>) -> Result<()> {
    info!("Creating a batch of email jobs...");

    // Create a batch of email notification jobs
    let email_jobs = vec![
        Job::new(
            "email_notifications".to_string(),
            json!({
                "to": "alice@example.com",
                "subject": "Welcome to our service!",
                "template": "welcome",
                "user_id": "user_001"
            }),
        ).as_high_priority(),
        Job::new(
            "email_notifications".to_string(),
            json!({
                "to": "bob@example.com",
                "subject": "Password reset requested",
                "template": "password_reset",
                "user_id": "user_002"
            }),
        ).as_critical(),
        Job::new(
            "email_notifications".to_string(),
            json!({
                "to": "charlie@example.com",
                "subject": "Monthly newsletter",
                "template": "newsletter",
                "user_id": "user_003"
            }),
        ),
    ];

    let batch = JobBatch::new("welcome_email_batch")
        .with_jobs(email_jobs)
        .with_batch_size(10)
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError)
        .with_metadata("campaign_id", "welcome_2024")
        .with_metadata("priority", "high");

    info!("Enqueueing batch with {} jobs...", batch.job_count());
    let batch_id = queue.enqueue_batch(batch).await?;
    info!("Batch enqueued successfully with ID: {}", batch_id);

    // Check batch status
    let batch_result = queue.get_batch_status(batch_id).await?;
    info!("Batch status: {:?}", batch_result.status);
    info!("Total jobs: {}, Pending: {}", batch_result.total_jobs, batch_result.pending_jobs);

    // Get all jobs in the batch
    let batch_jobs = queue.get_batch_jobs(batch_id).await?;
    info!("Retrieved {} jobs from batch", batch_jobs.len());

    for (i, job) in batch_jobs.iter().enumerate() {
        info!("Job {}: ID={}, Priority={:?}, Payload={}", 
              i + 1, job.id, job.priority, job.payload);
    }

    // Clean up
    queue.delete_batch(batch_id).await?;
    info!("Batch cleaned up successfully\n");

    Ok(())
}

/// Demonstrates processing large batches with automatic chunking
async fn large_batch_demo(queue: &Arc<JobQueue<Postgres>>) -> Result<()> {
    info!("Creating a large batch of 500 data processing jobs...");

    // Create a large batch that will benefit from bulk insertion
    let mut data_jobs = Vec::new();
    for i in 0..500 {
        data_jobs.push(
            Job::new(
                "data_processing".to_string(),
                json!({
                    "batch_index": i,
                    "data_type": "analytics",
                    "file_path": format!("/data/analytics/file_{}.csv", i),
                    "priority": if i % 10 == 0 { "high" } else { "normal" }
                }),
            ).with_max_attempts(5),
        );
    }

    let large_batch = JobBatch::new("analytics_processing_batch")
        .with_jobs(data_jobs)
        .with_batch_size(100) // Process in chunks of 100
        .with_partial_failure_handling(PartialFailureMode::CollectErrors)
        .with_metadata("dataset", "q4_2024_analytics")
        .with_metadata("department", "data_science");

    info!("Enqueueing large batch with {} jobs (will be processed in chunks)...", large_batch.job_count());
    
    let start_time = std::time::Instant::now();
    let batch_id = queue.enqueue_batch(large_batch).await?;
    let enqueue_duration = start_time.elapsed();
    
    info!("Large batch enqueued in {:?} with ID: {}", enqueue_duration, batch_id);

    // Verify batch was created correctly
    let batch_result = queue.get_batch_status(batch_id).await?;
    info!("Batch contains {} total jobs, {} pending", 
          batch_result.total_jobs, batch_result.pending_jobs);

    // Sample some jobs to verify they were inserted correctly
    let sample_jobs = queue.get_batch_jobs(batch_id).await?;
    info!("First 5 jobs in batch:");
    for job in sample_jobs.iter().take(5) {
        let batch_index = job.payload["batch_index"].as_u64().unwrap();
        info!("  Job {}: batch_index={}, file_path={}", 
              job.id, batch_index, job.payload["file_path"]);
    }

    // Clean up
    queue.delete_batch(batch_id).await?;
    info!("Large batch cleaned up successfully\n");

    Ok(())
}

/// Demonstrates batch processing with workers
async fn batch_worker_demo(
    queue: &Arc<JobQueue<Postgres>>,
    stats_collector: &Arc<dyn StatisticsCollector>,
) -> Result<()> {
    info!("Setting up workers to process batched jobs...");

    // Create a job handler that processes different types of jobs
    let handler: hammerwork::worker::JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            let job_type = job.payload.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
            
            match job_type {
                "email" => {
                    let to = job.payload["to"].as_str().unwrap_or("unknown");
                    info!("📧 Sending email to: {}", to);
                    // Simulate email sending
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
                "sms" => {
                    let phone = job.payload["phone"].as_str().unwrap_or("unknown");
                    info!("📱 Sending SMS to: {}", phone);
                    // Simulate SMS sending
                    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
                }
                "push" => {
                    let user_id = job.payload["user_id"].as_str().unwrap_or("unknown");
                    info!("🔔 Sending push notification to user: {}", user_id);
                    // Simulate push notification
                    tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
                }
                "webhook" => {
                    let url = job.payload["url"].as_str().unwrap_or("unknown");
                    // Simulate some webhook calls failing
                    if url.contains("example.com") {
                        warn!("🚨 Webhook failed for URL: {}", url);
                        return Err(HammerworkError::Processing(
                            format!("Failed to call webhook: {}", url)
                        ));
                    }
                    info!("🌐 Calling webhook: {}", url);
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }
                _ => {
                    error!("❌ Unknown job type: {}", job_type);
                    return Err(HammerworkError::Processing(
                        format!("Unknown job type: {}", job_type)
                    ));
                }
            }
            
            Ok(())
        })
    });

    // Create workers for different notification channels
    let email_worker = Worker::new(
        queue.clone(),
        "notifications".to_string(),
        handler.clone(),
    )
    .with_poll_interval(std::time::Duration::from_millis(100))
    .with_max_retries(3)
    .with_stats_collector(stats_collector.clone());

    // Start worker pool
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(email_worker);

    // Create a mixed batch of notification jobs
    let notification_jobs = vec![
        // Email notifications
        Job::new(
            "notifications".to_string(),
            json!({
                "type": "email",
                "to": "user1@example.com",
                "subject": "Order confirmation",
                "order_id": "ORD-001"
            }),
        ).as_high_priority(),
        Job::new(
            "notifications".to_string(),
            json!({
                "type": "email",
                "to": "user2@example.com",
                "subject": "Shipping update",
                "order_id": "ORD-002"
            }),
        ),
        
        // SMS notifications
        Job::new(
            "notifications".to_string(),
            json!({
                "type": "sms",
                "phone": "+1234567890",
                "message": "Your order is ready for pickup"
            }),
        ).as_high_priority(),
        
        // Push notifications
        Job::new(
            "notifications".to_string(),
            json!({
                "type": "push",
                "user_id": "user_123",
                "title": "Special offer!",
                "body": "Get 20% off your next order"
            }),
        ),
        
        // Webhook calls (some will fail)
        Job::new(
            "notifications".to_string(),
            json!({
                "type": "webhook",
                "url": "https://api.partner.com/notify",
                "payload": {"event": "order_created", "order_id": "ORD-003"}
            }),
        ),
        Job::new(
            "notifications".to_string(),
            json!({
                "type": "webhook",
                "url": "https://broken.example.com/webhook", // This will fail
                "payload": {"event": "order_updated", "order_id": "ORD-004"}
            }),
        ),
    ];

    let notification_batch = JobBatch::new("mixed_notifications_batch")
        .with_jobs(notification_jobs)
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError)
        .with_metadata("notification_type", "order_updates");

    info!("Enqueueing mixed notification batch...");
    let batch_id = queue.enqueue_batch(notification_batch).await?;

    // Start processing
    info!("Starting workers to process batch...");
    let worker_handle = tokio::spawn(async move {
        if let Err(e) = worker_pool.start().await {
            error!("Worker pool error: {}", e);
        }
    });

    // Monitor batch progress
    for i in 0..30 { // Wait up to 30 seconds
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        let batch_result = queue.get_batch_status(batch_id).await?;
        info!("Batch progress: {}/{} completed, {} failed, {} pending", 
              batch_result.completed_jobs, 
              batch_result.total_jobs,
              batch_result.failed_jobs,
              batch_result.pending_jobs);

        if batch_result.pending_jobs == 0 {
            info!("✅ All jobs in batch have been processed!");
            break;
        }

        if i == 29 {
            warn!("⏰ Timeout waiting for batch to complete");
        }
    }

    // Stop workers
    worker_handle.abort();

    // Final batch status
    let final_result = queue.get_batch_status(batch_id).await?;
    info!("Final batch result:");
    info!("  Status: {:?}", final_result.status);
    info!("  Success rate: {:.1}%", final_result.success_rate());
    info!("  Failure rate: {:.1}%", final_result.failure_rate());

    if !final_result.job_errors.is_empty() {
        info!("  Job errors:");
        for (job_id, error) in &final_result.job_errors {
            info!("    {}: {}", job_id, error);
        }
    }

    // Clean up
    queue.delete_batch(batch_id).await?;
    info!("Notification batch cleaned up successfully\n");

    Ok(())
}

/// Demonstrates different partial failure handling modes
async fn failure_modes_demo(queue: &Arc<JobQueue<Postgres>>) -> Result<()> {
    info!("Testing different partial failure handling modes...");

    // Demo PartialFailureMode::ContinueOnError
    info!("Testing ContinueOnError mode...");
    let continue_jobs = vec![
        Job::new("test_queue".to_string(), json!({"action": "succeed", "id": 1})),
        Job::new("test_queue".to_string(), json!({"action": "fail", "id": 2})),
        Job::new("test_queue".to_string(), json!({"action": "succeed", "id": 3})),
    ];

    let continue_batch = JobBatch::new("continue_on_error_batch")
        .with_jobs(continue_jobs)
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

    let continue_batch_id = queue.enqueue_batch(continue_batch).await?;
    info!("ContinueOnError batch created with ID: {}", continue_batch_id);

    // Demo PartialFailureMode::FailFast
    info!("Testing FailFast mode...");
    let failfast_jobs = vec![
        Job::new("test_queue".to_string(), json!({"action": "succeed", "id": 4})),
        Job::new("test_queue".to_string(), json!({"action": "fail", "id": 5})),
        Job::new("test_queue".to_string(), json!({"action": "succeed", "id": 6})),
    ];

    let failfast_batch = JobBatch::new("fail_fast_batch")
        .with_jobs(failfast_jobs)
        .with_partial_failure_handling(PartialFailureMode::FailFast);

    let failfast_batch_id = queue.enqueue_batch(failfast_batch).await?;
    info!("FailFast batch created with ID: {}", failfast_batch_id);

    // Demo PartialFailureMode::CollectErrors
    info!("Testing CollectErrors mode...");
    let collect_jobs = vec![
        Job::new("test_queue".to_string(), json!({"action": "succeed", "id": 7})),
        Job::new("test_queue".to_string(), json!({"action": "fail", "id": 8})),
        Job::new("test_queue".to_string(), json!({"action": "timeout", "id": 9})),
        Job::new("test_queue".to_string(), json!({"action": "succeed", "id": 10})),
    ];

    let collect_batch = JobBatch::new("collect_errors_batch")
        .with_jobs(collect_jobs)
        .with_partial_failure_handling(PartialFailureMode::CollectErrors);

    let collect_batch_id = queue.enqueue_batch(collect_batch).await?;
    info!("CollectErrors batch created with ID: {}", collect_batch_id);

    // Show the batches were created with different failure modes
    let continue_result = queue.get_batch_status(continue_batch_id).await?;
    let failfast_result = queue.get_batch_status(failfast_batch_id).await?;
    let collect_result = queue.get_batch_status(collect_batch_id).await?;

    info!("Batch failure modes configured:");
    info!("  ContinueOnError batch: {} jobs", continue_result.total_jobs);
    info!("  FailFast batch: {} jobs", failfast_result.total_jobs);
    info!("  CollectErrors batch: {} jobs", collect_result.total_jobs);

    // Clean up all batches
    queue.delete_batch(continue_batch_id).await?;
    queue.delete_batch(failfast_batch_id).await?;
    queue.delete_batch(collect_batch_id).await?;
    info!("All failure mode demo batches cleaned up successfully\n");

    Ok(())
}