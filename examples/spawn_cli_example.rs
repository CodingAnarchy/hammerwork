//! Example demonstrating spawn CLI commands
//!
//! This example shows how to use the spawn CLI commands with both PostgreSQL and MySQL.
//! Run this example with:
//! ```sh
//! # PostgreSQL
//! cargo run --example spawn_cli_example --features postgres
//!
//! # MySQL
//! cargo run --example spawn_cli_example --features mysql
//! ```

use hammerwork::{Job, JobQueue, Result, queue::DatabaseQueue};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio::time::sleep;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DataProcessingJob {
    file_path: String,
    batch_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchProcessingJob {
    batch_id: String,
    start_offset: usize,
    end_offset: usize,
}

async fn setup_database() -> Result<()> {
    #[cfg(feature = "postgres")]
    {
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = sqlx::PgPool::connect(&database_url).await?;
        let queue = JobQueue::new(pool);

        // Create parent job with spawn config
        let parent_job = Job::new(
            "data_processing".to_string(),
            json!({
                "type": "DataProcessingJob",
                "file_path": "/data/large_dataset.csv",
                "batch_size": 1000,
                "_spawn_config": {
                    "handler": "batch_spawner",
                    "max_spawn_count": 10,
                    "operation_id": format!("op-{}", Uuid::new_v4()),
                    "inherit_priority": true,
                    "spawn_delay_ms": 100
                }
            }),
        )
        .with_workflow(Uuid::new_v4(), "Large Dataset Processing");

        let parent_id = queue.enqueue(parent_job).await?;
        println!("Created parent job: {}", parent_id);

        // Simulate job completion and spawning children
        sleep(Duration::from_millis(500)).await;

        // Create child jobs that depend on parent
        for i in 0..5 {
            let child_job = Job::new(
                "batch_processing".to_string(),
                json!({
                    "type": "BatchProcessingJob",
                    "batch_id": format!("batch-{}", i),
                    "start_offset": i * 1000,
                    "end_offset": (i + 1) * 1000
                }),
            )
            .depends_on_jobs(&[parent_id])
            .with_workflow(Uuid::new_v4(), "Large Dataset Processing");

            let child_id = queue.enqueue(child_job).await?;
            println!("Created child job {}: {}", i, child_id);
        }

        println!("\nPostgreSQL spawn example completed!");
        println!("\nTry these CLI commands:");
        println!("cargo hammerwork spawn list");
        println!("cargo hammerwork spawn tree {}", parent_id);
        println!("cargo hammerwork spawn stats --detailed");
        println!("cargo hammerwork spawn lineage {} --descendants", parent_id);
    }

    #[cfg(feature = "mysql")]
    {
        let database_url = std::env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:hammerwork@localhost:3307/hammerwork".to_string());

        let pool = sqlx::MySqlPool::connect(&database_url).await?;
        let queue = JobQueue::new(pool);

        // Create parent job with spawn config
        let parent_job = Job::new(
            "image_processing".to_string(),
            json!({
                "type": "ImageProcessingJob",
                "album_id": "album-123",
                "total_images": 100,
                "_spawn_config": {
                    "handler": "image_spawner",
                    "max_spawn_count": 20,
                    "operation_id": format!("img-op-{}", Uuid::new_v4()),
                    "inherit_priority": true,
                    "batch_size": 5
                }
            }),
        )
        .with_workflow(Uuid::new_v4(), "Album Processing");

        let parent_id = queue.enqueue(parent_job).await?;
        println!("Created parent job: {}", parent_id);

        // Simulate job completion and spawning children
        sleep(Duration::from_millis(500)).await;

        // Create child jobs that depend on parent
        for i in 0..10 {
            let child_job = Job::new(
                "image_resize".to_string(),
                json!({
                    "type": "ImageResizeJob",
                    "image_ids": vec![i*5, i*5+1, i*5+2, i*5+3, i*5+4],
                    "target_sizes": ["thumbnail", "medium", "large"]
                }),
            )
            .depends_on_jobs(&[parent_id])
            .with_workflow(Uuid::new_v4(), "Album Processing");

            let child_id = queue.enqueue(child_job).await?;
            println!("Created child job {}: {}", i, child_id);
        }

        println!("\nMySQL spawn example completed!");
        println!("\nTry these CLI commands:");
        println!("cargo hammerwork spawn list");
        println!("cargo hammerwork spawn tree {}", parent_id);
        println!("cargo hammerwork spawn stats --detailed");
        println!("cargo hammerwork spawn lineage {} --descendants", parent_id);
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    println!("Hammerwork Spawn CLI Example");
    println!("============================\n");

    // Set up example spawn jobs
    setup_database().await?;

    println!("\n\nAdditional spawn CLI commands to explore:");
    println!("----------------------------------------");
    println!("# List recent spawn operations:");
    println!("cargo hammerwork spawn list --recent --limit 10");
    println!();
    println!("# Show spawn operations for specific queue:");
    println!("cargo hammerwork spawn list --queue data_processing");
    println!();
    println!("# Show spawn tree in different formats:");
    println!("cargo hammerwork spawn tree <job-id> --format json");
    println!("cargo hammerwork spawn tree <job-id> --format mermaid");
    println!();
    println!("# Show spawn statistics for last 12 hours:");
    println!("cargo hammerwork spawn stats --hours 12 --detailed");
    println!();
    println!("# Show jobs with pending spawn operations:");
    println!("cargo hammerwork spawn pending --show-config");
    println!();
    println!("# Monitor spawn operations (requires Ctrl+C to stop):");
    println!("cargo hammerwork spawn monitor --interval 3");

    Ok(())
}
