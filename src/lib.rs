//! # Hammerwork
//!
//! A high-performance, database-driven job queue for Rust with comprehensive features for production workloads.
//!
//! ## Features
//!
//! - **Multi-database support**: PostgreSQL and MySQL backends with feature flags
//! - **Job prioritization**: Five priority levels with weighted and strict scheduling algorithms
//! - **Job result storage**: Store and retrieve job execution results with TTL support
//! - **Cron scheduling**: Full cron expression support with timezone awareness
//! - **Rate limiting**: Token bucket rate limiting with configurable burst limits
//! - **Monitoring**: Prometheus metrics and advanced alerting (enabled by default)
//! - **Job timeouts**: Per-job and worker-level timeout configuration
//! - **Statistics**: Comprehensive job statistics and dead job management
//! - **Async/await**: Built on Tokio for high concurrency
//! - **Type-safe**: Leverages Rust's type system for reliability
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use hammerwork::{Job, Worker, WorkerPool, JobQueue, Result, worker::JobHandler};
//! use serde_json::json;
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//!     // Setup database connection (requires PostgreSQL or MySQL)
//!     # #[cfg(feature = "postgres")]
//!     let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
//!     # #[cfg(feature = "mysql")]
//!     # let pool = sqlx::MySqlPool::connect("mysql://localhost/hammerwork").await?;
//!     
//!     let queue = Arc::new(JobQueue::new(pool));
//!     
//!     // Initialize database tables
//!     # #[cfg(any(feature = "postgres", feature = "mysql"))]
//!     {
//!         use hammerwork::queue::DatabaseQueue;
//!         queue.create_tables().await?;
//!     }
//!
//!     // Create job handler
//!     let handler: JobHandler = Arc::new(|job: Job| {
//!         Box::pin(async move {
//!             println!("Processing job: {:?}", job.payload);
//!             // Your job processing logic here
//!             Ok(())
//!         })
//!     });
//!
//!     // Create and start worker
//!     let worker = Worker::new(queue.clone(), "default".to_string(), handler);
//!     let mut pool = WorkerPool::new();
//!     pool.add_worker(worker);
//!
//!     // Enqueue a job
//!     # #[cfg(any(feature = "postgres", feature = "mysql"))]
//!     {
//!         use hammerwork::queue::DatabaseQueue;
//!         let job = Job::new("default".to_string(), json!({"task": "send_email"}));
//!         queue.enqueue(job).await?;
//!     }
//!
//!     // Start processing jobs
//!     Ok(pool.start().await?)
//! }
//! ```
//!
//! ## Core Concepts
//!
//! ### Jobs
//!
//! Jobs are the fundamental unit of work in Hammerwork. Each job has:
//! - A unique UUID identifier
//! - A queue name for routing
//! - A JSON payload containing work data
//! - Priority level (Background, Low, Normal, High, Critical)
//! - Optional scheduling and timeout configuration
//!
//! ### Workers
//!
//! Workers poll queues for pending jobs and execute them using provided handlers.
//! Workers support:
//! - Configurable polling intervals and retry logic
//! - Priority-aware job selection with weighted or strict algorithms
//! - Rate limiting and throttling
//! - Automatic timeout detection and handling
//! - Statistics collection and metrics reporting
//!
//! ### Queues
//!
//! The job queue provides a database-backed persistent store for jobs with:
//! - ACID transactions for reliable job state management
//! - Optimized indexes for high-performance job polling
//! - Support for delayed jobs and cron-based recurring jobs
//! - Dead job management and bulk operations
//!
//! ## Feature Flags
//!
//! - `postgres` - Enable PostgreSQL database support
//! - `mysql` - Enable MySQL database support
//! - `metrics` - Enable Prometheus metrics collection (default)
//! - `alerting` - Enable webhook/Slack/email alerting (default)

pub mod batch;
pub mod cron;
pub mod error;
pub mod job;
pub mod migrations;
pub mod priority;
pub mod queue;
pub mod rate_limit;
pub mod stats;
pub mod worker;

#[cfg(feature = "metrics")]
pub mod metrics;

#[cfg(feature = "alerting")]
pub mod alerting;

pub use batch::{BatchId, BatchResult, BatchStatus, JobBatch, PartialFailureMode};
pub use cron::{CronError, CronSchedule};
pub use error::HammerworkError;
pub use job::{Job, JobId, JobStatus, ResultConfig, ResultStorage};
pub use priority::{
    JobPriority, PriorityError, PrioritySelectionStrategy, PriorityStats, PriorityWeights,
};
pub use queue::JobQueue;
pub use rate_limit::{RateLimit, RateLimiter, ThrottleConfig};
pub use stats::{
    DeadJobSummary, InMemoryStatsCollector, JobStatistics, QueueStats, StatisticsCollector,
};
pub use worker::{
    AutoscaleConfig, AutoscaleMetrics, BatchProcessingStats, JobHandler, JobHandlerWithResult,
    JobResult, Worker, WorkerPool,
};

#[cfg(feature = "metrics")]
pub use metrics::{MetricsConfig, PrometheusMetricsCollector};

#[cfg(feature = "alerting")]
pub use alerting::{Alert, AlertManager, AlertSeverity, AlertTarget, AlertType, AlertingConfig};

/// Convenient type alias for Results with [`HammerworkError`] as the error type.
///
/// This is used throughout the crate for consistent error handling.
pub type Result<T> = std::result::Result<T, HammerworkError>;
