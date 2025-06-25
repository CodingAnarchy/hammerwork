# Hammerwork

A high-performance, database-driven job queue for Rust with support for both PostgreSQL and MySQL.

## Features

- **Multi-database support**: Works with both PostgreSQL and MySQL
- **Async/await**: Built on Tokio for high concurrency
- **Reliable**: Uses database transactions for job processing
- **Retries**: Configurable retry logic with exponential backoff
- **Delayed jobs**: Schedule jobs to run at specific times
- **Worker pools**: Multiple workers can process jobs concurrently
- **Type-safe**: Leverages Rust's type system for safety

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
hammerwork = "0.1"

# Choose your database
hammerwork = { version = "0.1", features = ["postgres"] }
# or
hammerwork = { version = "0.1", features = ["mysql"] }
```

## Quick Start

### PostgreSQL Example

```rust
use hammerwork::{job::Job, queue::JobQueue, worker::{Worker, WorkerPool}};
use serde_json::json;
use sqlx::PgPool;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to database
    let pool = PgPool::connect("postgresql://localhost/mydb").await?;
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
            println!("Processing job: {} - {}", job.id, job.payload);
            // Your job processing logic here
            Ok(())
        })
    });

    // Create and start workers
    let worker = Worker::new(queue.clone(), "email".to_string(), handler)
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(3);

    let mut pool = WorkerPool::new();
    pool.add_worker(worker);

    // Enqueue a job
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;
        let job = Job::new("email".to_string(), json!({"to": "user@example.com"}));
        queue.enqueue(job).await?;
    }

    // Start processing (runs forever)
    pool.start().await?;
    Ok(())
}
```

### MySQL Example

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

## Job Types

### Basic Job

```rust
use hammerwork::job::Job;
use serde_json::json;

let job = Job::new("email_queue".to_string(), json!({
    "to": "user@example.com",
    "subject": "Welcome!",
    "body": "Thanks for signing up"
}));
```

### Delayed Job

```rust
use chrono::Duration;

let delayed_job = Job::with_delay(
    "notifications".to_string(),
    json!({"message": "Reminder"}),
    Duration::hours(1)
);
```

### Job with Custom Retry Logic

```rust
let job = Job::new("processing".to_string(), json!({"data": "important"}))
    .with_max_attempts(5);
```

## Worker Configuration

```rust
let worker = Worker::new(queue, "my_queue".to_string(), handler)
    .with_poll_interval(Duration::from_millis(500))  // How often to check for jobs
    .with_max_retries(3)                             // Max retry attempts
    .with_retry_delay(Duration::from_secs(30));      // Delay between retries
```

## Database Schema

Hammerwork creates a single table `hammerwork_jobs` with the following structure:

- `id`: Unique job identifier (UUID)
- `queue_name`: Name of the queue
- `payload`: JSON payload data
- `status`: Current job status (pending, running, completed, failed, retrying)
- `attempts`: Number of processing attempts
- `max_attempts`: Maximum allowed attempts
- `created_at`: When the job was created
- `scheduled_at`: When the job should be processed
- `started_at`: When processing began
- `completed_at`: When processing finished
- `error_message`: Error details if job failed

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.