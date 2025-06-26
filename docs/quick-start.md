# Quick Start Guide

This guide will help you get started with Hammerwork quickly.

## PostgreSQL Example

```rust
use hammerwork::{job::Job, queue::JobQueue, worker::{Worker, WorkerPool}};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to database
    let pool = PgPool::connect("postgres://localhost/mydb").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize tables
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;
        queue.create_tables().await?;
    }

    // Create a job handler
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            println!("Processing job: {:?}", job.payload);
            // Your job processing logic here
            Ok(())
        })
    });

    // Create and start a worker
    let worker = Worker::new(Arc::clone(&queue), "default".to_string(), handler);
    
    // Create worker pool
    let mut pool = WorkerPool::new();
    pool.add_worker(worker);

    // Enqueue a job
    let job = Job::new("default".to_string(), json!({"task": "send_email", "user_id": 123}));
    queue.enqueue(job).await?;

    // Start processing (this will run indefinitely)
    pool.start().await?;
    
    Ok(())
}
```

## MySQL Example

```rust
use hammerwork::{job::Job, queue::JobQueue, worker::{Worker, WorkerPool}};
use serde_json::json;
use sqlx::MySqlPool;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to database
    let pool = MySqlPool::connect("mysql://localhost/mydb").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize tables
    #[cfg(feature = "mysql")]
    {
        use hammerwork::queue::DatabaseQueue;
        queue.create_tables().await?;
    }

    // Create handler and worker (same as PostgreSQL example)
    // ...
    
    Ok(())
}
```

## Next Steps

- [Job Types and Configuration](job-types.md)
- [Worker Configuration](worker-configuration.md)
- [Cron Scheduling](cron-scheduling.md)
- [Monitoring and Metrics](monitoring.md)
- [Priority System](priority-system.md)