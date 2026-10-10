//! Job archival and retention system for Hammerwork.
//!
//! This module provides automated job archiving capabilities to manage database
//! growth and support compliance requirements. Jobs can be automatically archived
//! based on configurable policies, with support for compression and selective restoration.
//!
//! # Overview
//!
//! The archival system consists of three main components:
//! - [`ArchivalPolicy`]: Defines when and how jobs should be archived
//! - [`ArchivalConfig`]: Configuration for archive storage and compression
//! - [`JobArchiver`]: Service that executes archival operations
//!
//! # Quick Start
//!
//! ```rust
//! use hammerwork::archive::{ArchivalPolicy, ArchivalConfig, ArchivalReason};
//! use chrono::Duration;
//!
//! // Create an archival policy
//! let policy = ArchivalPolicy::new()
//!     .archive_completed_after(Duration::days(7))
//!     .archive_failed_after(Duration::days(30))
//!     .purge_archived_after(Duration::days(365))
//!     .compress_archived_payloads(true);
//!
//! let config = ArchivalConfig::new().with_compression_level(9);
//! let reason = ArchivalReason::Automatic;
//!
//! // These can be used with queue.archive_jobs() method
//! assert!(policy.enabled);
//! assert_eq!(config.compression_level, 9);
//! ```

use crate::{Job, JobId, JobStatus, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Unique identifier for an archival policy.
pub type ArchivalPolicyId = String;

/// Reasons why a job was archived.
///
/// This enum tracks the reason a job was moved to the archive table,
/// which is useful for auditing and compliance purposes.
///
/// # Examples
///
/// ```rust
/// use hammerwork::archive::ArchivalReason;
///
/// // Create different archival reasons
/// let automatic = ArchivalReason::Automatic;
/// let manual = ArchivalReason::Manual;
/// let compliance = ArchivalReason::Compliance;
/// let maintenance = ArchivalReason::Maintenance;
///
/// // Test display formatting
/// assert_eq!(format!("{}", automatic), "Automatic");
/// assert_eq!(format!("{}", manual), "Manual");
/// assert_eq!(format!("{}", compliance), "Compliance");
/// assert_eq!(format!("{}", maintenance), "Maintenance");
///
/// // Test default value
/// assert_eq!(ArchivalReason::default(), ArchivalReason::Automatic);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ArchivalReason {
    /// Job was archived due to automatic policy.
    #[default]
    Automatic,
    /// Job was manually archived by an administrator.
    Manual,
    /// Job was archived due to compliance requirements.
    Compliance,
    /// Job was archived due to database maintenance.
    Maintenance,
}

impl ArchivalReason {
    /// The stable string form stored in the `archival_reason` database column.
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalReason;
    ///
    /// assert_eq!(ArchivalReason::Manual.as_str(), "Manual");
    /// ```
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Manual => "Manual",
            Self::Compliance => "Compliance",
            Self::Maintenance => "Maintenance",
        }
    }

    /// Parses the value stored in the `archival_reason` database column.
    ///
    /// Accepts the plain form written by [`ArchivalReason::as_str`] as well as the
    /// JSON-quoted form (`"Manual"`) that older versions may have stored, ignoring
    /// ASCII case. Returns `None` for an unrecognised value.
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalReason;
    ///
    /// assert_eq!(ArchivalReason::parse_from_db("Manual"), Some(ArchivalReason::Manual));
    /// assert_eq!(ArchivalReason::parse_from_db("\"Compliance\""), Some(ArchivalReason::Compliance));
    /// assert_eq!(ArchivalReason::parse_from_db("bogus"), None);
    /// ```
    pub fn parse_from_db(value: &str) -> Option<Self> {
        let cleaned = value.trim().trim_matches('"');
        [
            Self::Automatic,
            Self::Manual,
            Self::Compliance,
            Self::Maintenance,
        ]
        .into_iter()
        .find(|reason| reason.as_str().eq_ignore_ascii_case(cleaned))
    }
}

impl std::fmt::Display for ArchivalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// WebSocket events for archive operations.
///
/// These events are emitted during archival operations and can be consumed
/// by WebSocket clients for real-time dashboard updates.
///
/// # Examples
///
/// ```rust
/// use hammerwork::archive::{ArchiveEvent, ArchivalReason, ArchivalStats};
/// use uuid::Uuid;
/// use chrono::Utc;
///
/// // Job archived event
/// let job_archived = ArchiveEvent::JobArchived {
///     job_id: Uuid::new_v4(),
///     queue: "email_queue".to_string(),
///     reason: ArchivalReason::Automatic,
/// };
///
/// // Bulk operation started
/// let operation_id = "bulk_op_123".to_string();
/// let bulk_started = ArchiveEvent::BulkArchiveStarted {
///     operation_id: operation_id.clone(),
///     estimated_jobs: 1000,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArchiveEvent {
    /// A job was archived
    JobArchived {
        job_id: JobId,
        queue: String,
        reason: ArchivalReason,
    },
    /// A job was restored from archive
    JobRestored {
        job_id: JobId,
        queue: String,
        restored_by: Option<String>,
    },
    /// A bulk archive operation was started
    BulkArchiveStarted {
        operation_id: String,
        estimated_jobs: u64,
    },
    /// Progress update for a bulk archive operation
    BulkArchiveProgress {
        operation_id: String,
        jobs_processed: u64,
        total: u64,
    },
    /// A bulk archive operation completed
    BulkArchiveCompleted {
        operation_id: String,
        stats: ArchivalStats,
    },
    /// Jobs were purged from the archive
    JobsPurged {
        count: u64,
        older_than: DateTime<Utc>,
    },
}

/// Configuration for job archival policies.
///
/// This struct defines when jobs should be archived based on their status and age.
/// Different retention periods can be configured for different job statuses.
///
/// # Examples
///
/// ```rust
/// use hammerwork::archive::ArchivalPolicy;
/// use chrono::Duration;
///
/// // Archive completed jobs after 7 days, failed jobs after 30 days
/// let policy = ArchivalPolicy::new()
///     .archive_completed_after(Duration::days(7))
///     .archive_failed_after(Duration::days(30))
///     .purge_archived_after(Duration::days(365));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivalPolicy {
    /// How long to keep completed jobs before archiving.
    pub archive_completed_after: Option<Duration>,
    /// How long to keep failed jobs before archiving.
    pub archive_failed_after: Option<Duration>,
    /// How long to keep dead jobs before archiving.
    pub archive_dead_after: Option<Duration>,
    /// How long to keep timed out jobs before archiving.
    pub archive_timed_out_after: Option<Duration>,
    /// How long to keep archived jobs before purging completely.
    pub purge_archived_after: Option<Duration>,
    /// Whether to compress payloads when archiving.
    pub compress_payloads: bool,
    /// Maximum number of jobs to archive in a single batch.
    pub batch_size: usize,
    /// Whether this policy is enabled.
    pub enabled: bool,
}

impl Default for ArchivalPolicy {
    fn default() -> Self {
        Self {
            archive_completed_after: Some(Duration::days(30)),
            archive_failed_after: Some(Duration::days(90)),
            archive_dead_after: Some(Duration::days(90)),
            archive_timed_out_after: Some(Duration::days(90)),
            purge_archived_after: Some(Duration::days(365)),
            compress_payloads: true,
            batch_size: 1000,
            enabled: true,
        }
    }
}

impl ArchivalPolicy {
    /// Creates a new archival policy with default settings.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    ///
    /// let policy = ArchivalPolicy::new();
    /// assert!(policy.enabled);
    /// assert!(policy.compress_payloads);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets how long to keep completed jobs before archiving.
    ///
    /// # Arguments
    ///
    /// * `duration` - Time to keep completed jobs before archiving
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    /// use chrono::Duration;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .archive_completed_after(Duration::days(7));
    /// ```
    pub fn archive_completed_after(mut self, duration: Duration) -> Self {
        self.archive_completed_after = Some(duration);
        self
    }

    /// Sets how long to keep failed jobs before archiving.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    /// use chrono::Duration;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .archive_failed_after(Duration::days(30));
    /// assert_eq!(policy.archive_failed_after, Some(Duration::days(30)));
    /// ```
    pub fn archive_failed_after(mut self, duration: Duration) -> Self {
        self.archive_failed_after = Some(duration);
        self
    }

    /// Sets how long to keep dead jobs before archiving.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    /// use chrono::Duration;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .archive_dead_after(Duration::days(14));
    /// assert_eq!(policy.archive_dead_after, Some(Duration::days(14)));
    /// ```
    pub fn archive_dead_after(mut self, duration: Duration) -> Self {
        self.archive_dead_after = Some(duration);
        self
    }

    /// Sets how long to keep timed out jobs before archiving.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    /// use chrono::Duration;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .archive_timed_out_after(Duration::days(21));
    /// assert_eq!(policy.archive_timed_out_after, Some(Duration::days(21)));
    /// ```
    pub fn archive_timed_out_after(mut self, duration: Duration) -> Self {
        self.archive_timed_out_after = Some(duration);
        self
    }

    /// Sets how long to keep archived jobs before purging completely.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    /// use chrono::Duration;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .purge_archived_after(Duration::days(365));
    /// assert_eq!(policy.purge_archived_after, Some(Duration::days(365)));
    /// ```
    pub fn purge_archived_after(mut self, duration: Duration) -> Self {
        self.purge_archived_after = Some(duration);
        self
    }

    /// Sets whether to compress payloads when archiving.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .compress_archived_payloads(true);
    /// assert!(policy.compress_payloads);
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .compress_archived_payloads(false);
    /// assert!(!policy.compress_payloads);
    /// ```
    pub fn compress_archived_payloads(mut self, compress: bool) -> Self {
        self.compress_payloads = compress;
        self
    }

    /// Sets the maximum number of jobs to archive in a single batch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .with_batch_size(500);
    /// assert_eq!(policy.batch_size, 500);
    /// ```
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Enables or disables this archival policy.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .enabled(false);
    /// assert!(!policy.enabled);
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .enabled(true);
    /// assert!(policy.enabled);
    /// ```
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Checks if a job with the given status and age should be archived.
    ///
    /// # Arguments
    ///
    /// * `status` - Current status of the job
    /// * `age` - How long ago the job finished (completed, failed, etc.)
    ///
    /// # Returns
    ///
    /// `true` if the job should be archived according to this policy
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalPolicy;
    /// use hammerwork::JobStatus;
    /// use chrono::Duration;
    ///
    /// let policy = ArchivalPolicy::new()
    ///     .archive_completed_after(Duration::days(7))
    ///     .archive_failed_after(Duration::days(30));
    ///
    /// // Job completed 10 days ago - should be archived
    /// assert!(policy.should_archive(&JobStatus::Completed, Duration::days(10)));
    ///
    /// // Job completed 5 days ago - should not be archived yet
    /// assert!(!policy.should_archive(&JobStatus::Completed, Duration::days(5)));
    ///
    /// // Failed job 40 days ago - should be archived
    /// assert!(policy.should_archive(&JobStatus::Failed, Duration::days(40)));
    ///
    /// // Pending job - should never be archived
    /// assert!(!policy.should_archive(&JobStatus::Pending, Duration::days(100)));
    /// ```
    pub fn should_archive(&self, status: &JobStatus, age: Duration) -> bool {
        if !self.enabled {
            return false;
        }

        match status {
            JobStatus::Completed => self
                .archive_completed_after
                .is_some_and(|threshold| age >= threshold),
            JobStatus::Failed => self
                .archive_failed_after
                .is_some_and(|threshold| age >= threshold),
            JobStatus::Dead => self
                .archive_dead_after
                .is_some_and(|threshold| age >= threshold),
            JobStatus::TimedOut => self
                .archive_timed_out_after
                .is_some_and(|threshold| age >= threshold),
            _ => false,
        }
    }
}

/// Configuration for archive storage and compression settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivalConfig {
    /// Default compression level (0-9, where 9 is maximum compression).
    pub compression_level: u32,
    /// Whether to check every compressed payload before storing it: the compressed bytes
    /// are decompressed and compared with the original, and a mismatch fails the archival
    /// batch (its transaction is rolled back, so no job is moved).
    pub verify_compression: bool,
}

impl Default for ArchivalConfig {
    fn default() -> Self {
        Self {
            compression_level: 6, // Balanced compression/speed
            verify_compression: true,
        }
    }
}

impl ArchivalConfig {
    /// Creates a new archival configuration with default settings.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalConfig;
    ///
    /// let config = ArchivalConfig::new();
    /// assert_eq!(config.compression_level, 6);
    /// assert!(config.verify_compression);
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the compression level for archived payloads.
    ///
    /// # Arguments
    ///
    /// * `level` - Compression level from 0 (no compression) to 9 (maximum compression)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalConfig;
    ///
    /// let config = ArchivalConfig::new()
    ///     .with_compression_level(9);
    /// assert_eq!(config.compression_level, 9);
    ///
    /// // Values above 9 are clamped to 9
    /// let config = ArchivalConfig::new()
    ///     .with_compression_level(15);
    /// assert_eq!(config.compression_level, 9);
    /// ```
    pub fn with_compression_level(mut self, level: u32) -> Self {
        self.compression_level = level.min(9);
        self
    }

    /// Sets whether to verify each compressed payload round-trips before it is stored
    /// (see [`ArchivalConfig::verify_compression`]).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalConfig;
    ///
    /// let config = ArchivalConfig::new()
    ///     .with_compression_verification(false);
    /// assert!(!config.verify_compression);
    ///
    /// let config = ArchivalConfig::new()
    ///     .with_compression_verification(true);
    /// assert!(config.verify_compression);
    /// ```
    pub fn with_compression_verification(mut self, verify: bool) -> Self {
        self.verify_compression = verify;
        self
    }
}

/// Statistics about archival operations.
///
/// This struct contains detailed information about the results of an archival operation,
/// including performance metrics and compression statistics.
///
/// # Examples
///
/// ```rust
/// use hammerwork::archive::ArchivalStats;
/// use chrono::Utc;
/// use std::time::Duration;
///
/// let stats = ArchivalStats {
///     jobs_archived: 150,
///     jobs_purged: 25,
///     bytes_archived: 1024 * 1024, // 1MB
///     bytes_purged: 500 * 1024,    // 500KB
///     compression_ratio: 0.7,      // 30% size reduction
///     operation_duration: Duration::from_secs(45),
///     last_run_at: Utc::now(),
/// };
///
/// assert_eq!(stats.jobs_archived, 150);
/// assert_eq!(stats.compression_ratio, 0.7);
/// assert!(stats.operation_duration.as_secs() > 0);
///
/// // Test default values
/// let default_stats = ArchivalStats::default();
/// assert_eq!(default_stats.jobs_archived, 0);
/// assert_eq!(default_stats.compression_ratio, 1.0);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivalStats {
    /// Number of jobs archived in the last operation.
    pub jobs_archived: u64,
    /// Number of jobs purged in the last operation.
    pub jobs_purged: u64,
    /// Total size of data archived (in bytes).
    pub bytes_archived: u64,
    /// Total size of data purged (in bytes).
    pub bytes_purged: u64,
    /// Compression ratio achieved (original_size / compressed_size).
    pub compression_ratio: f64,
    /// Time taken for the last archival operation.
    pub operation_duration: std::time::Duration,
    /// Last time archival was run.
    pub last_run_at: DateTime<Utc>,
}

impl Default for ArchivalStats {
    fn default() -> Self {
        Self {
            jobs_archived: 0,
            jobs_purged: 0,
            bytes_archived: 0,
            bytes_purged: 0,
            compression_ratio: 1.0,
            operation_duration: std::time::Duration::from_secs(0),
            last_run_at: Utc::now(),
        }
    }
}

/// Information about an archived job.
///
/// This struct represents a job that has been moved to the archive table,
/// containing metadata about the original job and archival information.
///
/// # Examples
///
/// ```rust
/// use hammerwork::archive::{ArchivedJob, ArchivalReason};
/// use hammerwork::{JobId, JobStatus};
/// use chrono::Utc;
/// use uuid::Uuid;
///
/// let job_id = Uuid::new_v4();
/// let now = Utc::now();
///
/// let archived_job = ArchivedJob {
///     id: job_id,
///     queue_name: "email_queue".to_string(),
///     status: JobStatus::Completed,
///     created_at: now,
///     archived_at: now,
///     archival_reason: ArchivalReason::Automatic,
///     original_payload_size: Some(1024),
///     payload_compressed: true,
///     archived_by: Some("scheduler".to_string()),
/// };
///
/// assert_eq!(archived_job.queue_name, "email_queue");
/// assert_eq!(archived_job.status, JobStatus::Completed);
/// assert_eq!(archived_job.archival_reason, ArchivalReason::Automatic);
/// assert!(archived_job.payload_compressed);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedJob {
    /// Unique identifier of the archived job.
    pub id: JobId,
    /// Name of the queue the job belonged to.
    pub queue_name: String,
    /// Original job status before archiving.
    pub status: JobStatus,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job was archived.
    pub archived_at: DateTime<Utc>,
    /// Reason for archiving.
    pub archival_reason: ArchivalReason,
    /// Size of the original payload in bytes.
    pub original_payload_size: Option<usize>,
    /// Whether the payload was compressed.
    pub payload_compressed: bool,
    /// Who or what archived the job.
    pub archived_by: Option<String>,
}

/// Upper bound on the number of `batch_size` batches a single
/// [`JobArchiver::archive_jobs_with_progress`] or [`JobArchiver::archive_jobs_with_events`]
/// call processes before returning.
pub const MAX_ARCHIVAL_BATCHES_PER_OPERATION: usize = 10_000;

/// Service for managing job archival operations.
///
/// The `JobArchiver` provides methods to archive jobs based on policies,
/// restore archived jobs, and manage archival configuration.
#[derive(Debug)]
pub struct JobArchiver<DB>
where
    DB: sqlx::Database,
{
    /// Database connection pool.
    #[allow(dead_code)]
    pool: sqlx::Pool<DB>,
    /// Archival policies by queue name.
    policies: HashMap<String, ArchivalPolicy>,
    /// Policy for queues without one of their own, and for operations over all queues.
    default_policy: ArchivalPolicy,
    /// Global archival configuration.
    config: ArchivalConfig,
}

/// Result of one [`JobArchiver::run_scheduled_pass`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScheduledArchivalPass {
    /// Jobs moved to the archive table.
    pub jobs_archived: u64,
    /// Archived jobs deleted because they were older than `purge_archived_after`.
    pub jobs_purged: u64,
    /// Whether the pass stopped early because `should_stop` returned `true`.
    pub stopped_early: bool,
}

impl<DB> JobArchiver<DB>
where
    DB: sqlx::Database,
{
    /// Creates a new job archiver with the given database pool.
    ///
    /// # Arguments
    ///
    /// * `pool` - Database connection pool
    ///
    /// # Examples
    ///
    /// ## Basic Usage
    ///
    /// ```rust,no_run
    /// use hammerwork::archive::JobArchiver;
    /// use sqlx::PgPool;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = PgPool::connect("postgresql://localhost/hammerwork").await?;
    /// let archiver = JobArchiver::new(pool);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// ## Integration with JobQueue (using public pool field)
    ///
    /// ```rust,no_run
    /// use hammerwork::{JobQueue, archive::JobArchiver};
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    /// let queue = Arc::new(JobQueue::new(pool.clone()));
    ///
    /// // Access the public pool field to create an archiver
    /// let archiver = JobArchiver::new(queue.pool.clone());
    ///
    /// // Both the queue and archiver share the same database connection pool
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// ## Multiple Archivers with Shared Pool
    ///
    /// ```rust,no_run
    /// use hammerwork::{JobQueue, archive::JobArchiver};
    /// use std::sync::Arc;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    /// let queue = Arc::new(JobQueue::new(pool.clone()));
    ///
    /// // Create multiple archivers sharing the same pool
    /// let archiver1 = JobArchiver::new(queue.pool.clone());
    /// let archiver2 = JobArchiver::new(queue.pool.clone());
    ///
    /// // All components share the same connection pool for efficiency
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(pool: sqlx::Pool<DB>) -> Self {
        Self {
            pool,
            policies: HashMap::new(),
            default_policy: ArchivalPolicy::default(),
            config: ArchivalConfig::default(),
        }
    }

    /// Sets the policy used for queues without a policy of their own and for operations
    /// over all queues (`queue_name: None`). Defaults to [`ArchivalPolicy::default`].
    pub fn set_default_policy(&mut self, policy: ArchivalPolicy) {
        self.default_policy = policy;
    }

    /// Builder form of [`set_default_policy`](Self::set_default_policy).
    pub fn with_default_policy(mut self, policy: ArchivalPolicy) -> Self {
        self.default_policy = policy;
        self
    }

    /// Builder form of [`set_config`](Self::set_config).
    pub fn with_config(mut self, config: ArchivalConfig) -> Self {
        self.config = config;
        self
    }

    /// The policy used for queues without one of their own.
    pub fn default_policy(&self) -> &ArchivalPolicy {
        &self.default_policy
    }

    /// One scheduled archival pass over every queue with the
    /// [default policy](Self::set_default_policy): archives eligible jobs batch by batch
    /// with [`ArchivalReason::Automatic`], then, when the policy sets
    /// `purge_archived_after`, deletes archived jobs older than that. This is what
    /// [`WorkerPool::with_archival`](crate::WorkerPool::with_archival) runs on its
    /// interval. Does nothing when the policy is disabled.
    ///
    /// `should_stop` is checked before each batch and before the purge; when it returns
    /// `true` the pass returns early with `stopped_early` set. A batch that has started
    /// always commits or rolls back as a whole.
    ///
    /// Safe to run from several processes at once: each batch locks its rows with
    /// `FOR UPDATE SKIP LOCKED` and moves them in one transaction, so a job is archived
    /// exactly once, and purging is an idempotent delete.
    pub async fn run_scheduled_pass<Q, S>(
        &self,
        queue: &Q,
        archived_by: Option<&str>,
        should_stop: S,
    ) -> Result<ScheduledArchivalPass>
    where
        Q: crate::queue::DatabaseQueue,
        S: Fn() -> bool,
    {
        let policy = &self.default_policy;
        let mut pass = ScheduledArchivalPass::default();
        if !policy.enabled {
            return Ok(pass);
        }

        let (stats, stopped) = self
            .archive_in_batches(
                queue,
                None,
                policy,
                ArchivalReason::Automatic,
                archived_by,
                |_| {},
                &should_stop,
            )
            .await?;
        pass.jobs_archived = stats.jobs_archived;

        if let Some(after) = policy.purge_archived_after {
            if stopped || should_stop() {
                pass.stopped_early = true;
                return Ok(pass);
            }
            pass.jobs_purged = queue.purge_archived_jobs(Utc::now() - after).await?;
        }
        pass.stopped_early = stopped;
        Ok(pass)
    }

    /// Sets the archival policy for a specific queue.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Name of the queue
    /// * `policy` - Archival policy to apply
    pub fn set_policy(&mut self, queue_name: impl Into<String>, policy: ArchivalPolicy) {
        self.policies.insert(queue_name.into(), policy);
    }

    /// Gets the archival policy for a specific queue.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Name of the queue
    ///
    /// # Returns
    ///
    /// The archival policy if one exists, otherwise `None`
    pub fn get_policy(&self, queue_name: &str) -> Option<&ArchivalPolicy> {
        self.policies.get(queue_name)
    }

    /// Removes the archival policy for a specific queue.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Name of the queue
    ///
    /// # Returns
    ///
    /// The removed policy if one existed, otherwise `None`
    pub fn remove_policy(&mut self, queue_name: &str) -> Option<ArchivalPolicy> {
        self.policies.remove(queue_name)
    }

    /// Sets the global archival configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Archival configuration to use
    pub fn set_config(&mut self, config: ArchivalConfig) {
        self.config = config;
    }

    /// Gets the current archival configuration.
    pub fn get_config(&self) -> &ArchivalConfig {
        &self.config
    }

    /// Archive jobs with real-time progress reporting and WebSocket events.
    ///
    /// This method provides enhanced archival capabilities with:
    /// - Unique operation ID for tracking
    /// - Progress callbacks for real-time updates
    /// - WebSocket event publishing for dashboard integration
    /// - Batch processing for large datasets
    ///
    /// Jobs are archived in batches of the policy's `batch_size` until no eligible
    /// jobs remain (bounded by [`MAX_ARCHIVAL_BATCHES_PER_OPERATION`]). The callback
    /// receives `(processed, total)`: once with `processed == 0` before the first batch,
    /// then after every batch, so the last call reports the final `jobs_archived`.
    ///
    /// # Arguments
    ///
    /// * `queue` - Database queue implementation
    /// * `queue_name` - Optional queue name to filter jobs
    /// * `reason` - Reason for archival
    /// * `archived_by` - Who initiated the archival
    /// * `progress_callback` - Optional callback for progress updates
    ///
    /// # Returns
    ///
    /// Tuple of (operation_id, final_stats)
    ///
    /// # Examples
    ///
    #[cfg_attr(feature = "postgres", doc = "```rust,no_run")]
    #[cfg_attr(not(feature = "postgres"), doc = "```rust,ignore")]
    /// use hammerwork::{JobQueue, archive::{JobArchiver, ArchivalReason}};
    /// use std::sync::Arc;
    ///
    /// # async fn example(queue: Arc<JobQueue<sqlx::Postgres>>) -> hammerwork::Result<()> {
    /// let mut archiver = JobArchiver::new(queue.pool.clone());
    ///
    /// let (operation_id, stats) = archiver.archive_jobs_with_progress(
    ///     queue.as_ref(),
    ///     Some("email_queue"),
    ///     ArchivalReason::Manual,
    ///     Some("admin"),
    ///     Some(Box::new(|processed, total| {
    ///         println!("Progress: {}/{}", processed, total);
    ///     }))
    /// ).await?;
    ///
    /// println!("Operation {} completed, archived {} jobs", operation_id, stats.jobs_archived);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn archive_jobs_with_progress<Q>(
        &self,
        queue: &Q,
        queue_name: Option<&str>,
        reason: ArchivalReason,
        archived_by: Option<&str>,
        progress_callback: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> Result<(String, ArchivalStats)>
    where
        Q: crate::queue::DatabaseQueue,
    {
        let operation_id = Uuid::new_v4().to_string();
        let policy = self.policy_for(queue_name);

        // Estimate total jobs to be archived (this is a simplified estimation)
        let estimated_jobs = self
            .estimate_archival_jobs(queue, queue_name, policy)
            .await?;

        if let Some(callback) = &progress_callback {
            callback(0, estimated_jobs);
        }

        let (stats, _) = self
            .archive_in_batches(
                queue,
                queue_name,
                policy,
                reason,
                archived_by,
                |processed| {
                    if let Some(callback) = &progress_callback {
                        callback(processed, estimated_jobs.max(processed));
                    }
                },
                &|| false,
            )
            .await?;

        // Always report a final state, even when nothing was archived.
        if stats.jobs_archived == 0
            && let Some(callback) = &progress_callback
        {
            callback(0, estimated_jobs);
        }

        Ok((operation_id, stats))
    }

    /// Archive jobs with WebSocket event publishing for real-time dashboard updates.
    ///
    /// Jobs are archived in batches of the policy's `batch_size` until no eligible jobs
    /// remain, like [`JobArchiver::archive_jobs_with_progress`]. Publishes
    /// [`ArchiveEvent::BulkArchiveStarted`], then one [`ArchiveEvent::BulkArchiveProgress`]
    /// for every batch except the last, then [`ArchiveEvent::BulkArchiveCompleted`] with
    /// the accumulated statistics.
    ///
    /// # Arguments
    ///
    /// * `queue` - Database queue implementation
    /// * `queue_name` - Optional queue name to filter jobs
    /// * `reason` - Reason for archival
    /// * `archived_by` - Who initiated the archival
    /// * `event_publisher` - Function to publish archive events
    ///
    /// # Returns
    ///
    /// Tuple of (operation_id, final_stats)
    pub async fn archive_jobs_with_events<Q, F>(
        &self,
        queue: &Q,
        queue_name: Option<&str>,
        reason: ArchivalReason,
        archived_by: Option<&str>,
        event_publisher: F,
    ) -> Result<(String, ArchivalStats)>
    where
        Q: crate::queue::DatabaseQueue,
        F: Fn(ArchiveEvent) + Send + Sync,
    {
        let operation_id = Uuid::new_v4().to_string();
        let policy = self.policy_for(queue_name);

        // Estimate total jobs to be archived
        let estimated_jobs = self
            .estimate_archival_jobs(queue, queue_name, policy)
            .await?;

        event_publisher(ArchiveEvent::BulkArchiveStarted {
            operation_id: operation_id.clone(),
            estimated_jobs,
        });

        // Report progress between batches. The final batch is reported by the
        // completion event, so a single-batch run publishes only started + completed.
        let mut pending_progress: Option<u64> = None;
        let (stats, _) = self
            .archive_in_batches(
                queue,
                queue_name,
                policy,
                reason,
                archived_by,
                |processed| {
                    if let Some(previous) = pending_progress.replace(processed) {
                        event_publisher(ArchiveEvent::BulkArchiveProgress {
                            operation_id: operation_id.clone(),
                            jobs_processed: previous,
                            total: estimated_jobs.max(previous),
                        });
                    }
                },
                &|| false,
            )
            .await?;

        event_publisher(ArchiveEvent::BulkArchiveCompleted {
            operation_id: operation_id.clone(),
            stats: stats.clone(),
        });

        Ok((operation_id, stats))
    }

    /// The policy for `queue_name`, or the default policy when none is configured.
    fn policy_for(&self, queue_name: Option<&str>) -> &ArchivalPolicy {
        queue_name
            .and_then(|name| self.policies.get(name))
            .unwrap_or(&self.default_policy)
    }

    /// Runs `archive_jobs` repeatedly, one `policy.batch_size` batch at a time, until no
    /// more jobs are eligible, and returns the accumulated statistics and whether
    /// `should_stop` ended the loop.
    ///
    /// `on_batch` is called after every batch that archived at least one job, with the
    /// total number of jobs archived so far. The loop stops when a batch archives fewer
    /// than `batch_size` jobs, when `should_stop` (checked before each batch) returns
    /// `true`, or after [`MAX_ARCHIVAL_BATCHES_PER_OPERATION`] batches so that a queue
    /// that keeps producing eligible jobs cannot keep one call running forever.
    #[allow(clippy::too_many_arguments)]
    async fn archive_in_batches<Q, F, S>(
        &self,
        queue: &Q,
        queue_name: Option<&str>,
        policy: &ArchivalPolicy,
        reason: ArchivalReason,
        archived_by: Option<&str>,
        mut on_batch: F,
        should_stop: &S,
    ) -> Result<(ArchivalStats, bool)>
    where
        Q: crate::queue::DatabaseQueue,
        F: FnMut(u64),
        S: Fn() -> bool,
    {
        let started = std::time::Instant::now();
        let mut total = ArchivalStats::default();
        let mut weighted_ratio = 0.0;
        let mut stopped = false;

        for _ in 0..MAX_ARCHIVAL_BATCHES_PER_OPERATION {
            if should_stop() {
                stopped = true;
                break;
            }
            let batch = queue
                .archive_jobs(
                    queue_name,
                    policy,
                    &self.config,
                    reason.clone(),
                    archived_by,
                )
                .await?;

            if batch.jobs_archived == 0 {
                break;
            }

            total.jobs_archived += batch.jobs_archived;
            total.bytes_archived += batch.bytes_archived;
            weighted_ratio += batch.compression_ratio * batch.jobs_archived as f64;
            total.last_run_at = batch.last_run_at;
            on_batch(total.jobs_archived);

            if (batch.jobs_archived as usize) < policy.batch_size {
                break;
            }
        }

        if total.jobs_archived > 0 {
            total.compression_ratio = weighted_ratio / total.jobs_archived as f64;
        }
        total.operation_duration = started.elapsed();
        Ok((total, stopped))
    }

    /// Estimate the number of jobs that would be archived by a policy.
    async fn estimate_archival_jobs<Q>(
        &self,
        queue: &Q,
        queue_name: Option<&str>,
        policy: &ArchivalPolicy,
    ) -> Result<u64>
    where
        Q: crate::queue::DatabaseQueue,
    {
        // Estimate based on queue statistics and policy configuration
        if let Some(queue_name) = queue_name {
            let stats = queue.get_queue_stats(queue_name).await?;
            let mut estimate = 0u64;

            // Add completed jobs if policy archives them
            if policy.archive_completed_after.is_some() {
                estimate += stats.completed_count;
            }

            // Add failed jobs if policy archives them
            if policy.archive_failed_after.is_some() {
                estimate += stats.statistics.failed;
            }

            // Add dead jobs if policy archives them
            if policy.archive_dead_after.is_some() {
                estimate += stats.dead_count;
            }

            // Add timed out jobs if policy archives them
            if policy.archive_timed_out_after.is_some() {
                estimate += stats.timed_out_count;
            }

            Ok(estimate)
        } else {
            // For all queues, this would require more complex querying
            // Use a conservative estimate based on policy scope
            let base_estimate = if policy.archive_completed_after.is_some()
                && policy.archive_failed_after.is_some()
            {
                2000 // High estimate for policies that archive multiple status types
            } else if policy.archive_completed_after.is_some()
                || policy.archive_failed_after.is_some()
            {
                1000 // Medium estimate for selective policies
            } else {
                100 // Low estimate for very limited policies
            };

            Ok(base_estimate)
        }
    }
}

/// Trait for database-specific archival operations.
///
/// This trait is implemented by the database queue implementations to provide
/// archival functionality specific to each database backend.
pub trait ArchivalOperations {
    /// Archives jobs that match the given criteria.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Name of the queue to archive jobs from
    /// * `policy` - Archival policy to apply
    /// * `config` - Archival configuration
    /// * `reason` - Reason for archiving
    /// * `archived_by` - Who or what is performing the archival
    ///
    /// # Returns
    ///
    /// Statistics about the archival operation
    fn archive_jobs(
        &self,
        queue_name: Option<&str>,
        policy: &ArchivalPolicy,
        config: &ArchivalConfig,
        reason: ArchivalReason,
        archived_by: Option<&str>,
    ) -> impl std::future::Future<Output = Result<ArchivalStats>> + Send;

    /// Restores an archived job back to the active queue.
    ///
    /// # Arguments
    ///
    /// * `job_id` - ID of the job to restore
    ///
    /// # Returns
    ///
    /// The restored job
    fn restore_job(&self, job_id: JobId) -> impl std::future::Future<Output = Result<Job>> + Send;

    /// Lists archived jobs with optional filtering.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Optional queue name to filter by
    /// * `limit` - Maximum number of jobs to return
    /// * `offset` - Number of jobs to skip
    ///
    /// # Returns
    ///
    /// List of archived job information
    fn list_archived_jobs(
        &self,
        queue_name: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> impl std::future::Future<Output = Result<Vec<ArchivedJob>>> + Send;

    /// Purges archived jobs that are older than the specified date.
    ///
    /// # Arguments
    ///
    /// * `older_than` - Date threshold for purging
    ///
    /// # Returns
    ///
    /// Number of jobs purged
    fn purge_archived_jobs(
        &self,
        older_than: DateTime<Utc>,
    ) -> impl std::future::Future<Output = Result<u64>> + Send;

    /// Gets statistics about archived jobs.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Optional queue name to filter by
    ///
    /// # Returns
    ///
    /// Archival statistics
    fn get_archival_stats(
        &self,
        queue_name: Option<&str>,
    ) -> impl std::future::Future<Output = Result<ArchivalStats>> + Send;
}

/// Serializes and, when the policy asks for it and it helps, gzip-compresses a job
/// payload for the archive table.
///
/// Returns `(stored_bytes, is_compressed, original_size)`.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn encode_archived_payload(
    payload: &serde_json::Value,
    policy: &ArchivalPolicy,
    config: &ArchivalConfig,
) -> Result<(Vec<u8>, bool, usize)> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let payload_json = serde_json::to_vec(payload)?;
    let original_size = payload_json.len();
    if !policy.compress_payloads {
        return Ok((payload_json, false, original_size));
    }

    let mut encoder = GzEncoder::new(Vec::new(), Compression::new(config.compression_level));
    encoder.write_all(&payload_json)?;
    let compressed = encoder.finish()?;
    if compressed.len() < original_size {
        if config.verify_compression {
            verify_compressed_payload(&compressed, &payload_json)?;
        }
        Ok((compressed, true, original_size))
    } else {
        Ok((payload_json, false, original_size))
    }
}

/// Checks that `compressed` decompresses to exactly `original`.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
fn verify_compressed_payload(compressed: &[u8], original: &[u8]) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let mut decompressed = Vec::with_capacity(original.len());
    let round_trips = GzDecoder::new(compressed)
        .read_to_end(&mut decompressed)
        .is_ok()
        && decompressed == original;
    if round_trips {
        Ok(())
    } else {
        Err(crate::HammerworkError::Archive {
            message: "compressed payload failed verification: it does not decompress to the \
                      original payload"
                .to_string(),
        })
    }
}

/// Reverses [`encode_archived_payload`].
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn decode_archived_payload(
    stored: &[u8],
    is_compressed: bool,
) -> Result<serde_json::Value> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    if is_compressed {
        let mut decompressed = Vec::new();
        GzDecoder::new(stored).read_to_end(&mut decompressed)?;
        Ok(serde_json::from_slice(&decompressed)?)
    } else {
        Ok(serde_json::from_slice(stored)?)
    }
}

/// The job statuses a policy can archive, as `(retention, status, timestamp_column)`.
///
/// A job with `status` is eligible once `timestamp_column` is older than `retention`;
/// statuses whose retention is `None` are not archived.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn archival_candidates(
    policy: &ArchivalPolicy,
) -> [(Option<Duration>, &'static str, &'static str); 4] {
    [
        (policy.archive_completed_after, "Completed", "completed_at"),
        (policy.archive_failed_after, "Failed", "failed_at"),
        (policy.archive_dead_after, "Dead", "failed_at"),
        (policy.archive_timed_out_after, "TimedOut", "timed_out_at"),
    ]
}

/// Resets an archived job so it can be processed again after a restore.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn reset_for_restore(mut job: Job) -> Job {
    job.status = JobStatus::Pending;
    job.attempts = 0;
    job.scheduled_at = Utc::now();
    job.started_at = None;
    job.completed_at = None;
    job.failed_at = None;
    job.timed_out_at = None;
    job.error_message = None;
    job.result_data = None;
    job.result_stored_at = None;
    job.result_expires_at = None;
    job
}

/// Parses the `status` column of the archive table (the job's status before it was
/// archived), accepting both the plain and the legacy JSON-quoted form.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn parse_archived_status(value: &str) -> JobStatus {
    match value.trim_matches('"') {
        "Pending" => JobStatus::Pending,
        "Running" => JobStatus::Running,
        "Completed" => JobStatus::Completed,
        "Failed" => JobStatus::Failed,
        "Dead" => JobStatus::Dead,
        "TimedOut" => JobStatus::TimedOut,
        "Retrying" => JobStatus::Retrying,
        "Archived" => JobStatus::Archived,
        _ => JobStatus::Dead, // Fallback for unknown status values
    }
}

/// Parses the `dependency_status` column of the archive table.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn parse_archived_dependency_status(
    value: Option<&str>,
) -> crate::workflow::DependencyStatus {
    value
        .and_then(|s| {
            crate::workflow::DependencyStatus::parse_from_db(&s.trim_matches('"').to_lowercase())
                .ok()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archival_reason_db_round_trip() {
        for reason in [
            ArchivalReason::Automatic,
            ArchivalReason::Manual,
            ArchivalReason::Compliance,
            ArchivalReason::Maintenance,
        ] {
            assert_eq!(reason.to_string(), reason.as_str());
            assert_eq!(
                ArchivalReason::parse_from_db(reason.as_str()),
                Some(reason.clone())
            );
            // Legacy JSON-quoted form written by serde_json::to_string
            let quoted = serde_json::to_string(&reason).unwrap();
            assert_eq!(ArchivalReason::parse_from_db(&quoted), Some(reason));
        }
        assert_eq!(
            ArchivalReason::parse_from_db("manual"),
            Some(ArchivalReason::Manual)
        );
        assert_eq!(ArchivalReason::parse_from_db(""), None);
        assert_eq!(ArchivalReason::parse_from_db("Unknown"), None);
    }

    #[test]
    fn test_archived_payload_round_trip() {
        let payload = serde_json::json!({"data": "x".repeat(500)});
        let config = ArchivalConfig::new();

        let compressing = ArchivalPolicy::new().compress_archived_payloads(true);
        let (stored, compressed, original) =
            encode_archived_payload(&payload, &compressing, &config).unwrap();
        assert!(compressed);
        assert!(stored.len() < original);
        assert_eq!(
            decode_archived_payload(&stored, compressed).unwrap(),
            payload
        );

        let plain = ArchivalPolicy::new().compress_archived_payloads(false);
        let (stored, compressed, original) =
            encode_archived_payload(&payload, &plain, &config).unwrap();
        assert!(!compressed);
        assert_eq!(stored.len(), original);
        assert_eq!(
            decode_archived_payload(&stored, compressed).unwrap(),
            payload
        );
    }

    #[test]
    fn test_parse_archived_columns() {
        assert_eq!(parse_archived_status("Completed"), JobStatus::Completed);
        assert_eq!(parse_archived_status("\"TimedOut\""), JobStatus::TimedOut);
        assert_eq!(
            parse_archived_dependency_status(Some("satisfied")),
            crate::workflow::DependencyStatus::Satisfied
        );
        // The archive table's column default is 'Pending', which is not a dependency status.
        assert_eq!(
            parse_archived_dependency_status(Some("Pending")),
            crate::workflow::DependencyStatus::None
        );
        assert_eq!(
            parse_archived_dependency_status(None),
            crate::workflow::DependencyStatus::None
        );
    }

    #[test]
    fn test_archival_policy_default() {
        let policy = ArchivalPolicy::default();
        assert!(policy.enabled);
        assert!(policy.compress_payloads);
        assert_eq!(policy.batch_size, 1000);
        assert!(policy.archive_completed_after.is_some());
    }

    #[test]
    fn test_archival_policy_builder() {
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::days(7))
            .archive_failed_after(Duration::days(30))
            .purge_archived_after(Duration::days(365))
            .compress_archived_payloads(true)
            .with_batch_size(500)
            .enabled(true);

        assert_eq!(policy.archive_completed_after, Some(Duration::days(7)));
        assert_eq!(policy.archive_failed_after, Some(Duration::days(30)));
        assert_eq!(policy.purge_archived_after, Some(Duration::days(365)));
        assert!(policy.compress_payloads);
        assert_eq!(policy.batch_size, 500);
        assert!(policy.enabled);
    }

    #[test]
    fn test_should_archive() {
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::days(7))
            .archive_failed_after(Duration::days(30));

        // Test completed jobs
        assert!(policy.should_archive(&JobStatus::Completed, Duration::days(8)));
        assert!(!policy.should_archive(&JobStatus::Completed, Duration::days(6)));

        // Test failed jobs
        assert!(policy.should_archive(&JobStatus::Failed, Duration::days(31)));
        assert!(!policy.should_archive(&JobStatus::Failed, Duration::days(29)));

        // Test other statuses
        assert!(!policy.should_archive(&JobStatus::Pending, Duration::days(100)));
        assert!(!policy.should_archive(&JobStatus::Running, Duration::days(100)));
    }

    #[test]
    fn test_should_archive_disabled_policy() {
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::days(1))
            .enabled(false);

        assert!(!policy.should_archive(&JobStatus::Completed, Duration::days(10)));
    }

    #[test]
    fn test_archival_config_default() {
        let config = ArchivalConfig::default();
        assert_eq!(config.compression_level, 6);
        assert!(config.verify_compression);
    }

    #[test]
    fn test_archival_config_builder() {
        let config = ArchivalConfig::new()
            .with_compression_level(9)
            .with_compression_verification(false);

        assert_eq!(config.compression_level, 9);
        assert!(!config.verify_compression);
    }

    #[test]
    fn test_archival_config_ignores_removed_max_payload_size() {
        let config: ArchivalConfig = serde_json::from_value(serde_json::json!({
            "compression_level": 3,
            "max_payload_size": 1024,
            "verify_compression": false
        }))
        .unwrap();
        assert_eq!(config.compression_level, 3);
        assert!(!config.verify_compression);
    }

    #[test]
    fn test_verify_compressed_payload() {
        let original = serde_json::to_vec(&serde_json::json!({"data": "y".repeat(300)})).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&original).unwrap();
        let policy = ArchivalPolicy::new().compress_archived_payloads(true);
        for verify in [true, false] {
            let config = ArchivalConfig::new().with_compression_verification(verify);
            let (stored, compressed, _) =
                encode_archived_payload(&payload, &policy, &config).unwrap();
            assert!(compressed);
            assert!(verify_compressed_payload(&stored, &original).is_ok());
        }

        let (stored, _, _) =
            encode_archived_payload(&payload, &policy, &ArchivalConfig::new()).unwrap();
        // Corrupt data, truncated data and data of another payload all fail.
        let mut corrupt = stored.clone();
        let middle = corrupt.len() / 2;
        corrupt[middle] ^= 0xff;
        assert!(verify_compressed_payload(&corrupt, &original).is_err());
        assert!(verify_compressed_payload(&stored[..stored.len() - 4], &original).is_err());
        let err = verify_compressed_payload(&stored, b"{}").unwrap_err();
        assert!(err.to_string().contains("failed verification"), "{err}");
    }

    #[test]
    fn test_archival_reason_display() {
        assert_eq!(ArchivalReason::Automatic.to_string(), "Automatic");
        assert_eq!(ArchivalReason::Manual.to_string(), "Manual");
        assert_eq!(ArchivalReason::Compliance.to_string(), "Compliance");
        assert_eq!(ArchivalReason::Maintenance.to_string(), "Maintenance");
    }
}
