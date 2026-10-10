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
    queue::{DatabaseQueue, JobOutcome, JobQueue, RecordedOutcome},
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
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
    time::sleep,
};
use tracing::{debug, error, info, warn};

/// Default time a worker waits for an in-flight job to finish after shutdown is requested.
pub const DEFAULT_SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

/// Default lease a worker holds on a running job. The lease is taken when the job is
/// claimed and renewed every third of this period; if the worker dies, the job becomes
/// eligible for [`DatabaseQueue::requeue_stale_jobs`] once the lease expires.
pub const DEFAULT_LEASE_DURATION: Duration = crate::queue::DEFAULT_LEASE_DURATION;

/// Default interval at which a worker's monitoring task updates its queue depth metric
/// and checks its alert thresholds ([`Worker::with_monitoring_interval`]).
pub const DEFAULT_MONITORING_INTERVAL: Duration = Duration::from_secs(30);

/// Floor on the monitoring interval.
#[cfg(any(feature = "metrics", feature = "alerting"))]
const MIN_MONITORING_INTERVAL: Duration = Duration::from_millis(10);

/// Default interval at which a [`WorkerPool`] runs the stale job reaper.
pub const DEFAULT_STALE_JOB_REAPER_INTERVAL: Duration = Duration::from_secs(60);

/// Upper bound on a computed retry delay. Larger values (from a custom strategy or an
/// unbounded backoff) are clamped so the retry timestamp stays representable in both
/// databases.
pub const MAX_RETRY_DELAY: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// Cap on the exponential backoff applied after consecutive worker loop errors.
const MAX_ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// Floor on the error backoff so a zero poll interval can never cause a hot loop.
const MIN_ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Floor on the wait between polls of an idle worker, so a zero poll interval can never
/// cause a hot loop.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Floor on the heartbeat interval derived from the lease duration.
const MIN_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);

/// Delay before a [`WorkerPool`] restarts a worker that died unexpectedly.
const WORKER_RESTART_DELAY: Duration = Duration::from_secs(1);

type HandlerFuture = Pin<Box<dyn Future<Output = Result<JobResult>> + Send>>;

/// Future adapter that turns a panic during `poll` into an `Err` carrying the payload.
struct CatchUnwind<F> {
    inner: F,
}

impl<F: Future + Unpin> Future for CatchUnwind<F> {
    type Output = std::thread::Result<F::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = &mut self.inner;
        match std::panic::catch_unwind(AssertUnwindSafe(|| Pin::new(&mut *inner).poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

/// Describe a panic payload for an error message.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Build the error used when a job handler panics.
fn handler_panic_error(payload: &(dyn std::any::Any + Send)) -> HammerworkError {
    HammerworkError::Worker {
        message: format!("Job handler panicked: {}", panic_message(payload)),
    }
}

/// Convert a retry delay into the timestamp at which the job should run again.
///
/// The delay is clamped to [`MAX_RETRY_DELAY`] and the conversion never panics.
fn retry_at_from_delay(now: DateTime<Utc>, retry_delay: Duration) -> DateTime<Utc> {
    let delay = chrono::Duration::from_std(retry_delay.min(MAX_RETRY_DELAY))
        .unwrap_or(chrono::Duration::MAX);
    now.checked_add_signed(delay)
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// How many attempts a job gets: its own `max_attempts`, lowered by the worker's cap
/// (`Worker::with_max_retries`) when one is set.
fn effective_attempt_limit(job_max_attempts: i32, worker_cap: Option<i32>) -> i32 {
    match worker_cap {
        Some(cap) => job_max_attempts.min(cap),
        None => job_max_attempts,
    }
}

/// Aborts a background task when dropped, so it never outlives its owner.
#[cfg(any(feature = "metrics", feature = "alerting"))]
struct AbortOnDrop(tokio::task::JoinHandle<()>);

#[cfg(any(feature = "metrics", feature = "alerting"))]
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Result of one attempt to acquire a job in the worker loop.
enum Acquired {
    // Boxed: a `Job` is much larger than the other variants.
    Job(Box<Job>),
    Idle,
    Shutdown,
}

/// Whether shutdown was requested (or the shutdown sender was dropped), without waiting.
fn shutdown_requested(shutdown_rx: &mut mpsc::Receiver<()>) -> bool {
    !matches!(
        shutdown_rx.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    )
}

/// Exponential backoff after `consecutive_errors` failed loop iterations.
fn error_backoff(base: Duration, consecutive_errors: u32) -> Duration {
    let base = base.max(MIN_ERROR_BACKOFF);
    let exponent = consecutive_errors.saturating_sub(1).min(16);
    base.checked_mul(1u32 << exponent)
        .unwrap_or(MAX_ERROR_BACKOFF)
        .min(MAX_ERROR_BACKOFF.max(base))
}

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
///
/// A [`WorkerPool`] does not autoscale until it is given a configuration with
/// [`WorkerPool::with_autoscaling`]. [`AutoscaleConfig::default`] (and the presets) are
/// enabled, so `with_autoscaling(AutoscaleConfig::default())` turns autoscaling on; see
/// [`WorkerPool::with_autoscaling`] for which workers it starts and retires.
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
    /// Unused: scale-down is decided from the queue depth averaged over
    /// `evaluation_window` and limited by `cooldown_period`.
    #[deprecated(
        since = "1.15.6",
        note = "never applied; use `evaluation_window` and `cooldown_period` to slow scale-down"
    )]
    pub idle_timeout: Duration,
}

impl Default for AutoscaleConfig {
    #[allow(deprecated)]
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

#[allow(deprecated)]
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

    /// Unused; see [`AutoscaleConfig::idle_timeout`].
    #[deprecated(
        since = "1.15.6",
        note = "never applied; use `evaluation_window` and `cooldown_period` to slow scale-down"
    )]
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
    /// Current number of active workers: those of the autoscaled queue when
    /// autoscaling is enabled, otherwise all of the pool's workers
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

/// What a [`WorkerPool`] supervisor needs to apply autoscaling decisions.
struct Scaling<DB: Database> {
    /// Cloned for every worker started by a scale-up. Only workers of its queue are
    /// started or retired.
    template: Worker<DB>,
    /// Desired number of workers on the template's queue, published by the
    /// autoscaling task
    desired: watch::Receiver<usize>,
    /// Workers on the template's queue when the pool started
    initial: usize,
}

/// Wait for the next value on `rx`. Returns `None` once the sender is gone, and never
/// completes when there is no receiver.
async fn wait_for_change(rx: &mut Option<watch::Receiver<usize>>) -> Option<usize> {
    match rx {
        Some(rx) => match rx.changed().await {
            Ok(()) => Some(*rx.borrow_and_update()),
            Err(_) => None,
        },
        None => std::future::pending().await,
    }
}

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
#[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
#[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
#[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
#[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    max_retries: Option<i32>,
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
    /// Permits for the throttle's `max_concurrent`, shared by clones of this worker
    concurrency_limit: Option<Arc<tokio::sync::Semaphore>>,
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
    /// How long to wait for an in-flight job to finish after shutdown is requested
    shutdown_grace_period: Duration,
    /// Lease held on a running job, renewed by heartbeats
    lease_duration: Duration,
    /// How often the monitoring task updates metrics and checks alerts, when set
    /// explicitly (otherwise the metrics collector's update interval, or 30 seconds)
    monitoring_interval: Option<Duration>,
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
            concurrency_limit: self.concurrency_limit.clone(),
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
            shutdown_grace_period: self.shutdown_grace_period,
            lease_duration: self.lease_duration,
            monitoring_interval: self.monitoring_interval,
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
    /// - no cap on attempts: each job runs up to its own
    ///   [`max_attempts`](crate::Job::with_max_attempts) (3 unless set); see
    ///   [`with_max_retries`](Self::with_max_retries)
    /// - 30 second retry delay
    /// - a [lease](Self::with_lease_duration) of [`DEFAULT_LEASE_DURATION`] (5 minutes)
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
            max_retries: None,
            retry_delay: Duration::from_secs(30),
            default_retry_strategy: None,
            default_timeout: None,
            priority_weights: None,
            stats_collector: None,
            rate_limiter: None,
            throttle_config: None,
            concurrency_limit: None,
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
            shutdown_grace_period: DEFAULT_SHUTDOWN_GRACE_PERIOD,
            lease_duration: DEFAULT_LEASE_DURATION,
            monitoring_interval: None,
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
            max_retries: None,
            retry_delay: Duration::from_secs(30),
            default_retry_strategy: None,
            default_timeout: None,
            priority_weights: None,
            stats_collector: None,
            rate_limiter: None,
            throttle_config: None,
            concurrency_limit: None,
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
            shutdown_grace_period: DEFAULT_SHUTDOWN_GRACE_PERIOD,
            lease_duration: DEFAULT_LEASE_DURATION,
            monitoring_interval: None,
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    /// Intervals below [`MIN_POLL_INTERVAL`] (10ms), including zero, wait
    /// [`MIN_POLL_INTERVAL`], so an idle worker never polls in a tight loop.
    ///
    /// # Arguments
    ///
    /// * `interval` - Time between polling attempts
    ///
    /// # Examples
    ///
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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

    /// Caps the number of attempts this worker gives any job.
    ///
    /// A job's own [`max_attempts`](crate::Job::with_max_attempts) is the source of
    /// truth: a failing (or timing-out) job runs up to `max_attempts` times and then
    /// becomes `Dead` (or `TimedOut`). This setting only lowers that limit: the
    /// effective limit is `min(job.max_attempts, max_retries)`. By default there is no
    /// cap.
    ///
    /// Previously this value replaced the job's `max_attempts` (default 3), so jobs
    /// with `with_max_attempts(5)` stopped after 3 runs in a non-retried `Failed`
    /// state, and `with_max_attempts(1)` still ran 3 times.
    ///
    /// # Arguments
    ///
    /// * `max_retries` - Maximum number of attempts per job on this worker
    ///
    /// # Examples
    ///
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
    /// use hammerwork::Worker;
    /// # use std::sync::Arc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(hammerwork::JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|job| Box::pin(async move { Ok(()) }));
    ///
    /// // Expensive calls: never run a job more than twice on this worker, even if the
    /// // job itself allows more attempts. To give jobs *more* attempts, raise their own
    /// // limit with `Job::with_max_attempts`; this setting can only lower it.
    /// let expensive_worker = Worker::new(queue, "expensive".to_string(), handler)
    ///     .with_max_retries(2);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_max_retries(mut self, max_retries: i32) -> Self {
        self.max_retries = Some(max_retries);
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    /// `Duration::ZERO` means no default timeout (it would otherwise time every job
    /// out before its handler ran).
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time a job can run before being terminated
    ///
    /// # Examples
    ///
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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

    /// Set how long the worker waits for an in-flight job after shutdown is requested.
    ///
    /// When [`WorkerPool::shutdown`] (or the worker's shutdown channel) fires while a
    /// job is running, the worker stops polling but lets the job finish for up to this
    /// long. Only after the grace period is the handler cancelled; the job then stays
    /// `Running` until its lease expires and the stale job reaper reclaims it.
    ///
    /// Defaults to [`DEFAULT_SHUTDOWN_GRACE_PERIOD`] (30 seconds).
    ///
    /// # Examples
    ///
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
    /// use hammerwork::{Worker, JobQueue};
    /// use std::{sync::Arc, time::Duration};
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/test").await?;
    /// # let queue = Arc::new(JobQueue::new(pool));
    /// # let handler: hammerwork::worker::JobHandler = Arc::new(|_job| Box::pin(async { Ok(()) }));
    /// let worker = Worker::new(queue, "emails".to_string(), handler)
    ///     .with_shutdown_grace_period(Duration::from_secs(120));
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_shutdown_grace_period(mut self, grace_period: Duration) -> Self {
        self.shutdown_grace_period = grace_period;
        self
    }

    /// Set the lease the worker holds on a running job.
    ///
    /// The lease is taken in the same statement that claims the job
    /// ([`DatabaseQueue::dequeue_leased`]). While the handler runs, the worker records a
    /// heartbeat and extends the lease every third of this duration (via
    /// [`DatabaseQueue::heartbeat_job`]). If the worker process dies, the lease stops
    /// being renewed and [`DatabaseQueue::requeue_stale_jobs`] reclaims the job once it
    /// expires; a reaper never reclaims a job whose lease is still valid, whatever its
    /// own staleness window. Shorter leases recover crashed jobs faster at the cost of
    /// more heartbeat writes for long-running jobs; jobs that finish within a third of
    /// the lease never write one.
    ///
    /// `Duration::ZERO` takes a lease that never expires and sends no heartbeats: the
    /// reaper never reclaims this worker's jobs, so if the worker dies an operator has
    /// to re-run them (for example with `cargo hammerwork job retry`).
    ///
    /// Defaults to [`DEFAULT_LEASE_DURATION`] (5 minutes). Requires migration
    /// `015_add_job_leases`.
    pub fn with_lease_duration(mut self, lease: Duration) -> Self {
        self.lease_duration = lease;
        self
    }

    /// Set how often the worker's monitoring task runs. Without this, it runs at the
    /// [metrics collector](Self::with_metrics_collector)'s update interval
    /// (`MetricsConfig::update_interval`), or every [`DEFAULT_MONITORING_INTERVAL`]
    /// (30 seconds) without one.
    ///
    /// With a [metrics collector](Self::with_metrics_collector) or alerting configured,
    /// a background task updates the queue depth metric and checks the alert thresholds
    /// (queue depth, worker starvation, and with a statistics collector the error rate
    /// and processing time) at this interval. These checks never run in the job polling
    /// loop, so a slow alert target or the queue depth `COUNT(*)` never delays
    /// dequeuing. Values below 10ms are raised to 10ms.
    pub fn with_monitoring_interval(mut self, interval: Duration) -> Self {
        self.monitoring_interval = Some(interval);
        self
    }

    /// The lease this worker holds on running jobs. See [`Worker::with_lease_duration`].
    pub fn lease_duration(&self) -> Duration {
        self.lease_duration
    }

    /// Apply a [`WorkerConfig`](crate::config::WorkerConfig).
    ///
    /// Sets the poll interval, default job timeout, priority weights and default retry
    /// strategy. `pool_size` and the autoscaling settings apply to a [`WorkerPool`]; see
    /// [`WorkerPool::from_config`].
    pub fn with_config(self, config: &crate::config::WorkerConfig) -> Self {
        self.with_poll_interval(config.polling_interval)
            .with_default_timeout(config.job_timeout)
            .with_priority_weights(config.priority_weights.clone())
            .with_default_retry_strategy(config.retry_strategy.clone())
    }

    /// Apply the worker-related sections of a [`HammerworkConfig`](crate::HammerworkConfig).
    ///
    /// Applies `worker` (see [`with_config`](Self::with_config)), the throttle from
    /// `rate_limiting` that matches this worker's queue, and `alerting` (when the
    /// `alerting` feature is enabled). Events are not applied here because the
    /// [`EventManager`](crate::events::EventManager) must be shared with any webhook or
    /// stream managers: build it once with `EventManager::new(config.events.clone())`
    /// and pass it to [`with_event_manager`](Self::with_event_manager).
    ///
    /// # Examples
    ///
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
    /// use hammerwork::{HammerworkConfig, JobQueue, Worker};
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> hammerwork::Result<()> {
    /// let config = HammerworkConfig::from_file("hammerwork.toml")?;
    /// let queue = Arc::new(JobQueue::<sqlx::Postgres>::from_config(&config).await?);
    /// let handler: hammerwork::worker::JobHandler = Arc::new(|_job| Box::pin(async { Ok(()) }));
    ///
    /// let worker = Worker::new(queue, "email".to_string(), handler)
    ///     .with_hammerwork_config(&config);
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_hammerwork_config(mut self, config: &crate::HammerworkConfig) -> Self {
        self = self.with_config(&config.worker);

        if let Some(throttle) = config.rate_limiting.throttle_for(&self.queue_name) {
            self = self.with_throttle_config(throttle);
        }

        #[cfg(feature = "alerting")]
        {
            self = self.with_alerting_config(config.alerting.clone());
        }

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

    /// Configure throttling for this worker.
    ///
    /// - `rate_per_minute` limits how often jobs are dequeued.
    /// - `max_concurrent` limits how many jobs run at once (at least one).
    /// - `backoff_on_error` is the base delay after a polling error.
    ///
    /// The limits are shared by every clone of this worker, so they apply to a whole
    /// [`WorkerPool`] built from it. A throttle with `enabled == false` is ignored.
    pub fn with_throttle_config(mut self, throttle_config: ThrottleConfig) -> Self {
        if !throttle_config.enabled {
            return self;
        }
        // If the throttle config has a rate limit, create a rate limiter
        if let Some(rate_limit) = throttle_config.to_rate_limit() {
            self.rate_limiter = Some(RateLimiter::new(rate_limit));
        }
        self.concurrency_limit = throttle_config.max_concurrent.map(|max| {
            let permits = usize::try_from(max)
                .unwrap_or(usize::MAX)
                .clamp(1, tokio::sync::Semaphore::MAX_PERMITS);
            Arc::new(tokio::sync::Semaphore::new(permits))
        });
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
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

    /// Run the worker until a shutdown signal arrives on `shutdown_rx` (or its sender
    /// is dropped).
    ///
    /// Shutdown is graceful: while idle the worker stops immediately, but a job that is
    /// already running gets up to the configured
    /// [shutdown grace period](Worker::with_shutdown_grace_period) to finish and record
    /// its outcome before it is cancelled.
    ///
    /// Errors inside the loop (database failures while dequeuing or recording a job's
    /// outcome) are logged and followed by an exponential backoff starting at the
    /// throttle's `backoff_on_error` (or the poll interval), so a failing database never
    /// causes a hot loop. A panicking job handler fails the job instead of killing the
    /// worker.
    pub async fn run(&self, mut shutdown_rx: mpsc::Receiver<()>) -> Result<()> {
        info!("Worker started for queue: {}", self.queue_name);

        // Start background monitoring task for metrics and alerting. The guard aborts it
        // however `run` ends: a normal return, an error, a panic, or this future being
        // dropped (for example by a pool restarting the worker).
        #[cfg(any(feature = "metrics", feature = "alerting"))]
        let _monitoring_task = AbortOnDrop(self.start_monitoring_task());

        let mut consecutive_errors: u32 = 0;

        loop {
            // Idle phase. Only the waits (poll interval, rate limiter) are interrupted by
            // shutdown; the dequeue itself always runs to completion. Dropping a dequeue
            // future mid-transaction can return a pooled connection with an open
            // transaction and row locks still held.
            if shutdown_requested(&mut shutdown_rx) {
                info!("Worker shutting down for queue: {}", self.queue_name);
                break;
            }
            // Held until this iteration's job is done, so at most `max_concurrent`
            // workers sharing this throttle dequeue or run jobs at once.
            let _permit = match &self.concurrency_limit {
                Some(limit) => tokio::select! {
                    biased;
                    _ = shutdown_rx.recv() => {
                        info!("Worker shutting down for queue: {}", self.queue_name);
                        break;
                    }
                    // The semaphore is never closed, so this is always a permit.
                    permit = Arc::clone(limit).acquire_owned() => permit.ok(),
                },
                None => None,
            };
            let acquired = self.acquire_job(&mut shutdown_rx).await;

            let (outcome, shutting_down) = match acquired {
                Ok(Acquired::Shutdown) => {
                    info!("Worker shutting down for queue: {}", self.queue_name);
                    break;
                }
                Ok(Acquired::Job(job)) => self.run_job_until_done(*job, &mut shutdown_rx).await,
                Ok(Acquired::Idle) => (Ok(()), false),
                Err(e) => {
                    error!("Error dequeuing job from queue {}: {}", self.queue_name, e);
                    (Err(e), false)
                }
            };

            match outcome {
                Ok(()) => consecutive_errors = 0,
                Err(e) => {
                    consecutive_errors = consecutive_errors.saturating_add(1);
                    if !shutting_down {
                        let backoff = error_backoff(self.error_backoff_base(), consecutive_errors);
                        warn!(
                            "Worker for queue {} hit an error ({} in a row), backing off for {:?}: {}",
                            self.queue_name, consecutive_errors, backoff, e
                        );
                        tokio::select! {
                            biased;
                            _ = shutdown_rx.recv() => {
                                info!("Worker shutting down for queue: {}", self.queue_name);
                                break;
                            }
                            _ = sleep(backoff) => {}
                        }
                    }
                }
            }

            if shutting_down {
                info!("Worker shutting down for queue: {}", self.queue_name);
                break;
            }
        }

        Ok(())
    }

    /// Process `job` to completion. If shutdown is requested meanwhile, keep going for
    /// up to the shutdown grace period, then cancel.
    ///
    /// Returns the processing outcome and whether shutdown was requested.
    async fn run_job_until_done(
        &self,
        job: Job,
        shutdown_rx: &mut mpsc::Receiver<()>,
    ) -> (Result<()>, bool) {
        let job_id = job.id;
        let processing = self.process_job(job);
        tokio::pin!(processing);

        let outcome = tokio::select! {
            biased;
            outcome = &mut processing => return (self.log_process_outcome(job_id, outcome), false),
            _ = shutdown_rx.recv() => {
                info!(
                    "Shutdown requested while job {} is running; waiting up to {:?} for it to finish",
                    job_id, self.shutdown_grace_period
                );
                tokio::time::timeout(self.shutdown_grace_period, &mut processing).await
            }
        };

        match outcome {
            Ok(outcome) => (self.log_process_outcome(job_id, outcome), true),
            Err(_) => {
                warn!(
                    "Job {} did not finish within the {:?} shutdown grace period and was \
                     cancelled; it stays Running until its lease expires and the stale job \
                     reaper reclaims it",
                    job_id, self.shutdown_grace_period
                );
                (Ok(()), true)
            }
        }
    }

    fn log_process_outcome(&self, job_id: crate::job::JobId, outcome: Result<()>) -> Result<()> {
        if let Err(ref e) = outcome {
            error!(
                "Failed to record the outcome of job {} on queue {}: {}",
                job_id, self.queue_name, e
            );
        }
        outcome
    }

    /// Base delay for the error backoff: the throttle's `backoff_on_error` if set,
    /// otherwise the poll interval.
    fn error_backoff_base(&self) -> Duration {
        self.throttle_config
            .as_ref()
            .and_then(|throttle| throttle.backoff_on_error)
            .unwrap_or(self.poll_interval)
    }

    /// Wait for capacity and try to dequeue a job.
    ///
    /// Returns `Idle` after waiting for the poll interval when the queue is empty or
    /// paused, `Shutdown` when shutdown is requested during a wait, and `Err` when the
    /// dequeue itself fails. Database calls are never interrupted by shutdown.
    async fn acquire_job(&self, shutdown_rx: &mut mpsc::Receiver<()>) -> Result<Acquired> {
        // Alert checks and queue depth metrics run in the monitoring task
        // (`start_monitoring_task`), never here: a slow alert endpoint or a COUNT(*) must
        // not hold up dequeuing.

        // Take a rate limit token before dequeuing. It is handed back when no job is
        // claimed, so empty or paused polls never spend the budget.
        let rate_token = match &self.rate_limiter {
            Some(rate_limiter) => {
                let permit = tokio::select! {
                    biased;
                    _ = shutdown_rx.recv() => return Ok(Acquired::Shutdown),
                    permit = rate_limiter.acquire() => permit,
                };
                if let Err(e) = permit {
                    warn!("Rate limiter error: {}", e);
                    return Ok(self.idle_wait(shutdown_rx).await);
                }
                Some(rate_limiter)
            }
            None => None,
        };
        let job = self.claim_job().await;
        if !matches!(job, Ok(Some(_)))
            && let Some(rate_limiter) = rate_token
        {
            rate_limiter.refund();
        }

        match job? {
            Some(job) => {
                debug!(
                    "Processing job: {} with priority: {:?}",
                    job.id, job.priority
                );
                Ok(Acquired::Job(Box::new(job)))
            }
            // No job available (or the queue is paused): wait before polling again.
            None => Ok(self.idle_wait(shutdown_rx).await),
        }
    }

    /// Claim the next job of this worker's queue, with this worker's lease.
    async fn claim_job(&self) -> Result<Option<Job>> {
        // Check if the queue is paused
        match self.queue.is_queue_paused(&self.queue_name).await {
            Ok(true) => {
                debug!(
                    "Queue '{}' is paused, skipping job dequeue",
                    self.queue_name
                );
                return Ok(None);
            }
            Ok(false) => {}
            Err(e) => {
                // The dequeue checks the pause itself, so carry on.
                warn!("Failed to check queue pause status: {}", e);
            }
        }

        self.queue
            .dequeue_leased(
                &self.queue_name,
                self.priority_weights.as_ref(),
                self.lease_duration,
            )
            .await
    }

    /// Sleep for the poll interval (at least [`MIN_POLL_INTERVAL`]), returning early
    /// with `Shutdown` if shutdown is requested meanwhile.
    async fn idle_wait(&self, shutdown_rx: &mut mpsc::Receiver<()>) -> Acquired {
        tokio::select! {
            biased;
            _ = shutdown_rx.recv() => Acquired::Shutdown,
            _ = sleep(self.poll_interval.max(MIN_POLL_INTERVAL)) => Acquired::Idle,
        }
    }

    /// The number of attempts `job` gets on this worker: its own `max_attempts`,
    /// lowered by [`with_max_retries`](Self::with_max_retries) when that is set.
    fn attempt_limit(&self, job: &Job) -> i32 {
        effective_attempt_limit(job.max_attempts, self.max_retries)
    }

    /// When a failed or timed-out run of `job` should be retried.
    ///
    /// Uses, in order: the job's own retry strategy, the worker's default strategy,
    /// and the fixed retry delay.
    fn retry_at_for(&self, job: &Job) -> DateTime<Utc> {
        let next_attempt = u32::try_from(job.attempts.saturating_add(1)).unwrap_or(u32::MAX);
        let retry_delay = if let Some(ref job_strategy) = job.retry_strategy {
            job_strategy.calculate_delay(next_attempt)
        } else if let Some(ref default_strategy) = self.default_retry_strategy {
            default_strategy.calculate_delay(next_attempt)
        } else {
            self.retry_delay
        };
        retry_at_from_delay(Utc::now(), retry_delay)
    }

    /// Record the outcome of this run of `job`.
    ///
    /// Returns `None` when the job is no longer `Running` this run: it was reclaimed by
    /// the stale-job reaper, changed by an operator, or is being run again by another
    /// worker. The outcome is then discarded so this (late) worker cannot overwrite the
    /// job's newer state, and the caller skips its hooks and events.
    async fn record_outcome(
        &self,
        job: &Job,
        outcome: JobOutcome,
    ) -> Result<Option<RecordedOutcome>> {
        self.record(job, outcome, None).await
    }

    /// Record the outcome of this run of `job`; with `children`, record its completion
    /// and enqueue the children in one transaction
    /// ([`DatabaseQueue::complete_job_run_with_children`]). See
    /// [`record_outcome`](Self::record_outcome).
    async fn record(
        &self,
        job: &Job,
        outcome: JobOutcome,
        children: Option<Vec<Job>>,
    ) -> Result<Option<RecordedOutcome>> {
        // Record the outcome in a spawned task so that it always runs to commit or
        // rollback, even if this future is dropped (for example when the shutdown grace
        // period expires). Cancelling a database transaction mid-flight can leave its
        // connection in the pool with the transaction still open.
        let queue = Arc::clone(&self.queue);
        let finished_job = job.clone();
        let recorded = tokio::spawn(async move {
            match children {
                Some(children) => {
                    queue
                        .complete_job_run_with_children(&finished_job, children)
                        .await
                }
                None => queue.finish_job_run(&finished_job, outcome).await,
            }
        })
        .await
        .map_err(|e| HammerworkError::Worker {
            message: format!("recording the outcome of job {} failed: {e}", job.id),
        })??;
        match recorded {
            Some(recorded) => {
                if let Some(next_run_at) = recorded.next_run_at {
                    info!(
                        "Rescheduled recurring job {} for its next run at {}",
                        job.id, next_run_at
                    );
                }
                if !recorded.unblocked.is_empty() {
                    debug!(
                        "Job {} completed; {} dependent job(s) are now runnable",
                        job.id,
                        recorded.unblocked.len()
                    );
                }
                if !recorded.cancelled.is_empty() {
                    warn!(
                        "Job {} failed; {} dependent, workflow or batch job(s) were failed with it",
                        job.id,
                        recorded.cancelled.len()
                    );
                }
                Ok(Some(recorded))
            }
            None => {
                warn!(
                    "Job {} is no longer running attempt {} (it was reclaimed as stale, \
                     changed by an operator or is running again elsewhere); discarding the \
                     outcome of this run",
                    job.id, job.attempts
                );
                Ok(None)
            }
        }
    }

    async fn process_job(&self, job: Job) -> Result<()> {
        let job_id = job.id;
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
            queue_name: job.queue_name.clone(),
            event_type: JobEventType::Started,
            priority: job.priority,
            processing_time_ms: None,
            error_message: None,
            timestamp: start_time,
        })
        .await;

        // The job's own max_attempts decides whether a failed run is retried.
        let attempts_left = job.attempts < self.attempt_limit(&job);

        // Only the handler sees an encrypted job's plaintext payload; hooks, events and
        // the recorded outcome use the stored (redacted) job. A payload that cannot be
        // decrypted (no engine, missing key, tampered data) fails this run.
        let handler_job = match self.queue.decrypt_job(job.clone()).await {
            Ok(handler_job) => handler_job,
            Err(e) => return self.handle_failure(&job, e, attempts_left).await,
        };

        // Spawning reads `_spawn_config` from the decrypted payload: for an encrypted job the
        // stored payload is a placeholder or redacted, so the configuration is only visible
        // here.
        let spawn_parent = self.spawn_manager.is_some().then(|| handler_job.clone());

        // Determine timeout duration (job-specific or default). A zero timeout, on the job
        // or as the worker default, means none (see `Job::with_timeout` and
        // `with_default_timeout`).
        let timeout_duration = job
            .timeout
            .filter(|timeout| !timeout.is_zero())
            .or(self.default_timeout.filter(|timeout| !timeout.is_zero()));

        // Ok(handler result), or Err(timeout) when the handler ran out of time.
        let handler_result = match timeout_duration {
            Some(timeout) => tokio::time::timeout(timeout, self.execute_handler(handler_job))
                .await
                .map_err(|_| timeout),
            None => Ok(self.execute_handler(handler_job).await),
        };

        match handler_result {
            Ok(Ok(job_result)) => {
                self.handle_success(&job, spawn_parent, job_result, start_time, attempts_left)
                    .await
            }
            Ok(Err(e)) => self.handle_failure(&job, e, attempts_left).await,
            Err(timeout) => self.handle_timeout(&job, timeout, attempts_left).await,
        }
    }

    /// The child jobs `job` spawns, with their configuration, or `None` when it spawns
    /// none (no spawn manager, no `_spawn_config`, or no handler for its queue).
    ///
    /// `spawn_parent` is the decrypted job: an encrypted job's stored payload does not
    /// carry the spawn configuration. A malformed configuration or a failing spawn
    /// handler is an error, which fails the run.
    async fn prepare_spawn(
        &self,
        job: &Job,
        spawn_parent: Option<&Job>,
    ) -> Result<Option<(Vec<Job>, crate::spawn::SpawnConfig)>> {
        let Some(spawn_manager) = &self.spawn_manager else {
            return Ok(None);
        };
        let parent = spawn_parent.unwrap_or(job);
        let Some(config) = crate::spawn::spawn_config_from_payload(&parent.payload)? else {
            return Ok(None);
        };
        match spawn_manager
            .prepare_spawn(parent, &config, self.queue.clone())
            .await?
        {
            Some(children) => Ok(Some((children, config))),
            None => {
                debug!(
                    "No spawn handler registered for job type: {}",
                    job.queue_name
                );
                Ok(None)
            }
        }
    }

    async fn handle_success(
        &self,
        job: &Job,
        spawn_parent: Option<Job>,
        job_result: JobResult,
        start_time: DateTime<Utc>,
        attempts_left: bool,
    ) -> Result<()> {
        let job_id = job.id;
        debug!("Job {} completed successfully", job_id);

        let processing_time_ms = (Utc::now() - start_time).num_milliseconds().max(0) as u64;

        // Store the result before recording the completion, so that anyone who sees the
        // job as Completed can also read its result.
        if let Some(result_data) = job_result.data
            && let crate::job::ResultStorage::Database = job.result_config.storage
        {
            let expires_at = job
                .result_config
                .ttl
                .map(|ttl| crate::queue::saturating_add_to(Utc::now(), ttl));

            if let Err(e) = self
                .queue
                .store_job_result(job_id, result_data, expires_at)
                .await
            {
                error!("Failed to store result for job {}: {}", job_id, e);
            } else {
                debug!("Stored result for job {}", job_id);
            }
        }

        // Build the children the job spawns before completing it: they are enqueued in
        // the transaction that completes the parent, and a spawn failure fails this run
        // (so it is retried) instead of completing the parent without its children.
        let spawn = match self.prepare_spawn(job, spawn_parent.as_ref()).await {
            Ok(spawn) => spawn,
            Err(e) => {
                warn!("Failed to spawn child jobs for job {}: {}", job_id, e);
                return self.handle_failure(job, e, attempts_left).await;
            }
        };
        let (children, spawn_config) = match spawn {
            Some((children, config)) => (Some(children), Some(config)),
            None => (None, None),
        };

        // Completing also reschedules a recurring job, and makes dependents whose
        // dependencies have all completed runnable, in the same transaction.
        let recorded = match self.record(job, JobOutcome::Completed, children).await {
            Ok(Some(recorded)) => recorded,
            Ok(None) => return Ok(()),
            // Enqueueing a child failed, so nothing was written and the job is still
            // Running this run: fail the run so it is retried.
            Err(e) if spawn_config.is_some() => {
                warn!("Failed to enqueue the child jobs of job {}: {}", job_id, e);
                return self.handle_failure(job, e, attempts_left).await;
            }
            Err(e) => return Err(e),
        };

        if let (Some(config), Some(spawn_manager)) = (spawn_config, &self.spawn_manager) {
            let spawn_result = crate::spawn::SpawnResult {
                parent_job_id: job_id,
                spawned_jobs: recorded.spawned.clone(),
                spawned_at: Utc::now(),
                spawn_operation_id: config.operation_id,
            };
            info!(
                "Job {} spawned {} child jobs: {:?}",
                job_id,
                spawn_result.spawned_jobs.len(),
                spawn_result.spawned_jobs
            );
            let parent = spawn_parent.as_ref().unwrap_or(job);
            if let Err(e) = spawn_manager
                .notify_spawn_complete(parent, &spawn_result)
                .await
            {
                warn!("Spawn completion callback for job {} failed: {}", job_id, e);
            }
        }

        // Update batch statistics for successful completion
        if self.batch_processing_enabled
            && let Some(batch_id) = job.batch_id
        {
            self.update_batch_stats(|stats| {
                stats.jobs_completed += 1;
                stats.total_processing_time_ms += processing_time_ms;
                stats.update_average_processing_time();
            });

            if let Err(e) = self.check_and_update_batch_status(batch_id).await {
                warn!(
                    "Failed to update batch status for batch {}: {}",
                    batch_id, e
                );
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
            queue_name: job.queue_name.clone(),
            event_type: JobEventType::Completed,
            priority: job.priority,
            processing_time_ms: Some(processing_time_ms),
            error_message: None,
            timestamp: Utc::now(),
        })
        .await;

        Ok(())
    }

    async fn handle_failure(
        &self,
        job: &Job,
        error: HammerworkError,
        attempts_left: bool,
    ) -> Result<()> {
        let job_id = job.id;
        error!("Job {} failed: {}", job_id, error);
        let error_message = error.to_string();

        // Record span error status
        #[cfg(feature = "tracing")]
        {
            let span = tracing::Span::current();
            span.record("error", true);
            span.record("error.type", "job_failure");
            span.record("error.message", &error_message);
            span.record("job.will_retry", attempts_left);
        }

        let outcome = if attempts_left {
            JobOutcome::Retry {
                retry_at: self.retry_at_for(job),
                error: error_message.clone(),
                timed_out: false,
            }
        } else {
            warn!(
                "Job {} failed on its last attempt ({} of {}), marking it dead",
                job_id,
                job.attempts,
                self.attempt_limit(job)
            );
            JobOutcome::Dead {
                error: error_message.clone(),
            }
        };
        let Some(recorded) = self.record_outcome(job, outcome).await? else {
            return Ok(());
        };

        self.after_failed_run(job, &error_message, attempts_left)
            .await;

        if attempts_left {
            info!(
                "Retrying job {} (attempt {} of {})",
                job_id,
                job.attempts + 1,
                self.attempt_limit(job)
            );
            self.event_hooks
                .fire_job_retry(job.clone(), error_message.clone());
            self.record_event(JobEvent {
                job_id,
                queue_name: job.queue_name.clone(),
                event_type: JobEventType::Retried,
                priority: job.priority,
                processing_time_ms: None,
                error_message: Some(error_message),
                timestamp: Utc::now(),
            })
            .await;
        } else {
            debug!("Job {} run ended with status {:?}", job_id, recorded.status);
            self.event_hooks
                .fire_job_fail(job.clone(), error_message.clone());
            self.record_event(JobEvent {
                job_id,
                queue_name: job.queue_name.clone(),
                event_type: JobEventType::Dead,
                priority: job.priority,
                processing_time_ms: None,
                error_message: Some(error_message),
                timestamp: Utc::now(),
            })
            .await;
        }

        Ok(())
    }

    /// A timed-out run is retried like a failure while the job has attempts left, and
    /// only ends the job as `TimedOut` on its last attempt.
    async fn handle_timeout(
        &self,
        job: &Job,
        timeout: Duration,
        attempts_left: bool,
    ) -> Result<()> {
        let job_id = job.id;
        let error_message = format!("Job timed out after {:?}", timeout);
        warn!("Job {} timed out after {:?}", job_id, timeout);

        let outcome = if attempts_left {
            JobOutcome::Retry {
                retry_at: self.retry_at_for(job),
                error: error_message.clone(),
                timed_out: true,
            }
        } else {
            JobOutcome::TimedOut {
                error: error_message.clone(),
            }
        };
        if self.record_outcome(job, outcome).await?.is_none() {
            return Ok(());
        }

        // Record span timeout status
        #[cfg(feature = "tracing")]
        {
            let span = tracing::Span::current();
            span.record("error", true);
            span.record("error.type", "timeout");
            span.record("error.message", &error_message);
            span.record("job.will_retry", attempts_left);
        }

        self.after_failed_run(job, &error_message, attempts_left)
            .await;

        // Fire job timeout event hook
        self.event_hooks.fire_job_timeout(job.clone(), timeout);

        // Record timeout event
        self.record_event(JobEvent {
            job_id,
            queue_name: job.queue_name.clone(),
            event_type: JobEventType::TimedOut,
            priority: job.priority,
            processing_time_ms: Some(timeout.as_millis() as u64),
            error_message: Some(error_message.clone()),
            timestamp: Utc::now(),
        })
        .await;

        if attempts_left {
            info!(
                "Retrying timed-out job {} (attempt {} of {})",
                job_id,
                job.attempts + 1,
                self.attempt_limit(job)
            );
            self.event_hooks
                .fire_job_retry(job.clone(), error_message.clone());
            self.record_event(JobEvent {
                job_id,
                queue_name: job.queue_name.clone(),
                event_type: JobEventType::Retried,
                priority: job.priority,
                processing_time_ms: None,
                error_message: Some(error_message),
                timestamp: Utc::now(),
            })
            .await;
        }

        Ok(())
    }

    /// Batch statistics after a failed (or timed-out) run.
    async fn after_failed_run(&self, job: &Job, error_message: &str, attempts_left: bool) {
        if !self.batch_processing_enabled {
            return;
        }
        let Some(batch_id) = job.batch_id else {
            return;
        };
        self.update_batch_stats(|stats| {
            stats.jobs_failed += 1;
        });
        if attempts_left {
            return;
        }
        if let Err(e) = self
            .handle_batch_job_failure(batch_id, job.id, error_message)
            .await
        {
            warn!(
                "Failed to handle batch job failure for batch {}: {}",
                batch_id, e
            );
        }
        if let Err(e) = self.check_and_update_batch_status(batch_id).await {
            warn!(
                "Failed to update batch status for batch {}: {}",
                batch_id, e
            );
        }
    }
    /// Create the handler future for `job`, unifying both handler types.
    fn start_handler(&self, job: Job) -> HandlerFuture {
        match &self.handler {
            JobHandlerType::Legacy(handler) => {
                // Execute legacy handler and convert () to JobResult
                let future = handler(job);
                Box::pin(async move { future.await.map(|_| JobResult::success()) })
            }
            // Execute enhanced handler directly
            JobHandlerType::WithResult(handler) => handler(job),
        }
    }

    /// Execute a job handler, returning a unified result.
    ///
    /// A panic in the handler (while creating or polling its future) is caught and
    /// turned into an error, so the job goes through the normal retry/failure path
    /// instead of killing the worker. While the handler runs, the job's lease is
    /// renewed in the background.
    async fn execute_handler(&self, job: Job) -> Result<JobResult> {
        let job_id = job.id;
        // The run whose lease is renewed: the job as dequeued (same id, attempts and
        // started_at), without its payload.
        let run = Job {
            payload: serde_json::Value::Null,
            ..job.clone()
        };
        let handler_future =
            match std::panic::catch_unwind(AssertUnwindSafe(|| self.start_handler(job))) {
                Ok(future) => future,
                Err(payload) => {
                    error!("Handler for job {} panicked", job_id);
                    return Err(handler_panic_error(payload.as_ref()));
                }
            };

        let guarded = CatchUnwind {
            inner: handler_future,
        };

        tokio::select! {
            biased;
            outcome = guarded => outcome.unwrap_or_else(|payload| {
                error!("Handler for job {} panicked", job_id);
                Err(handler_panic_error(payload.as_ref()))
            }),
            () = self.maintain_lease(&run) => Err(HammerworkError::Worker {
                message: "job lease heartbeat stopped unexpectedly".to_string(),
            }),
        }
    }

    /// Renew the lease on a running job until the surrounding future is dropped.
    ///
    /// Heartbeats every third of the lease duration; the first one is sent after that
    /// interval, so short jobs never pay for a heartbeat write. Never completes.
    async fn maintain_lease(&self, run: &Job) {
        let job_id = run.id;
        if self.lease_duration.is_zero() {
            return std::future::pending().await;
        }
        let interval = (self.lease_duration / 3).max(MIN_HEARTBEAT_INTERVAL);
        loop {
            sleep(interval).await;
            match self.queue.heartbeat_job(run, self.lease_duration).await {
                Ok(true) => debug!("Renewed lease on job {}", job_id),
                Ok(false) => {
                    warn!(
                        "Lost the lease on job {}: it is no longer Running this run (it may \
                         have been reclaimed as stale); the handler keeps running",
                        job_id
                    );
                    return std::future::pending().await;
                }
                Err(e) => warn!("Failed to renew the lease on job {}: {}", job_id, e),
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

        // PartialFailureMode (e.g. FailFast failing the remaining jobs) was already
        // applied by DatabaseQueue::finish_job_run, in the transaction that ended the job.

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
        if let Some(stats_collector) = &self.stats_collector
            && let Err(e) = stats_collector.record_event(event.clone()).await
        {
            warn!("Failed to record statistics event: {}", e);
        }

        // Record to metrics collector
        #[cfg(feature = "metrics")]
        if let Some(metrics_collector) = &self.metrics_collector
            && let Err(e) = metrics_collector.record_job_event(&event).await
        {
            warn!("Failed to record metrics event: {}", e);
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
        ) && let Ok(mut last_time) = self.last_job_time.write()
        {
            *last_time = event.timestamp;
        }
    }

    /// Start a background monitoring task for metrics and alerting
    #[cfg(any(feature = "metrics", feature = "alerting"))]
    fn start_monitoring_task(&self) -> tokio::task::JoinHandle<()> {
        let queue_name = self.queue_name.clone();

        let queue = Arc::clone(&self.queue);

        #[cfg(feature = "alerting")]
        let last_job_time = Arc::clone(&self.last_job_time);

        #[cfg(feature = "metrics")]
        let metrics_collector = self.metrics_collector.clone();

        #[cfg(feature = "alerting")]
        let alert_manager = self.alert_manager.clone();

        #[cfg(feature = "alerting")]
        let stats_collector = self.stats_collector.clone();

        // An explicit interval wins, then the metrics collector's update interval.
        #[cfg(feature = "metrics")]
        let collector_interval = metrics_collector.as_ref().map(|m| m.update_interval());
        #[cfg(not(feature = "metrics"))]
        let collector_interval: Option<Duration> = None;
        let monitor_every = self
            .monitoring_interval
            .or(collector_interval)
            .unwrap_or(DEFAULT_MONITORING_INTERVAL);

        tokio::spawn(async move {
            // `interval` panics on a zero period.
            let mut interval = tokio::time::interval(monitor_every.max(MIN_MONITORING_INTERVAL));

            loop {
                interval.tick().await;

                // Update queue depth metrics and check queue depth alerts
                #[cfg(feature = "metrics")]
                let wants_depth = metrics_collector.is_some();
                #[cfg(not(feature = "metrics"))]
                let wants_depth = false;
                #[cfg(feature = "alerting")]
                let wants_depth = wants_depth || alert_manager.is_some();
                let queue_depth = if wants_depth {
                    match queue.get_queue_depth(&queue_name).await {
                        Ok(depth) => Some(depth),
                        Err(e) => {
                            warn!("Failed to get queue depth for monitoring: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                #[cfg(feature = "metrics")]
                if let (Some(metrics_collector), Some(queue_depth)) =
                    (&metrics_collector, queue_depth)
                    && let Err(e) = metrics_collector
                        .update_queue_depth(&queue_name, queue_depth)
                        .await
                {
                    warn!("Failed to update queue depth metrics: {}", e);
                }

                #[cfg(feature = "alerting")]
                if let (Some(alert_manager), Some(queue_depth)) = (&alert_manager, queue_depth)
                    && let Err(e) = alert_manager
                        .check_queue_depth(&queue_name, queue_depth)
                        .await
                {
                    warn!("Failed to check queue depth alerts: {}", e);
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
                    if let Some(last_time_value) = last_time_value
                        && let Err(e) = alert_manager
                            .check_worker_starvation(&queue_name, last_time_value)
                            .await
                    {
                        warn!("Failed to check worker starvation: {}", e);
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

/// A pool of [`Worker`]s run and supervised together.
///
/// [`WorkerPool::start`] spawns every worker plus a supervisor. A worker that dies
/// unexpectedly (for example because of a bug that panics outside a job handler) is
/// logged and restarted. [`WorkerPool::shutdown`] (or dropping the pool) stops all
/// workers gracefully: each finishes its in-flight job within its shutdown grace period.
///
/// The pool also runs a stale job reaper that periodically calls
/// [`DatabaseQueue::requeue_stale_jobs`] to reclaim jobs left `Running` by workers
/// that crashed. Configure it with [`WorkerPool::with_stale_job_reaper`] or disable it
/// with [`WorkerPool::without_stale_job_reaper`].
///
/// Optionally, the pool also enforces the retention of encrypted jobs by calling
/// [`DatabaseQueue::purge_expired_encrypted_jobs`] periodically
/// ([`WorkerPool::with_encrypted_job_purge`]).
///
/// These maintenance tasks run each pass in its own task, so shutting the pool down
/// never cancels a pass part-way through its database transaction.
pub struct WorkerPool<DB: Database> {
    workers: Vec<Worker<DB>>,
    /// Signals shutdown to the supervisor started by `start` (dropping it also does)
    shutdown_signal: Option<watch::Sender<bool>>,
    /// Becomes `true` once every worker has stopped after a shutdown
    supervisor_done: Option<watch::Receiver<bool>>,
    /// How often the stale job reaper runs (`None` disables it)
    reaper_interval: Option<Duration>,
    /// Fallback staleness threshold for jobs without a lease (`None`: the longest
    /// worker lease duration)
    reaper_older_than: Option<Duration>,
    /// How often encrypted jobs whose retention ended are purged (`None`: never)
    encrypted_job_purge_interval: Option<Duration>,
    /// Automatic archival and purge of archived jobs (`None`: off)
    archival: Option<PoolArchival>,
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
    /// Create an empty pool. Autoscaling is off; enable it with
    /// [`with_autoscaling`](Self::with_autoscaling).
    pub fn new() -> Self {
        Self {
            workers: Vec::new(),
            shutdown_signal: None,
            supervisor_done: None,
            reaper_interval: Some(DEFAULT_STALE_JOB_REAPER_INTERVAL),
            reaper_older_than: None,
            encrypted_job_purge_interval: None,
            archival: None,
            stats_collector: None,
            worker_template: None,
            autoscale_config: AutoscaleConfig::disabled(),
            autoscale_metrics: Arc::new(std::sync::RwLock::new(AutoscaleMetrics::default())),
            queue_depth_history: Arc::new(std::sync::RwLock::new(Vec::new())),
            autoscale_task: None,
        }
    }

    /// Build a pool from a [`WorkerConfig`](crate::config::WorkerConfig).
    ///
    /// Adds `pool_size` copies of `worker` (at least one), uses it as the autoscaling
    /// template, and configures autoscaling from `autoscaling_enabled`, `min_workers`
    /// and `max_workers`. Apply the per-worker settings to `worker` first with
    /// [`Worker::with_config`] or [`Worker::with_hammerwork_config`].
    pub fn from_config(worker: Worker<DB>, config: &crate::config::WorkerConfig) -> Self {
        let mut pool = Self::new()
            .with_autoscaling(config.autoscale_config())
            .with_worker_template(worker.clone());
        for _ in 1..config.pool_size {
            pool.add_worker(worker.clone());
        }
        pool.add_worker(worker);
        pool
    }

    /// Build a pool from a [`HammerworkConfig`](crate::config::HammerworkConfig).
    ///
    /// Like [`WorkerPool::from_config`] with `config.worker`, and also schedules the
    /// encrypted job retention purge every `encryption.purge_interval_secs`, when set
    /// ([`WorkerPool::with_encrypted_job_purge`]), and automatic archival every
    /// `archive.check_interval` when `archive.enabled` ([`WorkerPool::with_archival`]
    /// with [`ArchiveConfig::archival_policy`](crate::config::ArchiveConfig::archival_policy)
    /// and [`ArchiveConfig::archival_config`](crate::config::ArchiveConfig::archival_config)).
    /// Validates the `encryption`, `worker` and `archive` sections.
    pub fn from_hammerwork_config(
        worker: Worker<DB>,
        config: &crate::config::HammerworkConfig,
    ) -> Result<Self> {
        config.encryption.validate()?;
        config.worker.validate()?;
        config.archive.validate()?;
        let mut pool = Self::from_config(worker, &config.worker);
        pool.encrypted_job_purge_interval = config.encryption.purge_interval();
        if config.archive.enabled {
            pool = pool.with_archival(
                config.archive.archival_policy(),
                config.archive.archival_config(),
                config.archive.check_interval,
            );
        }
        Ok(pool)
    }

    pub fn with_stats_collector(mut self, stats_collector: Arc<dyn StatisticsCollector>) -> Self {
        self.stats_collector = Some(stats_collector);
        self
    }

    /// Configure autoscaling for the worker pool.
    ///
    /// Autoscaling is off unless enabled here (or by
    /// [`from_config`](Self::from_config) with `autoscaling_enabled`). It scales one
    /// queue: that of the [worker template](Self::with_worker_template), or of the first
    /// worker added when no template is set. It measures that queue's depth, starts
    /// clones of the template while it is deep and retires workers of that queue (never
    /// below `min_workers`) once it drains. Workers of other queues are never started
    /// or retired. A retired worker stops like on shutdown: it finishes its in-flight
    /// job within its [shutdown grace period](Worker::with_shutdown_grace_period).
    pub fn with_autoscaling(mut self, config: AutoscaleConfig) -> Self {
        self.autoscale_config = config;
        self
    }

    /// Disable autoscaling for the worker pool
    pub fn without_autoscaling(mut self) -> Self {
        self.autoscale_config = AutoscaleConfig::disabled();
        self
    }

    /// Configure the stale job reaper.
    ///
    /// Every `interval` the pool calls
    /// [`DatabaseQueue::requeue_stale_jobs`]`(older_than)`, which moves `Running` jobs
    /// whose lease expired back to `Pending` (or to `Dead` when they have no attempts
    /// left). Workers take their lease when they claim a job, so a job whose worker is
    /// alive is never reclaimed, whatever this pool's settings or those of other pools.
    /// `older_than` only applies to running jobs without a lease of their own (claimed
    /// by an older Hammerwork version); by default it is the longest
    /// [lease duration](Worker::with_lease_duration) of the pool's workers. The reaper
    /// is enabled by default with a 60 second interval.
    ///
    /// Requires migration `015_add_job_leases`. Running reapers in several pools or
    /// processes at once, with different settings, is safe.
    pub fn with_stale_job_reaper(mut self, interval: Duration, older_than: Duration) -> Self {
        self.reaper_interval = Some(interval);
        self.reaper_older_than = Some(older_than);
        self
    }

    /// Disable the stale job reaper (for example when it runs elsewhere, such as a
    /// scheduled `cargo hammerwork job requeue-stale`).
    pub fn without_stale_job_reaper(mut self) -> Self {
        self.reaper_interval = None;
        self
    }

    /// Purge encrypted jobs whose retention period ended every `interval`.
    ///
    /// The pool calls [`DatabaseQueue::purge_expired_encrypted_jobs`], which deletes
    /// finished encrypted jobs (and archived ones) past their `retention_delete_at`; it
    /// needs no encryption key. Off by default: without it, run
    /// `cargo hammerwork maintenance purge-encrypted` on a schedule. Running purges in
    /// several pools or processes at once is safe.
    ///
    /// The first purge runs when the pool starts. Each purge runs in its own task, so
    /// shutting the pool down never cancels one part-way through its transaction.
    pub fn with_encrypted_job_purge(mut self, interval: Duration) -> Self {
        self.encrypted_job_purge_interval = Some(interval);
        self
    }

    /// Disable the encrypted job retention purge (the default).
    pub fn without_encrypted_job_purge(mut self) -> Self {
        self.encrypted_job_purge_interval = None;
        self
    }

    /// Archive finished jobs, and purge old archived jobs, every `interval`.
    ///
    /// Each pass runs [`JobArchiver::run_scheduled_pass`](crate::archive::JobArchiver::run_scheduled_pass)
    /// with `policy` as the policy for every queue and `config` for compression: it
    /// moves completed, failed, dead and timed out jobs older than the policy's
    /// thresholds to `hammerwork_jobs_archive` (reason `Automatic`, archived by
    /// `"worker_pool"`), then deletes archived jobs older than `purge_archived_after`,
    /// when set. Results are logged. Off by default; a disabled `policy` does nothing.
    ///
    /// The first pass runs when the pool starts. Passes run in their own task, so
    /// workers are never blocked by them. On shutdown no new pass starts, and a running
    /// pass stops before its next batch; a batch that has started commits or rolls back
    /// as a whole. Running archival in several pools or processes at once is safe: rows
    /// are claimed with `FOR UPDATE SKIP LOCKED`, so each job is archived once.
    pub fn with_archival(
        mut self,
        policy: crate::archive::ArchivalPolicy,
        config: crate::archive::ArchivalConfig,
        interval: Duration,
    ) -> Self {
        self.archival = Some(PoolArchival {
            policy,
            config,
            interval,
        });
        self
    }

    /// Disable automatic archival (the default).
    pub fn without_archival(mut self) -> Self {
        self.archival = None;
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

        // If no worker template is set and autoscaling is enabled, use the first worker
        // as template (`start` also falls back to the first worker).
        if self.worker_template.is_none()
            && self.autoscale_config.enabled
            && self.workers.is_empty()
        {
            self.worker_template = Some(worker.clone());
        }

        self.workers.push(worker);
    }

    /// Start all workers and supervise them until the pool shuts down.
    ///
    /// Returns once every worker has stopped after [`WorkerPool::shutdown`] (or after
    /// the pool is dropped). Workers run in their own tasks, so dropping the returned
    /// future (for example in a `tokio::select!`) does not stop them; call
    /// [`WorkerPool::shutdown`] afterwards to stop them gracefully.
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting worker pool with {} workers", self.workers.len());

        let (signal_tx, signal_rx) = watch::channel(false);
        let (done_tx, done_rx) = watch::channel(false);
        self.shutdown_signal = Some(signal_tx);
        self.supervisor_done = Some(done_rx);

        let workers = std::mem::take(&mut self.workers);
        let maintenance: Vec<tokio::task::JoinHandle<()>> = [
            self.start_stale_job_reaper(&workers, signal_rx.clone()),
            self.start_encrypted_job_purge(&workers, signal_rx.clone()),
            self.start_archival(&workers, signal_rx.clone()),
        ]
        .into_iter()
        .flatten()
        .collect();

        // Start autoscaling task if enabled. It publishes the desired number of workers
        // on the template's queue, which the supervisor applies by starting workers from
        // the template or retiring workers of that queue.
        let scaling = if self.autoscale_config.enabled {
            self.start_autoscaling_task(&workers)
        } else {
            None
        };
        if let Ok(mut metrics) = self.autoscale_metrics.write() {
            metrics.active_workers = match &scaling {
                Some(scaling) => scaling.initial,
                None => workers.len(),
            };
        }

        let supervisor = tokio::spawn(Self::supervise(
            workers,
            signal_rx,
            done_tx,
            maintenance,
            scaling,
        ));

        // Dropping a JoinHandle detaches the task, so the supervisor keeps running
        // even if this future is dropped.
        supervisor.await.map_err(|e| HammerworkError::Worker {
            message: format!("Worker pool supervisor failed: {}", e),
        })?
    }

    /// Spawn one worker task with its own shutdown channel.
    fn spawn_worker(
        set: &mut JoinSet<(usize, std::thread::Result<Result<()>>)>,
        index: usize,
        worker: Worker<DB>,
        delay: Option<Duration>,
    ) -> mpsc::Sender<()> {
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        set.spawn(async move {
            if let Some(delay) = delay {
                sleep(delay).await;
            }
            // Catch panics so the supervisor always knows which worker stopped.
            let run = CatchUnwind {
                inner: Box::pin(async move { worker.run(shutdown_rx).await }),
            };
            (index, run.await)
        });
        shutdown_tx
    }

    /// Supervise worker tasks: restart workers that stop unexpectedly, and on
    /// shutdown signal every worker and wait for all of them to stop.
    async fn supervise(
        workers: Vec<Worker<DB>>,
        mut signal_rx: watch::Receiver<bool>,
        done_tx: watch::Sender<bool>,
        maintenance: Vec<tokio::task::JoinHandle<()>>,
        scaling: Option<Scaling<DB>>,
    ) -> Result<()> {
        let mut templates: Vec<Worker<DB>> = workers.to_vec();
        let mut set = JoinSet::new();
        let mut senders: Vec<mpsc::Sender<()>> = workers
            .into_iter()
            .enumerate()
            .map(|(index, worker)| Self::spawn_worker(&mut set, index, worker, None))
            .collect();
        // Workers stopped by a scale-down: not restarted, and their slot is reused by
        // the next scale-up once their task has finished.
        let mut retired: Vec<bool> = vec![false; senders.len()];
        let mut finished: Vec<bool> = vec![false; senders.len()];
        let (template, mut desired_rx) = match scaling {
            Some(scaling) => (Some(scaling.template), Some(scaling.desired)),
            None => (None, None),
        };

        let mut shutting_down = *signal_rx.borrow();
        if shutting_down {
            for tx in &senders {
                let _ = tx.try_send(());
            }
        }

        loop {
            tokio::select! {
                // An error means the pool (and its signal sender) was dropped: treat it
                // as a shutdown request too.
                _ = signal_rx.changed(), if !shutting_down => {
                    info!("Worker pool shutting down; waiting for in-flight jobs");
                    shutting_down = true;
                    for tx in &senders {
                        let _ = tx.try_send(());
                    }
                }
                changed = wait_for_change(&mut desired_rx), if !shutting_down => {
                    let (Some(desired), Some(template)) = (changed, template.as_ref()) else {
                        // The autoscaler stopped: keep the current workers.
                        desired_rx = None;
                        continue;
                    };
                    // Only the template's queue is scaled: workers of other queues are
                    // never retired, and new workers are clones of the template.
                    let active: Vec<usize> = (0..senders.len())
                        .filter(|i| !retired[*i] && templates[*i].queue_name == template.queue_name)
                        .collect();
                    if desired > active.len() {
                        for _ in active.len()..desired {
                            let worker = template.clone();
                            let reusable = (0..senders.len()).find(|i| retired[*i] && finished[*i]);
                            let index = reusable.unwrap_or(senders.len());
                            let tx = Self::spawn_worker(&mut set, index, worker.clone(), None);
                            if index == senders.len() {
                                templates.push(worker);
                                senders.push(tx);
                                retired.push(false);
                                finished.push(false);
                            } else {
                                templates[index] = worker;
                                senders[index] = tx;
                                retired[index] = false;
                                finished[index] = false;
                            }
                        }
                        info!(
                            "Autoscaling: started workers for queue {}, now {}",
                            template.queue_name, desired
                        );
                    } else {
                        for index in active.iter().rev().take(active.len() - desired) {
                            retired[*index] = true;
                            let _ = senders[*index].try_send(());
                        }
                        if desired < active.len() {
                            info!(
                                "Autoscaling: retiring workers of queue {}, now {}",
                                template.queue_name, desired
                            );
                        }
                    }
                }
                joined = set.join_next() => {
                    let Some(joined) = joined else { break };
                    let index = match joined {
                        Ok((index, Ok(Ok(())))) => {
                            if shutting_down {
                                continue;
                            }
                            if retired[index] {
                                finished[index] = true;
                                continue;
                            }
                            warn!("Worker {} stopped unexpectedly", index);
                            index
                        }
                        Ok((index, Ok(Err(e)))) => {
                            error!("Worker {} failed: {}", index, e);
                            index
                        }
                        Ok((index, Err(payload))) => {
                            error!(
                                "Worker {} panicked: {}",
                                index,
                                panic_message(payload.as_ref())
                            );
                            index
                        }
                        Err(e) => {
                            // Only reachable if a worker task is aborted externally.
                            error!("Worker task ended abnormally: {}", e);
                            continue;
                        }
                    };
                    if retired[index] {
                        finished[index] = true;
                    } else if !shutting_down {
                        info!("Restarting worker {} in {:?}", index, WORKER_RESTART_DELAY);
                        senders[index] = Self::spawn_worker(
                            &mut set,
                            index,
                            templates[index].clone(),
                            Some(WORKER_RESTART_DELAY),
                        );
                    }
                }
            }
        }

        // Stops the maintenance loops; a pass already running finishes in its own task
        for task in maintenance {
            task.abort();
        }
        info!("All workers stopped");
        done_tx.send_replace(true);
        Ok(())
    }

    /// Spawn the periodic stale job reaper, if enabled.
    fn start_stale_job_reaper(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let interval = self.reaper_interval?;
        let queue = Arc::clone(&workers.first()?.queue);
        let older_than = self.reaper_older_than.unwrap_or_else(|| {
            workers
                .iter()
                .map(|worker| worker.lease_duration)
                .max()
                .filter(|lease| !lease.is_zero())
                .unwrap_or(DEFAULT_LEASE_DURATION)
        });

        Some(spawn_periodic(interval, signal_rx, move || {
            let queue = Arc::clone(&queue);
            async move {
                match queue.requeue_stale_jobs(older_than).await {
                    Ok(recovery) if !recovery.is_empty() => info!(
                        "Stale job reaper requeued {} and marked {} dead",
                        recovery.requeued.len(),
                        recovery.dead.len()
                    ),
                    Ok(_) => {}
                    Err(e) => warn!("Stale job reaper failed: {}", e),
                }
            }
        }))
    }

    /// Spawn the periodic encrypted job retention purge, if enabled.
    fn start_encrypted_job_purge(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let interval = self.encrypted_job_purge_interval?;
        let queue = Arc::clone(&workers.first()?.queue);
        Some(spawn_periodic(interval, signal_rx, move || {
            let queue = Arc::clone(&queue);
            async move {
                match queue.purge_expired_encrypted_jobs().await {
                    Ok(purge) if purge.total() > 0 => info!(
                        "Encrypted job purge deleted {} jobs and {} archived jobs",
                        purge.jobs, purge.archived_jobs
                    ),
                    Ok(_) => {}
                    Err(e) => warn!("Encrypted job purge failed: {}", e),
                }
            }
        }))
    }

    /// Spawn the periodic archival pass, if enabled.
    fn start_archival(
        &self,
        workers: &[Worker<DB>],
        signal_rx: watch::Receiver<bool>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        let archival = self.archival.clone()?;
        let queue = Arc::clone(&workers.first()?.queue);
        if !archival.policy.enabled {
            info!("Automatic archival is configured with a disabled policy; not starting it");
            return None;
        }
        let archiver = Arc::new(
            crate::archive::JobArchiver::new(queue.pool.clone())
                .with_default_policy(archival.policy)
                .with_config(archival.config),
        );
        let stop_rx = signal_rx.clone();
        Some(spawn_periodic(archival.interval, signal_rx, move || {
            let (queue, archiver, stop_rx) =
                (Arc::clone(&queue), Arc::clone(&archiver), stop_rx.clone());
            async move {
                let pass = archiver
                    .run_scheduled_pass(queue.as_ref(), Some("worker_pool"), || *stop_rx.borrow())
                    .await;
                match pass {
                    Ok(pass) if pass.jobs_archived > 0 || pass.jobs_purged > 0 => info!(
                        "Archival archived {} jobs and purged {} archived jobs{}",
                        pass.jobs_archived,
                        pass.jobs_purged,
                        if pass.stopped_early {
                            " (stopped early for shutdown)"
                        } else {
                            ""
                        }
                    ),
                    Ok(_) => debug!("Archival found nothing to archive or purge"),
                    Err(e) => warn!("Archival failed: {}", e),
                }
            }
        }))
    }

    /// Start the autoscaling background task.
    ///
    /// Returns the worker template and a channel carrying the desired worker count,
    /// which the supervisor applies, or `None` when there is no worker template.
    fn start_autoscaling_task(&mut self, workers: &[Worker<DB>]) -> Option<Scaling<DB>> {
        // Without an explicit template, scale the first worker's queue.
        let Some(worker_template) = self.worker_template.as_ref().or(workers.first()) else {
            warn!("Cannot start autoscaling: no worker template available");
            return None;
        };
        let mut template = worker_template.clone();
        let initial_workers = workers
            .iter()
            .filter(|worker| worker.queue_name == template.queue_name)
            .count();
        if let Some(stats_collector) = &self.stats_collector {
            template.stats_collector = Some(Arc::clone(stats_collector));
        }
        let queue = Arc::clone(&template.queue);
        let queue_name = template.queue_name.clone();
        let config = self.autoscale_config.clone();
        let metrics = Arc::clone(&self.autoscale_metrics);
        let history = Arc::clone(&self.queue_depth_history);
        let (desired_tx, desired_rx) = watch::channel(initial_workers);

        let task = tokio::spawn(async move {
            Self::autoscaling_loop(queue, queue_name, config, metrics, history, desired_tx).await;
        });

        if let Some(previous) = self.autoscale_task.replace(task) {
            previous.abort();
        }
        info!(
            "Autoscaling task started for queue: {}",
            template.queue_name
        );
        Some(Scaling {
            template,
            desired: desired_rx,
            initial: initial_workers,
        })
    }

    /// Main autoscaling evaluation loop
    async fn autoscaling_loop(
        queue: Arc<JobQueue<DB>>,
        queue_name: String,
        config: AutoscaleConfig,
        metrics: Arc<std::sync::RwLock<AutoscaleMetrics>>,
        history: QueueDepthHistory,
        desired: watch::Sender<usize>,
    ) {
        // `interval` panics on a zero period, so never pass one through.
        let mut interval =
            tokio::time::interval((config.evaluation_window / 2).max(Duration::from_millis(1)));

        loop {
            interval.tick().await;

            match Self::evaluate_scaling_decision(&queue, &queue_name, &config, &metrics, &history)
                .await
            {
                Ok(Some(count)) => {
                    // An error means the supervisor stopped: nothing left to scale.
                    if desired.send(count).is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(e) => warn!("Autoscaling evaluation error: {}", e),
            }
        }
    }

    /// Evaluate whether scaling up or down is needed.
    ///
    /// Returns the new worker count when the pool should scale.
    async fn evaluate_scaling_decision(
        queue: &Arc<JobQueue<DB>>,
        queue_name: &str,
        config: &AutoscaleConfig,
        metrics: &Arc<std::sync::RwLock<AutoscaleMetrics>>,
        history: &QueueDepthHistory,
    ) -> Result<Option<usize>> {
        // Get current queue depth
        let current_depth = queue.get_queue_depth(queue_name).await?;
        Ok(apply_queue_depth(
            current_depth,
            Utc::now(),
            config,
            metrics,
            history,
        ))
    }

    /// Get current autoscaling metrics
    pub fn get_autoscale_metrics(&self) -> AutoscaleMetrics {
        if let Ok(metrics) = self.autoscale_metrics.read() {
            metrics.clone()
        } else {
            AutoscaleMetrics::default()
        }
    }

    /// Shut the pool down gracefully.
    ///
    /// Signals every worker to stop polling, then waits until all of them have
    /// stopped. Each worker lets its in-flight job finish within its
    /// [shutdown grace period](Worker::with_shutdown_grace_period).
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down worker pool");

        // Stop autoscaling task
        if let Some(task) = &self.autoscale_task {
            task.abort();
            info!("Autoscaling task stopped");
        }

        if let Some(signal) = &self.shutdown_signal {
            signal.send_replace(true);
        }

        if let Some(done) = &self.supervisor_done {
            let mut done = done.clone();
            // An error means the supervisor is gone, so there is nothing to wait for.
            let _ = done.wait_for(|finished| *finished).await;
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

/// Record a queue depth sample and decide whether to scale.
///
/// Returns the new worker count (within `min_workers..=max_workers`) when the
/// average depth per worker crossed a threshold outside the cooldown period.
fn apply_queue_depth(
    current_depth: u64,
    now: DateTime<Utc>,
    config: &AutoscaleConfig,
    metrics: &Arc<std::sync::RwLock<AutoscaleMetrics>>,
    history: &QueueDepthHistory,
) -> Option<usize> {
    // Update queue depth history, dropping entries outside the evaluation window
    let avg_depth = match history.write() {
        Ok(mut hist) => {
            hist.push((now, current_depth));
            let cutoff = now
                - chrono::Duration::from_std(config.evaluation_window)
                    .unwrap_or(chrono::Duration::seconds(30));
            hist.retain(|(timestamp, _)| *timestamp > cutoff);
            if hist.is_empty() {
                current_depth as f64
            } else {
                hist.iter().map(|(_, depth)| *depth as f64).sum::<f64>() / hist.len() as f64
            }
        }
        Err(_) => current_depth as f64,
    };

    let mut m = metrics.write().ok()?;
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
        return None;
    }

    // Calculate queue depth per worker
    let depth_per_worker = if m.active_workers > 0 {
        avg_depth / m.active_workers as f64
    } else {
        avg_depth
    };

    let decision = if depth_per_worker > config.scale_up_threshold as f64
        && m.active_workers < config.max_workers
    {
        ScalingDecision::ScaleUp
    } else if depth_per_worker < config.scale_down_threshold as f64
        && m.active_workers > config.min_workers
    {
        ScalingDecision::ScaleDown
    } else {
        return None;
    };

    let new_count = match decision {
        ScalingDecision::ScaleUp => (m.active_workers + config.scale_step).min(config.max_workers),
        ScalingDecision::ScaleDown => {
            (m.active_workers.saturating_sub(config.scale_step)).max(config.min_workers)
        }
    };
    info!(
        "Autoscaling: {:?} from {} to {} workers (avg queue depth: {:.1})",
        decision, m.active_workers, new_count, m.avg_queue_depth
    );
    m.active_workers = new_count;
    m.last_scale_time = Some(now);
    Some(new_count)
}

/// Automatic archival settings of a [`WorkerPool`] ([`WorkerPool::with_archival`]).
#[derive(Debug, Clone)]
struct PoolArchival {
    policy: crate::archive::ArchivalPolicy,
    config: crate::archive::ArchivalConfig,
    interval: Duration,
}

/// Spawn a task that calls `run` every `interval` (first right away) until shutdown is
/// signalled on `signal_rx` (or its sender is dropped).
///
/// Each run is spawned as its own task and awaited, so neither the shutdown signal nor
/// aborting the returned handle cancels a run part-way: database work always reaches
/// commit or rollback instead of leaving a pooled connection inside a transaction.
fn spawn_periodic<F, Fut>(
    interval: Duration,
    mut signal_rx: watch::Receiver<bool>,
    run: F,
) -> tokio::task::JoinHandle<()>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(1)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = signal_rx.changed() => break,
                _ = ticker.tick() => {}
            }
            if *signal_rx.borrow() {
                break;
            }
            if let Err(e) = tokio::spawn(run()).await
                && e.is_panic()
            {
                error!("Periodic maintenance task panicked: {}", e);
            }
        }
    })
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
    fn test_error_handling() {
        let error = HammerworkError::Worker {
            message: "Test error".to_string(),
        };

        assert_eq!(error.to_string(), "Worker error: Test error");
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

        // Check remaining tokens (refill is continuous, so a little has come back)
        let remaining_tokens = rate_limiter.available_tokens();
        assert!((8.0..8.5).contains(&remaining_tokens), "{remaining_tokens}");
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
            .with_evaluation_window(Duration::from_secs(45));

        assert_eq!(config.min_workers, 2);
        assert_eq!(config.max_workers, 20);
        assert_eq!(config.scale_up_threshold, 8);
        assert_eq!(config.scale_down_threshold, 1);
        assert_eq!(config.cooldown_period, Duration::from_secs(120));
        assert_eq!(config.scale_step, 2);
        assert_eq!(config.evaluation_window, Duration::from_secs(45));
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

    #[test]
    fn test_retry_at_from_delay_never_panics_and_clamps() {
        let now = Utc::now();
        assert_eq!(
            retry_at_from_delay(now, Duration::from_secs(30)),
            now + chrono::Duration::seconds(30)
        );
        let max = now + chrono::Duration::from_std(MAX_RETRY_DELAY).unwrap();
        assert_eq!(retry_at_from_delay(now, Duration::MAX), max);
        assert_eq!(retry_at_from_delay(now, MAX_RETRY_DELAY * 2), max);
        assert_eq!(
            retry_at_from_delay(DateTime::<Utc>::MAX_UTC, Duration::from_secs(1)),
            DateTime::<Utc>::MAX_UTC
        );
    }

    #[test]
    fn test_error_backoff_grows_is_capped_and_never_zero() {
        assert_eq!(error_backoff(Duration::ZERO, 1), MIN_ERROR_BACKOFF);
        assert_eq!(
            error_backoff(Duration::from_secs(1), 1),
            Duration::from_secs(1)
        );
        assert_eq!(
            error_backoff(Duration::from_secs(1), 3),
            Duration::from_secs(4)
        );
        assert_eq!(
            error_backoff(Duration::from_secs(1), u32::MAX),
            MAX_ERROR_BACKOFF
        );
        // A base above the cap is honoured rather than shortened.
        assert_eq!(
            error_backoff(Duration::from_secs(120), 5),
            Duration::from_secs(120)
        );
    }

    #[tokio::test]
    async fn test_catch_unwind_turns_panics_into_errors() {
        let ok: HandlerFuture = Box::pin(async { Ok(JobResult::success()) });
        assert!(CatchUnwind { inner: ok }.await.unwrap().is_ok());

        let panicking: HandlerFuture = Box::pin(async {
            tokio::task::yield_now().await;
            panic!("boom")
        });
        let payload = CatchUnwind { inner: panicking }.await.unwrap_err();
        let error = handler_panic_error(payload.as_ref());
        assert_eq!(
            error.to_string(),
            "Worker error: Job handler panicked: boom"
        );

        let formatted: HandlerFuture = Box::pin(async { panic!("code {}", 42) });
        let payload = CatchUnwind { inner: formatted }.await.unwrap_err();
        assert_eq!(panic_message(payload.as_ref()), "code 42");
    }

    #[tokio::test]
    async fn spawn_periodic_runs_until_shutdown() {
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (signal_tx, signal_rx) = watch::channel(false);
        let counter = Arc::clone(&runs);
        let task = spawn_periodic(Duration::from_millis(10), signal_rx, move || {
            let counter = Arc::clone(&counter);
            async move {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        signal_tx.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the loop stops on shutdown")
            .unwrap();
        let seen = runs.load(std::sync::atomic::Ordering::SeqCst);
        assert!(seen >= 2, "ran {seen} times");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), seen);
    }

    /// Shutting down (or aborting the loop, as the pool's supervisor does) while a run is
    /// in flight must not cancel the run: it would drop a database transaction mid-way.
    #[tokio::test]
    async fn spawn_periodic_never_cancels_a_run() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let started = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(AtomicBool::new(false));
        let (signal_tx, signal_rx) = watch::channel(false);
        let (run_started, run_finished) = (Arc::clone(&started), Arc::clone(&finished));
        let task = spawn_periodic(Duration::from_secs(3600), signal_rx, move || {
            let (started, finished) = (Arc::clone(&run_started), Arc::clone(&run_finished));
            async move {
                started.notify_one();
                tokio::time::sleep(Duration::from_millis(200)).await;
                finished.store(true, Ordering::SeqCst);
            }
        });
        started.notified().await;
        signal_tx.send_replace(true);
        task.abort();
        assert!(!finished.load(Ordering::SeqCst));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            finished.load(Ordering::SeqCst),
            "the in-flight run completed after the loop was stopped"
        );
    }

    #[cfg(any(feature = "postgres", feature = "mysql"))]
    #[test]
    fn encrypted_job_purge_is_opt_in_and_configurable() {
        #[cfg(feature = "postgres")]
        type Db = sqlx::Postgres;
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        type Db = sqlx::MySql;

        let pool = WorkerPool::<Db>::new();
        assert_eq!(pool.encrypted_job_purge_interval, None);
        let pool = pool.with_encrypted_job_purge(Duration::from_secs(30));
        assert_eq!(
            pool.encrypted_job_purge_interval,
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            pool.without_encrypted_job_purge()
                .encrypted_job_purge_interval,
            None
        );
    }

    #[cfg(any(feature = "postgres", feature = "mysql"))]
    #[test]
    fn archival_is_opt_in() {
        #[cfg(feature = "postgres")]
        type Db = sqlx::Postgres;
        #[cfg(all(feature = "mysql", not(feature = "postgres")))]
        type Db = sqlx::MySql;

        let pool = WorkerPool::<Db>::new();
        assert!(pool.archival.is_none());
        let policy = crate::archive::ArchivalPolicy::new().with_batch_size(7);
        let pool = pool.with_archival(
            policy,
            crate::archive::ArchivalConfig::new().with_compression_level(2),
            Duration::from_secs(90),
        );
        let archival = pool.archival.as_ref().unwrap();
        assert_eq!(archival.interval, Duration::from_secs(90));
        assert_eq!(archival.policy.batch_size, 7);
        assert_eq!(archival.config.compression_level, 2);
        assert!(pool.without_archival().archival.is_none());
    }

    #[test]
    fn panic_message_describes_every_payload_kind() {
        assert_eq!(panic_message(&"static str"), "static str");
        assert_eq!(panic_message(&"owned".to_string()), "owned");
        assert_eq!(panic_message(&42_u32), "non-string panic payload");
        assert_eq!(
            handler_panic_error(&"boom").to_string(),
            "Worker error: Job handler panicked: boom"
        );
    }

    #[test]
    fn shutdown_requested_on_signal_or_dropped_sender() {
        let (tx, mut rx) = mpsc::channel(1);
        assert!(!shutdown_requested(&mut rx));
        tx.try_send(()).unwrap();
        assert!(shutdown_requested(&mut rx));
        drop(tx);
        assert!(
            shutdown_requested(&mut rx),
            "a dropped sender means shutdown"
        );
    }

    fn autoscale_state(
        active_workers: usize,
    ) -> (Arc<std::sync::RwLock<AutoscaleMetrics>>, QueueDepthHistory) {
        (
            Arc::new(std::sync::RwLock::new(AutoscaleMetrics {
                active_workers,
                ..Default::default()
            })),
            Arc::new(std::sync::RwLock::new(Vec::new())),
        )
    }

    fn fast_autoscale() -> AutoscaleConfig {
        AutoscaleConfig::new()
            .with_min_workers(1)
            .with_max_workers(3)
            .with_scale_up_threshold(4)
            .with_scale_down_threshold(1)
            .with_cooldown_period(Duration::ZERO)
            .with_evaluation_window(Duration::from_secs(30))
    }

    #[test]
    fn autoscaling_scales_up_within_max_workers() {
        let config = fast_autoscale().with_scale_step(5);
        let (metrics, history) = autoscale_state(1);
        // 10 jobs for 1 worker is above the threshold of 4 per worker; the step of 5
        // is capped at max_workers.
        let decision = apply_queue_depth(10, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, Some(3));
        let m = metrics.read().unwrap().clone();
        assert_eq!(m.active_workers, 3);
        assert_eq!(m.current_queue_depth, 10);
        assert_eq!(m.avg_queue_depth, 10.0);
        assert!(m.last_scale_time.is_some());

        // At max_workers it never goes higher, however deep the queue.
        let decision = apply_queue_depth(1000, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, None);
        assert_eq!(metrics.read().unwrap().active_workers, 3);
    }

    #[test]
    fn autoscaling_scales_down_to_min_workers() {
        let config = fast_autoscale().with_scale_step(2);
        let (metrics, history) = autoscale_state(3);
        let decision = apply_queue_depth(0, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, Some(1));
        let decision = apply_queue_depth(0, Utc::now(), &config, &metrics, &history);
        assert_eq!(decision, None, "never below min_workers");
        assert_eq!(metrics.read().unwrap().active_workers, 1);
    }

    #[test]
    fn autoscaling_holds_between_thresholds_and_during_cooldown() {
        let (metrics, history) = autoscale_state(2);
        // 6 jobs / 2 workers = 3 per worker: between the thresholds (1 and 4).
        let decision = apply_queue_depth(6, Utc::now(), &fast_autoscale(), &metrics, &history);
        assert_eq!(decision, None);

        let config = fast_autoscale().with_cooldown_period(Duration::from_secs(3600));
        let (metrics, history) = autoscale_state(1);
        let now = Utc::now();
        assert_eq!(
            apply_queue_depth(100, now, &config, &metrics, &history),
            Some(2),
            "the first decision is not in a cooldown"
        );
        assert_eq!(
            apply_queue_depth(
                100,
                now + chrono::Duration::seconds(1),
                &config,
                &metrics,
                &history
            ),
            None,
            "a second decision within the cooldown period is suppressed"
        );
        let m = metrics.read().unwrap().clone();
        assert_eq!(m.active_workers, 2);
        assert_eq!(m.time_since_last_scale, Duration::from_secs(1));
    }

    #[test]
    fn autoscaling_averages_depth_over_the_evaluation_window() {
        let config = fast_autoscale()
            .with_scale_up_threshold(100)
            .with_scale_down_threshold(0)
            .with_evaluation_window(Duration::from_secs(10));
        let (metrics, history) = autoscale_state(1);
        let start = Utc::now();
        for (offset, depth) in [(0, 30), (4, 10), (8, 20)] {
            apply_queue_depth(
                depth,
                start + chrono::Duration::seconds(offset),
                &config,
                &metrics,
                &history,
            );
        }
        assert_eq!(metrics.read().unwrap().avg_queue_depth, 20.0);

        // 12 seconds later the first sample (at 0s) is outside the 10s window.
        apply_queue_depth(
            40,
            start + chrono::Duration::seconds(12),
            &config,
            &metrics,
            &history,
        );
        assert_eq!(history.read().unwrap().len(), 3);
        let m = metrics.read().unwrap().clone();
        assert_eq!(m.avg_queue_depth, (10.0 + 20.0 + 40.0) / 3.0);
        assert_eq!(m.current_queue_depth, 40);
    }

    /// A worker on a pool that never connects (no database is needed to inspect how a
    /// worker is configured).
    #[cfg(feature = "postgres")]
    fn lazy_worker(queue_name: &str) -> Worker<sqlx::Postgres> {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/hammerwork_unused").unwrap();
        let handler: JobHandler = Arc::new(|_job| Box::pin(async { Ok(()) }));
        Worker::new(
            Arc::new(JobQueue::new(pool)),
            queue_name.to_string(),
            handler,
        )
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn with_config_applies_the_worker_section() {
        let config = crate::config::WorkerConfig {
            polling_interval: Duration::from_millis(250),
            job_timeout: Duration::from_secs(7),
            priority_weights: PriorityWeights::strict(),
            retry_strategy: RetryStrategy::fixed(Duration::from_secs(42)),
            ..Default::default()
        };
        let worker = lazy_worker("cfg").with_config(&config);
        assert_eq!(worker.poll_interval, Duration::from_millis(250));
        assert_eq!(worker.default_timeout, Some(Duration::from_secs(7)));
        assert!(worker.priority_weights.as_ref().unwrap().is_strict());
        assert_eq!(
            worker.default_retry_strategy,
            Some(RetryStrategy::fixed(Duration::from_secs(42)))
        );

        let worker = lazy_worker("cfg").with_weighted_priority();
        assert!(!worker.priority_weights.as_ref().unwrap().is_strict());
        let worker = worker.with_strict_priority();
        assert!(worker.priority_weights.as_ref().unwrap().is_strict());
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn with_hammerwork_config_applies_the_queue_throttle() {
        let mut config = crate::HammerworkConfig::new();
        config.worker.polling_interval = Duration::from_millis(100);
        config.rate_limiting.enabled = true;
        config.rate_limiting.default_throttle = ThrottleConfig::new().rate_per_minute(600);
        config.rate_limiting.queue_throttles.insert(
            "emails".to_string(),
            ThrottleConfig::new()
                .max_concurrent(2)
                .rate_per_minute(60)
                .backoff_on_error(Duration::from_secs(5)),
        );

        let emails = lazy_worker("emails").with_hammerwork_config(&config);
        assert_eq!(emails.poll_interval, Duration::from_millis(100));
        assert_eq!(
            emails.throttle_config.as_ref().unwrap().max_concurrent,
            Some(2)
        );
        assert_eq!(
            emails
                .concurrency_limit
                .as_ref()
                .unwrap()
                .available_permits(),
            2
        );
        assert!(emails.rate_limiter.is_some());
        assert_eq!(emails.error_backoff_base(), Duration::from_secs(5));

        // Other queues get the default throttle: a rate limit only.
        let other = lazy_worker("other").with_hammerwork_config(&config);
        assert_eq!(
            other.throttle_config.as_ref().unwrap().rate_per_minute,
            Some(600)
        );
        assert!(other.concurrency_limit.is_none());
        assert_eq!(
            other.error_backoff_base(),
            Duration::from_millis(100),
            "without backoff_on_error the poll interval is the backoff base"
        );

        // With rate limiting disabled no throttle applies.
        config.rate_limiting.enabled = false;
        let unthrottled = lazy_worker("emails").with_hammerwork_config(&config);
        assert!(unthrottled.throttle_config.is_none());
        assert!(unthrottled.rate_limiter.is_none());
    }

    #[cfg(all(feature = "postgres", feature = "alerting"))]
    #[tokio::test]
    async fn with_hammerwork_config_applies_alerting() {
        let mut config = crate::HammerworkConfig::new();
        config.alerting = AlertingConfig::new().alert_on_queue_depth(17);
        let worker = lazy_worker("alerts").with_hammerwork_config(&config);
        let manager = worker.alert_manager.as_ref().expect("an alert manager");
        assert_eq!(manager.config().queue_depth_threshold, Some(17));
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn disabled_throttle_is_ignored_and_max_concurrent_is_at_least_one() {
        let mut disabled = ThrottleConfig::new()
            .max_concurrent(1)
            .rate_per_minute(1)
            .backoff_on_error(Duration::from_secs(9));
        disabled.enabled = false;
        let worker = lazy_worker("t").with_throttle_config(disabled);
        assert!(worker.throttle_config.is_none());
        assert!(worker.rate_limiter.is_none());
        assert!(worker.concurrency_limit.is_none());
        assert_eq!(worker.error_backoff_base(), worker.poll_interval);

        let worker = lazy_worker("t").with_throttle_config(ThrottleConfig::new().max_concurrent(0));
        assert_eq!(
            worker
                .concurrency_limit
                .as_ref()
                .unwrap()
                .available_permits(),
            1,
            "max_concurrent = 0 would block the worker forever"
        );

        // Clones share the limit, so it applies across a pool.
        let clone = worker.clone();
        assert!(Arc::ptr_eq(
            worker.concurrency_limit.as_ref().unwrap(),
            clone.concurrency_limit.as_ref().unwrap()
        ));
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_from_config_uses_pool_size_and_autoscaling() {
        let config = crate::config::WorkerConfig {
            pool_size: 3,
            autoscaling_enabled: true,
            min_workers: 2,
            max_workers: 6,
            ..Default::default()
        };
        let pool = WorkerPool::from_config(lazy_worker("p").with_config(&config), &config);
        assert_eq!(pool.workers.len(), 3);
        assert!(pool.worker_template.is_some());
        assert!(pool.autoscale_config.enabled);
        assert_eq!(pool.autoscale_config.min_workers, 2);
        assert_eq!(pool.autoscale_config.max_workers, 6);

        let config = crate::config::WorkerConfig {
            pool_size: 0,
            ..Default::default()
        };
        let pool = WorkerPool::from_config(lazy_worker("p"), &config);
        assert_eq!(pool.workers.len(), 1, "a pool always has a worker");
        assert!(!pool.autoscale_config.enabled);
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_from_hammerwork_config_schedules_purge_and_validates_encryption() {
        let mut config = crate::HammerworkConfig::new();
        config.worker.pool_size = 2;
        config.encryption.purge_interval_secs = Some(90);
        let pool = WorkerPool::from_hammerwork_config(lazy_worker("p"), &config).unwrap();
        assert_eq!(pool.workers.len(), 2);
        assert_eq!(
            pool.encrypted_job_purge_interval,
            Some(Duration::from_secs(90))
        );

        config.encryption.purge_interval_secs = Some(0);
        assert!(
            WorkerPool::from_hammerwork_config(lazy_worker("p"), &config).is_err(),
            "an invalid encryption section is rejected"
        );
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn pool_stats_collector_is_applied_to_workers() {
        use crate::stats::InMemoryStatsCollector;
        let stats: Arc<dyn StatisticsCollector> = Arc::new(InMemoryStatsCollector::new_default());
        let mut pool = WorkerPool::new().with_stats_collector(Arc::clone(&stats));
        pool.add_worker(lazy_worker("s"));
        assert!(Arc::ptr_eq(
            pool.workers[0].stats_collector.as_ref().unwrap(),
            &stats
        ));
        assert!(Arc::ptr_eq(
            pool.stats_collector().as_ref().unwrap(),
            &stats
        ));

        // With autoscaling on, the first worker becomes the template.
        let mut pool = WorkerPool::new().with_autoscaling(AutoscaleConfig::new());
        assert!(pool.worker_template.is_none());
        pool.add_worker(lazy_worker("s"));
        assert!(pool.worker_template.is_some());
        let mut pool = WorkerPool::new().without_autoscaling();
        pool.add_worker(lazy_worker("s"));
        assert!(pool.worker_template.is_none());

        // #64 H9: a plain pool does not autoscale.
        let mut pool = WorkerPool::new();
        assert!(!pool.autoscale_config.enabled);
        pool.add_worker(lazy_worker("s"));
        assert!(pool.worker_template.is_none());
    }

    #[cfg(all(feature = "postgres", feature = "webhooks"))]
    #[tokio::test]
    async fn lifecycle_events_carry_the_job_event_details() {
        let worker = lazy_worker("events");
        let job_id = uuid::Uuid::new_v4();
        let event = |event_type, error: Option<&str>| JobEvent {
            job_id,
            queue_name: "events".to_string(),
            event_type,
            priority: crate::priority::JobPriority::High,
            processing_time_ms: Some(12),
            error_message: error.map(str::to_string),
            timestamp: Utc::now(),
        };

        let completed = worker.convert_to_lifecycle_event(&event(JobEventType::Completed, None));
        assert_eq!(completed.event_type, JobLifecycleEventType::Completed);
        assert_eq!(completed.job_id, job_id);
        assert_eq!(completed.priority, crate::priority::JobPriority::High);
        assert!(completed.error.is_none());
        assert_eq!(completed.metadata["worker_queue"], "events");
        assert_eq!(completed.metadata["processing_time_ms"], "12");

        for (event_type, lifecycle, error_type) in [
            (
                JobEventType::TimedOut,
                JobLifecycleEventType::TimedOut,
                Some("timeout"),
            ),
            (
                JobEventType::Failed,
                JobLifecycleEventType::Failed,
                Some("processing_error"),
            ),
            (
                JobEventType::Dead,
                JobLifecycleEventType::Dead,
                Some("max_retries_exceeded"),
            ),
            (JobEventType::Retried, JobLifecycleEventType::Retried, None),
            (JobEventType::Started, JobLifecycleEventType::Started, None),
        ] {
            let converted = worker.convert_to_lifecycle_event(&event(event_type, Some("bad")));
            assert_eq!(converted.event_type, lifecycle);
            let error = converted.error.expect("the error message is kept");
            assert_eq!(error.message, "bad");
            assert_eq!(error.error_type.as_deref(), error_type);
        }
    }

    /// #64 M10: the monitoring task stops however the worker's `run` ends, including
    /// when its future is dropped or aborted (as a pool does when a worker panics).
    #[cfg(all(feature = "postgres", feature = "metrics"))]
    #[tokio::test]
    async fn monitoring_task_stops_when_run_is_dropped() {
        let metrics = Arc::new(
            PrometheusMetricsCollector::new(crate::metrics::MetricsConfig::default()).unwrap(),
        );
        let worker = lazy_worker("monitor")
            .with_metrics_collector(Arc::clone(&metrics))
            .with_poll_interval(Duration::from_millis(10));
        // This test and the worker.
        assert_eq!(Arc::strong_count(&metrics), 2);
        let (_shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let run = tokio::spawn(async move { worker.run(shutdown_rx).await });

        // The monitoring task holds its own handle on the collector once it runs.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&metrics) < 3 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(Arc::strong_count(&metrics), 3);

        run.abort();
        let _ = run.await;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while Arc::strong_count(&metrics) > 1 && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            Arc::strong_count(&metrics),
            1,
            "the monitoring task outlived the worker"
        );
    }

    /// #64 M5: a zero poll interval waits at least MIN_POLL_INTERVAL between polls.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn zero_poll_interval_does_not_hot_loop() {
        let worker = lazy_worker("idle").with_poll_interval(Duration::ZERO);
        let (_shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let started = std::time::Instant::now();
        for _ in 0..5 {
            assert!(matches!(
                worker.idle_wait(&mut shutdown_rx).await,
                Acquired::Idle
            ));
        }
        assert!(started.elapsed() >= MIN_POLL_INTERVAL * 5);
    }
}
