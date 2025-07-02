# Quick Start Guide

This guide will help you get started with Hammerwork quickly using the current API and best practices.

## Prerequisites

Before starting, you need to set up your database schema using the migration tool:

```bash
# For PostgreSQL
cargo hammerwork migrate --database-url postgresql://localhost/hammerwork

# For MySQL  
cargo hammerwork migrate --database-url mysql://localhost/hammerwork
```

## Dependencies

Add Hammerwork to your `Cargo.toml`:

```toml
[dependencies]
# PostgreSQL support with default features (metrics, alerting)
hammerwork = { version = "1.5", features = ["postgres"] }

# MySQL support
hammerwork = { version = "1.5", features = ["mysql"] }

# With distributed tracing
hammerwork = { version = "1.5", features = ["postgres", "tracing"] }

# Minimal installation without default features
hammerwork = { version = "1.5", features = ["postgres"], default-features = false }

# Additional dependencies for examples
serde_json = "1.0"
tokio = { version = "1.0", features = ["full"] }
tracing = "0.1"
tracing-subscriber = "0.3"
```

## PostgreSQL Example

```rust
use hammerwork::{
    Result,
    job::Job,
    queue::{JobQueue, DatabaseQueue},
    worker::{Worker, WorkerPool},
    stats::{InMemoryStatsCollector, StatisticsCollector},
    rate_limit::RateLimit,
};
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt().init();

    // Connect to PostgreSQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost/hammerwork".to_string());
    
    let pool = Pool::<Postgres>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Create statistics collector for monitoring
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default()) 
        as Arc<dyn StatisticsCollector>;

    // Create a job handler with proper error handling
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Processing job: {} with payload: {}", job.id, job.payload);

            // Simulate some work
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Your job processing logic here
            match job.payload.get("action").and_then(|v| v.as_str()) {
                Some("send_email") => {
                    let to = job.payload.get("to").and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    info!("Sending email to: {}", to);
                    // Email sending logic here
                    Ok(())
                }
                Some("process_data") => {
                    info!("Processing data");
                    // Data processing logic here
                    Ok(())
                }
                Some("fail") => {
                    // Simulate failure for testing
                    Err(hammerwork::HammerworkError::Worker {
                        message: "Simulated failure".to_string(),
                    })
                }
                _ => {
                    info!("Unknown action, skipping");
                    Ok(())
                }
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create workers with comprehensive configuration
    let email_worker = Worker::new(queue.clone(), "email".to_string(), handler.clone())
        .with_poll_interval(Duration::from_secs(1))
        .with_max_retries(3)
        .with_default_timeout(Duration::from_secs(30))
        .with_rate_limit(RateLimit::per_second(2))
        .with_stats_collector(Arc::clone(&stats_collector));

    let data_worker = Worker::new(queue.clone(), "data_processing".to_string(), handler.clone())
        .with_poll_interval(Duration::from_secs(2))
        .with_max_retries(2)
        .with_default_timeout(Duration::from_secs(60))
        .with_stats_collector(Arc::clone(&stats_collector));

    // Create worker pool with statistics
    let mut worker_pool = WorkerPool::new()
        .with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(email_worker);
    worker_pool.add_worker(data_worker);

    // Enqueue some test jobs
    let job1 = Job::new("email".to_string(), json!({
        "action": "send_email",
        "to": "user@example.com",
        "subject": "Welcome!"
    }));

    let job2 = Job::new("data_processing".to_string(), json!({
        "action": "process_data",
        "dataset": "customer_records.csv"
    }))
    .with_timeout(Duration::from_secs(120))  // Custom timeout
    .with_max_attempts(5);                   // More retry attempts

    // Job with custom priority
    let urgent_job = Job::new("email".to_string(), json!({
        "action": "send_email", 
        "to": "admin@example.com",
        "subject": "Urgent Alert"
    }))
    .with_priority(hammerwork::priority::JobPriority::High);

    // Enqueue jobs
    let job1_id = queue.enqueue(job1).await?;
    let job2_id = queue.enqueue(job2).await?;
    let urgent_id = queue.enqueue(urgent_job).await?;

    info!("Enqueued jobs: {}, {}, {}", job1_id, job2_id, urgent_id);

    // In a real application, you would start the worker pool to run indefinitely:
    // worker_pool.start().await?;

    // For this example, let's process jobs for 10 seconds
    tokio::spawn(async move {
        worker_pool.start().await.unwrap();
    });

    // Wait a bit to see some processing
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Show statistics
    let system_stats = stats_collector
        .get_system_statistics(Duration::from_secs(300))
        .await?;
    
    info!("System Statistics:");
    info!("  Total processed: {}", system_stats.total_processed);
    info!("  Completed: {}", system_stats.completed);
    info!("  Failed: {}", system_stats.failed);
    info!("  Error rate: {:.2}%", system_stats.error_rate * 100.0);

    info!("Quick start completed successfully!");
    Ok(())
}
```

## MySQL Example

```rust
use hammerwork::{
    Result,
    job::Job,
    queue::{JobQueue, DatabaseQueue},
    worker::{Worker, WorkerPool},
    stats::{InMemoryStatsCollector, StatisticsCollector},
};
use serde_json::json;
use sqlx::{MySql, Pool};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // Connect to MySQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "mysql://localhost/hammerwork".to_string());
    
    let pool = Pool::<MySql>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Create statistics collector
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default()) 
        as Arc<dyn StatisticsCollector>;

    // Create job handler (same pattern as PostgreSQL)
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Processing job: {} with payload: {}", job.id, job.payload);
            
            // Simulate work
            tokio::time::sleep(Duration::from_secs(1)).await;
            
            // Process based on job type
            let job_type = job.payload.get("type").and_then(|v| v.as_str())
                .unwrap_or("unknown");
                
            match job_type {
                "image_processing" => {
                    let image_url = job.payload.get("image_url").and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    info!("Processing image: {}", image_url);
                    
                    // Simulate longer processing for images
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    
                    if image_url.contains("corrupt") {
                        return Err(hammerwork::HammerworkError::Worker {
                            message: "Corrupted image file".to_string(),
                        });
                    }
                    
                    Ok(())
                }
                "email" => {
                    let to = job.payload.get("to").and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    info!("Sending email to: {}", to);
                    Ok(())
                }
                _ => {
                    info!("Unknown job type: {}", job_type);
                    Ok(())
                }
            }
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create workers for different job types
    let image_worker = Worker::new(queue.clone(), "image_processing".to_string(), handler.clone())
        .with_poll_interval(Duration::from_secs(1))
        .with_max_retries(2)
        .with_default_timeout(Duration::from_secs(120))  // Longer timeout for images
        .with_stats_collector(Arc::clone(&stats_collector));

    let email_worker = Worker::new(queue.clone(), "email_queue".to_string(), handler.clone())
        .with_poll_interval(Duration::from_millis(500))
        .with_max_retries(3)
        .with_default_timeout(Duration::from_secs(30))
        .with_stats_collector(Arc::clone(&stats_collector));

    // Set up worker pool
    let mut worker_pool = WorkerPool::new()
        .with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(image_worker);
    worker_pool.add_worker(email_worker);

    // Enqueue test jobs
    let image_job = Job::new("image_processing".to_string(), json!({
        "type": "image_processing",
        "image_url": "https://example.com/photo.jpg",
        "filters": ["resize", "compress"]
    }));

    let email_job = Job::new("email_queue".to_string(), json!({
        "type": "email",
        "to": "user@example.com",
        "subject": "Processing Complete",
        "template": "notification"
    }));

    queue.enqueue(image_job).await?;
    queue.enqueue(email_job).await?;

    info!("Jobs enqueued successfully!");
    info!("In a real application, start the worker pool:");
    info!("worker_pool.start().await?;");

    Ok(())
}
```

## Key Features Demonstrated

### 1. Proper Error Handling
- Use `hammerwork::HammerworkError::Worker` for job failures
- Jobs that return errors will be retried according to retry policy

### 2. Job Configuration
```rust
let job = Job::new("queue_name".to_string(), json!({"data": "value"}))
    .with_timeout(Duration::from_secs(60))           // Custom timeout
    .with_max_attempts(5)                            // Retry attempts
    .with_priority(JobPriority::High)                // Job priority
    .with_delay(Duration::from_secs(30));            // Delay execution
```

### 3. Worker Configuration
```rust
let worker = Worker::new(queue.clone(), "queue_name".to_string(), handler)
    .with_poll_interval(Duration::from_secs(1))      // How often to check for jobs
    .with_max_retries(3)                             // Default retry attempts
    .with_default_timeout(Duration::from_secs(30))   // Default job timeout
    .with_rate_limit(RateLimit::per_second(2))       // Rate limiting
    .with_stats_collector(stats_collector);          // Statistics collection
```

### 4. Statistics and Monitoring
```rust
// Get system-wide statistics
let stats = stats_collector
    .get_system_statistics(Duration::from_secs(300))
    .await?;

// Get queue-specific statistics  
let queue_stats = stats_collector
    .get_queue_statistics("email", Duration::from_secs(300))
    .await?;

// Get database statistics
let db_stats = queue.get_queue_stats("email").await?;
```

## Environment Setup

Create a `.env` file for your database configuration:

```env
# PostgreSQL
DATABASE_URL=postgresql://username:password@localhost/hammerwork

# MySQL
DATABASE_URL=mysql://username:password@localhost/hammerwork
```

## Running the Examples

```bash
# Set up the database first
cargo hammerwork migrate --database-url postgresql://localhost/hammerwork

# Run PostgreSQL example
DATABASE_URL=postgresql://localhost/hammerwork cargo run --example postgres_example --features postgres

# Run MySQL example  
DATABASE_URL=mysql://localhost/hammerwork cargo run --example mysql_example --features mysql
```

## Production Considerations

1. **Database Migrations**: Always run migrations before deploying
2. **Connection Pooling**: Configure appropriate pool sizes for your workload
3. **Error Handling**: Implement proper error handling and alerting
4. **Monitoring**: Use the statistics collectors and consider metrics export
5. **Resource Management**: Configure appropriate timeouts and rate limits
6. **Graceful Shutdown**: Implement signal handling for clean worker shutdown

## Next Steps

- **[Job Types & Configuration](job-types.md)** - Learn about job creation, priorities, and scheduling
- **[Worker Configuration](worker-configuration.md)** - Advanced worker setup and optimization
- **[Cron Scheduling](cron-scheduling.md)** - Set up recurring jobs with cron expressions
- **[Priority System](priority-system.md)** - Understand job prioritization and scheduling
- **[Batch Operations](batch-operations.md)** - Process multiple jobs efficiently
- **[Monitoring & Alerting](monitoring.md)** - Set up comprehensive monitoring
- **[Job Dependencies & Workflows](workflows.md)** - Create complex job pipelines
- **[Database Migrations](migrations.md)** - Manage database schema evolution