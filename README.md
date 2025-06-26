# Hammerwork

A high-performance, database-driven job queue for Rust with support for both PostgreSQL and MySQL, featuring job prioritization, cron scheduling, timeouts, dead job management, and comprehensive statistics collection.

## Features

- **Multi-database support**: Works with both PostgreSQL and MySQL
- **Job prioritization**: Five priority levels (Background, Low, Normal, High, Critical) with weighted and strict scheduling
- **Cron scheduling**: Full cron expression support with timezone-aware recurring jobs
- **Job timeouts**: Per-job and worker-level timeout configuration with automatic timeout detection
- **Statistics & monitoring**: Comprehensive job statistics, dead job management, and performance tracking
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
hammerwork = "0.4"

# Choose your database
hammerwork = { version = "0.4", features = ["postgres"] }
# or
hammerwork = { version = "0.4", features = ["mysql"] }
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

### Job with Priority and Timeout

```rust
use hammerwork::job::{Job, JobPriority};
use chrono::Duration;

// High priority job with timeout
let job = Job::new("processing".to_string(), json!({"data": "important"}))
    .as_high_priority()
    .with_timeout(Duration::seconds(300))
    .with_max_attempts(5);

// Critical priority job
let urgent_job = Job::new("alerts".to_string(), json!({"alert": "system down"}))
    .as_critical();
```

### Recurring Job with Cron Schedule

```rust
use hammerwork::cron::CronSchedule;
use chrono_tz::US::Eastern;

// Daily backup at 2 AM Eastern time
let backup_job = Job::new("backup".to_string(), json!({"type": "daily_backup"}))
    .with_cron("0 0 2 * * *")? // seconds, minutes, hours, day, month, weekday
    .with_timezone(Eastern)
    .as_recurring();

// Weekly report every Monday at 9 AM
let report_job = Job::new("reports".to_string(), json!({"type": "weekly"}))
    .with_cron_schedule(CronSchedule::weekdays_at_9am())
    .as_recurring();
```

## Worker Configuration

### Basic Worker

```rust
let worker = Worker::new(queue, "my_queue".to_string(), handler)
    .with_poll_interval(Duration::from_millis(500))  // How often to check for jobs
    .with_max_retries(3)                             // Max retry attempts
    .with_retry_delay(Duration::from_secs(30));      // Delay between retries
```

### Worker with Priority Configuration

```rust
use hammerwork::priority::{PriorityWeights, JobPriority};

// Worker with custom priority weights
let priority_weights = PriorityWeights::builder()
    .critical(50)
    .high(20)
    .normal(10)
    .low(5)
    .background(1)
    .fairness_factor(0.1) // Prevent starvation
    .build();

let worker = Worker::new(queue, "priority_queue".to_string(), handler)
    .with_priority_weights(priority_weights)
    .with_default_timeout(Duration::from_secs(600)); // 10 minute default timeout

// Worker with strict priority (highest priority jobs always first)
let strict_worker = Worker::new(queue, "urgent_queue".to_string(), handler)
    .with_strict_priority();
```

### Worker with Statistics

```rust
use hammerwork::stats::InMemoryStatsCollector;
use std::sync::Arc;

let stats_collector = Arc::new(InMemoryStatsCollector::new());
let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_stats_collector(stats_collector.clone());

// View statistics
let queue_stats = stats_collector.get_queue_stats("monitored_queue").await?;
println!("Jobs processed: {}", queue_stats.completed_count);
println!("Average processing time: {:?}", queue_stats.avg_processing_time);
println!("Error rate: {:.2}%", queue_stats.error_rate * 100.0);
```

## Job Prioritization

Hammerwork provides a comprehensive job prioritization system with five priority levels and multiple scheduling algorithms.

### Priority Levels

```rust
use hammerwork::job::{Job, JobPriority};

// Five priority levels available
let background_job = Job::new("cleanup".to_string(), payload).as_background();    // Lowest priority
let low_job = Job::new("analytics".to_string(), payload).as_low_priority();      // Low priority  
let normal_job = Job::new("email".to_string(), payload);                         // Default priority
let high_job = Job::new("notifications".to_string(), payload).as_high_priority(); // High priority
let critical_job = Job::new("alerts".to_string(), payload).as_critical();        // Highest priority

// Set priority explicitly
let custom_job = Job::new("custom".to_string(), payload)
    .with_priority(JobPriority::High);
```

### Priority Scheduling Algorithms

#### Weighted Priority Scheduling (Default)

```rust
use hammerwork::priority::PriorityWeights;

let weights = PriorityWeights::builder()
    .critical(50)     // Critical jobs are 50x more likely to be selected
    .high(20)         // High jobs are 20x more likely
    .normal(10)       // Normal jobs baseline
    .low(5)           // Low jobs are 2x less likely
    .background(1)    // Background jobs are 10x less likely
    .fairness_factor(0.1) // 10% chance to select from lower priorities (prevents starvation)
    .build();

let worker = Worker::new(queue, "priority_queue".to_string(), handler)
    .with_priority_weights(weights);
```

#### Strict Priority Scheduling

```rust
// Highest priority jobs are always processed first
let worker = Worker::new(queue, "urgent_queue".to_string(), handler)
    .with_strict_priority();
```

### Priority Statistics

```rust
use hammerwork::stats::InMemoryStatsCollector;

let stats = stats_collector.get_priority_stats().await?;
println!("Critical jobs: {} ({:.1}%)", stats.critical, stats.critical_percentage());
println!("High jobs: {} ({:.1}%)", stats.high, stats.high_percentage());
println!("Most active priority: {:?}", stats.most_active_priority());

// Detect priority starvation
if stats.has_starvation_risk(0.05) { // 5% threshold
    println!("Warning: Lower priority jobs may be starved");
}
```

## Cron Scheduling

Hammerwork supports comprehensive cron-based job scheduling with timezone awareness for recurring jobs.

### Basic Cron Jobs

```rust
use hammerwork::cron::CronSchedule;
use chrono_tz::US::Eastern;

// Using cron expressions (6-field format: seconds, minutes, hours, day, month, weekday)
let daily_backup = Job::new("backup".to_string(), json!({"type": "daily"}))
    .with_cron("0 0 2 * * *")?  // Every day at 2:00 AM
    .with_timezone(Eastern)
    .as_recurring();

// Using built-in presets
let hourly_cleanup = Job::new("cleanup".to_string(), json!({"type": "temp_files"}))
    .with_cron_schedule(CronSchedule::every_hour())
    .as_recurring();

let weekday_reports = Job::new("reports".to_string(), json!({"type": "daily"}))
    .with_cron_schedule(CronSchedule::weekdays_at_9am())
    .as_recurring();
```

### Advanced Cron Scheduling

```rust
// Custom schedules with timezone support
let monthly_billing = Job::new("billing".to_string(), json!({"cycle": "monthly"}))
    .with_cron("0 0 9 1 * *")?  // 1st of every month at 9 AM
    .with_timezone(chrono_tz::America::New_York)
    .as_recurring();

// Complex schedules
let business_hours_sync = Job::new("sync".to_string(), json!({"type": "data"}))
    .with_cron("0 */15 9-17 * * 1-5")?  // Every 15 minutes, 9 AM to 5 PM, weekdays
    .as_recurring();
```

### Cron Job Management

```rust
use hammerwork::queue::DatabaseQueue;

// Enqueue recurring jobs
queue.enqueue_cron_job(daily_backup).await?;

// Get jobs ready for execution
let due_jobs = queue.get_due_cron_jobs(None).await?; // All queues
let queue_jobs = queue.get_due_cron_jobs(Some("backup")).await?; // Specific queue

// Manage recurring jobs
let recurring_jobs = queue.get_recurring_jobs("backup").await?;
queue.disable_recurring_job(&job.id).await?;
queue.enable_recurring_job(&job.id).await?;

// Manual rescheduling
queue.reschedule_cron_job(&job.id).await?;
```

## Job Timeouts

Configure timeouts at the job level or worker level to prevent jobs from running indefinitely.

### Job-Level Timeouts

```rust
use chrono::Duration;

// Set timeout when creating jobs
let timeout_job = Job::new("processing".to_string(), payload)
    .with_timeout(Duration::seconds(300)); // 5 minute timeout

// Timeout takes precedence over worker defaults
let priority_timeout = Job::new("urgent".to_string(), payload)
    .as_critical()
    .with_timeout(Duration::seconds(60)); // 1 minute for urgent jobs
```

### Worker-Level Default Timeouts

```rust
// Set default timeout for all jobs processed by this worker
let worker = Worker::new(queue, "processing_queue".to_string(), handler)
    .with_default_timeout(Duration::seconds(600)); // 10 minute default

// Job-specific timeouts override worker defaults
// Job without timeout uses worker default (600s)
// Job with timeout uses its own setting
```

### Timeout Handling

```rust
// Jobs that exceed their timeout are automatically marked as TimedOut
// Timeout events are recorded in statistics
let stats = stats_collector.get_queue_stats("processing_queue").await?;
println!("Timed out jobs: {}", stats.timed_out_count);
println!("Timeout rate: {:.2}%", (stats.timed_out_count as f64 / stats.total_count as f64) * 100.0);

// Timed out jobs can be retried or marked as dead
if job.status == JobStatus::TimedOut {
    // Handle timeout - retry, mark dead, or take other action
}
```

## Statistics and Dead Job Management

Comprehensive monitoring and management capabilities for job queues.

### Statistics Collection

```rust
use hammerwork::stats::{InMemoryStatsCollector, JobStatistics};
use std::sync::Arc;

let stats_collector = Arc::new(InMemoryStatsCollector::new());

// Configure worker with statistics
let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_stats_collector(stats_collector.clone());

// View comprehensive statistics
let stats = stats_collector.get_job_statistics().await?;
println!("Total jobs processed: {}", stats.total_processed);
println!("Success rate: {:.2}%", stats.success_rate * 100.0);
println!("Average processing time: {:?}", stats.avg_processing_time);
println!("Jobs per minute: {:.1}", stats.throughput_per_minute);

// Queue-specific statistics
let queue_stats = stats_collector.get_queue_stats("monitored_queue").await?;
println!("Queue completed: {}", queue_stats.completed_count);
println!("Queue failed: {}", queue_stats.failed_count);
println!("Queue error rate: {:.2}%", queue_stats.error_rate * 100.0);
```

### Dead Job Management

```rust
use hammerwork::queue::DatabaseQueue;

// Get dead jobs for analysis
let dead_jobs = queue.get_dead_jobs(50, 0).await?; // 50 jobs, offset 0
let queue_dead_jobs = queue.get_dead_jobs_by_queue("failed_queue", 20, 0).await?;

// Analyze dead job patterns
let dead_summary = queue.get_dead_job_summary("failed_queue").await?;
println!("Total dead jobs: {}", dead_summary.total_count);
println!("Common errors:");
for (error, count) in dead_summary.error_frequency {
    println!("  {}: {} occurrences", error, count);
}

// Retry dead jobs
for dead_job in dead_jobs {
    if should_retry(&dead_job) {
        queue.retry_dead_job(&dead_job.id).await?;
    }
}

// Clean up old dead jobs
let cutoff = Utc::now() - chrono::Duration::days(30);
let purged_count = queue.purge_dead_jobs(Some(cutoff)).await?;
println!("Purged {} old dead jobs", purged_count);
```

### Advanced Monitoring

```rust
// Real-time monitoring
use tokio::time::{interval, Duration};

let mut monitor_interval = interval(Duration::from_secs(60));
loop {
    monitor_interval.tick().await;
    
    let stats = stats_collector.get_job_statistics().await?;
    let priority_stats = stats_collector.get_priority_stats().await?;
    
    // Alert on high error rates
    if stats.error_rate > 0.05 { // 5% error rate threshold
        eprintln!("HIGH ERROR RATE: {:.2}%", stats.error_rate * 100.0);
    }
    
    // Alert on priority starvation
    if priority_stats.has_starvation_risk(0.02) { // 2% threshold
        eprintln!("PRIORITY STARVATION DETECTED");
    }
    
    // Performance monitoring
    if stats.avg_processing_time > Duration::from_secs(300) { // 5 minute threshold
        eprintln!("SLOW PROCESSING: {:?} average", stats.avg_processing_time);
    }
}
```

## Database Schema

Hammerwork creates a single table `hammerwork_jobs` with the following structure:

### Core Fields
- `id`: Unique job identifier (UUID)
- `queue_name`: Name of the queue
- `payload`: JSON payload data
- `status`: Current job status (pending, running, completed, failed, retrying, dead, timed_out)
- `attempts`: Number of processing attempts
- `max_attempts`: Maximum allowed attempts

### Priority System
- `priority`: Job priority level (0=Background, 1=Low, 2=Normal, 3=High, 4=Critical)

### Timing & Scheduling
- `created_at`: When the job was created
- `scheduled_at`: When the job should be processed
- `started_at`: When processing began
- `completed_at`: When processing finished
- `failed_at`: When the job failed permanently
- `timed_out_at`: When the job timed out

### Timeout Configuration
- `timeout_seconds`: Job-specific timeout in seconds

### Cron Scheduling
- `cron_schedule`: Cron expression for recurring jobs
- `next_run_at`: Next scheduled execution time
- `recurring`: Whether this is a recurring job
- `timezone`: Timezone for cron calculations

### Error Handling
- `error_message`: Error details if job failed

### Optimized Indexes
- `idx_hammerwork_jobs_queue_status_priority_scheduled`: Priority-aware job polling
- `idx_recurring_next_run`: Efficient recurring job queries
- `idx_cron_schedule`: Cron job management

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

- [`postgres_example.rs`](examples/postgres_example.rs): PostgreSQL job processing with timeouts and statistics
- [`mysql_example.rs`](examples/mysql_example.rs): MySQL job processing with multiple workers and priority configuration
- [`cron_example.rs`](examples/cron_example.rs): Comprehensive cron scheduling with timezone support
- [`priority_example.rs`](examples/priority_example.rs): Job prioritization with weighted and strict scheduling

Run examples with:

```bash
# PostgreSQL example (requires running PostgreSQL)
cargo run --example postgres_example --features postgres

# MySQL example (requires running MySQL)
cargo run --example mysql_example --features mysql

# Cron scheduling example (requires running PostgreSQL)
cargo run --example cron_example --features postgres

# Priority system example (requires running PostgreSQL)
cargo run --example priority_example --features postgres
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