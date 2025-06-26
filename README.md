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

## Development & Testing

### Local Development Setup

Hammerwork includes comprehensive integration testing with Docker containers:

```bash
# Clone the repository
git clone https://github.com/CodingAnarchy/hammerwork.git
cd hammerwork

# Start database containers and run all tests
make integration-all

# Or run specific database tests
make integration-postgres  # PostgreSQL only
make integration-mysql     # MySQL only

# Run unit tests only
make test-unit

# Performance benchmarks
make benchmark
```

### Prerequisites

- **Docker & Docker Compose**: For database containers
- **Rust 1.75+**: For building and running tests
- **Make** (optional): For convenient commands

### Available Commands

```bash
# Database management
make start-db              # Start PostgreSQL and MySQL containers
make stop-db               # Stop database containers
make status-db             # Check database status
make logs-db               # View database logs

# Testing
make test                  # Run all tests
make test-unit             # Unit tests only
make test-integration      # Integration tests (no database)
make integration-postgres  # PostgreSQL integration tests
make integration-mysql     # MySQL integration tests
make performance           # Performance benchmarks

# Development
make build                 # Build the project
make lint                  # Run clippy linting
make format                # Format code
make check                 # Run cargo check

# Cleanup
make clean                 # Clean build artifacts
make cleanup               # Full cleanup (containers, volumes, images)
```

### Integration Testing

The project includes comprehensive integration tests that validate:

- ✅ Job queue functionality with real databases
- ✅ PostgreSQL and MySQL compatibility
- ✅ Worker pool management and job processing
- ✅ Performance benchmarks and regression testing
- ✅ Containerized deployment scenarios
- ✅ End-to-end job lifecycle management

See [docs/integration-testing.md](docs/integration-testing.md) for detailed testing documentation.

### CI/CD Pipeline

GitHub Actions automatically runs:

- **Unit Tests**: Fast validation without dependencies
- **PostgreSQL Integration**: Full database testing with PostgreSQL 16
- **MySQL Integration**: Full database testing with MySQL 8.0
- **Docker Tests**: Containerized deployment validation
- **Security Audit**: Vulnerability scanning
- **Performance Tests**: Benchmark regression detection (scheduled daily)

## Examples

Complete working examples are available in the `examples/` directory:

- [`postgres_example.rs`](examples/postgres_example.rs): PostgreSQL job processing
- [`mysql_example.rs`](examples/mysql_example.rs): MySQL job processing with multiple workers

Run examples with:

```bash
# PostgreSQL example (requires running PostgreSQL)
cargo run --example postgres_example --features postgres

# MySQL example (requires running MySQL)
cargo run --example mysql_example --features mysql
```

## Contributing

Contributions are welcome! Please follow these steps:

1. **Fork the repository**
2. **Create a feature branch**: `git checkout -b feature/amazing-feature`
3. **Run tests**: `make integration-all`
4. **Make your changes** with appropriate tests
5. **Ensure all tests pass**: `make ci`
6. **Commit your changes**: `git commit -m 'Add amazing feature'`
7. **Push to the branch**: `git push origin feature/amazing-feature`
8. **Create a Pull Request**

Please ensure your code:
- ✅ Includes appropriate tests
- ✅ Follows Rust formatting (`cargo fmt`)
- ✅ Passes all linting (`cargo clippy`)
- ✅ Maintains or improves test coverage
- ✅ Includes documentation for new features

## License

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.