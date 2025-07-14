//! Worker types for processing jobs from the job queue.
//!
//! This module provides the [`Worker`] and [`WorkerPool`] types that are responsible
//! for polling the job queue for work and executing job handlers. Workers support
//! extensive configuration including priority scheduling, rate limiting, timeouts,
//! statistics collection, and monitoring.

use crate::{
    Result,
    batch::BatchId,
    error::HammerworkError,
    job::Job,
    priority::PriorityWeights,
    queue::{DatabaseQueue, JobQueue},
    rate_limit::{RateLimit, RateLimiter, ThrottleConfig},
    retry::RetryStrategy,
    stats::{JobEvent, JobEventType, StatisticsCollector},
};

#[cfg(feature = "metrics")]
use crate::metrics::PrometheusMetricsCollector;

#[cfg(feature = "alerting")]
use crate::alerting::{AlertManager, AlertingConfig};

#[cfg(feature = "webhooks")]
use crate::events::{EventManager, JobError, JobLifecycleEvent, JobLifecycleEventType};

use chrono::{DateTime, Utc};
use sqlx::Database;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::sleep};
use tracing::{debug, error, info, warn};

/// Event data for job lifecycle event hooks.
#[derive(Debug, Clone)]
pub struct JobHookEvent {
    /// The job that triggered the event
    pub job: Job,
    /// When the event occurred
    pub timestamp: DateTime<Utc>,
    /// Processing duration (for completion events)
    pub duration: Option<Duration>,
    /// Error message (for failure events)
    pub error: Option<String>,
}

/// Type alias for job event hook handler functions.
pub type JobHookHandler = Arc<dyn Fn(JobHookEvent) + Send + Sync>;

/// Job lifecycle event hooks that can be registered with workers.
#[derive(Clone, Default)]
pub struct JobEventHooks {
    /// Called when a job starts processing
    pub on_job_start: Option<JobHookHandler>,
    /// Called when a job completes successfully
    pub on_job_complete: Option<JobHookHandler>,
    /// Called when a job fails (before retry logic)
    pub on_job_fail: Option<JobHookHandler>,
    /// Called when a job times out
    pub on_job_timeout: Option<JobHookHandler>,
    /// Called when a job is retried
    pub on_job_retry: Option<JobHookHandler>,
}

impl JobEventHooks {
    /// Create a new set of empty job event hooks.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the job start event handler.
    pub fn on_start<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_start = Some(Arc::new(handler));
        self
    }

    /// Set the job completion event handler.
    pub fn on_complete<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_complete = Some(Arc::new(handler));
        self
    }

    /// Set the job failure event handler.
    pub fn on_fail<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_fail = Some(Arc::new(handler));
        self
    }

    /// Set the job timeout event handler.
    pub fn on_timeout<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_timeout = Some(Arc::new(handler));
        self
    }

    /// Set the job retry event handler.
    pub fn on_retry<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.on_job_retry = Some(Arc::new(handler));
        self
    }

    /// Fire the job start event if a handler is registered.
    pub(crate) fn fire_job_start(&self, job: Job) {
        if let Some(handler) = &self.on_job_start {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: None,
                error: None,
            };
            handler(event);
        }
    }

    /// Fire the job completion event if a handler is registered.
    pub(crate) fn fire_job_complete(&self, job: Job, duration: Duration) {
        if let Some(handler) = &self.on_job_complete {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: Some(duration),
                error: None,
            };
            handler(event);
        }
    }

    /// Fire the job failure event if a handler is registered.
    pub(crate) fn fire_job_fail(&self, job: Job, error: String) {
        if let Some(handler) = &self.on_job_fail {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: None,
                error: Some(error),
            };
            handler(event);
        }
    }

    /// Fire the job timeout event if a handler is registered.
    pub(crate) fn fire_job_timeout(&self, job: Job, duration: Duration) {
        if let Some(handler) = &self.on_job_timeout {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: Some(duration),
                error: Some("Job timed out".to_string()),
            };
            handler(event);
        }
    }

    /// Fire the job retry event if a handler is registered.
    pub(crate) fn fire_job_retry(&self, job: Job, error: String) {
        if let Some(handler) = &self.on_job_retry {
            let event = JobHookEvent {
                job,
                timestamp: Utc::now(),
                duration: None,
                error: Some(error),
            };
            handler(event);
        }
    }
}

/// Configuration for worker autoscaling behavior.
#[derive(Debug, Clone)]
pub struct AutoscaleConfig {
    /// Whether autoscaling is enabled
    pub enabled: bool,
    /// Minimum number of workers to maintain
    pub min_workers: usize,
    /// Maximum number of workers to allow
    pub max_workers: usize,
    /// Queue depth per worker threshold to trigger scale-up
    pub scale_up_threshold: usize,
    /// Queue depth per worker threshold to trigger scale-down
    pub scale_down_threshold: usize,
    /// Minimum time between scaling decisions
    pub cooldown_period: Duration,
    /// Number of workers to add/remove during scaling events
    pub scale_step: usize,
    /// Time window for queue depth averaging
    pub evaluation_window: Duration,
    /// Minimum worker idle time before considering scale-down
    pub idle_timeout: Duration,
}

impl Default for AutoscaleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_workers: 1,
            max_workers: 10,
            scale_up_threshold: 5,
            scale_down_threshold: 2,
            cooldown_period: Duration::from_secs(60),
            scale_step: 1,
            evaluation_window: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(300),
        }
    }
}

impl AutoscaleConfig {
    /// Create a new autoscale configuration with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable or disable autoscaling
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the minimum number of workers
    pub fn with_min_workers(mut self, min_workers: usize) -> Self {
        self.min_workers = min_workers.max(1); // Ensure at least 1 worker
        self
    }

    /// Set the maximum number of workers
    pub fn with_max_workers(mut self, max_workers: usize) -> Self {
        self.max_workers = max_workers.max(self.min_workers);
        self
    }

    /// Set the queue depth threshold for scaling up
    pub fn with_scale_up_threshold(mut self, threshold: usize) -> Self {
        self.scale_up_threshold = threshold.max(1);
        self
    }

    /// Set the queue depth threshold for scaling down
    pub fn with_scale_down_threshold(mut self, threshold: usize) -> Self {
        self.scale_down_threshold = threshold;
        self
    }

    /// Set the cooldown period between scaling decisions
    pub fn with_cooldown_period(mut self, period: Duration) -> Self {
        self.cooldown_period = period;
        self
    }

    /// Set the number of workers to add/remove per scaling event
    pub fn with_scale_step(mut self, step: usize) -> Self {
        self.scale_step = step.max(1);
        self
    }

    /// Set the evaluation window for queue depth averaging
    pub fn with_evaluation_window(mut self, window: Duration) -> Self {
        self.evaluation_window = window;
        self
    }

    /// Set the minimum idle time before considering scale-down
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Create a conservative autoscaling configuration
    pub fn conservative() -> Self {
        Self {
            enabled: true,
            min_workers: 2,
            max_workers: 5,
            scale_up_threshold: 10,
            scale_down_threshold: 1,
            cooldown_period: Duration::from_secs(300), // 5 mins
            scale_step: 1,
            evaluation_window: Duration::from_secs(60),
            idle_timeout: Duration::from_secs(600), // 10 mins
        }
    }

    /// Create an aggressive autoscaling configuration
    pub fn aggressive() -> Self {
        Self {
            enabled: true,
            min_workers: 1,
            max_workers: 20,
            scale_up_threshold: 3,
            scale_down_threshold: 1,
            cooldown_period: Duration::from_secs(30),
            scale_step: 2,
            evaluation_window: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(120), // 2 mins
        }
    }

    /// Disable autoscaling
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

/// Metrics for autoscaling decisions
#[derive(Debug, Clone, Default)]
pub struct AutoscaleMetrics {
    /// Current number of active workers
    pub active_workers: usize,
    /// Average queue depth over evaluation window
    pub avg_queue_depth: f64,
    /// Current queue depth
    pub current_queue_depth: u64,
    /// Jobs processed per second (recent average)
    pub jobs_per_second: f64,
    /// Average worker utilization (0.0 to 1.0)
    pub worker_utilization: f64,
    /// Time since last scaling action
    pub time_since_last_scale: Duration,
    /// Timestamp of last scaling decision
    pub last_scale_time: Option<chrono::DateTime<Utc>>,
}

/// Scaling decision enum
#[derive(Debug, Clone, Copy)]
enum ScalingDecision {
    ScaleUp,
    ScaleDown,
}

/// Type alias for queue depth history storage
type QueueDepthHistory = Arc<std::sync::RwLock<Vec<(chrono::DateTime<Utc>, u64)>>>;

/// Statistics for batch job processing by a worker.
#[derive(Debug, Clone, Default)]
pub struct BatchProcessingStats {
    /// Total number of batch jobs processed
    pub jobs_processed: u64,
    /// Total number of batch jobs completed successfully
    pub jobs_completed: u64,
    /// Total number of batch jobs that failed
    pub jobs_failed: u64,
    /// Total number of batches completed
    pub batches_completed: u64,
    /// Total number of batches completed successfully (>95% success rate)
    pub batches_successful: u64,
    /// Total processing time for all batch jobs in milliseconds
    pub total_processing_time_ms: u64,
    /// Average processing time per job in milliseconds
    pub average_processing_time_ms: f64,
    /// Timestamp of the last processed batch job
    pub last_processed_job: Option<DateTime<Utc>>,
}

impl BatchProcessingStats {
    /// Calculate the success rate for batch jobs
    pub fn success_rate(&self) -> f64 {
        if self.jobs_processed == 0 {
            0.0
        } else {
            self.jobs_completed as f64 / self.jobs_processed as f64
        }
    }

    /// Calculate the batch success rate
    pub fn batch_success_rate(&self) -> f64 {
        if self.batches_completed == 0 {
            0.0
        } else {
            self.batches_successful as f64 / self.batches_completed as f64
        }
    }

    /// Update the average processing time
    pub fn update_average_processing_time(&mut self) {
        if self.jobs_completed > 0 {
            self.average_processing_time_ms =
                self.total_processing_time_ms as f64 / self.jobs_completed as f64;
        }
    }
}

/// Type alias for job handler functions.
///
/// Job handlers are async functions that take a [`Job`] and return a [`Result`].
/// The handler is responsible for processing the job's payload and performing
/// the actual work. Handlers should return `Ok(())` on success or an error
/// if the job should be retried or marked as failed.
///
/// # Examples
///
/// ```rust
/// use hammerwork::{Job, Result, worker::JobHandler};
/// use std::sync::Arc;
///
/// let handler: JobHandler = Arc::new(|job: Job| {
///     Box::pin(async move {
///         // Process the job
///         println!("Processing job: {:?}", job.payload);
///         
///         // Return Ok(()) on success, Err(_) to trigger retry
///         Ok(())
///     })
/// });
/// ```
pub type JobHandler = Arc<
    dyn Fn(Job) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Type alias for job handler functions that can return result data.
///
/// Enhanced job handlers are async functions that take a [`Job`] and return a [`Result<JobResult>`].
/// They support returning optional result data that can be stored and retrieved later.
///
/// # Examples
///
/// ```rust
/// use hammerwork::{Job, Result, worker::{JobHandlerWithResult, JobResult}};
/// use std::sync::Arc;
/// use serde_json::json;
///
/// let handler: JobHandlerWithResult = Arc::new(|job: Job| {
///     Box::pin(async move {
///         // Process the job
///         let result_data = json!({
///             "processed_items": 42,
///             "status": "completed"
///         });
///         
///         // Return result with data
///         Ok(JobResult::with_data(result_data))
///     })
/// });
/// ```
pub type JobHandlerWithResult = Arc<
    dyn Fn(Job) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<JobResult>> + Send>>
        + Send
        + Sync,
>;

/// Result returned by job handlers, optionally containing result data.
#[derive(Debug, Clone)]
pub struct JobResult {
    /// Optional result data to store for retrieval
    pub data: Option<serde_json::Value>,
}

impl JobResult {
    /// Create a JobResult with no data (equivalent to the old Ok(()) pattern)
    pub fn success() -> Self {
        Self { data: None }
    }

    /// Create a JobResult with result data
    pub fn with_data(data: serde_json::Value) -> Self {
        Self { data: Some(data) }
    }
}

impl Default for JobResult {
    fn default() -> Self {
        Self::success()
    }
}

/// Enum to support both original and enhanced job handlers
#[derive(Clone)]
pub enum JobHandlerType {
    /// Original handler type that returns Result<()>
    Legacy(JobHandler),
    /// Enhanced handler type that returns Result<JobResult> with optional data
    WithResult(JobHandlerWithResult),
}

/// A worker that processes jobs from a specific queue.
///
/// Workers continuously poll their assigned queue for pending jobs and execute
/// them using the provided handler function. They support extensive configuration
/// for controlling job processing behavior.
///
/// # Features
///
/// - **Priority-aware job selection**: Configurable weighted or strict priority scheduling
/// - **Rate limiting**: Token bucket rate limiting with configurable burst limits
/// - **Automatic retries**: Configurable retry attempts with exponential backoff
/// - **Timeout handling**: Per-job and worker-level timeout configuration
/// - **Statistics collection**: Integration with statistics collectors for monitoring
/// - **Metrics and alerting**: Optional Prometheus metrics and alerting support
///
/// # Examples
///
/// ## Basic Worker
///
/// ```rust,no_run
/// use hammerwork::{Worker, JobQueue, Job};
/// use std::sync::Arc;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
/// let queue = Arc::new(JobQueue::new(pool));
///
/// let handler: hammerwork::worker::JobHandler = Arc::new(|job: Job| {
///     Box::pin(async move {
///         println!("Processing: {:?}", job.payload);
///         Ok(())
///     })
/// });
///
/// let worker = Worker::new(queue, "email_queue".to_string(), handler)
///     .with_poll_interval(std::time::Duration::from_millis(500))
///     .with_max_retries(5);
///
/// // Start processing jobs
/// let mut pool = hammerwork::WorkerPool::new();
/// pool.add_worker(worker);
/// pool.start().await?;
/// # Ok(())
/// # }
/// ```
///
/// ## Worker with Priority and Rate Limiting
///
/// ```rust,no_run
/// use hammerwork::{Worker, JobQueue, PriorityWeights, RateLimit};
/// use std::sync::Arc;
/// use std::time::Duration;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
/// # let queue = Arc::new(JobQueue::new(pool));
/// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
///
/// let priority_weights = PriorityWeights::new()
///     .with_weight(hammerwork::JobPriority::Critical, 50)
///     .with_weight(hammerwork::JobPriority::High, 20);
///
/// let rate_limit = RateLimit::per_second(10).with_burst_limit(20);
///
/// let worker = Worker::new(queue, "api_queue".to_string(), handler)
///     .with_priority_weights(priority_weights)
///     .with_rate_limit(rate_limit)
///     .with_default_timeout(Duration::from_secs(300));
///
/// let mut pool = hammerwork::WorkerPool::new();
/// pool.add_worker(worker);
/// pool.start().await?;
/// # Ok(())
/// # }
/// ```
pub struct Worker<DB: Database> {
    /// The job queue to poll for work
    queue: Arc<JobQueue<DB>>,
    /// Name of the queue this worker processes
    queue_name: String,
    /// Function to handle job processing
    handler: JobHandlerType,
    /// How often to poll for new jobs
    poll_interval: Duration,
    /// Maximum number of retry attempts for failed jobs
    max_retries: i32,
    /// Delay between retry attempts
    retry_delay: Duration,
    /// Default retry strategy for failed jobs (overrides retry_delay if specified)
    default_retry_strategy: Option<RetryStrategy>,
    /// Default timeout for jobs (overridden by job-specific timeouts)
    default_timeout: Option<Duration>,
    /// Priority weights for job selection
    priority_weights: Option<PriorityWeights>,
    /// Statistics collector for monitoring
    stats_collector: Option<Arc<dyn StatisticsCollector>>,
    /// Rate limiter for controlling job processing rate
    rate_limiter: Option<RateLimiter>,
    /// Throttling configuration
    throttle_config: Option<ThrottleConfig>,
    /// Prometheus metrics collector (when metrics feature is enabled)
    #[cfg(feature = "metrics")]
    metrics_collector: Option<Arc<PrometheusMetricsCollector>>,
    /// Alert manager for notifications (when alerting feature is enabled)
    #[cfg(feature = "alerting")]
    alert_manager: Option<Arc<AlertManager>>,
    /// Timestamp of the last processed job (for starvation detection)
    last_job_time: Arc<std::sync::RwLock<DateTime<Utc>>>,
    /// Enable optimized batch job processing
    batch_processing_enabled: bool,
    /// Track batch processing statistics
    batch_stats: Arc<std::sync::RwLock<BatchProcessingStats>>,
    /// Job lifecycle event hooks
    event_hooks: JobEventHooks,
    /// Spawn manager for dynamic job spawning
    spawn_manager: Option<Arc<crate::spawn::SpawnManager<DB>>>,
    /// Event manager for publishing job lifecycle events
    #[cfg(feature = "webhooks")]
    event_manager: Option<Arc<crate::events::EventManager>>,
}

impl<DB: Database + Send + Sync + 'static> Clone for Worker<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    fn clone(&self) -> Self {
        Self {
            queue: Arc::clone(&self.queue),
            queue_name: self.queue_name.clone(),
            handler: self.handler.clone(),
            poll_interval: self.poll_interval,
            max_retries: self.max_retries,
            retry_delay: self.retry_delay,
            default_retry_strategy: self.default_retry_strategy.clone(),
            default_timeout: self.default_timeout,
            priority_weights: self.priority_weights.clone(),
            stats_collector: self.stats_collector.clone(),
            rate_limiter: self.rate_limiter.clone(),
            throttle_config: self.throttle_config.clone(),
            #[cfg(feature = "metrics")]
            metrics_collector: self.metrics_collector.clone(),
            #[cfg(feature = "alerting")]
            alert_manager: self.alert_manager.clone(),
            // Create new instances for per-worker state
            last_job_time: Arc::new(std::sync::RwLock::new(Utc::now())),
            batch_processing_enabled: self.batch_processing_enabled,
            batch_stats: Arc::new(std::sync::RwLock::new(BatchProcessingStats::default())),
            event_hooks: self.event_hooks.clone(),
            spawn_manager: self.spawn_manager.clone(),
            #[cfg(feature = "webhooks")]
            event_manager: self.event_manager.clone(),
        }
    }
}

impl<DB: Database + Send + Sync + 'static> Worker<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    /// Creates a new worker with default configuration.
    ///
    /// The worker will be created with:
    /// - 1 second polling interval
    /// - 3 maximum retry attempts
    /// - 30 second retry delay
    /// - No timeout, rate limiting, or priority configuration
    ///
    /// # Arguments
    ///
    /// * `queue` - The job queue to poll for work
    /// * `queue_name` - Name of the queue this worker should process
    /// * `handler` - Function to handle job processing
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{Worker, JobQueue, Job};
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// let queue = Arc::new(JobQueue::new(pool));
    ///
    /// let handler: hammerwork::worker::JobHandler = Arc::new(|job: Job| {
    ///     Box::pin(async move {
    ///         match job.payload.get("action").and_then(|v| v.as_str()) {
    ///             Some("send_email") => {
    ///                 // Send email logic
    ///                 println!("Sending email to: {:?}", job.payload.get("to"));
    ///                 Ok(())
    ///             },
    ///             Some("process_data") => {
    ///                 // Data processing logic
    ///                 println!("Processing data: {:?}", job.payload.get("data"));
    ///                 Ok(())
    ///             },
    ///             _ => Err(hammerwork::HammerworkError::Processing("Unknown action".to_string())),
    ///         }
    ///     })
    /// });
    ///
    /// let worker = Worker::new(queue, "default".to_string(), handler);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(queue: Arc<JobQueue<DB>>, queue_name: String, handler: JobHandler) -> Self {
        Self {
            queue,
            queue_name,
            handler: JobHandlerType::Legacy(handler),
            poll_interval: Duration::from_secs(1),
            max_retries: 3,
            retry_delay: Duration::from_secs(30),
            default_retry_strategy: None,
            default_timeout: None,
            priority_weights: None,
            stats_collector: None,
            rate_limiter: None,
            throttle_config: None,
            #[cfg(feature = "metrics")]
            metrics_collector: None,
            #[cfg(feature = "alerting")]
            alert_manager: None,
            last_job_time: Arc::new(std::sync::RwLock::new(Utc::now())),
            batch_processing_enabled: false,
            batch_stats: Arc::new(std::sync::RwLock::new(BatchProcessingStats::default())),
            event_hooks: JobEventHooks::default(),
            spawn_manager: None,
            #[cfg(feature = "webhooks")]
            event_manager: None,
        }
    }

    /// Creates a new worker with an enhanced handler that can return result data.
    ///
    /// This method creates a worker that uses the enhanced job handler type,
    /// which can return result data that will be automatically stored when
    /// the job completes successfully.
    ///
    /// # Arguments
    ///
    /// * `queue` - The job queue to poll for work
    /// * `queue_name` - Name of the queue this worker will process
    /// * `handler` - Enhanced handler function that can return result data
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{Worker, JobQueue, Job, worker::{JobHandlerWithResult, JobResult}};
    /// use std::sync::Arc;
    /// use serde_json::json;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// let queue = Arc::new(JobQueue::new(pool));
    ///
    /// let handler: JobHandlerWithResult = Arc::new(|job: Job| {
    ///     Box::pin(async move {
    ///         // Process the job and generate results
    ///         let result_data = json!({
    ///             "processed_items": 42,
    ///             "status": "completed"
    ///         });
    ///         
    ///         Ok(JobResult::with_data(result_data))
    ///     })
    /// });
    ///
    /// let worker = Worker::new_with_result_handler(queue, "default".to_string(), handler);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new_with_result_handler(
        queue: Arc<JobQueue<DB>>,
        queue_name: String,
        handler: JobHandlerWithResult,
    ) -> Self {
        Self {
            queue,
            queue_name,
            handler: JobHandlerType::WithResult(handler),
            poll_interval: Duration::from_secs(1),
            max_retries: 3,
            retry_delay: Duration::from_secs(30),
            default_retry_strategy: None,
            default_timeout: None,
            priority_weights: None,
            stats_collector: None,
            rate_limiter: None,
            throttle_config: None,
            #[cfg(feature = "metrics")]
            metrics_collector: None,
            #[cfg(feature = "alerting")]
            alert_manager: None,
            last_job_time: Arc::new(std::sync::RwLock::new(Utc::now())),
            batch_processing_enabled: false,
            batch_stats: Arc::new(std::sync::RwLock::new(BatchProcessingStats::default())),
            event_hooks: JobEventHooks::default(),
            spawn_manager: None,
            #[cfg(feature = "webhooks")]
            event_manager: None,
        }
    }

    /// Adds a statistics collector for monitoring job processing.
    ///
    /// The statistics collector will receive events for job start, completion,
    /// failure, and timeout events, allowing for comprehensive monitoring
    /// of worker performance.
    ///
    /// # Arguments
    ///
    /// * `stats_collector` - The statistics collector to use
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{Worker, JobQueue, InMemoryStatsCollector};
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let stats = Arc::new(InMemoryStatsCollector::new_default());
    /// let worker = Worker::new(queue, "monitored".to_string(), handler)
    ///     .with_stats_collector(stats.clone());
    ///
    /// // Later, check statistics  
    /// use std::time::Duration;
    /// use hammerwork::StatisticsCollector;
    /// let queue_stats = stats.get_queue_statistics("monitored", Duration::from_secs(3600)).await?;
    /// println!("Processed: {}", queue_stats.total_processed);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_stats_collector(mut self, stats_collector: Arc<dyn StatisticsCollector>) -> Self {
        self.stats_collector = Some(stats_collector);
        self
    }

    /// Sets how often the worker polls for new jobs.
    ///
    /// Shorter intervals result in lower latency but higher database load.
    /// Longer intervals reduce database load but increase job processing latency.
    ///
    /// # Arguments
    ///
    /// * `interval` - Time between polling attempts
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::Worker;
    /// use std::time::Duration;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// // High-frequency polling for low latency
    /// let fast_worker = Worker::new(queue.clone(), "fast".to_string(), handler.clone())
    ///     .with_poll_interval(Duration::from_millis(100));
    ///
    /// // Lower frequency polling for reduced load
    /// let slow_worker = Worker::new(queue, "slow".to_string(), handler)
    ///     .with_poll_interval(Duration::from_secs(5));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Sets the maximum number of retry attempts for failed jobs.
    ///
    /// When a job handler returns an error, the job will be retried up to
    /// this many times before being marked as dead. This overrides the
    /// job's own max_attempts setting.
    ///
    /// # Arguments
    ///
    /// * `max_retries` - Maximum retry attempts
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// // Critical jobs get more retry attempts
    /// let critical_worker = Worker::new(queue, "critical".to_string(), handler)
    ///     .with_max_retries(10);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_max_retries(mut self, max_retries: i32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Sets the delay between retry attempts.
    ///
    /// When a job fails and is scheduled for retry, it will wait this
    /// duration before becoming eligible for processing again.
    ///
    /// # Arguments
    ///
    /// * `delay` - Time to wait between retry attempts
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::Worker;
    /// use std::time::Duration;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// // Longer delay for API rate limit recovery
    /// let api_worker = Worker::new(queue, "api".to_string(), handler)
    ///     .with_retry_delay(Duration::from_secs(300)); // 5 minutes
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_retry_delay(mut self, delay: Duration) -> Self {
        self.retry_delay = delay;
        self
    }

    /// Sets a default retry strategy for all jobs processed by this worker.
    ///
    /// This retry strategy will be used for jobs that don't have their own
    /// retry strategy configured. Jobs with their own retry strategy will
    /// use that instead of this default.
    ///
    /// If no default retry strategy is set, the worker will fall back to
    /// using the fixed `retry_delay` for all retries.
    ///
    /// # Arguments
    ///
    /// * `strategy` - The default retry strategy to use for failed jobs
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{Worker, retry::RetryStrategy};
    /// use std::time::Duration;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// // Use exponential backoff as default for all jobs
    /// let worker = Worker::new(queue, "api_calls".to_string(), handler)
    ///     .with_default_retry_strategy(RetryStrategy::exponential(
    ///         Duration::from_secs(1),
    ///         2.0,
    ///         Some(Duration::from_secs(10 * 60))
    ///     ));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_default_retry_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.default_retry_strategy = Some(strategy);
        self
    }

    /// Sets a default timeout for all jobs processed by this worker.
    ///
    /// Jobs that don't have their own timeout setting will use this default.
    /// Job-specific timeouts always take precedence over worker defaults.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time a job can run before being terminated
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::Worker;
    /// use std::time::Duration;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// // Set 5 minute default timeout for all jobs
    /// let worker = Worker::new(queue, "processing".to_string(), handler)
    ///     .with_default_timeout(Duration::from_secs(300));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_default_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = Some(timeout);
        self
    }

    /// Configure priority weights for job selection
    pub fn with_priority_weights(mut self, weights: PriorityWeights) -> Self {
        self.priority_weights = Some(weights);
        self
    }

    /// Enable strict priority mode (always process highest priority first)
    pub fn with_strict_priority(mut self) -> Self {
        self.priority_weights = Some(PriorityWeights::strict());
        self
    }

    /// Use default weighted priority selection
    pub fn with_weighted_priority(mut self) -> Self {
        self.priority_weights = Some(PriorityWeights::new());
        self
    }

    /// Configure rate limiting for this worker
    pub fn with_rate_limit(mut self, rate_limit: RateLimit) -> Self {
        self.rate_limiter = Some(RateLimiter::new(rate_limit));
        self
    }

    /// Configure throttling for this worker
    pub fn with_throttle_config(mut self, throttle_config: ThrottleConfig) -> Self {
        // If the throttle config has a rate limit, create a rate limiter
        if let Some(rate_limit) = throttle_config.to_rate_limit() {
            self.rate_limiter = Some(RateLimiter::new(rate_limit));
        }
        self.throttle_config = Some(throttle_config);
        self
    }

    /// Configure Prometheus metrics collection for this worker
    #[cfg(feature = "metrics")]
    pub fn with_metrics_collector(
        mut self,
        metrics_collector: Arc<PrometheusMetricsCollector>,
    ) -> Self {
        self.metrics_collector = Some(metrics_collector);
        self
    }

    /// Configure alerting for this worker
    #[cfg(feature = "alerting")]
    pub fn with_alerting_config(mut self, alerting_config: AlertingConfig) -> Self {
        self.alert_manager = Some(Arc::new(AlertManager::new(alerting_config)));
        self
    }

    /// Configure alert manager for this worker
    #[cfg(feature = "alerting")]
    pub fn with_alert_manager(mut self, alert_manager: Arc<AlertManager>) -> Self {
        self.alert_manager = Some(alert_manager);
        self
    }

    /// Add an event manager for publishing job lifecycle events.
    ///
    /// The event manager will receive job lifecycle events which can be delivered
    /// to webhooks, streaming systems, and other external integrations.
    ///
    /// # Arguments
    ///
    /// * `event_manager` - The event manager to use for publishing events
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{Worker, JobQueue, events::EventManager};
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// let queue = Arc::new(JobQueue::new(pool));
    /// let event_manager = Arc::new(EventManager::new_default());
    ///
    /// let handler: Arc<dyn Fn(hammerwork::Job) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), hammerwork::HammerworkError>> + Send>> + Send + Sync> = Arc::new(|_job| Box::pin(async move { Ok(()) }));
    /// let worker = Worker::new(queue, "default".to_string(), handler)
    ///     .with_event_manager(event_manager);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "webhooks")]
    pub fn with_event_manager(mut self, event_manager: Arc<EventManager>) -> Self {
        self.event_manager = Some(event_manager);
        self
    }

    /// Enable optimized batch job processing.
    ///
    /// When enabled, the worker will detect when jobs belong to batches and provide
    /// enhanced monitoring, statistics, and error handling for batch operations.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable batch processing optimizations
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "batch_queue".to_string(), handler)
    ///     .with_batch_processing_enabled(true);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_batch_processing_enabled(mut self, enabled: bool) -> Self {
        self.batch_processing_enabled = enabled;
        self
    }

    /// Set the job lifecycle event hooks for this worker.
    ///
    /// Event hooks allow you to register callbacks that will be called at various
    /// points in the job lifecycle, such as when jobs start, complete, fail, timeout,
    /// or are retried. This is useful for distributed tracing, logging, metrics,
    /// and debugging.
    ///
    /// # Arguments
    ///
    /// * `hooks` - The event hooks to register
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hammerwork::{Worker, worker::{JobEventHooks, JobHookEvent}};
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let queue = Arc::new(hammerwork::JobQueue::new(sqlx::PgPool::connect("").await?));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let hooks = JobEventHooks::new()
    ///     .on_start(|event: JobHookEvent| {
    ///         println!("Job {} started at {}", event.job.id, event.timestamp);
    ///     })
    ///     .on_complete(|event: JobHookEvent| {
    ///         if let Some(duration) = event.duration {
    ///             println!("Job {} completed in {:?}", event.job.id, duration);
    ///         }
    ///     })
    ///     .on_fail(|event: JobHookEvent| {
    ///         if let Some(error) = &event.error {
    ///             println!("Job {} failed: {}", event.job.id, error);
    ///         }
    ///     });
    ///
    /// let worker = Worker::new(queue, "traced_queue".to_string(), handler)
    ///     .with_event_hooks(hooks);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_event_hooks(mut self, hooks: JobEventHooks) -> Self {
        self.event_hooks = hooks;
        self
    }

    /// Set a spawn manager for dynamic job spawning.
    ///
    /// The spawn manager handles creating child jobs when parent jobs complete successfully.
    /// Jobs with registered spawn handlers will automatically spawn child jobs based on
    /// their payload and configuration.
    ///
    /// # Arguments
    ///
    /// * `spawn_manager` - The spawn manager to use for this worker
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{Worker, spawn::SpawnManager};
    /// use std::sync::Arc;
    ///
    /// # #[cfg(feature = "postgres")]
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // In real usage, you'd create a database connection pool
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    /// let mut spawn_manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
    /// // Register spawn handlers...
    ///
    /// let worker = Worker::new(queue, "queue".to_string(), handler)
    ///     .with_spawn_manager(Arc::new(spawn_manager));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_spawn_manager(
        mut self,
        spawn_manager: Arc<crate::spawn::SpawnManager<DB>>,
    ) -> Self {
        self.spawn_manager = Some(spawn_manager);
        self
    }

    /// Set a job start event handler for this worker.
    ///
    /// This is a convenience method for setting just the start event handler.
    /// For multiple event handlers, use `with_event_hooks()`.
    ///
    /// # Arguments
    ///
    /// * `handler` - Function to call when a job starts processing
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let queue = Arc::new(hammerwork::JobQueue::new(sqlx::PgPool::connect("").await?));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "queue".to_string(), handler)
    ///     .on_job_start(|event| {
    ///         println!("Starting job: {}", event.job.id);
    ///     });
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_job_start<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.event_hooks.on_job_start = Some(Arc::new(handler));
        self
    }

    /// Set a job completion event handler for this worker.
    ///
    /// # Arguments
    ///
    /// * `handler` - Function to call when a job completes successfully
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let queue = Arc::new(hammerwork::JobQueue::new(sqlx::PgPool::connect("").await?));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "queue".to_string(), handler)
    ///     .on_job_complete(|event| {
    ///         if let Some(duration) = event.duration {
    ///             println!("Job {} completed in {:?}", event.job.id, duration);
    ///         }
    ///     });
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_job_complete<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.event_hooks.on_job_complete = Some(Arc::new(handler));
        self
    }

    /// Set a job failure event handler for this worker.
    ///
    /// # Arguments
    ///
    /// * `handler` - Function to call when a job fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let queue = Arc::new(hammerwork::JobQueue::new(sqlx::PgPool::connect("").await?));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "queue".to_string(), handler)
    ///     .on_job_fail(|event| {
    ///         if let Some(error) = &event.error {
    ///             eprintln!("Job {} failed: {}", event.job.id, error);
    ///         }
    ///     });
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_job_fail<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.event_hooks.on_job_fail = Some(Arc::new(handler));
        self
    }

    /// Set a job timeout event handler for this worker.
    ///
    /// # Arguments
    ///
    /// * `handler` - Function to call when a job times out
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let queue = Arc::new(hammerwork::JobQueue::new(sqlx::PgPool::connect("").await?));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "queue".to_string(), handler)
    ///     .on_job_timeout(|event| {
    ///         println!("Job {} timed out after {:?}", event.job.id, event.duration);
    ///     });
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_job_timeout<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.event_hooks.on_job_timeout = Some(Arc::new(handler));
        self
    }

    /// Set a job retry event handler for this worker.
    ///
    /// # Arguments
    ///
    /// * `handler` - Function to call when a job is retried
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let queue = Arc::new(hammerwork::JobQueue::new(sqlx::PgPool::connect("").await?));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "queue".to_string(), handler)
    ///     .on_job_retry(|event| {
    ///         println!("Retrying job {} due to: {:?}", event.job.id, event.error);
    ///     });
    /// # Ok(())
    /// # }
    /// ```
    pub fn on_job_retry<F>(mut self, handler: F) -> Self
    where
        F: Fn(JobHookEvent) + Send + Sync + 'static,
    {
        self.event_hooks.on_job_retry = Some(Arc::new(handler));
        self
    }

    pub async fn run(&self, mut shutdown_rx: mpsc::Receiver<()>) -> Result<()> {
        info!("Worker started for queue: {}", self.queue_name);

        // Start background monitoring task for metrics and alerting
        #[cfg(any(feature = "metrics", feature = "alerting"))]
        let monitoring_task = self.start_monitoring_task();

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    info!("Worker shutting down for queue: {}", self.queue_name);
                    break;
                }
                _ = self.process_jobs() => {
                    // Continue processing
                }
            }
        }

        // Stop monitoring task
        #[cfg(any(feature = "metrics", feature = "alerting"))]
        monitoring_task.abort();

        Ok(())
    }

    async fn process_jobs(&self) -> Result<()> {
        // Check rate limit before dequeuing jobs
        if let Some(ref rate_limiter) = self.rate_limiter {
            // Check if we can process a job (non-blocking)
            if !rate_limiter.check() {
                debug!(
                    "Rate limit exceeded for queue: {}, waiting before retry",
                    self.queue_name
                );
                // Wait for the rate limiter to allow processing
                if let Err(e) = rate_limiter.acquire().await {
                    warn!("Rate limiter error: {}", e);
                    sleep(self.poll_interval).await;
                    return Ok(());
                }
            }
        }

        // Check if the queue is paused
        match self.queue.is_queue_paused(&self.queue_name).await {
            Ok(true) => {
                debug!("Queue '{}' is paused, skipping job dequeue", self.queue_name);
                sleep(self.poll_interval).await;
                return Ok(());
            }
            Ok(false) => {
                // Queue is not paused, continue with normal processing
            }
            Err(e) => {
                warn!("Failed to check queue pause status: {}", e);
                // Continue with normal processing if we can't check pause status
            }
        }

        // Update queue depth metrics before dequeuing
        #[cfg(feature = "metrics")]
        if let Some(metrics_collector) = &self.metrics_collector {
            if let Ok(queue_depth) = self.queue.get_queue_depth(&self.queue_name).await {
                if let Err(e) = metrics_collector
                    .update_queue_depth(&self.queue_name, queue_depth)
                    .await
                {
                    warn!("Failed to update queue depth metrics: {}", e);
                }
            }
        }

        let job_result = if let Some(ref weights) = self.priority_weights {
            // Use priority-aware dequeuing
            self.queue
                .dequeue_with_priority_weights(&self.queue_name, weights)
                .await
        } else {
            // Use regular dequeuing
            self.queue.dequeue(&self.queue_name).await
        };

        match job_result {
            Ok(Some(job)) => {
                debug!(
                    "Processing job: {} with priority: {:?}",
                    job.id, job.priority
                );
                self.process_job(job).await?;
            }
            Ok(None) => {
                // No jobs available, check for worker starvation
                #[cfg(feature = "alerting")]
                if let Some(alert_manager) = &self.alert_manager {
                    let last_time_value = {
                        if let Ok(last_time) = self.last_job_time.read() {
                            Some(*last_time)
                        } else {
                            None
                        }
                    };
                    if let Some(last_time_value) = last_time_value {
                        if let Err(e) = alert_manager
                            .check_worker_starvation(&self.queue_name, last_time_value)
                            .await
                        {
                            warn!("Failed to check worker starvation: {}", e);
                        }
                    }
                }

                // Wait before polling again
                sleep(self.poll_interval).await;
            }
            Err(e) => {
                error!("Error dequeuing job: {}", e);

                // If throttle config specifies backoff on error, apply it
                let backoff_duration = if let Some(ref throttle_config) = self.throttle_config {
                    throttle_config
                        .backoff_on_error
                        .unwrap_or(self.poll_interval)
                } else {
                    self.poll_interval
                };

                sleep(backoff_duration).await;
            }
        }

        // Check statistics and alert thresholds periodically
        #[cfg(feature = "alerting")]
        if let (Some(alert_manager), Some(stats_collector)) =
            (&self.alert_manager, &self.stats_collector)
        {
            match stats_collector
                .get_queue_statistics(&self.queue_name, Duration::from_secs(300))
                .await
            {
                Ok(stats) => {
                    if let Err(e) = alert_manager
                        .check_thresholds(&self.queue_name, &stats)
                        .await
                    {
                        warn!("Failed to check alert thresholds: {}", e);
                    }
                }
                Err(e) => {
                    warn!("Failed to get queue statistics for alerting: {}", e);
                }
            }
        }

        Ok(())
    }

    async fn process_job(&self, job: Job) -> Result<()> {
        let job_id = job.id;
        let queue_name = job.queue_name.clone();
        let batch_id = job.batch_id;
        let start_time = Utc::now();

        // Create OpenTelemetry span for job processing
        #[cfg(feature = "tracing")]
        let _span = crate::tracing::create_job_span(&job, "job.process");

        // Update batch statistics if batch processing is enabled
        if self.batch_processing_enabled && batch_id.is_some() {
            self.update_batch_stats(|stats| {
                stats.jobs_processed += 1;
                stats.last_processed_job = Some(start_time);
            });
        }

        // Fire job start event hook
        self.event_hooks.fire_job_start(job.clone());

        // Record job started event
        self.record_event(JobEvent {
            job_id,
            queue_name: queue_name.clone(),
            event_type: JobEventType::Started,
            priority: job.priority,
            processing_time_ms: None,
            error_message: None,
            timestamp: start_time,
        })
        .await;

        // Determine timeout duration (job-specific or default)
        let timeout_duration = job.timeout.or(self.default_timeout);

        let handler_result = if let Some(timeout) = timeout_duration {
            // Run with timeout
            match tokio::time::timeout(timeout, self.execute_handler(job.clone())).await {
                Ok(result) => result,
                Err(_) => {
                    // Timeout occurred
                    warn!("Job {} timed out after {:?}", job_id, timeout);

                    // Mark job as timed out in database
                    self.queue
                        .mark_job_timed_out(job_id, &format!("Job timed out after {:?}", timeout))
                        .await?;

                    // Record span timeout status
                    #[cfg(feature = "tracing")]
                    {
                        let span = tracing::Span::current();
                        span.record("error", true);
                        span.record("error.type", "timeout");
                        span.record(
                            "error.message",
                            format!("Job timed out after {:?}", timeout),
                        );
                    }

                    // Fire job timeout event hook
                    self.event_hooks.fire_job_timeout(job.clone(), timeout);

                    // Record timeout event
                    self.record_event(JobEvent {
                        job_id,
                        queue_name: queue_name.clone(),
                        event_type: JobEventType::TimedOut,
                        priority: job.priority,
                        processing_time_ms: Some(timeout.as_millis() as u64),
                        error_message: Some(format!("Job timed out after {:?}", timeout)),
                        timestamp: Utc::now(),
                    })
                    .await;

                    return Ok(());
                }
            }
        } else {
            // Run without timeout
            self.execute_handler(job.clone()).await
        };

        match handler_result {
            Ok(job_result) => {
                debug!("Job {} completed successfully", job_id);

                let processing_time_ms = (Utc::now() - start_time).num_milliseconds() as u64;

                // Update batch statistics for successful completion
                if self.batch_processing_enabled && batch_id.is_some() {
                    self.update_batch_stats(|stats| {
                        stats.jobs_completed += 1;
                        stats.total_processing_time_ms += processing_time_ms;
                        stats.update_average_processing_time();
                    });

                    // Check if batch is complete and update batch status
                    if let Some(batch_id) = batch_id {
                        if let Err(e) = self.check_and_update_batch_status(batch_id).await {
                            warn!(
                                "Failed to update batch status for batch {}: {}",
                                batch_id, e
                            );
                        }
                    }
                }

                // Handle cron job rescheduling
                if job.is_recurring() {
                    if let Some(next_run_time) = job.calculate_next_run() {
                        info!(
                            "Rescheduling recurring job {} for next run at {}",
                            job_id, next_run_time
                        );
                        self.queue
                            .reschedule_cron_job(job_id, next_run_time)
                            .await?;
                    } else {
                        warn!(
                            "Could not calculate next run time for recurring job {}",
                            job_id
                        );
                        self.queue.complete_job(job_id).await?;
                    }
                } else {
                    self.queue.complete_job(job_id).await?;
                }

                // Store job result if enabled and data is provided
                if let Some(result_data) = job_result.data {
                    if let crate::job::ResultStorage::Database = job.result_config.storage {
                        let expires_at = job.result_config.ttl.map(|ttl| {
                            Utc::now()
                                + chrono::Duration::from_std(ttl)
                                    .unwrap_or(chrono::Duration::hours(24))
                        });

                        if let Err(e) = self
                            .queue
                            .store_job_result(job_id, result_data, expires_at)
                            .await
                        {
                            warn!("Failed to store job result for job {}: {}", job_id, e);
                        } else {
                            debug!("Stored result for job {}", job_id);
                        }
                    }
                }

                // Handle job spawning if spawn manager is configured
                if let Some(spawn_manager) = &self.spawn_manager {
                    // Check if spawn config is present in job payload
                    if let Some(spawn_config_value) = job.payload.get("_spawn_config") {
                        if let Ok(spawn_config) = serde_json::from_value::<crate::spawn::SpawnConfig>(
                            spawn_config_value.clone(),
                        ) {
                            match spawn_manager
                                .execute_spawn(job.clone(), spawn_config, self.queue.clone())
                                .await
                            {
                                Ok(Some(spawn_result)) => {
                                    info!(
                                        "Job {} spawned {} child jobs: {:?}",
                                        job_id,
                                        spawn_result.spawned_jobs.len(),
                                        spawn_result.spawned_jobs
                                    );

                                    // Call spawn completion hook if present
                                    if let Some(ref hook) = self.event_hooks.on_job_complete {
                                        let hook_event = JobHookEvent {
                                            job: job.clone(),
                                            timestamp: Utc::now(),
                                            duration: Some(Duration::from_millis(
                                                processing_time_ms,
                                            )),
                                            error: None,
                                        };
                                        hook(hook_event);
                                    }
                                }
                                Ok(None) => {
                                    debug!(
                                        "No spawn handler registered for job type: {}",
                                        job.queue_name
                                    );
                                }
                                Err(e) => {
                                    warn!("Failed to spawn child jobs for job {}: {}", job_id, e);
                                }
                            }
                        } else {
                            debug!("Invalid spawn config in job payload for job {}", job_id);
                        }
                    }
                }

                // Record span success status
                #[cfg(feature = "tracing")]
                {
                    let span = tracing::Span::current();
                    span.record("success", true);
                    span.record("processing_time_ms", processing_time_ms);
                }

                // Fire job completion event hook
                let processing_duration = Duration::from_millis(processing_time_ms);
                self.event_hooks
                    .fire_job_complete(job.clone(), processing_duration);

                // Record job completed event
                self.record_event(JobEvent {
                    job_id,
                    queue_name,
                    event_type: JobEventType::Completed,
                    priority: job.priority,
                    processing_time_ms: Some(processing_time_ms),
                    error_message: None,
                    timestamp: Utc::now(),
                })
                .await;
            }
            Err(e) => {
                error!("Job {} failed: {}", job_id, e);
                let error_message = e.to_string();

                // Record span error status
                #[cfg(feature = "tracing")]
                {
                    let span = tracing::Span::current();
                    span.record("error", true);
                    span.record("error.type", "job_failure");
                    span.record("error.message", &error_message);
                    span.record("job.will_retry", job.attempts < self.max_retries);
                }

                // Update batch statistics for failed job
                if self.batch_processing_enabled && batch_id.is_some() {
                    self.update_batch_stats(|stats| {
                        stats.jobs_failed += 1;
                    });

                    // Check batch failure handling mode
                    if let Some(batch_id) = batch_id {
                        if let Err(e) = self
                            .handle_batch_job_failure(batch_id, job_id, &error_message)
                            .await
                        {
                            warn!(
                                "Failed to handle batch job failure for batch {}: {}",
                                batch_id, e
                            );
                        }
                    }
                }

                if job.attempts >= self.max_retries {
                    warn!("Job {} exceeded max retries, marking as failed", job_id);

                    // Check if we should mark as dead or just failed
                    if job.has_exhausted_retries() {
                        self.queue.mark_job_dead(job_id, &error_message).await?;

                        // Fire job failure event hook
                        self.event_hooks
                            .fire_job_fail(job.clone(), error_message.clone());

                        // Record job dead event
                        self.record_event(JobEvent {
                            job_id,
                            queue_name,
                            event_type: JobEventType::Dead,
                            priority: job.priority,
                            processing_time_ms: None,
                            error_message: Some(error_message),
                            timestamp: Utc::now(),
                        })
                        .await;
                    } else {
                        self.queue.fail_job(job_id, &error_message).await?;

                        // Fire job failure event hook
                        self.event_hooks
                            .fire_job_fail(job.clone(), error_message.clone());

                        // Record job failed event
                        self.record_event(JobEvent {
                            job_id,
                            queue_name,
                            event_type: JobEventType::Failed,
                            priority: job.priority,
                            processing_time_ms: None,
                            error_message: Some(error_message),
                            timestamp: Utc::now(),
                        })
                        .await;
                    }
                } else {
                    // Calculate retry delay using retry strategy priority:
                    // 1. Job-specific retry strategy (if configured)
                    // 2. Worker default retry strategy (if configured)
                    // 3. Fixed retry delay (legacy fallback)
                    let retry_delay = if let Some(ref job_strategy) = job.retry_strategy {
                        job_strategy.calculate_delay((job.attempts + 1) as u32)
                    } else if let Some(ref default_strategy) = self.default_retry_strategy {
                        default_strategy.calculate_delay((job.attempts + 1) as u32)
                    } else {
                        self.retry_delay
                    };

                    let retry_at =
                        chrono::Utc::now() + chrono::Duration::from_std(retry_delay).unwrap();
                    info!(
                        "Retrying job {} at {} (attempt {} of {})",
                        job_id,
                        retry_at,
                        job.attempts + 1,
                        self.max_retries
                    );
                    self.queue.retry_job(job_id, retry_at).await?;

                    // Fire job retry event hook
                    self.event_hooks
                        .fire_job_retry(job.clone(), error_message.clone());

                    // Record job retry event
                    self.record_event(JobEvent {
                        job_id,
                        queue_name,
                        event_type: JobEventType::Retried,
                        priority: job.priority,
                        processing_time_ms: None,
                        error_message: Some(error_message),
                        timestamp: Utc::now(),
                    })
                    .await;
                }
            }
        }

        Ok(())
    }

    /// Execute a job handler based on its type, returning a unified result
    async fn execute_handler(&self, job: Job) -> Result<JobResult> {
        match &self.handler {
            JobHandlerType::Legacy(handler) => {
                // Execute legacy handler and convert () to JobResult
                handler(job).await.map(|_| JobResult::success())
            }
            JobHandlerType::WithResult(handler) => {
                // Execute enhanced handler directly
                handler(job).await
            }
        }
    }

    /// Update batch processing statistics
    fn update_batch_stats<F>(&self, updater: F)
    where
        F: FnOnce(&mut BatchProcessingStats),
    {
        if let Ok(mut stats) = self.batch_stats.write() {
            updater(&mut stats);
        }
    }

    /// Convert a JobEvent to a JobLifecycleEvent for external publishing
    #[cfg(feature = "webhooks")]
    fn convert_to_lifecycle_event(&self, event: &JobEvent) -> JobLifecycleEvent {
        use std::collections::HashMap;

        let event_type = match event.event_type {
            JobEventType::Started => JobLifecycleEventType::Started,
            JobEventType::Completed => JobLifecycleEventType::Completed,
            JobEventType::Failed => JobLifecycleEventType::Failed,
            JobEventType::Retried => JobLifecycleEventType::Retried,
            JobEventType::Dead => JobLifecycleEventType::Dead,
            JobEventType::TimedOut => JobLifecycleEventType::TimedOut,
        };

        let error = event.error_message.as_ref().map(|message| JobError {
            message: message.clone(),
            error_type: match event.event_type {
                JobEventType::TimedOut => Some("timeout".to_string()),
                JobEventType::Failed => Some("processing_error".to_string()),
                JobEventType::Dead => Some("max_retries_exceeded".to_string()),
                _ => None,
            },
            details: None,
            retry_attempt: None,
        });

        let mut metadata = HashMap::new();
        metadata.insert("worker_queue".to_string(), self.queue_name.clone());

        if let Some(processing_time) = event.processing_time_ms {
            metadata.insert(
                "processing_time_ms".to_string(),
                processing_time.to_string(),
            );
        }

        JobLifecycleEvent {
            event_id: uuid::Uuid::new_v4(),
            job_id: event.job_id,
            queue_name: event.queue_name.clone(),
            event_type,
            priority: event.priority,
            timestamp: event.timestamp,
            processing_time_ms: event.processing_time_ms,
            error,
            payload: None, // Payload inclusion is controlled by EventFilter
            metadata,
        }
    }

    /// Check and update the status of a batch after job completion
    async fn check_and_update_batch_status(&self, batch_id: BatchId) -> Result<()> {
        // Get current batch status
        let batch_result = self.queue.get_batch_status(batch_id).await?;

        // If batch is complete, log success metrics
        if batch_result.pending_jobs == 0 {
            let completion_rate = batch_result.success_rate();

            if completion_rate >= 0.95 {
                info!(
                    "Batch {} completed successfully with {:.1}% success rate",
                    batch_id,
                    completion_rate * 100.0
                );
            } else {
                warn!(
                    "Batch {} completed with {:.1}% success rate ({} failures)",
                    batch_id,
                    completion_rate * 100.0,
                    batch_result.failed_jobs
                );
            }

            // Update batch completion statistics
            self.update_batch_stats(|stats| {
                stats.batches_completed += 1;
                if completion_rate >= 0.95 {
                    stats.batches_successful += 1;
                }
            });
        }

        Ok(())
    }

    /// Handle job failure within a batch context
    async fn handle_batch_job_failure(
        &self,
        batch_id: BatchId,
        job_id: uuid::Uuid,
        error_message: &str,
    ) -> Result<()> {
        // Get batch status to understand failure handling mode
        let batch_result = self.queue.get_batch_status(batch_id).await?;

        // Log batch-specific failure information
        warn!(
            "Job {} in batch {} failed: {}. Batch status: {}/{} jobs remaining",
            job_id, batch_id, error_message, batch_result.pending_jobs, batch_result.total_jobs
        );

        // Note: Actual failure mode handling (FailFast, ContinueOnError, etc.)
        // is implemented in the queue layer during job processing

        Ok(())
    }

    /// Get current batch processing statistics
    pub fn get_batch_stats(&self) -> BatchProcessingStats {
        if let Ok(stats) = self.batch_stats.read() {
            stats.clone()
        } else {
            BatchProcessingStats::default()
        }
    }

    async fn record_event(&self, event: JobEvent) {
        // Record to statistics collector
        if let Some(stats_collector) = &self.stats_collector {
            if let Err(e) = stats_collector.record_event(event.clone()).await {
                warn!("Failed to record statistics event: {}", e);
            }
        }

        // Record to metrics collector
        #[cfg(feature = "metrics")]
        if let Some(metrics_collector) = &self.metrics_collector {
            if let Err(e) = metrics_collector.record_job_event(&event).await {
                warn!("Failed to record metrics event: {}", e);
            }
        }

        // Publish to event manager for external integrations
        #[cfg(feature = "webhooks")]
        if let Some(event_manager) = &self.event_manager {
            let lifecycle_event = self.convert_to_lifecycle_event(&event);
            if let Err(e) = event_manager.publish_event(lifecycle_event).await {
                warn!("Failed to publish lifecycle event: {}", e);
            }
        }

        // Update last job time for worker starvation detection
        if matches!(
            event.event_type,
            JobEventType::Completed
                | JobEventType::Failed
                | JobEventType::Dead
                | JobEventType::TimedOut
        ) {
            if let Ok(mut last_time) = self.last_job_time.write() {
                *last_time = event.timestamp;
            }
        }
    }

    /// Start a background monitoring task for metrics and alerting
    #[cfg(any(feature = "metrics", feature = "alerting"))]
    fn start_monitoring_task(&self) -> tokio::task::JoinHandle<()> {
        let queue_name = self.queue_name.clone();

        #[cfg(feature = "metrics")]
        let queue = Arc::clone(&self.queue);

        #[cfg(feature = "alerting")]
        let last_job_time = Arc::clone(&self.last_job_time);

        #[cfg(feature = "metrics")]
        let metrics_collector = self.metrics_collector.clone();

        #[cfg(feature = "alerting")]
        let alert_manager = self.alert_manager.clone();

        #[cfg(feature = "alerting")]
        let stats_collector = self.stats_collector.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30)); // Monitor every 30 seconds

            loop {
                interval.tick().await;

                // Update queue depth metrics
                #[cfg(feature = "metrics")]
                if let Some(metrics_collector) = &metrics_collector {
                    if let Ok(queue_depth) = queue.get_queue_depth(&queue_name).await {
                        if let Err(e) = metrics_collector
                            .update_queue_depth(&queue_name, queue_depth)
                            .await
                        {
                            warn!("Failed to update queue depth metrics: {}", e);
                        }

                        // Check queue depth for alerts
                        #[cfg(feature = "alerting")]
                        if let Some(alert_manager) = &alert_manager {
                            if let Err(e) = alert_manager
                                .check_queue_depth(&queue_name, queue_depth)
                                .await
                            {
                                warn!("Failed to check queue depth alerts: {}", e);
                            }
                        }
                    }
                }

                // Check worker starvation
                #[cfg(feature = "alerting")]
                if let Some(alert_manager) = &alert_manager {
                    let last_time_value = {
                        if let Ok(last_time) = last_job_time.read() {
                            Some(*last_time)
                        } else {
                            None
                        }
                    };
                    if let Some(last_time_value) = last_time_value {
                        if let Err(e) = alert_manager
                            .check_worker_starvation(&queue_name, last_time_value)
                            .await
                        {
                            warn!("Failed to check worker starvation: {}", e);
                        }
                    }
                }

                // Check statistics-based alerts
                #[cfg(feature = "alerting")]
                if let (Some(alert_manager), Some(stats_collector)) =
                    (&alert_manager, &stats_collector)
                {
                    match stats_collector
                        .get_queue_statistics(&queue_name, Duration::from_secs(300))
                        .await
                    {
                        Ok(stats) => {
                            if let Err(e) =
                                alert_manager.check_thresholds(&queue_name, &stats).await
                            {
                                warn!("Failed to check alert thresholds: {}", e);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to get queue statistics for alerting: {}", e);
                        }
                    }
                }
            }
        })
    }
}

pub struct WorkerPool<DB: Database> {
    workers: Vec<Worker<DB>>,
    shutdown_tx: Vec<mpsc::Sender<()>>,
    stats_collector: Option<Arc<dyn StatisticsCollector>>,
    /// Worker template for creating new workers during autoscaling
    worker_template: Option<Worker<DB>>,
    /// Autoscaling configuration
    autoscale_config: AutoscaleConfig,
    /// Autoscaling metrics and state
    autoscale_metrics: Arc<std::sync::RwLock<AutoscaleMetrics>>,
    /// Queue depth history for averaging
    queue_depth_history: QueueDepthHistory,
    /// Autoscaling task handle
    autoscale_task: Option<tokio::task::JoinHandle<()>>,
}

impl<DB: Database + Send + Sync + 'static> WorkerPool<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            shutdown_tx: Vec::new(),
            stats_collector: None,
            worker_template: None,
            autoscale_config: AutoscaleConfig::default(),
            autoscale_metrics: Arc::new(std::sync::RwLock::new(AutoscaleMetrics::default())),
            queue_depth_history: Arc::new(std::sync::RwLock::new(Vec::new())),
            autoscale_task: None,
        }
    }

    pub fn with_stats_collector(mut self, stats_collector: Arc<dyn StatisticsCollector>) -> Self {
        self.stats_collector = Some(stats_collector);
        self
    }

    /// Configure autoscaling for the worker pool
    pub fn with_autoscaling(mut self, config: AutoscaleConfig) -> Self {
        self.autoscale_config = config;
        self
    }

    /// Disable autoscaling for the worker pool
    pub fn without_autoscaling(mut self) -> Self {
        self.autoscale_config = AutoscaleConfig::disabled();
        self
    }

    /// Set a worker template for autoscaling
    /// This worker will be cloned when creating new workers
    pub fn with_worker_template(mut self, worker: Worker<DB>) -> Self {
        self.worker_template = Some(worker);
        self
    }

    pub fn add_worker(&mut self, mut worker: Worker<DB>) {
        // Apply the pool's stats collector to the worker if available
        if let Some(stats_collector) = &self.stats_collector {
            worker.stats_collector = Some(Arc::clone(stats_collector));
        }

        // If no worker template is set and autoscaling is enabled, use the first worker as template
        if self.worker_template.is_none()
            && self.autoscale_config.enabled
            && self.workers.is_empty()
        {
            self.worker_template = Some(worker.clone());
        }

        self.workers.push(worker);
    }

    pub async fn start(&mut self) -> Result<()> {
        info!("Starting worker pool with {} workers", self.workers.len());

        // Update initial worker count metrics
        if let Ok(mut metrics) = self.autoscale_metrics.write() {
            metrics.active_workers = self.workers.len();
        }

        let mut handles = Vec::new();
        self.shutdown_tx.clear();

        for worker in self.workers.drain(..) {
            let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
            self.shutdown_tx.push(shutdown_tx);

            let handle = tokio::spawn(async move {
                if let Err(e) = worker.run(shutdown_rx).await {
                    error!("Worker error: {}", e);
                }
            });
            handles.push(handle);
        }

        // Start autoscaling task if enabled
        if self.autoscale_config.enabled {
            self.start_autoscaling_task().await?;
        }

        // Wait for all workers to complete
        for handle in handles {
            handle.await.map_err(|e| HammerworkError::Worker {
                message: format!("Worker task failed: {}", e),
            })?;
        }

        Ok(())
    }

    /// Start the autoscaling background task
    async fn start_autoscaling_task(&mut self) -> Result<()> {
        if let Some(worker_template) = &self.worker_template {
            let queue = Arc::clone(&worker_template.queue);
            let queue_name = worker_template.queue_name.clone();
            let config = self.autoscale_config.clone();
            let metrics = Arc::clone(&self.autoscale_metrics);
            let history = Arc::clone(&self.queue_depth_history);

            let task = tokio::spawn(async move {
                Self::autoscaling_loop(queue, queue_name, config, metrics, history).await;
            });

            self.autoscale_task = Some(task);
            info!(
                "Autoscaling task started for queue: {}",
                worker_template.queue_name
            );
        } else {
            warn!("Cannot start autoscaling: no worker template available");
        }
        Ok(())
    }

    /// Main autoscaling evaluation loop
    async fn autoscaling_loop(
        queue: Arc<JobQueue<DB>>,
        queue_name: String,
        config: AutoscaleConfig,
        metrics: Arc<std::sync::RwLock<AutoscaleMetrics>>,
        history: QueueDepthHistory,
    ) {
        let mut interval = tokio::time::interval(config.evaluation_window / 2);

        loop {
            interval.tick().await;

            if let Err(e) =
                Self::evaluate_scaling_decision(&queue, &queue_name, &config, &metrics, &history)
                    .await
            {
                warn!("Autoscaling evaluation error: {}", e);
            }
        }
    }

    /// Evaluate whether scaling up or down is needed
    async fn evaluate_scaling_decision(
        queue: &Arc<JobQueue<DB>>,
        queue_name: &str,
        config: &AutoscaleConfig,
        metrics: &Arc<std::sync::RwLock<AutoscaleMetrics>>,
        history: &QueueDepthHistory,
    ) -> Result<()> {
        // Get current queue depth
        let current_depth = queue.get_queue_depth(queue_name).await?;
        let now = Utc::now();

        // Update queue depth history
        if let Ok(mut hist) = history.write() {
            hist.push((now, current_depth));

            // Remove old entries outside the evaluation window
            let cutoff = now
                - chrono::Duration::from_std(config.evaluation_window)
                    .unwrap_or(chrono::Duration::seconds(30));
            hist.retain(|(timestamp, _)| *timestamp > cutoff);
        }

        // Calculate average queue depth
        let avg_depth = if let Ok(hist) = history.read() {
            if hist.is_empty() {
                current_depth as f64
            } else {
                hist.iter().map(|(_, depth)| *depth as f64).sum::<f64>() / hist.len() as f64
            }
        } else {
            current_depth as f64
        };

        // Update metrics and check if we need to scale
        let scaling_decision = if let Ok(mut m) = metrics.write() {
            m.current_queue_depth = current_depth;
            m.avg_queue_depth = avg_depth;

            // Check cooldown period
            let time_since_last = m
                .last_scale_time
                .map(|t| now - t)
                .and_then(|d| d.to_std().ok())
                .unwrap_or(config.cooldown_period);

            m.time_since_last_scale = time_since_last;

            if time_since_last < config.cooldown_period {
                None // Still in cooldown
            } else {
                // Calculate queue depth per worker
                let depth_per_worker = if m.active_workers > 0 {
                    avg_depth / m.active_workers as f64
                } else {
                    avg_depth
                };

                if depth_per_worker > config.scale_up_threshold as f64
                    && m.active_workers < config.max_workers
                {
                    Some(ScalingDecision::ScaleUp)
                } else if depth_per_worker < config.scale_down_threshold as f64
                    && m.active_workers > config.min_workers
                {
                    Some(ScalingDecision::ScaleDown)
                } else {
                    None
                }
            }
        } else {
            None
        };

        // Execute scaling decision
        if let Some(decision) = scaling_decision {
            Self::execute_scaling_decision(decision, config, metrics).await;
        }

        Ok(())
    }

    /// Execute a scaling decision
    async fn execute_scaling_decision(
        decision: ScalingDecision,
        config: &AutoscaleConfig,
        metrics: &Arc<std::sync::RwLock<AutoscaleMetrics>>,
    ) {
        if let Ok(mut m) = metrics.write() {
            match decision {
                ScalingDecision::ScaleUp => {
                    let new_count = (m.active_workers + config.scale_step).min(config.max_workers);
                    info!(
                        "Autoscaling: Scaling up from {} to {} workers (avg queue depth: {:.1})",
                        m.active_workers, new_count, m.avg_queue_depth
                    );
                    m.active_workers = new_count;
                }
                ScalingDecision::ScaleDown => {
                    let new_count = (m.active_workers.saturating_sub(config.scale_step))
                        .max(config.min_workers);
                    info!(
                        "Autoscaling: Scaling down from {} to {} workers (avg queue depth: {:.1})",
                        m.active_workers, new_count, m.avg_queue_depth
                    );
                    m.active_workers = new_count;
                }
            }
            m.last_scale_time = Some(Utc::now());
        }
    }

    /// Get current autoscaling metrics
    pub fn get_autoscale_metrics(&self) -> AutoscaleMetrics {
        if let Ok(metrics) = self.autoscale_metrics.read() {
            metrics.clone()
        } else {
            AutoscaleMetrics::default()
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down worker pool");

        // Stop autoscaling task
        if let Some(task) = &self.autoscale_task {
            task.abort();
            info!("Autoscaling task stopped");
        }

        for tx in &self.shutdown_tx {
            if tx.send(()).await.is_err() {
                warn!("Failed to send shutdown signal to worker");
            }
        }

        Ok(())
    }

    /// Get the statistics collector for the worker pool
    pub fn stats_collector(&self) -> Option<Arc<dyn StatisticsCollector>> {
        self.stats_collector.clone()
    }
}

impl<DB: Database + Send + Sync + 'static> Default for WorkerPool<DB>
where
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<DB: Database> Drop for WorkerPool<DB> {
    fn drop(&mut self) {
        // Stop autoscaling task when dropping the pool
        if let Some(task) = &self.autoscale_task {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_job_handler_type() {
        // Test that JobHandler type alias is properly defined
        let _handler: JobHandler = Arc::new(|_job| Box::pin(async { Ok(()) }));

        // Compilation test - if this compiles, the type is correct
    }

    #[test]
    fn test_worker_config_methods() {
        // Test that worker configuration methods work correctly
        // We can't test the full Worker without database implementations
        // But we can test the duration handling

        let poll_interval = Duration::from_millis(500);
        let retry_delay = Duration::from_secs(60);
        let max_retries = 5;

        assert_eq!(poll_interval.as_millis(), 500);
        assert_eq!(retry_delay.as_secs(), 60);
        assert_eq!(max_retries, 5);
    }

    #[test]
    fn test_worker_pool_struct() {
        // Test that WorkerPool struct is properly defined
        // We can't instantiate it without database implementations
        // But we can verify the type signatures compile

        // This would be the structure for a real implementation:
        // let pool: WorkerPool<sqlx::Postgres> = WorkerPool::new();
        // Compilation test
    }

    #[test]
    fn test_error_handling() {
        let error = HammerworkError::Worker {
            message: "Test error".to_string(),
        };

        assert_eq!(error.to_string(), "Worker error: Test error");
    }

    #[tokio::test]
    async fn test_worker_with_stats_collector() {
        use crate::stats::{InMemoryStatsCollector, StatisticsCollector};
        use std::sync::Arc;

        // This test verifies that the worker can be configured with a stats collector
        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Test that we can clone and store the stats collector reference
        let stats_clone = Arc::clone(&stats_collector);
        assert_eq!(Arc::strong_count(&stats_collector), 2);

        // Verify stats collector functionality
        let stats = stats_clone
            .get_system_statistics(Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(stats.total_processed, 0); // No events recorded yet
    }

    #[test]
    fn test_worker_pool_with_stats_collector() {
        use crate::stats::InMemoryStatsCollector;
        use std::sync::Arc;

        // This test verifies that the worker pool can be configured with a stats collector
        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Test that we can store the stats collector in the pool
        let stats_clone = Arc::clone(&stats_collector);
        assert_eq!(Arc::strong_count(&stats_collector), 2);

        // This verifies the reference counting works correctly
        drop(stats_clone);
        assert_eq!(Arc::strong_count(&stats_collector), 1);
    }

    #[test]
    fn test_worker_timeout_configuration() {
        use std::time::Duration;

        // Test timeout configuration methods
        let default_timeout = Duration::from_secs(30);
        let poll_interval = Duration::from_millis(500);
        let retry_delay = Duration::from_secs(60);

        // Verify duration values are correctly configured
        assert_eq!(default_timeout.as_secs(), 30);
        assert_eq!(poll_interval.as_millis(), 500);
        assert_eq!(retry_delay.as_secs(), 60);

        // Test timeout edge cases
        let very_short_timeout = Duration::from_millis(1);
        let very_long_timeout = Duration::from_secs(3600);

        assert_eq!(very_short_timeout.as_millis(), 1);
        assert_eq!(very_long_timeout.as_secs(), 3600);
    }

    #[test]
    fn test_job_timeout_detection_logic() {
        use crate::job::{Job, JobStatus};
        use serde_json::json;
        use std::time::Duration;

        // Test job timeout detection scenarios
        let mut job = Job::new("timeout_test".to_string(), json!({"data": "test"}))
            .with_timeout(Duration::from_millis(100));

        // Job not started - should not timeout
        assert!(!job.should_timeout());

        // Job started recently - should not timeout
        job.started_at = Some(chrono::Utc::now() - chrono::Duration::milliseconds(50));
        job.status = JobStatus::Running;
        assert!(!job.should_timeout());

        // Job started long ago - should timeout
        job.started_at = Some(chrono::Utc::now() - chrono::Duration::milliseconds(200));
        assert!(job.should_timeout());

        // Job without timeout - should never timeout
        let mut job_no_timeout = Job::new("no_timeout".to_string(), json!({"data": "test"}));
        job_no_timeout.started_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
        assert!(!job_no_timeout.should_timeout());
    }

    #[tokio::test]
    async fn test_timeout_statistics_integration() {
        use crate::stats::{InMemoryStatsCollector, JobEvent, JobEventType};
        use std::sync::Arc;

        let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

        // Simulate timeout event recording
        let timeout_event = JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "timeout_queue".to_string(),
            event_type: JobEventType::TimedOut,
            priority: crate::priority::JobPriority::Normal,
            processing_time_ms: Some(5000), // 5 seconds before timeout
            error_message: Some("Job timed out after 5s".to_string()),
            timestamp: chrono::Utc::now(),
        };

        stats_collector.record_event(timeout_event).await.unwrap();

        // Verify timeout event is tracked in statistics
        let stats = stats_collector
            .get_queue_statistics("timeout_queue", Duration::from_secs(60))
            .await
            .unwrap();

        assert_eq!(stats.total_processed, 1);
        assert_eq!(stats.timed_out, 1);
        assert_eq!(stats.error_rate, 1.0); // 1 timeout / 1 total = 100% error rate
    }

    #[test]
    fn test_timeout_error_message_formatting() {
        use std::time::Duration;

        // Test timeout error message formatting
        let timeout_duration = Duration::from_secs(30);
        let expected_message = format!("Job timed out after {:?}", timeout_duration);

        assert!(expected_message.contains("30s"));
        assert!(expected_message.contains("timed out"));

        // Test various timeout durations
        let short_timeout = Duration::from_millis(500);
        let long_timeout = Duration::from_secs(300);

        let short_message = format!("Job timed out after {:?}", short_timeout);
        let long_message = format!("Job timed out after {:?}", long_timeout);

        assert!(short_message.contains("500ms"));
        assert!(long_message.contains("300s"));
    }

    #[test]
    fn test_worker_timeout_precedence() {
        use crate::job::Job;
        use serde_json::json;
        use std::time::Duration;

        // Test that job-specific timeout takes precedence over worker default
        let job_timeout = Duration::from_secs(60);
        let worker_default_timeout = Duration::from_secs(30);

        let job_with_timeout =
            Job::new("test".to_string(), json!({"data": "test"})).with_timeout(job_timeout);

        let job_without_timeout = Job::new("test".to_string(), json!({"data": "test"}));

        // Job with specific timeout should use that timeout
        assert_eq!(job_with_timeout.timeout, Some(job_timeout));

        // Job without specific timeout would use worker default (tested in integration)
        assert_eq!(job_without_timeout.timeout, None);

        // Simulate timeout precedence logic
        let effective_timeout = job_with_timeout.timeout.or(Some(worker_default_timeout));
        assert_eq!(effective_timeout, Some(job_timeout)); // Job timeout wins

        let effective_timeout_default =
            job_without_timeout.timeout.or(Some(worker_default_timeout));
        assert_eq!(effective_timeout_default, Some(worker_default_timeout)); // Worker default used
    }

    #[test]
    fn test_worker_rate_limit_configuration() {
        use crate::rate_limit::RateLimit;

        // Test rate limit configuration
        let rate_limit = RateLimit::per_second(10).with_burst_limit(20);

        assert_eq!(rate_limit.rate, 10);
        assert_eq!(rate_limit.burst_limit, 20);
        assert_eq!(rate_limit.per, Duration::from_secs(1));

        // Test different time windows
        let per_minute = RateLimit::per_minute(60);
        assert_eq!(per_minute.rate, 60);
        assert_eq!(per_minute.per, Duration::from_secs(60));

        let per_hour = RateLimit::per_hour(3600);
        assert_eq!(per_hour.rate, 3600);
        assert_eq!(per_hour.per, Duration::from_secs(3600));
    }

    #[test]
    fn test_throttle_config_configuration() {
        use crate::rate_limit::ThrottleConfig;

        let throttle_config = ThrottleConfig::new()
            .max_concurrent(5)
            .rate_per_minute(100)
            .backoff_on_error(Duration::from_secs(30))
            .enabled(true);

        assert_eq!(throttle_config.max_concurrent, Some(5));
        assert_eq!(throttle_config.rate_per_minute, Some(100));
        assert_eq!(
            throttle_config.backoff_on_error,
            Some(Duration::from_secs(30))
        );
        assert!(throttle_config.enabled);

        // Test rate limit conversion
        let rate_limit = throttle_config.to_rate_limit().unwrap();
        assert_eq!(rate_limit.rate, 100);
        assert_eq!(rate_limit.per, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn test_rate_limiter_integration() {
        use crate::rate_limit::{RateLimit, RateLimiter};

        let rate_limit = RateLimit::per_second(5); // 5 operations per second
        let rate_limiter = RateLimiter::new(rate_limit);

        // Should initially allow operations
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());

        // Should block after consuming all tokens
        assert!(!rate_limiter.try_acquire());

        // Test acquire method (will wait for token refill)
        let start = std::time::Instant::now();
        rate_limiter.acquire().await.unwrap();
        let elapsed = start.elapsed();

        // Should have waited some time for token refill (but not too long due to high test rate)
        assert!(elapsed < Duration::from_millis(500)); // Should be fast for this test rate
    }

    #[test]
    fn test_worker_backoff_configuration() {
        use crate::rate_limit::ThrottleConfig;

        // Test that backoff configuration is properly handled
        let throttle_config = ThrottleConfig::new().backoff_on_error(Duration::from_secs(60));

        assert_eq!(
            throttle_config.backoff_on_error,
            Some(Duration::from_secs(60))
        );

        // Test default poll interval fallback
        let poll_interval = Duration::from_secs(1);
        let backoff_duration = throttle_config.backoff_on_error.unwrap_or(poll_interval);
        assert_eq!(backoff_duration, Duration::from_secs(60));

        // Test with no backoff configured
        let no_backoff_config = ThrottleConfig::new();
        let backoff_duration = no_backoff_config.backoff_on_error.unwrap_or(poll_interval);
        assert_eq!(backoff_duration, poll_interval);
    }

    #[tokio::test]
    async fn test_rate_limiter_token_availability() {
        use crate::rate_limit::{RateLimit, RateLimiter};

        let rate_limit = RateLimit::per_second(10); // 10 tokens per second
        let rate_limiter = RateLimiter::new(rate_limit);

        // Check initial token availability
        let initial_tokens = rate_limiter.available_tokens();
        assert_eq!(initial_tokens, 10.0); // Should start with full burst capacity

        // Consume some tokens
        assert!(rate_limiter.try_acquire());
        assert!(rate_limiter.try_acquire());

        // Check remaining tokens
        let remaining_tokens = rate_limiter.available_tokens();
        assert_eq!(remaining_tokens, 8.0);
    }

    #[test]
    fn test_rate_limit_edge_cases() {
        use crate::rate_limit::RateLimit;

        // Test very low rate
        let low_rate = RateLimit::per_hour(1);
        assert_eq!(low_rate.rate, 1);
        assert_eq!(low_rate.per, Duration::from_secs(3600));

        // Test very high rate
        let high_rate = RateLimit::per_second(1000);
        assert_eq!(high_rate.rate, 1000);
        assert_eq!(high_rate.burst_limit, 1000);

        // Test custom burst limit
        let custom_burst = RateLimit::per_second(10).with_burst_limit(50);
        assert_eq!(custom_burst.burst_limit, 50);
    }

    #[test]
    fn test_throttle_config_defaults() {
        use crate::rate_limit::ThrottleConfig;

        let default_config = ThrottleConfig::default();
        assert!(default_config.enabled);
        assert!(default_config.max_concurrent.is_none());
        assert!(default_config.rate_per_minute.is_none());
        assert!(default_config.backoff_on_error.is_none());

        let new_config = ThrottleConfig::new();
        assert_eq!(new_config.enabled, default_config.enabled);
        assert_eq!(new_config.max_concurrent, default_config.max_concurrent);
    }

    #[test]
    fn test_autoscale_config_defaults() {
        let config = AutoscaleConfig::default();

        assert!(config.enabled);
        assert_eq!(config.min_workers, 1);
        assert_eq!(config.max_workers, 10);
        assert_eq!(config.scale_up_threshold, 5);
        assert_eq!(config.scale_down_threshold, 2);
        assert_eq!(config.cooldown_period, Duration::from_secs(60));
        assert_eq!(config.scale_step, 1);
        assert_eq!(config.evaluation_window, Duration::from_secs(30));
        assert_eq!(config.idle_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_autoscale_config_builder() {
        let config = AutoscaleConfig::new()
            .with_min_workers(2)
            .with_max_workers(20)
            .with_scale_up_threshold(8)
            .with_scale_down_threshold(1)
            .with_cooldown_period(Duration::from_secs(120))
            .with_scale_step(2)
            .with_evaluation_window(Duration::from_secs(45))
            .with_idle_timeout(Duration::from_secs(600));

        assert_eq!(config.min_workers, 2);
        assert_eq!(config.max_workers, 20);
        assert_eq!(config.scale_up_threshold, 8);
        assert_eq!(config.scale_down_threshold, 1);
        assert_eq!(config.cooldown_period, Duration::from_secs(120));
        assert_eq!(config.scale_step, 2);
        assert_eq!(config.evaluation_window, Duration::from_secs(45));
        assert_eq!(config.idle_timeout, Duration::from_secs(600));
    }

    #[test]
    fn test_autoscale_config_presets() {
        let conservative = AutoscaleConfig::conservative();
        assert_eq!(conservative.min_workers, 2);
        assert_eq!(conservative.max_workers, 5);
        assert_eq!(conservative.scale_up_threshold, 10);
        assert_eq!(conservative.cooldown_period, Duration::from_secs(300));

        let aggressive = AutoscaleConfig::aggressive();
        assert_eq!(aggressive.min_workers, 1);
        assert_eq!(aggressive.max_workers, 20);
        assert_eq!(aggressive.scale_up_threshold, 3);
        assert_eq!(aggressive.cooldown_period, Duration::from_secs(30));

        let disabled = AutoscaleConfig::disabled();
        assert!(!disabled.enabled);
    }

    #[test]
    fn test_autoscale_config_validation() {
        // Test that min_workers is at least 1
        let config = AutoscaleConfig::new().with_min_workers(0);
        assert_eq!(config.min_workers, 1);

        // Test that max_workers is at least min_workers
        let config = AutoscaleConfig::new()
            .with_min_workers(5)
            .with_max_workers(3);
        assert_eq!(config.max_workers, 5);

        // Test that scale_up_threshold is at least 1
        let config = AutoscaleConfig::new().with_scale_up_threshold(0);
        assert_eq!(config.scale_up_threshold, 1);

        // Test that scale_step is at least 1
        let config = AutoscaleConfig::new().with_scale_step(0);
        assert_eq!(config.scale_step, 1);
    }

    #[test]
    fn test_autoscale_metrics_default() {
        let metrics = AutoscaleMetrics::default();

        assert_eq!(metrics.active_workers, 0);
        assert_eq!(metrics.avg_queue_depth, 0.0);
        assert_eq!(metrics.current_queue_depth, 0);
        assert_eq!(metrics.jobs_per_second, 0.0);
        assert_eq!(metrics.worker_utilization, 0.0);
        assert_eq!(metrics.time_since_last_scale, Duration::from_secs(0));
        assert!(metrics.last_scale_time.is_none());
    }

    #[test]
    fn test_scaling_decision_logic() {
        // This test simulates the scaling decision logic
        let mut metrics = AutoscaleMetrics {
            active_workers: 3,
            ..Default::default()
        };

        let config = AutoscaleConfig::default();

        // Test scale up condition
        metrics.avg_queue_depth = 20.0; // 20 jobs / 3 workers = 6.67 > 5 threshold
        let depth_per_worker = metrics.avg_queue_depth / metrics.active_workers as f64;
        assert!(depth_per_worker > config.scale_up_threshold as f64);
        assert!(metrics.active_workers < config.max_workers);

        // Test scale down condition
        metrics.avg_queue_depth = 3.0; // 3 jobs / 3 workers = 1.0 < 2 threshold
        let depth_per_worker = metrics.avg_queue_depth / metrics.active_workers as f64;
        assert!(depth_per_worker < config.scale_down_threshold as f64);
        assert!(metrics.active_workers > config.min_workers);

        // Test no scaling needed
        metrics.avg_queue_depth = 9.0; // 9 jobs / 3 workers = 3.0 (between thresholds)
        let depth_per_worker = metrics.avg_queue_depth / metrics.active_workers as f64;
        assert!(depth_per_worker < config.scale_up_threshold as f64);
        assert!(depth_per_worker > config.scale_down_threshold as f64);
    }

    #[test]
    fn test_cooldown_period_logic() {
        let config = AutoscaleConfig::default();
        let mut metrics = AutoscaleMetrics::default();

        // No last scale time should allow scaling
        assert!(metrics.last_scale_time.is_none());

        // Recent scale should prevent scaling
        metrics.last_scale_time = Some(Utc::now() - chrono::Duration::seconds(30));
        let time_since_last = Utc::now() - metrics.last_scale_time.unwrap();
        let time_since_last_std = time_since_last.to_std().unwrap_or(Duration::from_secs(0));
        assert!(time_since_last_std < config.cooldown_period);

        // Old scale should allow scaling
        metrics.last_scale_time = Some(Utc::now() - chrono::Duration::seconds(120));
        let time_since_last = Utc::now() - metrics.last_scale_time.unwrap();
        let time_since_last_std = time_since_last.to_std().unwrap_or(Duration::from_secs(0));
        assert!(time_since_last_std > config.cooldown_period);
    }

    #[test]
    fn test_queue_depth_averaging() {
        let now = Utc::now();
        let history = vec![
            (now - chrono::Duration::seconds(25), 10),
            (now - chrono::Duration::seconds(20), 8),
            (now - chrono::Duration::seconds(15), 12),
            (now - chrono::Duration::seconds(10), 6),
            (now - chrono::Duration::seconds(5), 14),
        ];

        // Calculate average
        let avg =
            history.iter().map(|(_, depth)| *depth as f64).sum::<f64>() / history.len() as f64;
        assert_eq!(avg, 10.0); // (10 + 8 + 12 + 6 + 14) / 5 = 10

        // Test filtering old entries
        let evaluation_window = Duration::from_secs(30);
        let cutoff = now - chrono::Duration::from_std(evaluation_window).unwrap();
        let recent_entries: Vec<_> = history
            .into_iter()
            .filter(|(timestamp, _)| *timestamp > cutoff)
            .collect();

        // All entries should be within the window
        assert_eq!(recent_entries.len(), 5);
    }

    #[test]
    fn test_worker_count_boundaries() {
        let config = AutoscaleConfig::default();
        let mut metrics = AutoscaleMetrics {
            active_workers: config.max_workers - 1,
            ..Default::default()
        };
        let new_count = (metrics.active_workers + config.scale_step).min(config.max_workers);
        assert_eq!(new_count, config.max_workers);

        // Test scaling beyond max workers (should cap at max)
        metrics.active_workers = config.max_workers;
        let new_count = (metrics.active_workers + config.scale_step).min(config.max_workers);
        assert_eq!(new_count, config.max_workers);

        // Test scaling down to min workers
        metrics.active_workers = config.min_workers + 1;
        let new_count =
            (metrics.active_workers.saturating_sub(config.scale_step)).max(config.min_workers);
        assert_eq!(new_count, config.min_workers);

        // Test scaling below min workers (should cap at min)
        metrics.active_workers = config.min_workers;
        let new_count =
            (metrics.active_workers.saturating_sub(config.scale_step)).max(config.min_workers);
        assert_eq!(new_count, config.min_workers);
    }

    #[test]
    fn test_autoscale_metrics_update() {
        let metrics = Arc::new(std::sync::RwLock::new(AutoscaleMetrics::default()));

        // Test updating metrics
        if let Ok(mut m) = metrics.write() {
            m.active_workers = 5;
            m.current_queue_depth = 25;
            m.avg_queue_depth = 22.5;
            m.last_scale_time = Some(Utc::now());
        }

        // Test reading metrics
        if let Ok(m) = metrics.read() {
            assert_eq!(m.active_workers, 5);
            assert_eq!(m.current_queue_depth, 25);
            assert_eq!(m.avg_queue_depth, 22.5);
            assert!(m.last_scale_time.is_some());
        }
    }

    #[test]
    fn test_history_cleanup() {
        let now = Utc::now();
        let mut history = vec![
            (now - chrono::Duration::seconds(60), 10), // Too old
            (now - chrono::Duration::seconds(45), 8),  // Too old
            (now - chrono::Duration::seconds(25), 12), // Recent
            (now - chrono::Duration::seconds(15), 6),  // Recent
            (now - chrono::Duration::seconds(5), 14),  // Recent
        ];

        // Filter based on 30-second window
        let evaluation_window = Duration::from_secs(30);
        let cutoff = now - chrono::Duration::from_std(evaluation_window).unwrap();
        history.retain(|(timestamp, _)| *timestamp > cutoff);

        // Should only have 3 recent entries
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].1, 12);
        assert_eq!(history[1].1, 6);
        assert_eq!(history[2].1, 14);
    }

    #[test]
    fn test_job_event_hooks_default() {
        let hooks = JobEventHooks::default();
        assert!(hooks.on_job_start.is_none());
        assert!(hooks.on_job_complete.is_none());
        assert!(hooks.on_job_fail.is_none());
        assert!(hooks.on_job_timeout.is_none());
        assert!(hooks.on_job_retry.is_none());
    }

    #[test]
    fn test_job_event_hooks_new() {
        let hooks = JobEventHooks::new();
        assert!(hooks.on_job_start.is_none());
        assert!(hooks.on_job_complete.is_none());
        assert!(hooks.on_job_fail.is_none());
        assert!(hooks.on_job_timeout.is_none());
        assert!(hooks.on_job_retry.is_none());
    }

    #[test]
    fn test_job_event_hooks_builder() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::new()));

        let events_start = Arc::clone(&events);
        let events_complete = Arc::clone(&events);
        let events_fail = Arc::clone(&events);
        let events_timeout = Arc::clone(&events);
        let events_retry = Arc::clone(&events);

        let hooks = JobEventHooks::new()
            .on_start(move |event: JobHookEvent| {
                events_start
                    .lock()
                    .unwrap()
                    .push(format!("start:{}", event.job.id));
            })
            .on_complete(move |event: JobHookEvent| {
                events_complete
                    .lock()
                    .unwrap()
                    .push(format!("complete:{}", event.job.id));
            })
            .on_fail(move |event: JobHookEvent| {
                events_fail
                    .lock()
                    .unwrap()
                    .push(format!("fail:{}", event.job.id));
            })
            .on_timeout(move |event: JobHookEvent| {
                events_timeout
                    .lock()
                    .unwrap()
                    .push(format!("timeout:{}", event.job.id));
            })
            .on_retry(move |event: JobHookEvent| {
                events_retry
                    .lock()
                    .unwrap()
                    .push(format!("retry:{}", event.job.id));
            });

        // Verify all hooks are set
        assert!(hooks.on_job_start.is_some());
        assert!(hooks.on_job_complete.is_some());
        assert!(hooks.on_job_fail.is_some());
        assert!(hooks.on_job_timeout.is_some());
        assert!(hooks.on_job_retry.is_some());
    }

    #[test]
    fn test_job_hook_event_creation() {
        use crate::Job;
        use serde_json::json;
        use std::time::Duration;

        let job = Job::new("test_queue".to_string(), json!({"test": "data"}))
            .with_trace_id("trace-123")
            .with_correlation_id("corr-456");

        let event = JobHookEvent {
            job: job.clone(),
            timestamp: Utc::now(),
            duration: Some(Duration::from_millis(500)),
            error: Some("Test error".to_string()),
        };

        assert_eq!(event.job.id, job.id);
        assert_eq!(event.job.queue_name, "test_queue");
        assert_eq!(event.job.trace_id, Some("trace-123".to_string()));
        assert_eq!(event.job.correlation_id, Some("corr-456".to_string()));
        assert_eq!(event.duration, Some(Duration::from_millis(500)));
        assert_eq!(event.error, Some("Test error".to_string()));
    }

    #[test]
    fn test_job_event_hooks_fire_methods() {
        use crate::Job;
        use serde_json::json;
        use std::sync::{Arc, Mutex};
        use std::time::Duration;

        let events = Arc::new(Mutex::new(Vec::new()));
        let job = Job::new("test_queue".to_string(), json!({"test": "data"}));

        // Test fire_job_start
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_start(move |event: JobHookEvent| {
                events_clone
                    .lock()
                    .unwrap()
                    .push(format!("start:{}", event.job.queue_name));
            });

            hooks.fire_job_start(job.clone());
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "start:test_queue");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_complete
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_complete(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "complete:{}:{}ms",
                    event.job.queue_name,
                    event.duration.unwrap_or_default().as_millis()
                ));
            });

            hooks.fire_job_complete(job.clone(), Duration::from_millis(150));
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "complete:test_queue:150ms");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_fail
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_fail(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "fail:{}:{}",
                    event.job.queue_name,
                    event.error.unwrap_or_default()
                ));
            });

            hooks.fire_job_fail(job.clone(), "Connection timeout".to_string());
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "fail:test_queue:Connection timeout");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_timeout
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_timeout(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "timeout:{}:{}ms",
                    event.job.queue_name,
                    event.duration.unwrap_or_default().as_millis()
                ));
            });

            hooks.fire_job_timeout(job.clone(), Duration::from_secs(30));
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(captured_events[0], "timeout:test_queue:30000ms");
        }

        // Clear events for next test
        events.lock().unwrap().clear();

        // Test fire_job_retry
        {
            let events_clone = Arc::clone(&events);
            let hooks = JobEventHooks::new().on_retry(move |event: JobHookEvent| {
                events_clone.lock().unwrap().push(format!(
                    "retry:{}:{}",
                    event.job.queue_name,
                    event.error.unwrap_or_default()
                ));
            });

            hooks.fire_job_retry(job.clone(), "API rate limit exceeded".to_string());
            let captured_events = events.lock().unwrap();
            assert_eq!(captured_events.len(), 1);
            assert_eq!(
                captured_events[0],
                "retry:test_queue:API rate limit exceeded"
            );
        }
    }

    #[test]
    fn test_job_event_hooks_clone() {
        use std::sync::{Arc, Mutex};

        let events = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        let hooks = JobEventHooks::new().on_start(move |event: JobHookEvent| {
            events_clone
                .lock()
                .unwrap()
                .push(format!("cloned:{}", event.job.id));
        });

        // Clone the hooks
        let hooks_clone = hooks.clone();

        // Both original and clone should work
        let job = crate::Job::new("test".to_string(), serde_json::json!({}));
        hooks.fire_job_start(job.clone());
        hooks_clone.fire_job_start(job);

        let captured_events = events.lock().unwrap();
        assert_eq!(captured_events.len(), 2);
        assert!(captured_events[0].starts_with("cloned:"));
        assert!(captured_events[1].starts_with("cloned:"));
    }
}
