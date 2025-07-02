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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArchivalReason {
    /// Job was archived due to automatic policy.
    Automatic,
    /// Job was manually archived by an administrator.
    Manual,
    /// Job was archived due to compliance requirements.
    Compliance,
    /// Job was archived due to database maintenance.
    Maintenance,
}

impl Default for ArchivalReason {
    fn default() -> Self {
        Self::Automatic
    }
}

impl std::fmt::Display for ArchivalReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Automatic => write!(f, "Automatic"),
            Self::Manual => write!(f, "Manual"),
            Self::Compliance => write!(f, "Compliance"),
            Self::Maintenance => write!(f, "Maintenance"),
        }
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
    /// Maximum size in bytes for uncompressed payloads before archiving.
    pub max_payload_size: usize,
    /// Whether to validate compressed data integrity.
    pub verify_compression: bool,
}

impl Default for ArchivalConfig {
    fn default() -> Self {
        Self {
            compression_level: 6,          // Balanced compression/speed
            max_payload_size: 1024 * 1024, // 1MB
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
    /// assert_eq!(config.max_payload_size, 1024 * 1024);
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

    /// Sets the maximum payload size before archiving is required.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::archive::ArchivalConfig;
    ///
    /// let config = ArchivalConfig::new()
    ///     .with_max_payload_size(2048);
    /// assert_eq!(config.max_payload_size, 2048);
    /// ```
    pub fn with_max_payload_size(mut self, size: usize) -> Self {
        self.max_payload_size = size;
        self
    }

    /// Sets whether to verify compression integrity.
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
    /// Global archival configuration.
    config: ArchivalConfig,
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
            config: ArchivalConfig::default(),
        }
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
    /// ```rust,no_run
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

        // Get the archival policy for this queue
        let default_policy = ArchivalPolicy::default();
        let policy = queue_name
            .and_then(|name| self.policies.get(name))
            .unwrap_or(&default_policy);

        // Estimate total jobs to be archived (this is a simplified estimation)
        let estimated_jobs = self
            .estimate_archival_jobs(queue, queue_name, policy)
            .await?;

        // Publish bulk archive started event
        if let Some(callback) = &progress_callback {
            callback(0, estimated_jobs);
        }

        // Perform the actual archival
        let stats = queue
            .archive_jobs(queue_name, policy, &self.config, reason, archived_by)
            .await?;

        // Report completion
        if let Some(callback) = &progress_callback {
            callback(stats.jobs_archived, estimated_jobs);
        }

        Ok((operation_id, stats))
    }

    /// Archive jobs with WebSocket event publishing for real-time dashboard updates.
    ///
    /// This is a convenience method that wraps `archive_jobs_with_progress` and publishes
    /// WebSocket events for dashboard integration.
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

        // Get the archival policy for this queue
        let default_policy = ArchivalPolicy::default();
        let policy = queue_name
            .and_then(|name| self.policies.get(name))
            .unwrap_or(&default_policy);

        // Estimate total jobs to be archived
        let estimated_jobs = self
            .estimate_archival_jobs(queue, queue_name, policy)
            .await?;

        // Publish bulk archive started event
        event_publisher(ArchiveEvent::BulkArchiveStarted {
            operation_id: operation_id.clone(),
            estimated_jobs,
        });

        // Perform the actual archival
        let stats = queue
            .archive_jobs(queue_name, policy, &self.config, reason, archived_by)
            .await?;

        // Publish completion event
        event_publisher(ArchiveEvent::BulkArchiveCompleted {
            operation_id: operation_id.clone(),
            stats: stats.clone(),
        });

        Ok((operation_id, stats))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config.max_payload_size, 1024 * 1024);
        assert!(config.verify_compression);
    }

    #[test]
    fn test_archival_config_builder() {
        let config = ArchivalConfig::new()
            .with_compression_level(9)
            .with_max_payload_size(2048)
            .with_compression_verification(false);

        assert_eq!(config.compression_level, 9);
        assert_eq!(config.max_payload_size, 2048);
        assert!(!config.verify_compression);
    }

    #[test]
    fn test_archival_reason_display() {
        assert_eq!(ArchivalReason::Automatic.to_string(), "Automatic");
        assert_eq!(ArchivalReason::Manual.to_string(), "Manual");
        assert_eq!(ArchivalReason::Compliance.to_string(), "Compliance");
        assert_eq!(ArchivalReason::Maintenance.to_string(), "Maintenance");
    }
}
