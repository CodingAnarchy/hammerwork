//! Example demonstrating dynamic job spawning functionality.
//!
//! This example shows how to create jobs that spawn other jobs during execution,
//! useful for fan-out processing patterns like file processing, batch operations,
//! or workflow orchestration.
//!
//! Run this example with:
//! ```bash
//! cargo run --example spawn_example --features postgres
//! ```

use async_trait::async_trait;
use hammerwork::{
    Job, JobQueue, Worker, WorkerPool,
    queue::DatabaseQueue,
    spawn::{SpawnConfig, SpawnContext, SpawnHandler, SpawnManager},
    worker::JobHandler,
};
use serde_json::json;
use std::sync::Arc;
use tokio::time::{Duration, sleep};
use tracing::info;

// File processing spawn handler
struct FileProcessingHandler;

#[async_trait]
impl<DB: sqlx::Database + Send + Sync> SpawnHandler<DB> for FileProcessingHandler {
    async fn spawn_jobs(&self, context: SpawnContext<DB>) -> hammerwork::Result<Vec<Job>> {
        info!(
            "FileProcessingHandler: Processing job {}",
            context.parent_job.id
        );

        // Extract files list from parent job payload
        let files = context.parent_job.payload["files"]
            .as_array()
            .ok_or_else(|| hammerwork::error::HammerworkError::InvalidJobPayload {
                message: "Missing 'files' array in payload".to_string(),
            })?;

        let mut child_jobs = Vec::new();

        for (index, file) in files.iter().enumerate() {
            let file_path = file.as_str().unwrap_or("unknown");

            // Create a child job for processing each file
            let child_job = Job::new(
                "file_processor".to_string(),
                json!({
                    "file_path": file_path,
                    "parent_job_id": context.parent_job.id,
                    "batch_index": index,
                    "processing_mode": "individual"
                }),
            )
            .as_high_priority() // Child jobs get high priority
            .with_timeout(Duration::from_secs(30));

            child_jobs.push(child_job);
        }

        info!(
            "Created {} child jobs for file processing",
            child_jobs.len()
        );
        Ok(child_jobs)
    }

    async fn validate_spawn(
        &self,
        parent_job: &Job,
        config: &SpawnConfig,
    ) -> hammerwork::Result<()> {
        // Validate that we have files to process
        if let Some(files) = parent_job.payload["files"].as_array() {
            if files.is_empty() {
                return Err(hammerwork::error::HammerworkError::InvalidJobPayload {
                    message: "Files array is empty".to_string(),
                });
            }

            // Check against spawn limit
            if let Some(max_count) = config.max_spawn_count {
                if files.len() > max_count {
                    return Err(hammerwork::error::HammerworkError::InvalidJobPayload {
                        message: format!("Too many files: {} > {}", files.len(), max_count),
                    });
                }
            }
        } else {
            return Err(hammerwork::error::HammerworkError::InvalidJobPayload {
                message: "Missing 'files' field in payload".to_string(),
            });
        }

        Ok(())
    }

    async fn on_spawn_complete(
        &self,
        result: &hammerwork::spawn::SpawnResult,
    ) -> hammerwork::Result<()> {
        info!(
            "Spawn operation completed for parent job {}: {} child jobs created",
            result.parent_job_id,
            result.spawned_jobs.len()
        );
        Ok(())
    }
}

// Data processing spawn handler for aggregation jobs
struct DataAggregationHandler;

#[async_trait]
impl<DB: sqlx::Database + Send + Sync> SpawnHandler<DB> for DataAggregationHandler {
    async fn spawn_jobs(&self, context: SpawnContext<DB>) -> hammerwork::Result<Vec<Job>> {
        info!(
            "DataAggregationHandler: Processing job {}",
            context.parent_job.id
        );

        let chunk_size = context.parent_job.payload["chunk_size"]
            .as_u64()
            .unwrap_or(1000) as usize;

        let total_records = context.parent_job.payload["total_records"]
            .as_u64()
            .unwrap_or(10000) as usize;

        let mut child_jobs = Vec::new();
        let mut offset = 0;

        while offset < total_records {
            let limit = std::cmp::min(chunk_size, total_records - offset);

            let child_job = Job::new(
                "data_processor".to_string(),
                json!({
                    "offset": offset,
                    "limit": limit,
                    "parent_job_id": context.parent_job.id,
                    "table": context.parent_job.payload["table"],
                    "operation": "aggregate"
                }),
            )
            // Normal priority is the default, no need to set explicitly
            .with_timeout(Duration::from_secs(60));

            child_jobs.push(child_job);
            offset += chunk_size;
        }

        info!(
            "Created {} child jobs for data aggregation (total_records: {}, chunk_size: {})",
            child_jobs.len(),
            total_records,
            chunk_size
        );

        Ok(child_jobs)
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Connect to PostgreSQL database
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string());

    let pool = sqlx::PgPool::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Create and configure spawn manager
    let mut spawn_manager = SpawnManager::new();

    // Register spawn handlers for different job types
    spawn_manager.register_handler("file_batch", FileProcessingHandler);
    spawn_manager.register_handler("data_aggregation", DataAggregationHandler);

    let spawn_manager = Arc::new(spawn_manager);

    // Create job handlers
    let file_batch_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            info!("Processing file batch job: {}", job.id);

            // Simulate some processing time
            sleep(Duration::from_millis(500)).await;

            // The actual spawning will happen automatically when the job completes
            // due to the spawn manager configuration
            info!("File batch job {} completed successfully", job.id);
            Ok(())
        })
    });

    let file_processor_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            let file_path = job.payload["file_path"].as_str().unwrap_or("unknown");
            info!(
                "Processing individual file: {} (job: {})",
                file_path, job.id
            );

            // Simulate file processing
            sleep(Duration::from_millis(200)).await;

            info!("File processing completed for: {}", file_path);
            Ok(())
        })
    });

    let data_aggregation_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            info!("Processing data aggregation job: {}", job.id);

            // Simulate processing time
            sleep(Duration::from_millis(300)).await;

            info!("Data aggregation job {} completed successfully", job.id);
            Ok(())
        })
    });

    let data_processor_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            let offset = job.payload["offset"].as_u64().unwrap_or(0);
            let limit = job.payload["limit"].as_u64().unwrap_or(0);
            info!(
                "Processing data chunk: offset={}, limit={} (job: {})",
                offset, limit, job.id
            );

            // Simulate data processing
            sleep(Duration::from_millis(100)).await;

            info!("Data chunk processing completed");
            Ok(())
        })
    });

    // Create workers with spawn manager
    let file_batch_worker =
        Worker::new(queue.clone(), "file_batch".to_string(), file_batch_handler)
            .with_spawn_manager(spawn_manager.clone())
            .with_poll_interval(Duration::from_millis(500));

    let file_processor_worker = Worker::new(
        queue.clone(),
        "file_processor".to_string(),
        file_processor_handler,
    )
    .with_poll_interval(Duration::from_millis(100));

    let data_aggregation_worker = Worker::new(
        queue.clone(),
        "data_aggregation".to_string(),
        data_aggregation_handler,
    )
    .with_spawn_manager(spawn_manager.clone())
    .with_poll_interval(Duration::from_millis(500));

    let data_processor_worker = Worker::new(
        queue.clone(),
        "data_processor".to_string(),
        data_processor_handler,
    )
    .with_poll_interval(Duration::from_millis(100));

    // Create worker pool
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(file_batch_worker);
    worker_pool.add_worker(file_processor_worker);
    worker_pool.add_worker(data_aggregation_worker);
    worker_pool.add_worker(data_processor_worker);

    // Start workers
    info!("Starting worker pool...");
    worker_pool.start().await?;

    // Give workers time to start
    sleep(Duration::from_secs(1)).await;

    // Example 1: File processing job that spawns individual file processing jobs
    info!("=== Example 1: File Processing Spawn ===");

    let file_batch_job = Job::new(
        "file_batch".to_string(),
        json!({
            "files": [
                "data/file1.csv",
                "data/file2.csv",
                "data/file3.csv",
                "data/file4.csv"
            ],
            "batch_id": "batch_001",
            "_spawn_config": SpawnConfig {
                max_spawn_count: Some(10),
                inherit_priority: true,
                inherit_retry_strategy: true,
                inherit_timeout: false,
                inherit_trace_context: true,
                operation_id: Some("file_processing_001".to_string()),
            }
        }),
    )
    .as_high_priority()
    .with_correlation_id("file_batch_001");

    let job_id = queue.enqueue(file_batch_job).await?;
    info!("Enqueued file batch job: {}", job_id);

    // Wait for processing
    sleep(Duration::from_secs(3)).await;

    // Example 2: Data aggregation job that spawns chunk processing jobs
    info!("=== Example 2: Data Aggregation Spawn ===");

    let data_aggregation_job = Job::new(
        "data_aggregation".to_string(),
        json!({
            "table": "user_events",
            "total_records": 5000,
            "chunk_size": 1000,
            "operation": "monthly_summary",
            "_spawn_config": SpawnConfig {
                max_spawn_count: Some(20),
                inherit_priority: false, // Let chunks have normal priority
                inherit_retry_strategy: true,
                inherit_timeout: true,
                inherit_trace_context: true,
                operation_id: Some("data_agg_001".to_string()),
            }
        }),
    )
    .as_high_priority()
    .with_correlation_id("data_agg_001")
    .with_timeout(Duration::from_secs(120));

    let job_id = queue.enqueue(data_aggregation_job).await?;
    info!("Enqueued data aggregation job: {}", job_id);

    // Wait for processing
    sleep(Duration::from_secs(5)).await;

    // Example 3: Error handling - job with invalid spawn config
    info!("=== Example 3: Error Handling ===");

    let invalid_job = Job::new(
        "file_batch".to_string(),
        json!({
            "files": [], // Empty files array should cause validation error
            "_spawn_config": SpawnConfig::default()
        }),
    );

    let job_id = queue.enqueue(invalid_job).await?;
    info!("Enqueued invalid job: {}", job_id);

    // Wait for processing
    sleep(Duration::from_secs(2)).await;

    // Show queue statistics
    info!("=== Queue Statistics ===");
    for queue_name in &[
        "file_batch",
        "file_processor",
        "data_aggregation",
        "data_processor",
    ] {
        let stats = queue.get_queue_stats(queue_name).await?;
        info!(
            "Queue '{}': pending={}, completed={}, dead={}",
            queue_name, stats.pending_count, stats.completed_count, stats.dead_count
        );
    }

    info!("Stopping workers...");
    worker_pool.shutdown().await?;

    info!("Spawn example completed!");
    Ok(())
}
