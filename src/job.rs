//! Job types and utilities for representing work units in the job queue.
//!
//! This module provides the core [`Job`] struct and [`JobStatus`] enum that represent
//! individual units of work to be processed by workers. Jobs contain all the metadata
//! needed for scheduling, prioritization, retry logic, and lifecycle management.

use crate::cron::CronSchedule;
use crate::priority::JobPriority;
use crate::retry::RetryStrategy;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a job.
///
/// Each job gets a unique UUID when created to enable tracking and management
/// throughout its lifecycle.
pub type JobId = Uuid;

/// The current status of a job in its lifecycle.
///
/// Jobs progress through various states from creation to completion or failure.
/// This enum tracks the current state to enable proper job management and statistics.
///
/// # Examples
///
/// ```rust
/// use hammerwork::JobStatus;
///
/// // Check if a job is in a final state
/// let status = JobStatus::Completed;
/// let is_final = matches!(status, JobStatus::Completed | JobStatus::Dead | JobStatus::TimedOut);
/// assert!(is_final);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum JobStatus {
    /// Job is waiting to be processed by a worker.
    Pending,
    /// Job is currently being processed by a worker.
    Running,
    /// Job completed successfully.
    Completed,
    /// Job failed but may be retried if it hasn't exhausted retry attempts.
    Failed,
    /// Job failed permanently after exhausting all retry attempts.
    Dead,
    /// Job was terminated due to exceeding its timeout duration.
    TimedOut,
    /// Job failed but is scheduled for retry.
    Retrying,
}

/// Configuration for job result storage.
///
/// This enum determines where and how job results are stored when jobs complete successfully.
/// Different storage backends offer different trade-offs between performance, persistence,
/// and resource usage.
///
/// # Examples
///
/// ```rust
/// use hammerwork::job::ResultStorage;
///
/// // Store results in the database
/// let db_storage = ResultStorage::Database;
///
/// // Store results in memory (faster but not persistent)
/// let memory_storage = ResultStorage::Memory;
///
/// // Don't store results (default behavior)
/// let no_storage = ResultStorage::None;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResultStorage {
    /// Store results in the database (persistent across restarts).
    Database,
    /// Store results in memory (faster access but lost on restart).
    Memory,
    /// Don't store job results (default behavior).
    None,
}

impl Default for ResultStorage {
    fn default() -> Self {
        Self::None
    }
}

/// Configuration for job result storage and management.
///
/// This struct contains settings that control how job results are stored,
/// how long they're retained, and when they should be cleaned up.
///
/// # Examples
///
/// ```rust
/// use hammerwork::job::{ResultConfig, ResultStorage};
/// use std::time::Duration;
///
/// // Store results in database for 7 days
/// let config = ResultConfig::new(ResultStorage::Database)
///     .with_ttl(Duration::from_secs(7 * 24 * 60 * 60));
///
/// // Store results in memory for 1 hour
/// let config = ResultConfig::new(ResultStorage::Memory)
///     .with_ttl(Duration::from_secs(3600));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultConfig {
    /// Where to store the job results.
    pub storage: ResultStorage,
    /// How long to keep results before expiring them.
    pub ttl: Option<std::time::Duration>,
    /// Maximum size of result data in bytes (for validation).
    pub max_size_bytes: Option<usize>,
}

impl ResultConfig {
    /// Creates a new result configuration with the specified storage backend.
    ///
    /// # Arguments
    ///
    /// * `storage` - The storage backend to use for results
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::job::{ResultConfig, ResultStorage};
    ///
    /// let config = ResultConfig::new(ResultStorage::Database);
    /// assert_eq!(config.storage, ResultStorage::Database);
    /// assert!(config.ttl.is_none());
    /// ```
    pub fn new(storage: ResultStorage) -> Self {
        Self {
            storage,
            ttl: None,
            max_size_bytes: None,
        }
    }

    /// Sets the time-to-live (TTL) for stored results.
    ///
    /// After this duration elapses, the result will be eligible for cleanup.
    ///
    /// # Arguments
    ///
    /// * `ttl` - How long to keep results before they expire
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::job::{ResultConfig, ResultStorage};
    /// use std::time::Duration;
    ///
    /// let config = ResultConfig::new(ResultStorage::Database)
    ///     .with_ttl(Duration::from_secs(3600)); // 1 hour
    /// ```
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Sets the maximum size for result data.
    ///
    /// This is used to validate result data before storage to prevent
    /// extremely large results from impacting system performance.
    ///
    /// # Arguments
    ///
    /// * `max_bytes` - Maximum allowed size for result data
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::job::{ResultConfig, ResultStorage};
    ///
    /// let config = ResultConfig::new(ResultStorage::Database)
    ///     .with_max_size(1024 * 1024); // 1MB limit
    /// ```
    pub fn with_max_size(mut self, max_bytes: usize) -> Self {
        self.max_size_bytes = Some(max_bytes);
        self
    }
}

impl Default for ResultConfig {
    fn default() -> Self {
        Self::new(ResultStorage::None)
    }
}

/// A unit of work to be processed by the job queue.
///
/// Jobs are the fundamental building blocks of the Hammerwork system. Each job contains:
/// - A unique identifier for tracking
/// - Queue name for routing to appropriate workers
/// - JSON payload containing the work data
/// - Scheduling and retry configuration
/// - Priority level for queue ordering
/// - Optional cron schedule for recurring jobs
/// - Timeout configuration for automatic termination
///
/// # Examples
///
/// ## Basic Job Creation
///
/// ```rust
/// use hammerwork::Job;
/// use serde_json::json;
///
/// let job = Job::new("email_queue".to_string(), json!({
///     "to": "user@example.com",
///     "subject": "Welcome!",
///     "body": "Thanks for signing up"
/// }));
///
/// assert_eq!(job.queue_name, "email_queue");
/// assert_eq!(job.max_attempts, 3); // Default retry attempts
/// ```
///
/// ## Job with Priority and Timeout
///
/// ```rust
/// use hammerwork::{Job, JobPriority};
/// use serde_json::json;
/// use std::time::Duration;
///
/// let job = Job::new("processing".to_string(), json!({"data": "important"}))
///     .as_high_priority()
///     .with_timeout(Duration::from_secs(300))
///     .with_max_attempts(5);
///
/// assert_eq!(job.priority, JobPriority::High);
/// assert_eq!(job.timeout, Some(Duration::from_secs(300)));
/// assert_eq!(job.max_attempts, 5);
/// ```
///
/// ## Delayed Job
///
/// ```rust
/// use hammerwork::Job;
/// use serde_json::json;
/// use chrono::Duration;
///
/// let job = Job::with_delay(
///     "notifications".to_string(),
///     json!({"message": "Reminder"}),
///     Duration::hours(1)
/// );
///
/// // Job will be scheduled to run 1 hour from now
/// assert!(job.scheduled_at > job.created_at);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique identifier for this job.
    pub id: JobId,
    /// Name of the queue this job belongs to.
    pub queue_name: String,
    /// JSON payload containing the work data.
    pub payload: serde_json::Value,
    /// Current status of the job.
    pub status: JobStatus,
    /// Number of times this job has been attempted.
    pub attempts: i32,
    /// Maximum number of attempts before marking the job as dead.
    pub max_attempts: i32,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job should be processed (may be in the future for delayed jobs).
    pub scheduled_at: DateTime<Utc>,
    /// When the job started processing (if it has started).
    pub started_at: Option<DateTime<Utc>>,
    /// When the job completed successfully (if it completed).
    pub completed_at: Option<DateTime<Utc>>,
    /// When the job failed permanently (if it failed).
    pub failed_at: Option<DateTime<Utc>>,
    /// When the job timed out (if it timed out).
    pub timed_out_at: Option<DateTime<Utc>>,
    /// Maximum duration the job is allowed to run before timing out.
    pub timeout: Option<std::time::Duration>,
    /// Error message if the job failed.
    pub error_message: Option<String>,
    /// Priority level for queue ordering.
    pub priority: JobPriority,
    /// Cron expression for recurring jobs.
    pub cron_schedule: Option<String>,
    /// Next scheduled execution time for recurring jobs.
    pub next_run_at: Option<DateTime<Utc>>,
    /// Whether this is a recurring job.
    pub recurring: bool,
    /// Timezone for cron calculations.
    pub timezone: Option<String>,
    /// Batch ID if this job is part of a batch operation.
    pub batch_id: Option<crate::batch::BatchId>,
    /// Configuration for how job results should be stored.
    pub result_config: ResultConfig,
    /// The actual result data from job execution (if stored).
    pub result_data: Option<serde_json::Value>,
    /// When the result was stored (if applicable).
    pub result_stored_at: Option<DateTime<Utc>>,
    /// When the stored result will expire (if applicable).
    pub result_expires_at: Option<DateTime<Utc>>,
    /// Retry strategy for this job (overrides worker default if specified).
    pub retry_strategy: Option<RetryStrategy>,
    /// Job IDs this job depends on (must complete before this job can run).
    pub depends_on: Vec<JobId>,
    /// Job IDs that depend on this job (cached for performance).
    pub dependents: Vec<JobId>,
    /// Status of dependency resolution for this job.
    pub dependency_status: crate::workflow::DependencyStatus,
    /// ID of the workflow this job belongs to (if any).
    pub workflow_id: Option<crate::workflow::WorkflowId>,
    /// Name of the workflow this job belongs to (if any).
    pub workflow_name: Option<String>,
    /// Distributed trace identifier for cross-service tracing.
    pub trace_id: Option<String>,
    /// Business correlation identifier for grouping related operations.
    pub correlation_id: Option<String>,
    /// Parent span identifier for hierarchical tracing.
    pub parent_span_id: Option<String>,
    /// Serialized span context for trace propagation.
    pub span_context: Option<String>,
}

impl Job {
    /// Creates a new job with default settings.
    ///
    /// The job will be created with:
    /// - A unique UUID identifier
    /// - Normal priority level
    /// - 3 maximum retry attempts
    /// - Scheduled to run immediately
    /// - Pending status
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue this job should be processed by
    /// * `payload` - JSON data containing the work to be performed
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, JobStatus, JobPriority};
    /// use serde_json::json;
    ///
    /// let job = Job::new("email_queue".to_string(), json!({
    ///     "to": "user@example.com",
    ///     "subject": "Welcome!",
    ///     "template": "welcome"
    /// }));
    ///
    /// assert_eq!(job.queue_name, "email_queue");
    /// assert_eq!(job.status, JobStatus::Pending);
    /// assert_eq!(job.priority, JobPriority::Normal);
    /// assert_eq!(job.max_attempts, 3);
    /// assert_eq!(job.attempts, 0);
    /// assert!(!job.is_recurring());
    /// ```
    pub fn new(queue_name: String, payload: serde_json::Value) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            queue_name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            scheduled_at: now,
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: None,
            error_message: None,
            priority: JobPriority::default(),
            cron_schedule: None,
            next_run_at: None,
            recurring: false,
            timezone: None,
            batch_id: None,
            result_config: ResultConfig::default(),
            result_data: None,
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: None,
            depends_on: Vec::new(),
            dependents: Vec::new(),
            dependency_status: crate::workflow::DependencyStatus::None,
            workflow_id: None,
            workflow_name: None,
            trace_id: None,
            correlation_id: None,
            parent_span_id: None,
            span_context: None,
        }
    }

    /// Creates a new job scheduled to run after a delay.
    ///
    /// This is useful for implementing delayed notifications, retries with backoff,
    /// or any work that should be performed at a specific time in the future.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue this job should be processed by
    /// * `payload` - JSON data containing the work to be performed
    /// * `delay` - How long to wait before the job becomes eligible for processing
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use chrono::Duration;
    ///
    /// // Send a reminder email in 24 hours
    /// let job = Job::with_delay(
    ///     "email_queue".to_string(),
    ///     json!({
    ///         "to": "user@example.com",
    ///         "subject": "Don't forget to complete your profile",
    ///         "template": "reminder"
    ///     }),
    ///     Duration::hours(24)
    /// );
    ///
    /// // Job will be scheduled 24 hours from now
    /// assert!(job.scheduled_at > job.created_at);
    /// let delay_diff = job.scheduled_at - job.created_at;
    /// assert_eq!(delay_diff, Duration::hours(24));
    /// ```
    pub fn with_delay(
        queue_name: String,
        payload: serde_json::Value,
        delay: chrono::Duration,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            queue_name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            scheduled_at: now + delay,
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: None,
            error_message: None,
            priority: JobPriority::default(),
            cron_schedule: None,
            next_run_at: None,
            recurring: false,
            timezone: None,
            batch_id: None,
            result_config: ResultConfig::default(),
            result_data: None,
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: None,
            depends_on: Vec::new(),
            dependents: Vec::new(),
            dependency_status: crate::workflow::DependencyStatus::None,
            workflow_id: None,
            workflow_name: None,
            trace_id: None,
            correlation_id: None,
            parent_span_id: None,
            span_context: None,
        }
    }

    /// Sets the maximum number of retry attempts for this job.
    ///
    /// When a job fails, it will be retried up to this many times before being
    /// marked as dead. The default is 3 attempts.
    ///
    /// # Arguments
    ///
    /// * `max_attempts` - Maximum number of attempts (including the initial attempt)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// // Critical job that should be retried many times
    /// let job = Job::new("critical_task".to_string(), json!({"task": "important"}))
    ///     .with_max_attempts(10);
    ///
    /// assert_eq!(job.max_attempts, 10);
    /// ```
    pub fn with_max_attempts(mut self, max_attempts: i32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    /// Sets a timeout duration for this job.
    ///
    /// If the job takes longer than this duration to complete, it will be
    /// automatically terminated and marked as timed out. Job-level timeouts
    /// take precedence over worker-level default timeouts.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum duration the job is allowed to run
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// // API call that should timeout after 30 seconds
    /// let job = Job::new("api_call".to_string(), json!({"url": "https://api.example.com"}))
    ///     .with_timeout(Duration::from_secs(30));
    ///
    /// assert_eq!(job.timeout, Some(Duration::from_secs(30)));
    /// ```
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the priority level for this job.
    ///
    /// Priority affects the order in which jobs are processed by workers.
    /// Higher priority jobs are generally processed before lower priority ones.
    ///
    /// # Arguments
    ///
    /// * `priority` - The priority level to assign to this job
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, JobPriority};
    /// use serde_json::json;
    ///
    /// let job = Job::new("task".to_string(), json!({"data": "test"}))
    ///     .with_priority(JobPriority::High);
    ///
    /// assert_eq!(job.priority, JobPriority::High);
    /// assert!(job.is_high_priority());
    /// ```
    pub fn with_priority(mut self, priority: JobPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Sets the job as critical priority (highest priority).
    ///
    /// Critical jobs are processed with the highest priority and should be used
    /// sparingly for truly urgent work like system alerts or emergency responses.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, JobPriority};
    /// use serde_json::json;
    ///
    /// let job = Job::new("system_alert".to_string(), json!({"alert": "system_down"}))
    ///     .as_critical();
    ///
    /// assert_eq!(job.priority, JobPriority::Critical);
    /// assert!(job.is_critical());
    /// ```
    pub fn as_critical(mut self) -> Self {
        self.priority = JobPriority::Critical;
        self
    }

    /// Sets the job as high priority.
    ///
    /// High priority jobs are processed before normal priority jobs but after
    /// critical jobs. Suitable for user-facing operations or important business logic.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, JobPriority};
    /// use serde_json::json;
    ///
    /// let job = Job::new("user_notification".to_string(), json!({"user_id": 123}))
    ///     .as_high_priority();
    ///
    /// assert_eq!(job.priority, JobPriority::High);
    /// assert!(job.is_high_priority());
    /// ```
    pub fn as_high_priority(mut self) -> Self {
        self.priority = JobPriority::High;
        self
    }

    /// Sets the job as low priority.
    ///
    /// Low priority jobs are processed after normal priority jobs but before
    /// background jobs. Suitable for analytics, reporting, or non-urgent tasks.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, JobPriority};
    /// use serde_json::json;
    ///
    /// let job = Job::new("analytics".to_string(), json!({"event": "page_view"}))
    ///     .as_low_priority();
    ///
    /// assert_eq!(job.priority, JobPriority::Low);
    /// assert!(job.is_low_priority());
    /// ```
    pub fn as_low_priority(mut self) -> Self {
        self.priority = JobPriority::Low;
        self
    }

    /// Sets the job as background priority (lowest priority).
    ///
    /// Background jobs are processed only when no higher priority jobs are available.
    /// Suitable for cleanup tasks, maintenance, or work that can wait indefinitely.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, JobPriority};
    /// use serde_json::json;
    ///
    /// let job = Job::new("cleanup".to_string(), json!({"type": "temp_files"}))
    ///     .as_background();
    ///
    /// assert_eq!(job.priority, JobPriority::Background);
    /// assert!(job.is_background());
    /// ```
    pub fn as_background(mut self) -> Self {
        self.priority = JobPriority::Background;
        self
    }

    /// Sets a custom retry strategy for this job.
    ///
    /// The retry strategy determines how long to wait between retry attempts
    /// when the job fails. This overrides any default retry strategy configured
    /// on the worker.
    ///
    /// # Arguments
    ///
    /// * `strategy` - The retry strategy to use for this job
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, retry::RetryStrategy};
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// // Use exponential backoff for API calls
    /// let job = Job::new("api_call".to_string(), json!({"url": "https://api.example.com"}))
    ///     .with_retry_strategy(RetryStrategy::exponential(
    ///         Duration::from_secs(1),
    ///         2.0,
    ///         Some(Duration::from_secs(10 * 60))
    ///     ));
    /// ```
    pub fn with_retry_strategy(mut self, strategy: RetryStrategy) -> Self {
        self.retry_strategy = Some(strategy);
        self
    }

    /// Sets exponential backoff retry strategy for this job.
    ///
    /// This is a convenience method for the most common retry pattern.
    /// Each retry attempt waits exponentially longer than the previous one.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay for the first retry attempt
    /// * `multiplier` - Exponential growth multiplier (typically 2.0)
    /// * `max_delay` - Maximum delay to cap exponential growth
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// // Exponential backoff: 1s, 2s, 4s, 8s, 16s... (capped at 10 minutes)
    /// let job = Job::new("network_request".to_string(), json!({"url": "https://example.com"}))
    ///     .with_exponential_backoff(
    ///         Duration::from_secs(1),
    ///         2.0,
    ///         Duration::from_secs(10 * 60)
    ///     );
    /// ```
    pub fn with_exponential_backoff(
        mut self,
        base: std::time::Duration,
        multiplier: f64,
        max_delay: std::time::Duration,
    ) -> Self {
        self.retry_strategy = Some(RetryStrategy::exponential(
            base,
            multiplier,
            Some(max_delay),
        ));
        self
    }

    /// Sets linear backoff retry strategy for this job.
    ///
    /// Each retry attempt waits longer than the previous by a fixed increment.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay for the first retry attempt
    /// * `increment` - Amount to add for each subsequent attempt
    /// * `max_delay` - Optional maximum delay to cap growth
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// // Linear backoff: 10s, 20s, 30s, 40s... (capped at 2 minutes)
    /// let job = Job::new("database_operation".to_string(), json!({"query": "SELECT ..."}))
    ///     .with_linear_backoff(
    ///         Duration::from_secs(10),
    ///         Duration::from_secs(10),
    ///         Some(Duration::from_secs(2 * 60))
    ///     );
    /// ```
    pub fn with_linear_backoff(
        mut self,
        base: std::time::Duration,
        increment: std::time::Duration,
        max_delay: Option<std::time::Duration>,
    ) -> Self {
        self.retry_strategy = Some(RetryStrategy::linear(base, increment, max_delay));
        self
    }

    /// Sets Fibonacci sequence backoff retry strategy for this job.
    ///
    /// Each retry waits according to the Fibonacci sequence multiplied by the base delay.
    ///
    /// # Arguments
    ///
    /// * `base` - Base delay multiplied by Fibonacci numbers
    /// * `max_delay` - Optional maximum delay to cap growth
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// // Fibonacci backoff: 2s, 2s, 4s, 6s, 10s, 16s, 26s...
    /// let job = Job::new("file_processing".to_string(), json!({"file": "data.csv"}))
    ///     .with_fibonacci_backoff(
    ///         Duration::from_secs(2),
    ///         Some(Duration::from_secs(5 * 60))
    ///     );
    /// ```
    pub fn with_fibonacci_backoff(
        mut self,
        base: std::time::Duration,
        max_delay: Option<std::time::Duration>,
    ) -> Self {
        self.retry_strategy = Some(RetryStrategy::fibonacci(base, max_delay));
        self
    }

    /// Create a recurring job with a cron schedule
    pub fn with_cron_schedule(
        queue_name: String,
        payload: serde_json::Value,
        cron_schedule: CronSchedule,
    ) -> Result<Self, crate::cron::CronError> {
        let now = Utc::now();
        let next_run = cron_schedule.next_execution_from_now();

        Ok(Self {
            id: Uuid::new_v4(),
            queue_name,
            payload,
            status: JobStatus::Pending,
            attempts: 0,
            max_attempts: 3,
            created_at: now,
            scheduled_at: next_run.unwrap_or(now),
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: None,
            error_message: None,
            priority: JobPriority::default(),
            cron_schedule: Some(cron_schedule.expression.clone()),
            next_run_at: next_run,
            recurring: true,
            timezone: Some(cron_schedule.timezone.clone()),
            batch_id: None,
            result_config: ResultConfig::default(),
            result_data: None,
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: None,
            depends_on: Vec::new(),
            dependents: Vec::new(),
            dependency_status: crate::workflow::DependencyStatus::None,
            workflow_id: None,
            workflow_name: None,
            trace_id: None,
            correlation_id: None,
            parent_span_id: None,
            span_context: None,
        })
    }

    /// Add a cron schedule to an existing job
    pub fn with_cron(
        mut self,
        cron_schedule: CronSchedule,
    ) -> Result<Self, crate::cron::CronError> {
        let next_run = cron_schedule.next_execution_from_now();
        self.cron_schedule = Some(cron_schedule.expression.clone());
        self.next_run_at = next_run;
        self.recurring = true;
        self.timezone = Some(cron_schedule.timezone.clone());
        self.scheduled_at = next_run.unwrap_or(self.scheduled_at);
        Ok(self)
    }

    /// Set the job as recurring without a cron schedule (for manual rescheduling)
    pub fn as_recurring(mut self) -> Self {
        self.recurring = true;
        self
    }

    /// Set the timezone for the job
    pub fn with_timezone(mut self, timezone: String) -> Self {
        self.timezone = Some(timezone);
        self
    }

    /// Configure how job results should be stored.
    ///
    /// This allows jobs to store their results for later retrieval by other systems.
    /// Results can be stored in the database, memory, or not stored at all.
    ///
    /// # Arguments
    ///
    /// * `storage` - The storage backend to use for results
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, job::ResultStorage};
    /// use serde_json::json;
    ///
    /// let job = Job::new("data_processing".to_string(), json!({"input": "data"}))
    ///     .with_result_storage(ResultStorage::Database);
    /// ```
    pub fn with_result_storage(mut self, storage: ResultStorage) -> Self {
        self.result_config.storage = storage;
        self
    }

    /// Set the time-to-live (TTL) for stored job results.
    ///
    /// After this duration elapses, the result will be eligible for cleanup.
    /// This is useful for managing storage costs and compliance requirements.
    ///
    /// # Arguments
    ///
    /// * `ttl` - How long to keep results before they expire
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// let job = Job::new("report_generation".to_string(), json!({"type": "monthly"}))
    ///     .with_result_ttl(Duration::from_secs(7 * 24 * 60 * 60)); // 7 days
    /// ```
    pub fn with_result_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.result_config.ttl = Some(ttl);
        self
    }

    /// Configure complete result storage settings.
    ///
    /// This provides full control over how results are stored and managed.
    ///
    /// # Arguments
    ///
    /// * `config` - Complete result configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, job::{ResultConfig, ResultStorage}};
    /// use serde_json::json;
    /// use std::time::Duration;
    ///
    /// let config = ResultConfig::new(ResultStorage::Database)
    ///     .with_ttl(Duration::from_secs(3600))
    ///     .with_max_size(1024 * 1024); // 1MB
    ///
    /// let job = Job::new("large_processing".to_string(), json!({"data": "..."}))
    ///     .with_result_config(config);
    /// ```
    pub fn with_result_config(mut self, config: ResultConfig) -> Self {
        self.result_config = config;
        self
    }

    /// Check if the job has result storage configured.
    ///
    /// Returns `true` if the job is configured to store results in any backend
    /// other than `ResultStorage::None`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, job::ResultStorage};
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("test".to_string(), json!({}));
    /// assert!(!job1.has_result_storage());
    ///
    /// let job2 = Job::new("test".to_string(), json!({}))
    ///     .with_result_storage(ResultStorage::Database);
    /// assert!(job2.has_result_storage());
    /// ```
    pub fn has_result_storage(&self) -> bool {
        self.result_config.storage != ResultStorage::None
    }

    /// Check if the job has stored result data.
    ///
    /// Returns `true` if the job has result data available for retrieval.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("test".to_string(), json!({}));
    /// assert!(!job.has_result_data());
    /// ```
    pub fn has_result_data(&self) -> bool {
        self.result_data.is_some()
    }

    /// Check if the job is dead (failed all retry attempts)
    pub fn is_dead(&self) -> bool {
        self.status == JobStatus::Dead
    }

    /// Check if the job has timed out
    pub fn is_timed_out(&self) -> bool {
        self.status == JobStatus::TimedOut
    }

    /// Check if the job is critical priority
    pub fn is_critical(&self) -> bool {
        self.priority == JobPriority::Critical
    }

    /// Check if the job is high priority
    pub fn is_high_priority(&self) -> bool {
        self.priority == JobPriority::High
    }

    /// Check if the job is normal priority
    pub fn is_normal_priority(&self) -> bool {
        self.priority == JobPriority::Normal
    }

    /// Check if the job is low priority
    pub fn is_low_priority(&self) -> bool {
        self.priority == JobPriority::Low
    }

    /// Check if the job is background priority
    pub fn is_background(&self) -> bool {
        self.priority == JobPriority::Background
    }

    /// Get the priority level as a numeric value for comparison
    pub fn priority_value(&self) -> i32 {
        self.priority.as_i32()
    }

    /// Check if the job has exhausted all retry attempts
    pub fn has_exhausted_retries(&self) -> bool {
        self.attempts >= self.max_attempts
    }

    /// Checks if the job should timeout based on its start time and timeout setting.
    ///
    /// Returns `true` if the job has been running longer than its configured timeout
    /// duration. Returns `false` if the job hasn't started yet, has no timeout set,
    /// or is still within the timeout window.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use std::time::Duration;
    /// use chrono::Utc;
    ///
    /// let mut job = Job::new("test".to_string(), json!({"data": "test"}))
    ///     .with_timeout(Duration::from_secs(30));
    ///
    /// // Job hasn't started yet, so it shouldn't timeout
    /// assert!(!job.should_timeout());
    ///
    /// // Simulate job starting 45 seconds ago
    /// job.started_at = Some(Utc::now() - chrono::Duration::seconds(45));
    ///
    /// // Job should timeout since 45s > 30s timeout
    /// assert!(job.should_timeout());
    /// ```
    pub fn should_timeout(&self) -> bool {
        if let (Some(started_at), Some(timeout)) = (self.started_at, self.timeout) {
            let elapsed = Utc::now() - started_at;
            let timeout_duration = chrono::Duration::from_std(timeout).unwrap_or_default();
            elapsed >= timeout_duration
        } else {
            false
        }
    }

    /// Gets the duration since the job was created.
    ///
    /// This is useful for monitoring how long jobs have been in the system
    /// and identifying jobs that may be stuck or delayed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("test".to_string(), json!({"data": "test"}));
    /// let age = job.age();
    ///
    /// // Job was just created, so age should be very small
    /// assert!(age.num_milliseconds() >= 0);
    /// assert!(age.num_seconds() < 1);
    /// ```
    pub fn age(&self) -> chrono::Duration {
        Utc::now() - self.created_at
    }

    /// Gets the processing duration if the job has started.
    ///
    /// Returns the time between when the job started and when it finished
    /// (completed, failed, or timed out). If the job is still running,
    /// returns the time since it started. Returns `None` if the job hasn't
    /// started yet.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use chrono::Utc;
    ///
    /// let mut job = Job::new("test".to_string(), json!({"data": "test"}));
    ///
    /// // Job hasn't started, so no processing duration
    /// assert!(job.processing_duration().is_none());
    ///
    /// // Simulate job that started and completed
    /// let start_time = Utc::now() - chrono::Duration::seconds(10);
    /// let end_time = start_time + chrono::Duration::seconds(5);
    /// job.started_at = Some(start_time);
    /// job.completed_at = Some(end_time);
    ///
    /// let duration = job.processing_duration().unwrap();
    /// assert_eq!(duration.num_seconds(), 5);
    /// ```
    pub fn processing_duration(&self) -> Option<chrono::Duration> {
        self.started_at.map(|started| {
            self.completed_at
                .or(self.failed_at)
                .or(self.timed_out_at)
                .unwrap_or_else(Utc::now)
                - started
        })
    }

    /// Check if this is a recurring job
    pub fn is_recurring(&self) -> bool {
        self.recurring
    }

    /// Check if this job has a cron schedule
    pub fn has_cron_schedule(&self) -> bool {
        self.cron_schedule.is_some()
    }

    /// Get the cron schedule if it exists
    pub fn get_cron_schedule(&self) -> Option<Result<CronSchedule, crate::cron::CronError>> {
        self.cron_schedule
            .as_ref()
            .map(|expr| match &self.timezone {
                Some(tz) => CronSchedule::with_timezone(expr, tz),
                None => CronSchedule::new(expr),
            })
    }

    /// Calculate the next run time for a recurring job
    pub fn calculate_next_run(&self) -> Option<DateTime<Utc>> {
        if !self.recurring {
            return None;
        }

        if let Some(cron_schedule) = self.get_cron_schedule() {
            match cron_schedule {
                Ok(schedule) => schedule.next_execution_from_now(),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// Update the job for the next run (for recurring jobs)
    pub fn prepare_for_next_run(&mut self) -> Option<DateTime<Utc>> {
        if !self.recurring {
            return None;
        }

        let next_run = self.calculate_next_run();
        if let Some(next_time) = next_run {
            self.status = JobStatus::Pending;
            self.attempts = 0;
            self.scheduled_at = next_time;
            self.next_run_at = Some(next_time);
            self.started_at = None;
            self.completed_at = None;
            self.failed_at = None;
            self.timed_out_at = None;
            self.error_message = None;
        }
        next_run
    }

    /// Adds a dependency on another job.
    ///
    /// This job will not be executed until the specified job completes successfully.
    /// If the dependency job fails, this job's dependency status will be set to Failed.
    ///
    /// # Arguments
    ///
    /// * `job_id` - The ID of the job this job should depend on
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("step1".to_string(), json!({"data": "step1"}));
    /// let job2 = Job::new("step2".to_string(), json!({"data": "step2"}))
    ///     .depends_on(&job1.id);
    /// assert!(job2.has_dependencies());
    /// ```
    pub fn depends_on(mut self, job_id: &JobId) -> Self {
        self.depends_on.push(*job_id);
        self.dependency_status = crate::workflow::DependencyStatus::Waiting;
        self
    }

    /// Adds multiple dependencies on other jobs.
    ///
    /// This job will not be executed until all specified jobs complete successfully.
    ///
    /// # Arguments
    ///
    /// * `job_ids` - The IDs of the jobs this job should depend on
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("step1".to_string(), json!({}));
    /// let job2 = Job::new("step2".to_string(), json!({}));
    /// let final_job = Job::new("final".to_string(), json!({}))
    ///     .depends_on_jobs(&[job1.id, job2.id]);
    /// assert_eq!(final_job.depends_on.len(), 2);
    /// ```
    pub fn depends_on_jobs(mut self, job_ids: &[JobId]) -> Self {
        self.depends_on.extend_from_slice(job_ids);
        if !job_ids.is_empty() {
            self.dependency_status = crate::workflow::DependencyStatus::Waiting;
        }
        self
    }

    /// Sets the workflow this job belongs to.
    ///
    /// # Arguments
    ///
    /// * `workflow_id` - The ID of the workflow
    /// * `workflow_name` - The name of the workflow
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let workflow_id = Uuid::new_v4();
    /// let job = Job::new("test".to_string(), json!({}))
    ///     .with_workflow(workflow_id, "data_pipeline");
    ///
    /// assert_eq!(job.workflow_id, Some(workflow_id));
    /// assert_eq!(job.workflow_name, Some("data_pipeline".to_string()));
    /// ```
    pub fn with_workflow(
        mut self,
        workflow_id: crate::workflow::WorkflowId,
        workflow_name: impl Into<String>,
    ) -> Self {
        self.workflow_id = Some(workflow_id);
        self.workflow_name = Some(workflow_name.into());
        self
    }

    /// Checks if this job has any dependencies.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("independent".to_string(), json!({}));
    /// assert!(!job1.has_dependencies());
    ///
    /// let job2 = Job::new("dependent".to_string(), json!({}))
    ///     .depends_on(&job1.id);
    /// assert!(job2.has_dependencies());
    /// ```
    pub fn has_dependencies(&self) -> bool {
        !self.depends_on.is_empty()
    }

    /// Checks if this job is part of a workflow.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let job1 = Job::new("standalone".to_string(), json!({}));
    /// assert!(!job1.is_part_of_workflow());
    ///
    /// let job2 = Job::new("workflow_job".to_string(), json!({}))
    ///     .with_workflow(Uuid::new_v4(), "test_workflow");
    /// assert!(job2.is_part_of_workflow());
    /// ```
    pub fn is_part_of_workflow(&self) -> bool {
        self.workflow_id.is_some()
    }

    /// Checks if all dependencies for this job are satisfied.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("independent".to_string(), json!({}));
    /// assert!(job1.dependencies_satisfied());
    ///
    /// let job2 = Job::new("dependent".to_string(), json!({}))
    ///     .depends_on(&job1.id);
    /// assert!(!job2.dependencies_satisfied()); // Dependencies not satisfied yet
    /// ```
    pub fn dependencies_satisfied(&self) -> bool {
        matches!(
            self.dependency_status,
            crate::workflow::DependencyStatus::None | crate::workflow::DependencyStatus::Satisfied
        )
    }

    /// Checks if any dependencies for this job have failed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let mut job = Job::new("test".to_string(), json!({}));
    /// job.dependency_status = hammerwork::workflow::DependencyStatus::Failed;
    /// assert!(job.dependencies_failed());
    /// ```
    pub fn dependencies_failed(&self) -> bool {
        matches!(
            self.dependency_status,
            crate::workflow::DependencyStatus::Failed
        )
    }

    /// Gets the number of dependencies for this job.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    /// use uuid::Uuid;
    ///
    /// let job = Job::new("test".to_string(), json!({}))
    ///     .depends_on_jobs(&[Uuid::new_v4(), Uuid::new_v4()]);
    /// assert_eq!(job.dependency_count(), 2);
    /// ```
    pub fn dependency_count(&self) -> usize {
        self.depends_on.len()
    }

    /// Gets the number of jobs that depend on this job.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("test".to_string(), json!({}));
    /// assert_eq!(job.dependent_count(), 0);
    /// ```
    pub fn dependent_count(&self) -> usize {
        self.dependents.len()
    }

    /// Sets the distributed trace identifier for this job.
    ///
    /// The trace ID is used to track jobs across service boundaries in distributed
    /// systems. All related operations should share the same trace ID.
    ///
    /// # Arguments
    ///
    /// * `trace_id` - The distributed trace identifier
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("service_call".to_string(), json!({"data": "test"}))
    ///     .with_trace_id("trace-123-456");
    ///
    /// assert_eq!(job.trace_id, Some("trace-123-456".to_string()));
    /// ```
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Sets the business correlation identifier for this job.
    ///
    /// The correlation ID groups related business operations together, even if they
    /// span multiple traces or services. Use this to correlate jobs that process
    /// the same business entity or workflow.
    ///
    /// # Arguments
    ///
    /// * `correlation_id` - The business correlation identifier
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("order_processing".to_string(), json!({"order_id": 12345}))
    ///     .with_correlation_id("order-12345");
    ///
    /// assert_eq!(job.correlation_id, Some("order-12345".to_string()));
    /// ```
    pub fn with_correlation_id(mut self, correlation_id: impl Into<String>) -> Self {
        self.correlation_id = Some(correlation_id.into());
        self
    }

    /// Sets the parent span identifier for hierarchical tracing.
    ///
    /// Use this to create a parent-child relationship between spans, enabling
    /// hierarchical trace visualization in tracing systems.
    ///
    /// # Arguments
    ///
    /// * `parent_span_id` - The parent span identifier
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("child_task".to_string(), json!({"data": "test"}))
    ///     .with_parent_span_id("span-parent-123");
    ///
    /// assert_eq!(job.parent_span_id, Some("span-parent-123".to_string()));
    /// ```
    pub fn with_parent_span_id(mut self, parent_span_id: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent_span_id.into());
        self
    }

    /// Sets the serialized span context for trace propagation.
    ///
    /// The span context contains all the information needed to propagate tracing
    /// across service boundaries. This is typically a serialized representation
    /// of the current span context.
    ///
    /// # Arguments
    ///
    /// * `span_context` - The serialized span context
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("distributed_task".to_string(), json!({"data": "test"}))
    ///     .with_span_context("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01");
    ///
    /// assert_eq!(job.span_context, Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string()));
    /// ```
    pub fn with_span_context(mut self, span_context: impl Into<String>) -> Self {
        self.span_context = Some(span_context.into());
        self
    }

    /// Convenience method to set both trace ID and correlation ID.
    ///
    /// This is useful when the trace ID and correlation ID are the same,
    /// which is common in simple tracing scenarios.
    ///
    /// # Arguments
    ///
    /// * `id` - The identifier to use for both trace and correlation
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("task".to_string(), json!({"data": "test"}))
    ///     .with_tracing_id("unified-id-123");
    ///
    /// assert_eq!(job.trace_id, Some("unified-id-123".to_string()));
    /// assert_eq!(job.correlation_id, Some("unified-id-123".to_string()));
    /// ```
    pub fn with_tracing_id(mut self, id: impl Into<String>) -> Self {
        let id_string = id.into();
        self.trace_id = Some(id_string.clone());
        self.correlation_id = Some(id_string);
        self
    }

    /// Checks if this job has any tracing information.
    ///
    /// Returns `true` if any of the tracing fields (trace_id, correlation_id,
    /// parent_span_id, or span_context) are set.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("untraced".to_string(), json!({}));
    /// assert!(!job1.has_tracing_info());
    ///
    /// let job2 = Job::new("traced".to_string(), json!({}))
    ///     .with_trace_id("trace-123");
    /// assert!(job2.has_tracing_info());
    /// ```
    pub fn has_tracing_info(&self) -> bool {
        self.trace_id.is_some()
            || self.correlation_id.is_some()
            || self.parent_span_id.is_some()
            || self.span_context.is_some()
    }

    /// Gets the trace ID if available.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("test".to_string(), json!({}))
    ///     .with_trace_id("trace-123");
    ///
    /// assert_eq!(job.get_trace_id(), Some("trace-123"));
    /// ```
    pub fn get_trace_id(&self) -> Option<&str> {
        self.trace_id.as_deref()
    }

    /// Gets the correlation ID if available.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::Job;
    /// use serde_json::json;
    ///
    /// let job = Job::new("test".to_string(), json!({}))
    ///     .with_correlation_id("corr-456");
    ///
    /// assert_eq!(job.get_correlation_id(), Some("corr-456"));
    /// ```
    pub fn get_correlation_id(&self) -> Option<&str> {
        self.correlation_id.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_job_new() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});

        let job = Job::new(queue_name.clone(), payload.clone());

        assert_eq!(job.queue_name, queue_name);
        assert_eq!(job.payload, payload);
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.max_attempts, 3);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());
        assert!(job.failed_at.is_none());
        assert!(job.error_message.is_none());
        assert_eq!(job.created_at, job.scheduled_at);
    }

    #[test]
    fn test_job_with_delay() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});
        let delay = chrono::Duration::minutes(5);

        let job = Job::with_delay(queue_name.clone(), payload.clone(), delay);

        assert_eq!(job.queue_name, queue_name);
        assert_eq!(job.payload, payload);
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 0);
        assert_eq!(job.max_attempts, 3);
        assert!(job.scheduled_at > job.created_at);
        assert_eq!(job.scheduled_at - job.created_at, delay);
    }

    #[test]
    fn test_job_with_max_attempts() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});

        let job = Job::new(queue_name, payload).with_max_attempts(5);

        assert_eq!(job.max_attempts, 5);
    }

    #[test]
    fn test_job_with_delay_and_max_attempts() {
        let queue_name = "test_queue".to_string();
        let payload = json!({"key": "value"});
        let delay = chrono::Duration::hours(1);

        let job = Job::with_delay(queue_name, payload, delay).with_max_attempts(10);

        assert_eq!(job.max_attempts, 10);
        assert!(job.scheduled_at > job.created_at);
    }

    #[test]
    fn test_job_status_equality() {
        assert_eq!(JobStatus::Pending, JobStatus::Pending);
        assert_eq!(JobStatus::Running, JobStatus::Running);
        assert_eq!(JobStatus::Completed, JobStatus::Completed);
        assert_eq!(JobStatus::Failed, JobStatus::Failed);
        assert_eq!(JobStatus::Dead, JobStatus::Dead);
        assert_eq!(JobStatus::TimedOut, JobStatus::TimedOut);
        assert_eq!(JobStatus::Retrying, JobStatus::Retrying);

        assert_ne!(JobStatus::Pending, JobStatus::Running);
        assert_ne!(JobStatus::Completed, JobStatus::Failed);
        assert_ne!(JobStatus::Failed, JobStatus::Dead);
        assert_ne!(JobStatus::Dead, JobStatus::TimedOut);
        assert_ne!(JobStatus::TimedOut, JobStatus::Failed);
    }

    #[test]
    fn test_job_serialization() {
        let job = Job::new("test".to_string(), json!({"data": "test"}));

        let serialized = serde_json::to_string(&job).unwrap();
        let deserialized: Job = serde_json::from_str(&serialized).unwrap();

        assert_eq!(job.id, deserialized.id);
        assert_eq!(job.queue_name, deserialized.queue_name);
        assert_eq!(job.payload, deserialized.payload);
        assert_eq!(job.status, deserialized.status);
        assert_eq!(job.attempts, deserialized.attempts);
        assert_eq!(job.max_attempts, deserialized.max_attempts);
    }

    #[test]
    fn test_job_status_serialization() {
        let statuses = vec![
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Dead,
            JobStatus::TimedOut,
            JobStatus::Retrying,
        ];

        for status in statuses {
            let serialized = serde_json::to_string(&status).unwrap();
            let deserialized: JobStatus = serde_json::from_str(&serialized).unwrap();
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_job_dead_status_methods() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));

        // Initially not dead
        assert!(!job.is_dead());
        assert!(!job.has_exhausted_retries());

        // Simulate exhausting retries
        job.attempts = 3;
        job.max_attempts = 3;
        assert!(job.has_exhausted_retries());
        assert!(!job.is_dead()); // Still not dead until status is set

        // Mark as dead
        job.status = JobStatus::Dead;
        job.failed_at = Some(Utc::now());
        assert!(job.is_dead());
        assert!(job.has_exhausted_retries());
    }

    #[test]
    fn test_job_processing_duration() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));

        // No processing duration when not started
        assert!(job.processing_duration().is_none());

        // Set start time
        let start_time = Utc::now();
        job.started_at = Some(start_time);

        // Should have some duration now (very small)
        let duration = job.processing_duration().unwrap();
        assert!(duration.num_milliseconds() >= 0);

        // Set completion time
        let completion_time = start_time + chrono::Duration::seconds(5);
        job.completed_at = Some(completion_time);

        let final_duration = job.processing_duration().unwrap();
        assert_eq!(final_duration.num_seconds(), 5);
    }

    #[test]
    fn test_job_age() {
        let job = Job::new("test".to_string(), json!({"data": "test"}));
        let age = job.age();

        // Age should be very small (just created)
        assert!(age.num_milliseconds() >= 0);
        assert!(age.num_seconds() < 1);
    }

    #[test]
    fn test_job_with_timeout() {
        let timeout = std::time::Duration::from_secs(30);
        let job = Job::new("test".to_string(), json!({"data": "test"})).with_timeout(timeout);

        assert_eq!(job.timeout, Some(timeout));
        assert!(!job.is_timed_out()); // Not timed out until status is set
    }

    #[test]
    fn test_job_timeout_status_methods() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));

        // Initially not timed out
        assert!(!job.is_timed_out());

        // Set timed out status
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(Utc::now());
        assert!(job.is_timed_out());
    }

    #[test]
    fn test_job_should_timeout() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}))
            .with_timeout(std::time::Duration::from_millis(100));

        // Should not timeout before it starts
        assert!(!job.should_timeout());

        // Set start time to simulate job starting
        job.started_at = Some(Utc::now() - chrono::Duration::milliseconds(200));

        // Should timeout since 200ms > 100ms timeout
        assert!(job.should_timeout());

        // Job without timeout should never timeout
        let mut job_no_timeout = Job::new("test".to_string(), json!({"data": "test"}));
        job_no_timeout.started_at = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(!job_no_timeout.should_timeout());
    }

    #[test]
    fn test_job_with_delay_and_timeout() {
        let delay = chrono::Duration::minutes(5);
        let timeout = std::time::Duration::from_secs(120);

        let job = Job::with_delay("test".to_string(), json!({"data": "test"}), delay)
            .with_timeout(timeout)
            .with_max_attempts(5);

        assert_eq!(job.timeout, Some(timeout));
        assert_eq!(job.max_attempts, 5);
        assert!(job.scheduled_at > job.created_at);
        assert_eq!(job.scheduled_at - job.created_at, delay);
    }

    #[test]
    fn test_processing_duration_with_timeout() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));

        // Set start time and timed out time
        let start_time = Utc::now() - chrono::Duration::seconds(5);
        let timeout_time = start_time + chrono::Duration::seconds(3);

        job.started_at = Some(start_time);
        job.timed_out_at = Some(timeout_time);

        let duration = job.processing_duration().unwrap();
        assert_eq!(duration.num_seconds(), 3);
    }

    #[test]
    fn test_timeout_builder_methods() {
        let job = Job::new("test".to_string(), json!({"key": "value"}))
            .with_timeout(std::time::Duration::from_secs(120))
            .with_max_attempts(5);

        assert_eq!(job.timeout, Some(std::time::Duration::from_secs(120)));
        assert_eq!(job.max_attempts, 5);
        assert_eq!(job.queue_name, "test");
    }

    #[test]
    fn test_job_timeout_edge_cases() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));

        // Job without timeout should never timeout
        assert!(!job.should_timeout());

        // Job with timeout but not started should not timeout
        job.timeout = Some(std::time::Duration::from_millis(100));
        assert!(!job.should_timeout());

        // Job with timeout and started but within timeout window should not timeout
        job.started_at = Some(Utc::now() - chrono::Duration::milliseconds(50));
        assert!(!job.should_timeout());

        // Job with timeout and started beyond timeout window should timeout
        job.started_at = Some(Utc::now() - chrono::Duration::milliseconds(150));
        assert!(job.should_timeout());
    }

    #[test]
    fn test_job_status_transitions_with_timeout() {
        let mut job = Job::new("test".to_string(), json!({"data": "test"}));

        // Initial state
        assert_eq!(job.status, JobStatus::Pending);
        assert!(!job.is_timed_out());

        // Simulate timeout
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(Utc::now());

        assert!(job.is_timed_out());
        assert!(!job.is_dead()); // TimedOut is different from Dead
    }

    #[test]
    fn test_timeout_serialization_compatibility() {
        let original_job = Job::new("test_queue".to_string(), json!({"data": "test"}))
            .with_timeout(std::time::Duration::from_secs(300))
            .with_max_attempts(5);

        // Serialize and deserialize
        let serialized = serde_json::to_string(&original_job).unwrap();
        let deserialized: Job = serde_json::from_str(&serialized).unwrap();

        // Verify timeout field is preserved
        assert_eq!(original_job.timeout, deserialized.timeout);
        assert_eq!(original_job.timed_out_at, deserialized.timed_out_at);
        assert_eq!(original_job.status, deserialized.status);
    }

    #[test]
    fn test_job_with_all_timeout_fields() {
        let timeout_duration = std::time::Duration::from_secs(60);
        let mut job = Job::new("comprehensive_test".to_string(), json!({"test": true}))
            .with_timeout(timeout_duration)
            .with_max_attempts(3);

        // Simulate job lifecycle with timeout
        job.started_at = Some(Utc::now() - chrono::Duration::seconds(30));
        job.status = JobStatus::Running;

        // Should not timeout yet (30s < 60s)
        assert!(!job.should_timeout());

        // Simulate timeout occurring
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(Utc::now());
        job.error_message = Some("Job timed out after 60s".to_string());

        assert!(job.is_timed_out());
        assert_eq!(job.timeout, Some(timeout_duration));
        assert!(job.timed_out_at.is_some());
        assert!(job.error_message.is_some());
    }
}
