# Hammerwork

A high-performance, database-driven job queue for Rust with comprehensive features for production workloads.

## Features

- **📊 Web Dashboard**: Modern real-time web interface for monitoring queues, managing jobs, and system administration with authentication and WebSocket updates
- **🔍 Job Tracing & Correlation**: Comprehensive distributed tracing with OpenTelemetry integration, trace IDs, correlation IDs, and lifecycle event hooks
- **🔗 Job Dependencies & Workflows**: Create complex data processing pipelines with job dependencies, sequential chains, and parallel processing with synchronization barriers
- **Multi-database support**: PostgreSQL and MySQL backends with optimized dependency queries
- **Advanced retry strategies**: Exponential backoff, linear, Fibonacci, and custom retry patterns with jitter
- **Job prioritization**: Five priority levels with weighted and strict scheduling algorithms
- **Result storage**: Database and in-memory result storage with TTL and automatic cleanup
- **Worker autoscaling**: Dynamic worker pool scaling based on queue depth and configurable thresholds
- **Batch operations**: High-performance bulk job enqueuing with optimized worker processing
- **Cron scheduling**: Full cron expression support with timezone awareness
- **Rate limiting**: Token bucket rate limiting with configurable burst limits
- **Monitoring**: Prometheus metrics and advanced alerting (enabled by default)
- **Job timeouts**: Per-job and worker-level timeout configuration
- **Statistics**: Comprehensive job statistics and dead job management
- **Async/await**: Built on Tokio for high concurrency
- **Type-safe**: Leverages Rust's type system for reliability

## Installation

### Core Library

```toml
[dependencies]
# Default features include metrics and alerting
hammerwork = { version = "1.2", features = ["postgres"] }
# or
hammerwork = { version = "1.2", features = ["mysql"] }

# With distributed tracing
hammerwork = { version = "1.2", features = ["postgres", "tracing"] }

# Minimal installation
hammerwork = { version = "1.2", features = ["postgres"], default-features = false }
```

**Feature Flags**: `postgres`, `mysql`, `metrics` (default), `alerting` (default), `tracing` (optional)

### Web Dashboard (Optional)

```bash
# Install the web dashboard
cargo install hammerwork-web --features postgres

# Or add to your project
[dependencies]
hammerwork-web = { version = "1.2", features = ["postgres"] }
```

Start the dashboard:

```bash
hammerwork-web --database-url postgresql://localhost/hammerwork
# Dashboard available at http://localhost:8080
```

## Quick Start

See the [Quick Start Guide](docs/quick-start.md) for complete examples with PostgreSQL and MySQL.

## Documentation

- **[Quick Start Guide](docs/quick-start.md)** - Get started with PostgreSQL and MySQL
- **[Web Dashboard](hammerwork-web/README.md)** - Real-time web interface for queue monitoring and job management
- **[Job Tracing & Correlation](docs/tracing.md)** - Distributed tracing, correlation IDs, and OpenTelemetry integration
- **[Job Dependencies & Workflows](docs/workflows.md)** - Complex pipelines, job dependencies, and orchestration
- **[Job Types & Configuration](docs/job-types.md)** - Job creation, priorities, timeouts, cron jobs
- **[Worker Configuration](docs/worker-configuration.md)** - Worker setup, rate limiting, statistics
- **[Cron Scheduling](docs/cron-scheduling.md)** - Recurring jobs with timezone support  
- **[Priority System](docs/priority-system.md)** - Five-level priority system with weighted scheduling
- **[Batch Operations](docs/batch-operations.md)** - High-performance bulk job processing
- **[Database Migrations](docs/migrations.md)** - Progressive schema updates and database setup
- **[Monitoring & Alerting](docs/monitoring.md)** - Prometheus metrics and notification systems

## Basic Example

```rust
use hammerwork::{Job, Worker, WorkerPool, JobQueue, RetryStrategy, queue::DatabaseQueue};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup database and queue (migrations should already be run)
    let pool = sqlx::PgPool::connect("postgresql://localhost/mydb").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Create job handler
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            println!("Processing: {:?}", job.payload);
            Ok(())
        })
    });

    // Start worker with retry strategy
    let worker = Worker::new(queue.clone(), "default".to_string(), handler)
        .with_default_retry_strategy(RetryStrategy::exponential(
            Duration::from_secs(1), 2.0, Some(Duration::from_secs(60))
        ));
    let mut pool = WorkerPool::new();
    pool.add_worker(worker);

    // Enqueue jobs with advanced retry strategies
    let job = Job::new("default".to_string(), json!({"task": "send_email"}))
        .with_exponential_backoff(
            Duration::from_secs(2),
            2.0,
            Duration::from_secs(10 * 60)
        );
    queue.enqueue(job).await?;

    pool.start().await
}
```

## Workflow Example

Create complex data processing pipelines with job dependencies:

```rust
use hammerwork::{Job, JobGroup, FailurePolicy, queue::DatabaseQueue};
use serde_json::json;

// Sequential pipeline: job1 → job2 → job3
let job1 = Job::new("process_data".to_string(), json!({"input": "raw_data.csv"}));
let job2 = Job::new("transform_data".to_string(), json!({"format": "parquet"}))
    .depends_on(&job1.id);
let job3 = Job::new("export_data".to_string(), json!({"destination": "s3://bucket/"}))
    .depends_on(&job2.id);

// Parallel processing with synchronization barrier
let parallel_jobs = vec![
    Job::new("process_region_a".to_string(), json!({"region": "us-east"})),
    Job::new("process_region_b".to_string(), json!({"region": "us-west"})),
    Job::new("process_region_c".to_string(), json!({"region": "eu-west"})),
];
let final_job = Job::new("combine_results".to_string(), json!({"output": "summary.json"}));

let workflow = JobGroup::new("data_pipeline")
    .add_parallel_jobs(parallel_jobs)  // These run concurrently
    .then(final_job)                   // This waits for all parallel jobs
    .with_failure_policy(FailurePolicy::ContinueOnFailure);

// Enqueue the entire workflow
queue.enqueue_workflow(workflow).await?;
```

Jobs will only execute when their dependencies are satisfied, enabling sophisticated data processing pipelines and business workflows.

## Tracing Example

Enable comprehensive distributed tracing with OpenTelemetry integration:

```rust
use hammerwork::{Job, Worker, tracing::{TracingConfig, init_tracing}, queue::DatabaseQueue};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize distributed tracing
    let tracing_config = TracingConfig::new()
        .with_service_name("job-processor")
        .with_service_version("1.0.0")
        .with_environment("production")
        .with_otlp_endpoint("http://jaeger:4317");
    
    init_tracing(tracing_config).await?;

    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Create traced jobs with correlation for business workflows
    let trace_id = "trace-12345";
    let correlation_id = "order-67890";
    
    let payment_job = Job::new("payment_queue".to_string(), json!({
        "order_id": "67890",
        "amount": 299.99
    }))
    .with_trace_id(trace_id)
    .with_correlation_id(correlation_id);
    
    let email_job = Job::new("email_queue".to_string(), json!({
        "order_id": "67890", 
        "template": "order_confirmation"
    }))
    .with_trace_id(trace_id)
    .with_correlation_id(correlation_id)
    .depends_on(&payment_job.id);

    // Worker with lifecycle event hooks for observability
    let handler = Arc::new(|job: Job| Box::pin(async move {
        println!("Processing: {:?}", job.payload);
        // Your business logic here
        Ok(())
    }));

    let worker = Worker::new(queue.clone(), "payment_queue".to_string(), handler)
        .on_job_start(|event| {
            println!("Job {} started (trace: {}, correlation: {})", 
                event.job.id,
                event.job.trace_id.unwrap_or_default(),
                event.job.correlation_id.unwrap_or_default());
        })
        .on_job_complete(|event| {
            println!("Job {} completed in {:?}", 
                event.job.id, 
                event.duration.unwrap_or_default());
        })
        .on_job_fail(|event| {
            eprintln!("Job {} failed: {}", 
                event.job.id, 
                event.error.unwrap_or_default());
        });

    // Enqueue jobs - they'll be automatically traced
    queue.enqueue(payment_job).await?;
    queue.enqueue(email_job).await?;

    Ok(())
}
```

This enables end-to-end tracing across your entire job processing pipeline with automatic span creation, correlation tracking, and integration with observability platforms like Jaeger, Zipkin, or DataDog.

## Web Dashboard

Start the real-time web dashboard for monitoring and managing your job queues:

```bash
# Start with PostgreSQL
hammerwork-web --database-url postgresql://localhost/hammerwork

# Start with authentication
hammerwork-web \
  --database-url postgresql://localhost/hammerwork \
  --auth \
  --username admin \
  --password mypassword

# Start with custom configuration
hammerwork-web --config dashboard.toml
```

The dashboard provides:

- **Real-time Monitoring**: Live queue statistics, job counts, and throughput metrics
- **Job Management**: View, retry, cancel, and inspect jobs with detailed payload information
- **Queue Administration**: Clear queues, monitor performance, and manage priorities
- **Interactive Charts**: Throughput graphs and job status distributions
- **WebSocket Updates**: Real-time updates without page refresh
- **REST API**: Complete programmatic access to all dashboard features
- **Authentication**: Secure access with bcrypt password hashing and rate limiting

Access the dashboard at `http://localhost:8080` after starting the server.

## Database Setup

### Using Migrations (Recommended)

Hammerwork provides a migration system for progressive schema updates:

```bash
# Build the migration tool
cargo build --bin cargo-hammerwork --features postgres

# Run migrations
cargo hammerwork migrate --database-url postgresql://localhost/hammerwork

# Check migration status
cargo hammerwork status --database-url postgresql://localhost/hammerwork

# Start the web dashboard after migrations
hammerwork-web --database-url postgresql://localhost/hammerwork
```

### Application Usage

Once migrations are run, your application can use the queue directly:

```rust
// In your application - no setup needed, just use the queue
let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
let queue = Arc::new(JobQueue::new(pool));

// Start enqueuing jobs immediately
let job = Job::new("default".to_string(), json!({"task": "send_email"}));
queue.enqueue(job).await?;
```

### Database Schema

Hammerwork uses optimized tables with comprehensive indexing:
- **`hammerwork_jobs`** - Main job table with priorities, timeouts, cron scheduling, retry strategies, result storage, and distributed tracing fields
- **`hammerwork_batches`** - Batch metadata and tracking (v0.7.0+)
- **`hammerwork_job_results`** - Job result storage with TTL and expiration (v0.8.0+)
- **`hammerwork_migrations`** - Migration tracking for schema evolution

The schema supports all features including job prioritization, advanced retry strategies, timeouts, cron scheduling, batch processing, result storage with TTL, distributed tracing with trace/correlation IDs, worker autoscaling, and comprehensive lifecycle tracking. See [Database Migrations](docs/migrations.md) for details.

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
- `batch_example.rs` - Bulk job enqueuing and processing
- `worker_batch_example.rs` - Worker batch processing features
- `retry_strategies.rs` - Advanced retry patterns with exponential backoff and jitter
- `result_storage_example.rs` - Job result storage and retrieval
- `autoscaling_example.rs` - Dynamic worker pool scaling based on queue depth
- `tracing_example.rs` - Distributed tracing with OpenTelemetry and event hooks

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