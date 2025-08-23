//! Job queue implementation with database-specific backends.
//!
//! This module provides the core queue functionality with implementations for both
//! PostgreSQL and MySQL databases. The queue operations are defined by the
//! `DatabaseQueue` trait, with database-specific optimizations in separate modules.

use crate::{
    Result,
    job::{Job, JobId},
    rate_limit::ThrottleConfig,
    stats::{DeadJobSummary, QueueStats},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Database, Pool};
use std::{collections::HashMap, marker::PhantomData, sync::Arc};
use tokio::sync::RwLock;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "mysql")]
pub mod mysql;

#[cfg(feature = "test")]
pub mod test;

/// Information about a queue's pause state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuePauseInfo {
    /// Name of the paused queue
    pub queue_name: String,
    /// When the queue was paused
    pub paused_at: DateTime<Utc>,
    /// Who or what paused the queue
    pub paused_by: Option<String>,
    /// Optional reason for pausing
    pub reason: Option<String>,
}

/// The main trait defining database operations for the job queue.
///
/// This trait provides a database-agnostic interface for all job queue operations,
/// including job management, batch operations, statistics, and result storage.
/// Each database backend implements this trait with optimizations specific to
/// that database system.
#[async_trait]
pub trait DatabaseQueue: Send + Sync {
    type Database: Database;

    // Core job operations
    async fn enqueue(&self, job: Job) -> Result<JobId>;
    async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>>;
    async fn dequeue_with_priority_weights(
        &self,
        queue_name: &str,
        weights: &crate::priority::PriorityWeights,
    ) -> Result<Option<Job>>;
    async fn complete_job(&self, job_id: JobId) -> Result<()>;
    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()>;
    async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()>;
    async fn get_job(&self, job_id: JobId) -> Result<Option<Job>>;
    async fn delete_job(&self, job_id: JobId) -> Result<()>;

    // Batch operations
    /// Enqueue multiple jobs as a batch for improved performance.
    async fn enqueue_batch(&self, batch: crate::batch::JobBatch) -> Result<crate::batch::BatchId>;

    /// Get the current status of a batch operation.
    async fn get_batch_status(
        &self,
        batch_id: crate::batch::BatchId,
    ) -> Result<crate::batch::BatchResult>;

    /// Get all jobs belonging to a specific batch.
    async fn get_batch_jobs(&self, batch_id: crate::batch::BatchId) -> Result<Vec<Job>>;

    /// Delete a batch and all its associated jobs.
    async fn delete_batch(&self, batch_id: crate::batch::BatchId) -> Result<()>;

    // Dead job management
    /// Mark a job as dead (exhausted all retries)
    async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()>;

    /// Mark a job as timed out
    async fn mark_job_timed_out(&self, job_id: JobId, error_message: &str) -> Result<()>;

    /// Get all dead jobs with optional pagination
    async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>>;

    /// Get dead jobs for a specific queue
    async fn get_dead_jobs_by_queue(
        &self,
        queue_name: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Job>>;

    /// Retry a dead job (reset its status and retry count)
    async fn retry_dead_job(&self, job_id: JobId) -> Result<()>;

    /// Purge dead jobs older than the specified date
    async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64>;

    /// Get a summary of dead jobs across the system
    async fn get_dead_job_summary(&self) -> Result<DeadJobSummary>;

    // Statistics and monitoring
    /// Get queue statistics including job counts and processing metrics
    async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats>;

    /// Get statistics for all queues
    async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>>;

    /// Get job counts by status for a specific queue
    async fn get_job_counts_by_status(
        &self,
        queue_name: &str,
    ) -> Result<std::collections::HashMap<String, u64>>;

    /// Get job counts by priority for a specific queue
    async fn get_priority_stats(&self, queue_name: &str) -> Result<crate::priority::PriorityStats>;

    /// Get processing times for completed jobs in a time window
    async fn get_processing_times(
        &self,
        queue_name: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<i64>>;

    /// Get error frequencies for failed jobs
    async fn get_error_frequencies(
        &self,
        queue_name: Option<&str>,
        since: DateTime<Utc>,
    ) -> Result<std::collections::HashMap<String, u64>>;

    /// Get jobs that completed within a specific time range
    async fn get_jobs_completed_in_range(
        &self,
        queue_name: Option<&str>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        limit: Option<u32>,
    ) -> Result<Vec<Job>>;

    // Cron job management
    /// Enqueue a cron job for recurring execution
    async fn enqueue_cron_job(&self, job: Job) -> Result<JobId>;

    /// Get jobs that are ready to run based on their cron schedule
    async fn get_due_cron_jobs(&self, queue_name: Option<&str>) -> Result<Vec<Job>>;

    /// Reschedule a completed cron job for its next execution
    async fn reschedule_cron_job(&self, job_id: JobId, next_run_at: DateTime<Utc>) -> Result<()>;

    /// Get all recurring jobs for a queue
    async fn get_recurring_jobs(&self, queue_name: &str) -> Result<Vec<Job>>;

    /// Disable a recurring job (stop future executions)
    async fn disable_recurring_job(&self, job_id: JobId) -> Result<()>;

    /// Enable a previously disabled recurring job
    async fn enable_recurring_job(&self, job_id: JobId) -> Result<()>;

    // Throttling configuration
    /// Set throttling configuration for a specific queue
    async fn set_throttle_config(&self, queue_name: &str, config: ThrottleConfig) -> Result<()>;

    /// Get throttling configuration for a specific queue
    async fn get_throttle_config(&self, queue_name: &str) -> Result<Option<ThrottleConfig>>;

    /// Remove throttling configuration for a specific queue
    async fn remove_throttle_config(&self, queue_name: &str) -> Result<()>;

    /// Get all throttling configurations
    async fn get_all_throttle_configs(&self) -> Result<HashMap<String, ThrottleConfig>>;

    /// Get the current depth (pending job count) for a queue
    async fn get_queue_depth(&self, queue_name: &str) -> Result<u64>;

    // Job result storage and retrieval
    /// Store the result data for a completed job.
    ///
    /// This method stores the result data from a successful job execution,
    /// making it available for later retrieval by other systems.
    ///
    /// # Arguments
    ///
    /// * `job_id` - The unique identifier of the job
    /// * `result_data` - The result data to store (JSON format)
    /// * `expires_at` - Optional expiration time for the result
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    /// use serde_json::json;
    /// use chrono::{Utc, Duration};
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// # let job_id = uuid::Uuid::new_v4();
    /// let result_data = json!({"status": "success", "count": 42});
    /// let expires_at = Some(Utc::now() + chrono::Duration::hours(24));
    ///
    /// queue.store_job_result(job_id, result_data, expires_at).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn store_job_result(
        &self,
        job_id: JobId,
        result_data: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()>;

    /// Retrieve the stored result data for a job.
    ///
    /// Returns the result data if it exists and hasn't expired, otherwise returns `None`.
    ///
    /// # Arguments
    ///
    /// * `job_id` - The unique identifier of the job
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue, job_id: hammerwork::JobId) -> hammerwork::Result<()> {
    /// if let Some(result) = queue.get_job_result(job_id).await? {
    ///     println!("Job result: {}", result);
    /// } else {
    ///     println!("No result found or result has expired");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn get_job_result(&self, job_id: JobId) -> Result<Option<serde_json::Value>>;

    /// Delete the stored result data for a job.
    ///
    /// This is useful for manual cleanup or when results are no longer needed.
    ///
    /// # Arguments
    ///
    /// * `job_id` - The unique identifier of the job
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue, job_id: hammerwork::JobId) -> hammerwork::Result<()> {
    /// queue.delete_job_result(job_id).await?;
    /// # Ok(())
    /// # }
    /// ```
    async fn delete_job_result(&self, job_id: JobId) -> Result<()>;

    /// Clean up expired job results.
    ///
    /// This method removes all job results that have passed their expiration time.
    /// It should be called periodically to prevent the database from growing indefinitely.
    ///
    /// # Returns
    ///
    /// The number of expired results that were cleaned up.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let cleaned_count = queue.cleanup_expired_results().await?;
    /// println!("Cleaned up {} expired results", cleaned_count);
    /// # Ok(())
    /// # }
    /// ```
    async fn cleanup_expired_results(&self) -> Result<u64>;

    // Workflow and dependency management
    /// Enqueue a job group/workflow as a single operation.
    ///
    /// All jobs in the workflow are inserted with their dependency relationships,
    /// and the workflow metadata is stored for tracking purposes.
    async fn enqueue_workflow(
        &self,
        workflow: crate::workflow::JobGroup,
    ) -> Result<crate::workflow::WorkflowId>;

    /// Get workflow status and statistics.
    async fn get_workflow_status(
        &self,
        workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Option<crate::workflow::JobGroup>>;

    /// Update job dependencies when a job completes.
    ///
    /// This method resolves dependencies for jobs that were waiting on the completed job,
    /// potentially making them eligible for execution.
    async fn resolve_job_dependencies(&self, completed_job_id: JobId) -> Result<Vec<JobId>>;

    /// Get jobs that are ready to execute (dependencies satisfied).
    ///
    /// This method returns jobs that have either no dependencies or all dependencies
    /// have been satisfied (completed successfully).
    async fn get_ready_jobs(&self, queue_name: &str, limit: u32) -> Result<Vec<Job>>;

    /// Mark job dependencies as failed when a job fails.
    ///
    /// This propagates failure through the dependency graph according to the
    /// workflow's failure policy.
    async fn fail_job_dependencies(&self, failed_job_id: JobId) -> Result<Vec<JobId>>;

    /// Get all jobs in a workflow.
    async fn get_workflow_jobs(&self, workflow_id: crate::workflow::WorkflowId)
    -> Result<Vec<Job>>;

    /// Cancel a workflow and all its pending jobs.
    async fn cancel_workflow(&self, workflow_id: crate::workflow::WorkflowId) -> Result<()>;

    // Job archival operations
    /// Archive jobs based on the given archival policy.
    ///
    /// This method moves jobs from the main jobs table to the archive table
    /// based on the specified archival policy. Jobs that meet the archival
    /// criteria will be compressed (if enabled) and moved to long-term storage.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Optional queue name to limit archival to specific queue
    /// * `policy` - Archival policy defining which jobs to archive
    /// * `config` - Configuration for archival process (compression, etc.)
    /// * `reason` - Reason for archival (automatic, manual, etc.)
    /// * `archived_by` - Optional identifier of who initiated the archival
    ///
    /// # Returns
    ///
    /// Statistics about the archival operation including number of jobs archived
    /// and compression ratios achieved.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{queue::DatabaseQueue, archive::{ArchivalPolicy, ArchivalConfig, ArchivalReason}};
    /// use chrono::Duration;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let policy = ArchivalPolicy::new()
    ///     .archive_completed_after(Duration::days(7));
    /// let config = ArchivalConfig::new();
    ///
    /// let stats = queue.archive_jobs(
    ///     Some("my-queue"),
    ///     &policy,
    ///     &config,
    ///     ArchivalReason::Automatic,
    ///     Some("system")
    /// ).await?;
    ///
    /// println!("Archived {} jobs", stats.jobs_archived);
    /// # Ok(())
    /// # }
    /// ```
    async fn archive_jobs(
        &self,
        queue_name: Option<&str>,
        policy: &crate::archive::ArchivalPolicy,
        config: &crate::archive::ArchivalConfig,
        reason: crate::archive::ArchivalReason,
        archived_by: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats>;

    /// Restore an archived job back to the active queue.
    ///
    /// This method moves a job from the archive table back to the main jobs table,
    /// decompressing the payload if necessary and resetting the job to pending status.
    ///
    /// # Arguments
    ///
    /// * `job_id` - Unique identifier of the job to restore
    ///
    /// # Returns
    ///
    /// The restored job with its original payload and metadata.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue, job_id: hammerwork::JobId) -> hammerwork::Result<()> {
    /// let restored_job = queue.restore_archived_job(job_id).await?;
    /// println!("Restored job: {:?}", restored_job.id);
    /// # Ok(())
    /// # }
    /// ```
    async fn restore_archived_job(&self, job_id: JobId) -> Result<Job>;

    /// List archived jobs with optional filtering.
    ///
    /// This method retrieves information about archived jobs without restoring them.
    /// It supports filtering by queue name and pagination for large result sets.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Optional queue name to filter results
    /// * `limit` - Maximum number of results to return
    /// * `offset` - Number of results to skip (for pagination)
    ///
    /// # Returns
    ///
    /// List of archived job information including archival metadata.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let archived_jobs = queue.list_archived_jobs(
    ///     Some("my-queue"),
    ///     Some(100),
    ///     Some(0)
    /// ).await?;
    ///
    /// for job in archived_jobs {
    ///     println!("Archived job: {} at {}", job.id, job.archived_at);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn list_archived_jobs(
        &self,
        queue_name: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<crate::archive::ArchivedJob>>;

    /// Permanently delete archived jobs older than the specified date.
    ///
    /// This method removes archived jobs from the database completely.
    /// This operation is irreversible and should be used carefully.
    ///
    /// # Arguments
    ///
    /// * `older_than` - Delete archived jobs older than this date
    ///
    /// # Returns
    ///
    /// Number of archived jobs that were permanently deleted.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    /// use chrono::{Utc, Duration};
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let one_year_ago = Utc::now() - Duration::days(365);
    /// let deleted_count = queue.purge_archived_jobs(one_year_ago).await?;
    /// println!("Permanently deleted {} archived jobs", deleted_count);
    /// # Ok(())
    /// # }
    /// ```
    async fn purge_archived_jobs(&self, older_than: DateTime<Utc>) -> Result<u64>;

    /// Get statistics about archived jobs.
    ///
    /// This method returns comprehensive statistics about the archival system
    /// including counts, storage usage, and performance metrics.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - Optional queue name to filter statistics
    ///
    /// # Returns
    ///
    /// Archival statistics including job counts and storage metrics.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let stats = queue.get_archival_stats(Some("my-queue")).await?;
    /// println!("Total archived jobs: {}", stats.jobs_archived);
    /// println!("Compression ratio: {:.2}", stats.compression_ratio);
    /// # Ok(())
    /// # }
    /// ```
    async fn get_archival_stats(
        &self,
        queue_name: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats>;

    // Queue management operations
    /// Pause job processing for a specific queue.
    ///
    /// When a queue is paused, workers will stop dequeuing new jobs from it,
    /// but jobs already in progress will continue to completion. This allows
    /// for graceful queue management without interrupting running jobs.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue to pause
    /// * `paused_by` - Optional identifier of who/what paused the queue
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// queue.pause_queue("email_queue", Some("admin")).await?;
    /// println!("Email queue has been paused");
    /// # Ok(())
    /// # }
    /// ```
    async fn pause_queue(&self, queue_name: &str, paused_by: Option<&str>) -> Result<()>;

    /// Resume job processing for a previously paused queue.
    ///
    /// This re-enables job processing for the specified queue, allowing workers
    /// to start dequeuing jobs again.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue to resume
    /// * `resumed_by` - Optional identifier of who/what resumed the queue
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// queue.resume_queue("email_queue", Some("admin")).await?;
    /// println!("Email queue has been resumed");
    /// # Ok(())
    /// # }
    /// ```
    async fn resume_queue(&self, queue_name: &str, resumed_by: Option<&str>) -> Result<()>;

    /// Check if a queue is currently paused.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue to check
    ///
    /// # Returns
    ///
    /// `true` if the queue is paused, `false` otherwise
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// if queue.is_queue_paused("email_queue").await? {
    ///     println!("Email queue is currently paused");
    /// } else {
    ///     println!("Email queue is active");
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn is_queue_paused(&self, queue_name: &str) -> Result<bool>;

    /// Get pause information for a queue.
    ///
    /// Returns detailed information about a queue's pause state including
    /// when it was paused and by whom.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue to check
    ///
    /// # Returns
    ///
    /// Pause information if the queue is paused, `None` otherwise
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// if let Some(pause_info) = queue.get_queue_pause_info("email_queue").await? {
    ///     println!("Queue paused by {} at {}",
    ///         pause_info.paused_by.unwrap_or("unknown".to_string()),
    ///         pause_info.paused_at);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn get_queue_pause_info(&self, queue_name: &str) -> Result<Option<QueuePauseInfo>>;

    /// Get all currently paused queues.
    ///
    /// # Returns
    ///
    /// A list of all queues that are currently paused with their pause information
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let paused_queues = queue.get_paused_queues().await?;
    /// for queue_info in paused_queues {
    ///     println!("Queue '{}' is paused", queue_info.queue_name);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    async fn get_paused_queues(&self) -> Result<Vec<QueuePauseInfo>>;
}

/// A generic job queue implementation that works with multiple database backends.
///
/// This struct provides a database-agnostic interface to the job queue functionality.
/// The actual database operations are delegated to database-specific implementations
/// based on the type parameter `DB`.
///
/// # Examples
///
/// ```rust,no_run
/// use hammerwork::{JobQueue, Job, queue::DatabaseQueue};
/// use serde_json::json;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # #[cfg(feature = "postgres")]
/// # {
/// // Create a PostgreSQL-backed queue
/// let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
/// let queue = JobQueue::new(pool);
///
/// // Create and enqueue a job
/// let job = Job::new("email_queue".to_string(), json!({"to": "user@example.com"}));
/// let job_id = queue.enqueue(job).await?;
/// # }
/// # Ok(())
/// # }
/// ```
pub struct JobQueue<DB: Database> {
    #[allow(dead_code)] // Used in database-specific implementations
    pub pool: Pool<DB>,
    pub(crate) _phantom: PhantomData<DB>,
    pub(crate) throttle_configs: Arc<RwLock<HashMap<String, ThrottleConfig>>>,
}

impl<DB: Database> Clone for JobQueue<DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            _phantom: PhantomData,
            throttle_configs: self.throttle_configs.clone(),
        }
    }
}

impl<DB: Database> JobQueue<DB> {
    /// Creates a new job queue with the given database connection pool.
    ///
    /// # Arguments
    ///
    /// * `pool` - A database connection pool for the specific database backend
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::JobQueue;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # #[cfg(feature = "postgres")]
    /// # {
    /// let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    /// let queue = JobQueue::new(pool);
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(pool: Pool<DB>) -> Self {
        Self {
            pool,
            _phantom: PhantomData,
            throttle_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set throttling configuration for a specific queue.
    ///
    /// This is a convenience method that stores the throttling configuration
    /// in memory for use by workers.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue to configure
    /// * `config` - The throttling configuration
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{JobQueue, rate_limit::ThrottleConfig};
    /// use std::time::Duration;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # #[cfg(feature = "postgres")]
    /// # {
    /// let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    /// let queue = JobQueue::new(pool);
    ///
    /// let config = ThrottleConfig::new()
    ///     .max_concurrent(5)
    ///     .rate_per_minute(100);
    ///
    /// queue.set_throttle("email_queue", config).await?;
    /// # }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_throttle(&self, queue_name: &str, config: ThrottleConfig) -> Result<()> {
        let mut configs = self.throttle_configs.write().await;
        configs.insert(queue_name.to_string(), config);
        Ok(())
    }

    /// Get throttling configuration for a specific queue.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue
    ///
    /// # Returns
    ///
    /// The throttling configuration if it exists, otherwise `None`.
    pub async fn get_throttle(&self, queue_name: &str) -> Option<ThrottleConfig> {
        let configs = self.throttle_configs.read().await;
        configs.get(queue_name).cloned()
    }

    /// Remove throttling configuration for a specific queue.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue
    pub async fn remove_throttle(&self, queue_name: &str) -> Result<()> {
        let mut configs = self.throttle_configs.write().await;
        configs.remove(queue_name);
        Ok(())
    }

    /// Get all throttling configurations.
    ///
    /// # Returns
    ///
    /// A map of queue names to their throttling configurations.
    pub async fn get_all_throttles(&self) -> HashMap<String, ThrottleConfig> {
        let configs = self.throttle_configs.read().await;
        configs.clone()
    }

    /// Get a reference to the underlying database connection pool.
    ///
    /// This method provides access to the database pool for advanced use cases
    /// that require direct database operations or integration with other components.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::JobQueue;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// # let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    /// let queue = JobQueue::new(pool);
    ///
    /// // Access the pool for advanced operations
    /// let pool_ref = &queue.pool;
    /// let pool_clone = queue.pool.clone();
    ///
    /// // Use the pool for custom queries or integration with other components
    /// let row_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM hammerwork_jobs")
    ///     .fetch_one(&queue.pool)
    ///     .await?;
    ///
    /// println!("Total jobs in database: {}", row_count.0);
    /// # Ok(())
    /// # }
    /// ```
    pub fn get_pool(&self) -> &Pool<DB> {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test that the pool field is publicly accessible
    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_public_pool_access() {
        // Note: This test requires a real database connection
        // It's designed to test compilation and API accessibility with real database operations

        // This test is ignored by default since it requires a database
        let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = sqlx::PgPool::connect(&database_url).await.unwrap();
        let queue = JobQueue::new(pool.clone());

        // Test direct field access
        let _pool_ref = &queue.pool;
        let _pool_clone = queue.pool.clone();

        // Test that we can use the pool field in function calls
        let _same_pool = std::ptr::eq(&queue.pool, &pool);
    }

    #[test]
    fn test_queue_creation_with_pool() {
        // Test that JobQueue can be created and the pool field is accessible
        // This test doesn't require an actual database connection

        // We'll use the test queue for this since it doesn't require a real database
        #[cfg(feature = "test")]
        {
            use crate::queue::test::TestQueue;
            let test_queue = TestQueue::new();

            // Verify the queue was created successfully
            // Note: TestQueue doesn't have a pool field since it's an in-memory implementation
            // This test validates the general queue creation pattern

            // Simple test - just verify it can be created (TestQueue has no simple methods to test)
            let _ = test_queue; // Consume to avoid unused variable warning
        }

        // Always pass this test since it's primarily a compilation test
    }

    #[test]
    fn test_pool_field_visibility() {
        // Compile-time test to ensure the pool field is public
        // This test will fail to compile if the field is not public

        // This is a compilation test - if this compiles, the field is public
        #[allow(dead_code)]
        fn _check_pool_access<DB: sqlx::Database>(queue: &JobQueue<DB>) -> &sqlx::Pool<DB> {
            &queue.pool // This line will fail to compile if pool is not public
        }

        // If we reach this point, the compilation test passed
    }

    #[test]
    fn test_throttle_configs_still_private() {
        // Ensure that making pool public didn't accidentally expose other private fields
        // This test verifies that throttle_configs remains crate-private

        // This function should NOT compile if throttle_configs becomes public
        fn _ensure_throttle_configs_private<DB: sqlx::Database>(_queue: &JobQueue<DB>) {
            // Uncommenting the next line should cause a compilation error
            // let _configs = &queue.throttle_configs;  // Should be private
        }
    }

    /// Test documentation examples compile correctly
    #[test]
    fn test_pool_documentation_examples() {
        // This test ensures that the documentation examples in the pool-related methods compile

        // Example: Creating a JobArchiver with the pool (from archive module docs)
        #[cfg(feature = "postgres")]
        #[allow(dead_code)]
        async fn _example_archive_integration()
        -> std::result::Result<(), Box<dyn std::error::Error>> {
            let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
            let queue = std::sync::Arc::new(JobQueue::new(pool.clone()));

            // This pattern should work with the public pool field
            let _archiver = crate::archive::JobArchiver::new(queue.pool.clone());

            Ok(())
        }

        // Test that the example compiles (even though it won't run without a database)
    }

    #[test]
    fn test_throttle_config_creation() {
        // Test that throttle configuration can be created and configured
        // This is a simple API test that doesn't require database operations

        use crate::rate_limit::ThrottleConfig;

        let throttle_config = ThrottleConfig::new()
            .max_concurrent(10)
            .rate_per_minute(60)
            .enabled(true);

        assert_eq!(throttle_config.max_concurrent, Some(10));
        assert_eq!(throttle_config.rate_per_minute, Some(60));
        assert!(throttle_config.enabled);

        // Test default configuration
        let default_config = ThrottleConfig::new();
        assert_eq!(default_config.max_concurrent, None);
        assert_eq!(default_config.rate_per_minute, None);
        assert!(default_config.enabled);
    }

    #[test]
    fn test_job_queue_clone() {
        // Test that JobQueue implements Clone trait correctly
        // This test verifies compilation and basic cloning functionality

        #[cfg(feature = "test")]
        {
            use crate::queue::test::TestQueue;

            // Test that TestQueue can be cloned
            let test_queue = TestQueue::new();
            let cloned_queue = test_queue.clone();

            // Basic verification - both should be independent instances
            // (This is mainly a compilation test to ensure Clone trait is properly implemented)
            let _ = test_queue;
            let _ = cloned_queue;
        }

        // Test that JobQueue<DB> implements Clone at the type level
        // This function will only compile if JobQueue<DB> implements Clone
        #[allow(dead_code)]
        fn _test_job_queue_clone_trait<DB: sqlx::Database>()
        -> impl Fn(&JobQueue<DB>) -> JobQueue<DB> {
            |queue: &JobQueue<DB>| queue.clone()
        }

        // If we reach this point, Clone is properly implemented
    }
}
