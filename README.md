# Hammerwork

A high-performance, database-driven job queue for Rust with comprehensive features for production workloads.

## Features

- **Multi-database support**: PostgreSQL and MySQL backends
- **Job prioritization**: Five priority levels with weighted and strict scheduling algorithms
- **Cron scheduling**: Full cron expression support with timezone awareness
- **Rate limiting**: Token bucket rate limiting with configurable burst limits
- **Monitoring**: Prometheus metrics and advanced alerting (enabled by default)
- **Job timeouts**: Per-job and worker-level timeout configuration
- **Statistics**: Comprehensive job statistics and dead job management
- **Async/await**: Built on Tokio for high concurrency
- **Type-safe**: Leverages Rust's type system for reliability

## Installation

```toml
[dependencies]
# Default features include metrics and alerting
hammerwork = { version = "0.6", features = ["postgres"] }
# or
hammerwork = { version = "0.6", features = ["mysql"] }

# Minimal installation
hammerwork = { version = "0.6", features = ["postgres"], default-features = false }
```

**Feature Flags**: `postgres`, `mysql`, `metrics` (default), `alerting` (default)

## Quick Start

See the [Quick Start Guide](docs/quick-start.md) for complete examples with PostgreSQL and MySQL.

## Documentation

- **[Quick Start Guide](docs/quick-start.md)** - Get started with PostgreSQL and MySQL
- **[Job Types & Configuration](docs/job-types.md)** - Job creation, priorities, timeouts, cron jobs
- **[Worker Configuration](docs/worker-configuration.md)** - Worker setup, rate limiting, statistics
- **[Cron Scheduling](docs/cron-scheduling.md)** - Recurring jobs with timezone support  
- **[Priority System](docs/priority-system.md)** - Five-level priority system with weighted scheduling
- **[Monitoring & Alerting](docs/monitoring.md)** - Prometheus metrics and notification systems

## Basic Example

```rust
use hammerwork::{Job, Worker, WorkerPool, JobQueue};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup database and queue
    let pool = sqlx::PgPool::connect("postgresql://localhost/mydb").await?;
    let queue = Arc::new(JobQueue::new(pool));
    queue.create_tables().await?;

    // Create job handler
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            println!("Processing: {:?}", job.payload);
            Ok(())
        })
    });

    // Start worker
    let worker = Worker::new(queue.clone(), "default".to_string(), handler);
    let mut pool = WorkerPool::new();
    pool.add_worker(worker);

    // Enqueue jobs
    let job = Job::new("default".to_string(), json!({"task": "send_email"}));
    queue.enqueue(job).await?;

    pool.start().await
}
```





## Database Schema

Hammerwork uses a single `hammerwork_jobs` table with optimized indexes for performance. The schema supports all features including priorities, timeouts, cron scheduling, and comprehensive job lifecycle tracking.

## Development

Comprehensive testing with Docker containers:

```bash
# Start databases and run all tests
make integration-all

# Run specific database tests
make integration-postgres
make integration-mysql
```

See [docs/integration-testing.md](docs/integration-testing.md) for complete development setup.

## Examples

Working examples in `examples/`:
- `postgres_example.rs` - PostgreSQL with timeouts and statistics
- `mysql_example.rs` - MySQL with workers and priorities
- `cron_example.rs` - Cron scheduling with timezones
- `priority_example.rs` - Priority system demonstration

```bash
cargo run --example postgres_example --features postgres
```

## Contributing

1. Fork the repository and create a feature branch
2. Run tests: `make integration-all`
3. Ensure code follows Rust standards (`cargo fmt`, `cargo clippy`)
4. Submit a pull request with tests and documentation

## License

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.