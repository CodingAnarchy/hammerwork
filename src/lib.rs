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
//! use hammerwork::{Job, Worker, WorkerPool, JobQueue, Result, worker::JobHandler, queue::DatabaseQueue};
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
//!     // Note: Run database migrations first using `cargo hammerwork migrate`
//!     // or use the migration manager programmatically
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
//! ### Event System
//!
//! The event system provides real-time job lifecycle tracking with:
//! - Publish/subscribe pattern for job lifecycle events
//! - Webhook delivery with authentication and retry policies
//! - Streaming integration with Kafka, Kinesis, and Pub/Sub
//! - Flexible event filtering and routing
//! - Multiple serialization formats and partitioning strategies
//!
//! ### Configuration & Operations
//!
//! Comprehensive configuration and operational tooling:
//! - TOML-based configuration with environment variable overrides
//! - CLI tooling for database migrations, job management, and monitoring
//! - Development and production configuration presets
//! - Health checks and graceful shutdown support
//!
//! ## Event System Integration
//!
//! Hammerwork provides a comprehensive event system for integrating with external systems:
//!
//! ```rust,ignore
//! use hammerwork::{
//!     events::{EventManager, EventFilter, JobLifecycleEventType},
//!     webhooks::{WebhookManager, WebhookConfig, WebhookAuth, HttpMethod},
//!     streaming::{StreamManager, StreamConfig, StreamBackend, PartitioningStrategy},
//!     config::HammerworkConfig,
//! };
//! use std::sync::Arc;
//! use std::collections::HashMap;
//!
//! #[tokio::main]
//! async fn main() -> hammerwork::Result<()> {
//!     // Create event manager
//!     let event_manager = Arc::new(EventManager::new_default());
//!
//!     // Set up webhooks for job completion notifications
//!     let webhook_manager = WebhookManager::new(
//!         event_manager.clone(),
//!         Default::default()
//!     );
//!
//!     let webhook = WebhookConfig::new(
//!         "completion_hook".to_string(),
//!         "https://api.example.com/job-completed".to_string(),
//!     )
//!     .with_auth(WebhookAuth::Bearer {
//!         token: "your-api-token".to_string()
//!     })
//!     .with_filter(
//!         EventFilter::new()
//!             .with_event_types(vec![JobLifecycleEventType::Completed])
//!     );
//!
//!     webhook_manager.add_webhook(webhook).await?;
//!
//!     // Set up Kafka streaming for analytics
//!     let stream_manager = StreamManager::new_default(event_manager.clone());
//!
//!     let kafka_stream = StreamConfig {
//!         id: uuid::Uuid::new_v4(),
//!         name: "analytics_stream".to_string(),
//!         backend: StreamBackend::Kafka {
//!             brokers: vec!["localhost:9092".to_string()],
//!             topic: "job-events".to_string(),
//!             config: HashMap::new(),
//!         },
//!         filter: EventFilter::new().include_payload(),
//!         partitioning: PartitioningStrategy::QueueName,
//!         enabled: true,
//!         ..Default::default()
//!     };
//!
//!     stream_manager.add_stream(kafka_stream).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Configuration Management
//!
//! Hammerwork supports comprehensive configuration through TOML files and environment variables:
//!
//! ```rust,ignore
//! use hammerwork::config::HammerworkConfig;
//!
//! // Load from TOML file
//! let config = HammerworkConfig::from_file("hammerwork.toml")?;
//!
//! // Load from environment variables
//! let config = HammerworkConfig::from_env()?;
//!
//! // Create with builder pattern
//! let config = HammerworkConfig::new()
//!     .with_database_url("postgresql://localhost/hammerwork")
//!     .with_worker_pool_size(8)
//!     .with_events_enabled(true);
//!
//! # Ok::<(), hammerwork::HammerworkError>(())
//! ```
//!
//! ## CLI Integration
//!
//! Hammerwork includes a comprehensive CLI for operations:
//!
//! ```bash
//! # Database operations
//! cargo hammerwork migrate --database-url postgresql://localhost/hammerwork
//!
//! # Job management
//! cargo hammerwork job list --queue=email
//! cargo hammerwork job retry --job-id=abc123
//! cargo hammerwork job enqueue --queue=email --payload='{"to":"user@example.com"}'
//!
//! # Queue operations
//! cargo hammerwork queue stats --queue=email
//! cargo hammerwork queue clear --queue=test
//!
//! # Webhook management
//! cargo hammerwork webhook list
//! cargo hammerwork webhook test --webhook-id=abc123
//!
//! # Monitoring
//! cargo hammerwork monitor --tail --queue=email
//! ```
//!
//! ## Feature Flags
//!
//! - `postgres` - Enable PostgreSQL database support
//! - `mysql` - Enable MySQL database support
//! - `metrics` - Enable Prometheus metrics collection (default)
//! - `alerting` - Enable webhook/Slack/email alerting (default)
//! - `webhooks` - Enable webhook and event system features (default)
//! - `encryption` - Enable job payload encryption and PII protection
//! - `tracing` - Enable OpenTelemetry distributed tracing

pub mod archive;
pub mod batch;
pub mod config;
pub mod cron;
#[cfg(feature = "encryption")]
pub mod encryption;
pub mod error;
pub mod job;
pub mod migrations;
pub mod priority;
pub mod queue;
pub mod rate_limit;
pub mod retry;
pub mod spawn;
pub mod stats;
pub mod tracing;
pub mod worker;
pub mod workflow;

#[cfg(feature = "metrics")]
pub mod metrics;

#[cfg(feature = "alerting")]
pub mod alerting;

#[cfg(feature = "webhooks")]
pub mod events;

#[cfg(feature = "webhooks")]
pub mod webhooks;

pub mod streaming;

#[cfg(test)]
mod integration_tests;

pub use archive::{
    ArchivalConfig, ArchivalPolicy, ArchivalReason, ArchivalStats, ArchiveEvent, ArchivedJob,
    JobArchiver,
};
pub use batch::{BatchId, BatchResult, BatchStatus, JobBatch, PartialFailureMode};
pub use config::{
    ArchiveConfig, DatabaseConfig, HammerworkConfig, LoggingConfig, RateLimitingConfig,
    StreamingConfigs, StreamingGlobalSettings, WorkerConfig,
};
pub use streaming::StreamConfig;

#[cfg(feature = "webhooks")]
pub use config::{WebhookConfigs, WebhookGlobalSettings};
pub use cron::{CronError, CronSchedule};
#[cfg(feature = "encryption")]
pub use encryption::{
    EncryptedPayload, EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, EncryptionError,
    EncryptionKey, EncryptionMetadata, EncryptionStats, ExternalKmsConfig, KeyAuditRecord,
    KeyDerivationConfig, KeyManager, KeyManagerConfig, KeyManagerStats, KeyOperation, KeyPurpose,
    KeySource, KeyStatus, RetentionPolicy,
};
pub use error::HammerworkError;
pub use job::{Job, JobId, JobStatus, ResultConfig, ResultStorage};
pub use priority::{
    JobPriority, PriorityError, PrioritySelectionStrategy, PriorityStats, PriorityWeights,
};
pub use queue::JobQueue;
pub use rate_limit::{RateLimit, RateLimiter, ThrottleConfig};
pub use retry::{JitterType, RetryStrategy, fibonacci};
pub use spawn::{JobSpawnExt, SpawnConfig, SpawnHandler, SpawnManager, SpawnResult};
pub use stats::{
    DeadJobSummary, InMemoryStatsCollector, JobStatistics, QueueStats, StatisticsCollector,
};
#[cfg(feature = "webhooks")]
pub use webhooks::WebhookConfig;
pub use worker::{
    AutoscaleConfig, AutoscaleMetrics, BatchProcessingStats, JobEventHooks, JobHandler,
    JobHandlerWithResult, JobHookEvent, JobResult, Worker, WorkerPool,
};
pub use workflow::{DependencyStatus, FailurePolicy, JobGroup, WorkflowId, WorkflowStatus};

#[cfg(feature = "metrics")]
pub use metrics::{MetricsConfig, PrometheusMetricsCollector};

#[cfg(feature = "alerting")]
pub use alerting::{Alert, AlertManager, AlertSeverity, AlertTarget, AlertType, AlertingConfig};

#[cfg(feature = "webhooks")]
pub use events::{
    EventConfig, EventFilter, EventManager, EventManagerStats, EventSubscription, JobError,
    JobLifecycleEvent, JobLifecycleEventBuilder, JobLifecycleEventType,
};

#[cfg(feature = "webhooks")]
pub use webhooks::{
    HttpMethod, RetryPolicy, WebhookAuth, WebhookConfig as WebhookSettings, WebhookDelivery,
    WebhookManager, WebhookManagerConfig, WebhookManagerStats, WebhookStats,
};

pub use streaming::{
    BufferConfig, PartitionField, PartitioningStrategy, SerializationFormat, StreamBackend,
    StreamConfig as StreamSettings, StreamDelivery, StreamManager, StreamManagerConfig,
    StreamManagerGlobalStats, StreamProcessor, StreamRetryPolicy, StreamStats, StreamedEvent,
};

pub use tracing::{CorrelationId, TraceId};

#[cfg(feature = "tracing")]
pub use tracing::{
    TracingConfig, create_job_span, init_tracing, set_job_trace_context, shutdown_tracing,
};

/// Convenient type alias for Results with [`HammerworkError`] as the error type.
///
/// This is used throughout the crate for consistent error handling.
pub type Result<T> = std::result::Result<T, HammerworkError>;
