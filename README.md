# Hammerwork

A high-performance, database-driven job queue for Rust with comprehensive features for production workloads.

## Features

- **🔐 Job Encryption & PII Protection**: Enterprise-grade encryption for sensitive job payloads with AES-256-GCM and ChaCha20-Poly1305, field-level PII protection, and configurable retention policies
- **🗝️ Advanced Key Management**: Complete key lifecycle management with master key encryption, automatic rotation, audit trails, and external KMS integration
- **🚀 Dynamic Job Spawning**: Jobs can dynamically create child jobs during execution for fan-out processing patterns, with full parent-child relationship tracking and lineage management
- **📊 Web Dashboard**: Modern real-time web interface for monitoring queues, managing jobs, and system administration with authentication and WebSocket updates
- **🧪 TestQueue Framework**: Complete in-memory testing implementation with MockClock for deterministic testing of time-dependent features, workflows, and job processing
- **🔍 Job Tracing & Correlation**: Comprehensive distributed tracing with OpenTelemetry integration, trace IDs, correlation IDs, and lifecycle event hooks
- **🔗 Job Dependencies & Workflows**: Create complex data processing pipelines with job dependencies, sequential chains, and parallel processing with synchronization barriers
- **🗄️ Job Archiving & Retention**: Policy-driven archival with configurable retention periods, payload compression, and automated cleanup for compliance and performance
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
hammerwork = { version = "1.12", features = ["postgres"] }
# or
hammerwork = { version = "1.12", features = ["mysql"] }

# With encryption for PII protection
hammerwork = { version = "1.12", features = ["postgres", "encryption"] }

# With AWS KMS integration for enterprise key management
hammerwork = { version = "1.12", features = ["postgres", "encryption", "aws-kms"] }

# With Google Cloud KMS integration for enterprise key management
hammerwork = { version = "1.12", features = ["postgres", "encryption", "gcp-kms"] }

# With HashiCorp Vault KMS integration for enterprise key management
hammerwork = { version = "1.12", features = ["postgres", "encryption", "vault-kms"] }

# With distributed tracing
hammerwork = { version = "1.12", features = ["postgres", "tracing"] }

# Full feature set
hammerwork = { version = "1.12", features = ["postgres", "encryption", "aws-kms", "gcp-kms", "vault-kms", "tracing"] }

# Minimal installation
hammerwork = { version = "1.12", features = ["postgres"], default-features = false }
```

**Feature Flags**: `postgres`, `mysql`, `metrics` (default), `alerting` (default), `encryption` (optional), `aws-kms` (optional), `gcp-kms` (optional), `vault-kms` (optional), `tracing` (optional), `test` (for TestQueue)

### Web Dashboard (Optional)

```bash
# Install the web dashboard
cargo install hammerwork-web --features postgres

# Or add to your project
[dependencies]
hammerwork-web = { version = "1.12", features = ["postgres"] }
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
- **[TestQueue Framework](docs/testing.md)** - In-memory testing with MockClock for unit tests and time control
- **[Web Dashboard](hammerwork-web/README.md)** - Real-time web interface for queue monitoring and job management
- **[Job Tracing & Correlation](docs/tracing.md)** - Distributed tracing, correlation IDs, and OpenTelemetry integration
- **[Job Dependencies & Workflows](docs/workflows.md)** - Complex pipelines, job dependencies, and orchestration
- **[Dynamic Job Spawning](docs/job-spawning.md)** - Fan-out processing, parent-child relationships, and spawn tree visualization
- **[Job Archiving & Retention](docs/archiving.md)** - Policy-driven archival, compression, and compliance management
- **[Job Types & Configuration](docs/job-types.md)** - Job creation, priorities, timeouts, cron jobs
- **[Worker Configuration](docs/worker-configuration.md)** - Worker setup, rate limiting, statistics
- **[Cron Scheduling](docs/cron-scheduling.md)** - Recurring jobs with timezone support  
- **[Priority System](docs/priority-system.md)** - Five-level priority system with weighted scheduling
- **[Batch Operations](docs/batch-operations.md)** - High-performance bulk job processing
- **[Database Migrations](docs/migrations.md)** - Progressive schema updates and database setup
- **[Job Encryption & PII Protection](docs/encryption.md)** - Enterprise encryption, key management, and data protection
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

## Testing Example

Test your job processing logic with the in-memory `TestQueue` framework:

```rust
use hammerwork::queue::test::{TestQueue, MockClock};
use hammerwork::{Job, JobStatus, queue::DatabaseQueue};
use serde_json::json;
use chrono::Duration;

#[tokio::test]
async fn test_delayed_job_processing() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Schedule a job for 1 hour from now
    let future_time = clock.now() + Duration::hours(1);
    let job = Job::new("test_queue".to_string(), json!({"task": "delayed_task"}))
        .with_scheduled_at(future_time);
    
    let job_id = queue.enqueue(job).await.unwrap();
    
    // Job shouldn't be available immediately
    assert!(queue.dequeue("test_queue").await.unwrap().is_none());
    
    // Advance time past scheduled time
    clock.advance(Duration::hours(2));
    
    // Now job should be available for processing
    let dequeued = queue.dequeue("test_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.id, job_id);
    
    // Complete the job
    queue.complete_job(job_id).await.unwrap();
    
    // Verify completion
    let completed = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(completed.status, JobStatus::Completed);
}
```

The `TestQueue` provides complete compatibility with the `DatabaseQueue` trait while offering deterministic time control through `MockClock`, making it perfect for testing complex workflows, retry logic, and time-dependent job processing.

## Job Archiving Example

Configure automatic job archival for compliance and database performance:

```rust
use hammerwork::{
    archive::{ArchivalPolicy, ArchivalConfig, ArchivalReason},
    queue::DatabaseQueue
};
use chrono::Duration;

// Configure archival policy
let policy = ArchivalPolicy::new()
    .archive_completed_after(Duration::days(7))      // Archive completed jobs after 7 days
    .archive_failed_after(Duration::days(30))        // Keep failed jobs for 30 days
    .archive_dead_after(Duration::days(14))         // Archive dead jobs after 14 days
    .archive_timed_out_after(Duration::days(21))    // Archive timed out jobs after 21 days
    .purge_archived_after(Duration::days(365))      // Purge archived jobs after 1 year
    .compress_archived_payloads(true)               // Enable gzip compression
    .with_batch_size(1000)                          // Process up to 1000 jobs per batch
    .enabled(true);

let config = ArchivalConfig::new()
    .with_compression_level(6)                      // Balanced compression
    .with_compression_verification(true);           // Verify compression integrity

// Run archival (typically scheduled as a cron job)
let stats = queue.archive_jobs(
    Some("payment_queue"),                          // Optional: archive specific queue
    &policy,
    &config,
    ArchivalReason::Automatic,                      // Automatic, Manual, Compliance, Maintenance
    Some("scheduler")                               // Who initiated the archival
).await?;

println!("Archived {} jobs, saved {} bytes (compression ratio: {:.2})",
    stats.jobs_archived,
    stats.bytes_archived,
    stats.compression_ratio
);

// Restore an archived job if needed
let job = queue.restore_archived_job(job_id).await?;

// List archived jobs with filtering
let archived_jobs = queue.list_archived_jobs(
    Some("payment_queue"),     // Optional queue filter
    Some(100),                // Limit
    Some(0)                   // Offset for pagination
).await?;

// Purge old archived jobs for GDPR compliance
let purged = queue.purge_archived_jobs(
    Utc::now() - Duration::days(730)  // Delete jobs archived over 2 years ago
).await?;
```

Archival moves completed/failed jobs to a separate table with compressed payloads, reducing the main table size while maintaining compliance requirements.

## Job Encryption Example

Protect sensitive job payloads with enterprise-grade encryption:

```rust
use hammerwork::{
    Job, JobQueue, 
    encryption::{EncryptionConfig, EncryptionAlgorithm, KeySource, RetentionPolicy},
    queue::DatabaseQueue
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup database and queue with encryption
    let pool = sqlx::PgPool::connect("postgresql://localhost/mydb").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Configure encryption for PII protection
    let encryption_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
        .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()))
        // Or use AWS KMS for enterprise key management:
        // .with_key_source(KeySource::External("aws://alias/hammerwork-key?region=us-east-1".to_string()))
        // Or use Google Cloud KMS for enterprise key management:
        // .with_key_source(KeySource::External("gcp://projects/PROJECT/locations/LOCATION/keyRings/RING/cryptoKeys/KEY".to_string()))
        // Or use HashiCorp Vault KMS for enterprise key management:
        // .with_key_source(KeySource::External("vault://secret/hammerwork/encryption-key".to_string()))
        .with_key_rotation_enabled(true);

    // Create job with encrypted PII fields
    let payment_job = Job::new("payment_processing".to_string(), json!({
        "user_id": "user123",
        "credit_card": "4111-1111-1111-1111",  // PII - will be encrypted
        "ssn": "123-45-6789",                  // PII - will be encrypted  
        "amount": 299.99,
        "merchant": "Online Store"
    }))
    .with_encryption(encryption_config)
    .with_pii_fields(vec!["credit_card", "ssn"])  // Specify which fields contain PII
    .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(7 * 24 * 60 * 60))); // 7 days

    // Enqueue encrypted job
    queue.enqueue(payment_job).await?;

    // Job handler processes decrypted payload transparently
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            // Payload is automatically decrypted before reaching handler
            println!("Processing payment: {:?}", job.payload);
            
            // PII fields are available in plain text for processing
            let credit_card = job.payload["credit_card"].as_str().unwrap();
            let ssn = job.payload["ssn"].as_str().unwrap();
            
            // Your business logic here - encryption is transparent
            Ok(())
        })
    });

    Ok(())
}
```

Key features:
- **Automatic Encryption**: PII fields are automatically encrypted when jobs are enqueued
- **Transparent Decryption**: Job handlers receive decrypted payloads transparently
- **Field-Level Protection**: Only specified PII fields are encrypted, keeping metadata accessible
- **Retention Policies**: Automatic deletion of encrypted data after compliance periods
- **Key Management**: Enterprise key rotation, audit trails, and external KMS integration

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
- **`hammerwork_jobs`** - Main job table with priorities, timeouts, cron scheduling, retry strategies, result storage, distributed tracing, and encryption fields
- **`hammerwork_jobs_archive`** - Archive table for completed/failed jobs with compressed payloads (v1.3.0+)
- **`hammerwork_encryption_keys`** - Encrypted key storage with master key encryption and audit trails (v1.7.0+)
- **`hammerwork_batches`** - Batch metadata and tracking (v0.7.0+)
- **`hammerwork_job_results`** - Job result storage with TTL and expiration (v0.8.0+)
- **`hammerwork_migrations`** - Migration tracking for schema evolution

The schema supports all features including job prioritization, advanced retry strategies, timeouts, cron scheduling, batch processing, result storage with TTL, distributed tracing with trace/correlation IDs, worker autoscaling, job archival with compression, job encryption with PII protection, enterprise key management, and comprehensive lifecycle tracking. See [Database Migrations](docs/migrations.md) for details.

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
- `encryption_example.rs` - Job encryption, PII protection, and key management
- `aws_kms_encryption_example.rs` - AWS KMS integration for enterprise key management
- `gcp_kms_encryption_example.rs` - Google Cloud KMS integration for enterprise key management
- `vault_kms_encryption_example.rs` - HashiCorp Vault KMS integration for enterprise key management
- `key_management_example.rs` - Enterprise key lifecycle and audit trails

```bash
cargo run --example postgres_example --features postgres
cargo run --example vault_kms_encryption_example --features vault-kms
```

## Contributing

1. Fork the repository and create a feature branch
2. Run tests: `make integration-all`
3. Ensure code follows Rust standards (`cargo fmt`, `cargo clippy`)
4. Submit a pull request with tests and documentation

## License

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.