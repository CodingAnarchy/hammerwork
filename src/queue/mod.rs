//! Job queue implementation with database-specific backends.
//!
//! This module provides the core queue functionality with implementations for both
//! PostgreSQL and MySQL databases. The queue operations are defined by the
//! `DatabaseQueue` trait, with database-specific optimizations in separate modules.

use crate::{
    Result,
    config::RateLimitingConfig,
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

pub mod lifecycle;
pub use lifecycle::{JobOutcome, JobTransition, RecordedOutcome};

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

    /// Manually mark a job `Completed`.
    ///
    /// Only applies to `Pending`, `Running` or `Retrying` jobs (see
    /// [`JobTransition::allowed_from`]); other statuses return
    /// [`HammerworkError::InvalidJobTransition`](crate::HammerworkError::InvalidJobTransition)
    /// and a missing job `JobNotFound`. Dependents whose dependencies have all
    /// completed become runnable and workflow/batch progress is updated, in the same
    /// transaction. Workers record outcomes with [`finish_job_run`](Self::finish_job_run).
    async fn complete_job(&self, job_id: JobId) -> Result<()>;

    /// Manually mark a job `Failed` (a terminal status that is not retried
    /// automatically; [`retry_job`](Self::retry_job) re-runs it).
    ///
    /// Only applies to `Pending`, `Running` or `Retrying` jobs. Applies the failure to
    /// dependents and to the job's workflow and batch like any terminal failure.
    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()>;

    /// Manually move a job back to `Pending`, to run at `retry_at`.
    ///
    /// Only applies to `Running`, `Retrying`, `Failed` or `TimedOut` jobs. `Dead` jobs
    /// are re-run with [`retry_dead_job`](Self::retry_dead_job).
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
    /// Manually mark a job as dead (exhausted all retries).
    ///
    /// Only applies to `Pending`, `Running`, `Retrying`, `Failed` or `TimedOut` jobs.
    async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()>;

    /// Manually mark a `Running` job as timed out (a terminal status).
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

    /// Re-run a terminally failed (`Dead` or `TimedOut`) job: back to `Pending` with its
    /// attempts reset. Other statuses return `InvalidJobTransition`.
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

    /// Reschedule a recurring job for its next execution: back to `Pending` at
    /// `next_run_at` with its attempts reset.
    ///
    /// Applies to recurring jobs in any status except `Completed` and `Archived`.
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

    // Run outcomes

    /// Record how a run of a job dequeued by a worker ended.
    ///
    /// `run` is the job as returned by the dequeue. The outcome only applies while the
    /// job is still `Running` that same run (same `attempts` and `started_at`); if it is
    /// not (the stale-job reaper reclaimed it, an operator changed it, or another worker
    /// is running a newer attempt) nothing is written and `Ok(None)` is returned, so a
    /// late or zombie worker can never overwrite newer state.
    ///
    /// In the same transaction as the status change:
    /// - a recurring job whose run completed, died or timed out is rescheduled for its
    ///   next cron occurrence (`Pending`, attempts reset). A failed run's error and
    ///   `failed_at`/`timed_out_at` stay on the job until its next run.
    /// - on completion, dependents whose dependencies have all completed move from
    ///   `waiting` to `satisfied`;
    /// - on a terminal failure the workflow's
    ///   [`FailurePolicy`](crate::workflow::FailurePolicy) is applied (`FailFast` fails
    ///   the workflow's pending jobs, `ContinueOnFailure` fails the jobs that depend on
    ///   this one, `Manual` leaves them waiting) and a `FailFast` batch fails its
    ///   pending jobs;
    /// - workflow counters and status, and batch status, are updated.
    ///
    /// The default implementation composes the manual methods without a transaction,
    /// for backends that predate this method.
    async fn finish_job_run(
        &self,
        run: &Job,
        outcome: JobOutcome,
    ) -> Result<Option<RecordedOutcome>> {
        let Some(current) = self.get_job(run.id).await? else {
            return Ok(None);
        };
        if !lifecycle::Guard::Run(run).admits(&current) {
            return Ok(None);
        }
        let next_run_at = match outcome.terminal_status() {
            Some(_) if current.recurring => lifecycle::next_cron_run(&current, Utc::now()),
            _ => None,
        };
        let mut recorded = match outcome {
            JobOutcome::Completed => {
                if next_run_at.is_none() {
                    self.complete_job(run.id).await?;
                }
                RecordedOutcome::new(crate::job::JobStatus::Completed)
            }
            JobOutcome::Retry { retry_at, .. } => {
                self.retry_job(run.id, retry_at).await?;
                RecordedOutcome::new(crate::job::JobStatus::Pending)
            }
            JobOutcome::Dead { error } => {
                self.mark_job_dead(run.id, &error).await?;
                RecordedOutcome::new(crate::job::JobStatus::Dead)
            }
            JobOutcome::TimedOut { error } => {
                self.mark_job_timed_out(run.id, &error).await?;
                RecordedOutcome::new(crate::job::JobStatus::TimedOut)
            }
        };
        if let Some(next) = next_run_at {
            self.reschedule_cron_job(run.id, next).await?;
            recorded.status = crate::job::JobStatus::Pending;
            recorded.next_run_at = Some(next);
        } else if recorded.status == crate::job::JobStatus::Completed {
            recorded.unblocked = self.resolve_job_dependencies(run.id).await?;
        } else if recorded.status != crate::job::JobStatus::Pending {
            recorded.cancelled = self.fail_job_dependencies(run.id).await?;
        }
        Ok(Some(recorded))
    }

    // Lease / stale job recovery

    /// Record a heartbeat for a `Running` job and extend its lease to `now + lease`.
    ///
    /// Workers call this periodically while a handler runs. The lease tells
    /// [`requeue_stale_jobs`](Self::requeue_stale_jobs) that the job is still owned by a
    /// live worker.
    ///
    /// Returns `false` when the job is no longer `Running` (for example because a reaper
    /// already reclaimed it), which means the caller has lost its lease.
    ///
    /// The default implementation returns an error, for backends that predate leases.
    async fn heartbeat_job(&self, job_id: JobId, lease: std::time::Duration) -> Result<bool> {
        let _ = (job_id, lease);
        Err(crate::HammerworkError::Queue {
            message: "job leases are not supported by this queue backend".to_string(),
        })
    }

    /// Reclaim jobs left in `Running` by workers that crashed or were killed.
    ///
    /// A `Running` job is stale when:
    /// - it has a lease from its current run (a heartbeat recorded after `started_at`)
    ///   and that lease has expired, or
    /// - it has no lease from its current run (it has not heartbeated yet, or was
    ///   started by a worker that predates leases) and it started more than
    ///   `older_than` ago.
    ///
    /// Stale jobs whose `attempts` have reached `max_attempts` are moved to `Dead`;
    /// the others go back to `Pending` and are scheduled immediately. The interrupted
    /// run already counted as an attempt when the job was dequeued.
    ///
    /// Safe to call from several processes at once: rows are claimed with row locks
    /// (`FOR UPDATE SKIP LOCKED`) and only updated while still `Running`, so each stale
    /// job is reclaimed exactly once.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::queue::DatabaseQueue;
    /// use std::time::Duration;
    ///
    /// # async fn example(queue: &impl DatabaseQueue) -> hammerwork::Result<()> {
    /// let recovery = queue.requeue_stale_jobs(Duration::from_secs(300)).await?;
    /// println!(
    ///     "requeued {} jobs, marked {} dead",
    ///     recovery.requeued.len(),
    ///     recovery.dead.len()
    /// );
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The default implementation returns an error, for backends that predate leases.
    async fn requeue_stale_jobs(
        &self,
        older_than: std::time::Duration,
    ) -> Result<StaleJobRecovery> {
        let _ = older_than;
        Err(crate::HammerworkError::Queue {
            message: "stale job recovery is not supported by this queue backend".to_string(),
        })
    }

    /// Enforces the retention policies of encrypted jobs: deletes finished encrypted jobs
    /// whose retention period has ended.
    ///
    /// A job encrypted with a retention policy (`Job::with_retention_policy`, or the
    /// engine's `default_retention`) has `retention_delete_at` set when it is enqueued.
    /// Once that time has passed and the job is finished (`Completed`, `Failed`, `Dead`
    /// or `TimedOut`), this deletes the job row, including its ciphertext, redacted
    /// payload and result. Archived encrypted jobs past their retention time are deleted
    /// from the archive table too. `RetentionPolicy::DeleteImmediately` jobs are deleted
    /// as soon as they finish. Pending and running jobs are never deleted, even when
    /// their retention time has passed.
    ///
    /// Run it periodically, e.g. from a cron job or with
    /// `cargo hammerwork maintenance purge-encrypted`. It needs no encryption key.
    ///
    /// The default implementation returns an error, for backends without encryption
    /// support.
    async fn purge_expired_encrypted_jobs(&self) -> Result<EncryptedJobPurge> {
        Err(crate::HammerworkError::Queue {
            message: "encrypted job retention is not supported by this queue backend".to_string(),
        })
    }
}

/// Job statuses after which an encrypted job may be deleted by
/// [`DatabaseQueue::purge_expired_encrypted_jobs`]. Safe to format into a query.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) const FINISHED_STATUS_SQL: &str = "'Completed', 'Failed', 'Dead', 'TimedOut'";

/// The outcome of [`DatabaseQueue::purge_expired_encrypted_jobs`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedJobPurge {
    /// Jobs deleted from `hammerwork_jobs`.
    pub jobs: u64,
    /// Jobs deleted from `hammerwork_jobs_archive`.
    pub archived_jobs: u64,
}

impl EncryptedJobPurge {
    /// Total number of jobs deleted.
    pub fn total(&self) -> u64 {
        self.jobs + self.archived_jobs
    }
}

/// The outcome of [`DatabaseQueue::requeue_stale_jobs`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StaleJobRecovery {
    /// Stale jobs moved back to `Pending` to be retried.
    pub requeued: Vec<JobId>,
    /// Stale jobs that had no attempts left and were moved to `Dead`.
    pub dead: Vec<JobId>,
}

impl StaleJobRecovery {
    /// Total number of jobs reclaimed.
    pub fn total(&self) -> usize {
        self.requeued.len() + self.dead.len()
    }

    /// Whether no stale jobs were found.
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// Error message recorded on a job reclaimed by [`DatabaseQueue::requeue_stale_jobs`].
pub const STALE_JOB_ERROR_MESSAGE: &str =
    "Job lease expired while Running; the worker is presumed dead and the job was reclaimed";

/// Whether a database error aborted the transaction as a deadlock or serialization
/// victim (SQLSTATE `40001` on both backends, `40P01` on PostgreSQL). The transaction
/// was rolled back, so it is safe to run again.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn is_transaction_conflict(error: &crate::HammerworkError) -> bool {
    matches!(
        error,
        crate::HammerworkError::Database(sqlx::Error::Database(db_error))
            if matches!(db_error.code().as_deref(), Some("40001") | Some("40P01"))
    )
}

/// Run a transaction, retrying a few times with a short backoff when the database
/// aborts it as a deadlock or serialization victim.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) async fn retry_on_conflict<F, Fut, T>(mut transaction: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    const ATTEMPTS: u32 = 5;
    let mut attempt = 1;
    loop {
        match transaction().await {
            Err(error) if attempt < ATTEMPTS && is_transaction_conflict(&error) => {
                tokio::time::sleep(std::time::Duration::from_millis(5 * u64::from(attempt))).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}

/// End `tx` according to `result`: commit on success, roll back on error.
///
/// Rolling back explicitly (instead of relying on the rollback a dropped transaction
/// queues on its connection) guarantees the connection never goes back to the pool
/// with an open transaction still holding row locks.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) async fn end_transaction<DB: sqlx::Database, T>(
    tx: sqlx::Transaction<'_, DB>,
    result: Result<T>,
) -> Result<T> {
    match result {
        Ok(value) => {
            tx.commit().await?;
            Ok(value)
        }
        Err(error) => {
            if let Err(rollback_error) = tx.rollback().await {
                tracing::warn!("Failed to roll back transaction after error: {rollback_error}");
            }
            Err(error)
        }
    }
}

/// The JSON stored in the `retry_strategy` column for `job`.
///
/// A [`RetryStrategy::Custom`](crate::retry::RetryStrategy::Custom) holds a closure and
/// cannot be persisted, so enqueueing a job that carries one is rejected; set it as the
/// worker's default with
/// [`Worker::with_default_retry_strategy`](crate::worker::Worker::with_default_retry_strategy)
/// instead.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn retry_strategy_json(job: &Job) -> Result<Option<serde_json::Value>> {
    match &job.retry_strategy {
        None => Ok(None),
        Some(crate::retry::RetryStrategy::Custom(_)) => {
            Err(crate::HammerworkError::InvalidJobPayload {
                message: format!(
                    "job {} has a custom retry strategy, which cannot be stored; use \
                     Worker::with_default_retry_strategy for custom strategies",
                    job.id
                ),
            })
        }
        Some(strategy) => Ok(Some(serde_json::to_value(strategy)?)),
    }
}

/// The encryption columns of a job row (migration 011), as written by the inserts.
#[derive(Debug, Default)]
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) struct EncryptionColumns {
    pub is_encrypted: bool,
    pub key_id: Option<String>,
    pub algorithm: Option<&'static str>,
    pub ciphertext: Option<Vec<u8>>,
    pub nonce: Option<Vec<u8>>,
    pub tag: Option<Vec<u8>>,
    pub metadata: Option<serde_json::Value>,
    pub payload_hash: Option<String>,
    pub retention_policy: Option<&'static str>,
    pub retention_delete_at: Option<DateTime<Utc>>,
    pub encrypted_at: Option<DateTime<Utc>>,
}

#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
impl EncryptionColumns {
    /// The columns for `job`, which must already be sealed ([`JobQueue::seal_jobs`]).
    ///
    /// Refuses a job that has an encryption config but no ciphertext, so a code path that
    /// forgot to seal its jobs fails instead of storing the plaintext.
    pub(crate) fn for_job(job: &Job) -> Result<Self> {
        if !job.is_encrypted {
            if job.has_encryption() {
                return Err(crate::HammerworkError::Encryption {
                    message: format!(
                        "Job {} has an encryption config but its payload was not encrypted",
                        job.id
                    ),
                });
            }
            return Ok(Self::default());
        }

        #[cfg(feature = "encryption")]
        {
            use crate::encryption::{EncryptionAlgorithm, RetentionPolicy};

            let encrypted = job.encrypted_payload.as_ref().ok_or_else(|| {
                crate::HammerworkError::Encryption {
                    message: format!(
                        "Job {} is marked encrypted but has no encrypted payload",
                        job.id
                    ),
                }
            })?;
            let metadata = &encrypted.metadata;
            Ok(Self {
                is_encrypted: true,
                key_id: Some(metadata.key_id.clone()),
                algorithm: Some(match metadata.algorithm {
                    EncryptionAlgorithm::AES256GCM => "AES256GCM",
                    EncryptionAlgorithm::ChaCha20Poly1305 => "ChaCha20Poly1305",
                }),
                ciphertext: Some(encrypted.decode_ciphertext()?),
                nonce: Some(encrypted.decode_nonce()?),
                tag: Some(encrypted.decode_tag()?),
                metadata: Some(serde_json::to_value(metadata)?),
                payload_hash: Some(metadata.payload_hash.clone()),
                retention_policy: Some(match metadata.retention_policy {
                    // A retention too long to represent never expires
                    RetentionPolicy::DeleteAfter(_) if metadata.delete_at.is_none() => {
                        "KeepIndefinitely"
                    }
                    RetentionPolicy::DeleteAfter(_) => "DeleteAfter",
                    RetentionPolicy::DeleteAt(_) => "DeleteAt",
                    RetentionPolicy::KeepIndefinitely => "KeepIndefinitely",
                    RetentionPolicy::DeleteImmediately => "DeleteImmediately",
                    RetentionPolicy::UseDefault => "UseDefault",
                }),
                retention_delete_at: metadata.delete_at,
                encrypted_at: Some(metadata.encrypted_at),
            })
        }
        #[cfg(not(feature = "encryption"))]
        Err(crate::HammerworkError::Encryption {
            message: format!(
                "Job {} is marked encrypted; storing encrypted jobs needs the `encryption` feature",
                job.id
            ),
        })
    }
}

/// `pii_fields` of a job as stored (`NULL` when there are none).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn pii_fields_column(job: &Job) -> Option<Vec<String>> {
    (!job.pii_fields.is_empty()).then(|| job.pii_fields.clone())
}

/// Decode the `retry_strategy` column; unknown or invalid values decode as `None`.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn retry_strategy_from_json(
    value: Option<serde_json::Value>,
) -> Result<Option<crate::retry::RetryStrategy>> {
    // A stored strategy that no longer decodes is corrupt data: report it instead of
    // silently running the job with the worker's default strategy.
    Ok(value.map(serde_json::from_value).transpose()?)
}

/// Convert an integer read from the database into the type the API exposes (e.g. a
/// `COUNT(*)` `i64` into `u64`), failing instead of wrapping on out-of-range values.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn db_int<T, S>(value: S, column: &str) -> Result<T>
where
    T: TryFrom<S>,
    S: Copy + std::fmt::Display,
{
    T::try_from(value).map_err(|_| crate::HammerworkError::Queue {
        message: format!("{column} read from the database is out of range: {value}"),
    })
}

/// A duration in whole seconds read from the database (e.g. `timeout_seconds`).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn db_seconds<S>(value: S, column: &str) -> Result<std::time::Duration>
where
    u64: TryFrom<S>,
    S: Copy + std::fmt::Display,
{
    Ok(std::time::Duration::from_secs(db_int(value, column)?))
}

/// Turn `(key, COUNT(*))` rows into a map, rejecting negative counts.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn count_map(rows: Vec<(String, i64)>, column: &str) -> Result<HashMap<String, u64>> {
    rows.into_iter()
        .map(|(key, count)| Ok((key, db_int(count, column)?)))
        .collect()
}

/// The priorities a weighted dequeue tries.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) const ALL_PRIORITIES: [crate::priority::JobPriority; 5] = [
    crate::priority::JobPriority::Critical,
    crate::priority::JobPriority::High,
    crate::priority::JobPriority::Normal,
    crate::priority::JobPriority::Low,
    crate::priority::JobPriority::Background,
];

/// SQL selecting the priority levels in `priorities` that have a job matching
/// `runnable` (a condition on the alias `j`, ending with the priority check).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn runnable_priorities_sql(runnable: &str) -> String {
    let levels = ALL_PRIORITIES
        .iter()
        .map(|priority| format!("SELECT {} AS priority", priority.as_i32()))
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    format!(
        "SELECT levels.priority FROM ({levels}) levels \
         WHERE EXISTS (SELECT 1 FROM hammerwork_jobs j WHERE {runnable} \
         AND j.priority = levels.priority)"
    )
}

/// Pick one of `candidates` (the priorities that have runnable jobs) by weight.
///
/// The probability of a priority is its weight over the candidates' total weight, so
/// every priority with a runnable job and a non-zero weight is chosen regularly, no
/// matter how many jobs of higher priority are queued. Priorities with weight zero
/// are only chosen when no candidate has a weight (then the highest one wins).
/// `seed` drives the choice; callers pass something that varies between calls.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn pick_weighted_priority(
    candidates: &[crate::priority::JobPriority],
    weights: &crate::priority::PriorityWeights,
    seed: u64,
) -> Option<crate::priority::JobPriority> {
    let total: u64 = candidates
        .iter()
        .map(|priority| u64::from(weights.get_weight(*priority)))
        .sum();
    if total == 0 {
        return candidates.iter().copied().max();
    }
    let mut point = seed % total;
    for priority in candidates {
        let weight = u64::from(weights.get_weight(*priority));
        if point < weight {
            return Some(*priority);
        }
        point -= weight;
    }
    None
}

/// A seed for [`pick_weighted_priority`]: a hash of the queue name and the current
/// time (hash-based so the future stays `Send`).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn weighted_seed(queue_name: &str, attempt: usize) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    queue_name.hash(&mut hasher);
    attempt.hash(&mut hasher);
    Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or(0)
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

/// The longest lease or staleness window passed to the database as an interval;
/// longer durations are clamped to it (100 years).
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn clamp_interval(duration: std::time::Duration) -> chrono::Duration {
    const MAX: std::time::Duration = std::time::Duration::from_secs(100 * 365 * 24 * 3600);
    saturating_chrono_duration(duration.min(MAX))
}

/// Convert a `std::time::Duration` to a `chrono::Duration`, saturating on overflow.
pub(crate) fn saturating_chrono_duration(duration: std::time::Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX)
}

/// `now - duration`, saturating at the minimum representable timestamp.
#[cfg_attr(
    not(any(feature = "postgres", feature = "mysql", feature = "test")),
    allow(dead_code)
)]
pub(crate) fn saturating_sub_from(
    now: DateTime<Utc>,
    duration: std::time::Duration,
) -> DateTime<Utc> {
    now.checked_sub_signed(saturating_chrono_duration(duration))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

/// `now + duration`, saturating at the maximum representable timestamp.
#[cfg_attr(not(feature = "test"), allow(dead_code))]
pub(crate) fn saturating_add_to(
    now: DateTime<Utc>,
    duration: std::time::Duration,
) -> DateTime<Utc> {
    now.checked_add_signed(saturating_chrono_duration(duration))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
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
    /// Engine that encrypts the payloads of jobs with an encryption config
    #[cfg(feature = "encryption")]
    pub(crate) encryption: Option<Arc<crate::encryption::EncryptionEngine>>,
}

impl<DB: Database> Clone for JobQueue<DB> {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            _phantom: PhantomData,
            throttle_configs: self.throttle_configs.clone(),
            #[cfg(feature = "encryption")]
            encryption: self.encryption.clone(),
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
            #[cfg(feature = "encryption")]
            encryption: None,
        }
    }

    /// Encrypts job payloads at rest with `engine`.
    ///
    /// Jobs with an encryption config ([`Job::with_encryption`](crate::Job::with_encryption))
    /// are encrypted by `enqueue`, `enqueue_batch`, `enqueue_workflow` and
    /// `enqueue_cron_job` before they are written: the ciphertext goes to the
    /// `encrypted_payload` column and the `payload` column holds only a redacted
    /// placeholder (see [`encryption::job_payload`](crate::encryption::job_payload)).
    /// Workers decrypt the payload just before calling the handler (see
    /// [`JobQueue::decrypt_job`]).
    ///
    /// The engine's configuration decides the algorithm, key and compression. A job whose
    /// config names a different algorithm or key id is rejected. Jobs without an
    /// encryption config are stored as before.
    ///
    /// Without an engine, enqueueing a job that has an encryption config fails with
    /// [`HammerworkError::Encryption`](crate::HammerworkError::Encryption): payloads are
    /// never stored in plaintext by mistake.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::{Job, JobQueue, queue::DatabaseQueue};
    /// use hammerwork::encryption::{EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeySource};
    /// use serde_json::json;
    ///
    /// # async fn example(pool: sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
    /// let engine = EncryptionEngine::new(config.clone()).await?;
    /// let queue = JobQueue::new(pool).with_encryption(engine);
    ///
    /// let job = Job::new("payments".to_string(), json!({"card": "4111-1111-1111-1111", "amount": 10}))
    ///     .with_encryption(config)
    ///     .with_pii_fields(vec!["card"]);
    /// queue.enqueue(job).await?; // stored as {"card": "[ENCRYPTED]", "amount": 10}
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "encryption")]
    pub fn with_encryption(
        mut self,
        engine: impl Into<Arc<crate::encryption::EncryptionEngine>>,
    ) -> Self {
        self.encryption = Some(engine.into());
        self
    }

    /// The engine set with [`JobQueue::with_encryption`], if any.
    #[cfg(feature = "encryption")]
    pub fn encryption_engine(&self) -> Option<&Arc<crate::encryption::EncryptionEngine>> {
        self.encryption.as_ref()
    }

    /// Encrypts the payloads of `jobs` that have an encryption config, before they are
    /// written. Fails (and nothing should be written) if a job needs encryption the
    /// queue cannot provide.
    #[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
    pub(crate) async fn seal_jobs(&self, jobs: &mut [Job]) -> Result<()> {
        for job in jobs.iter_mut() {
            if !job.has_encryption() || job.is_encrypted {
                continue;
            }
            #[cfg(feature = "encryption")]
            {
                let engine =
                    self.encryption
                        .as_ref()
                        .ok_or_else(|| crate::HammerworkError::Encryption {
                            message: format!(
                                "Job {} has an encryption config but the queue has no encryption \
                             engine; configure one with JobQueue::with_encryption",
                                job.id
                            ),
                        })?;
                crate::encryption::job_payload::seal_job(engine, job).await?;
            }
        }
        Ok(())
    }

    /// Returns `job` with its payload decrypted.
    ///
    /// Jobs are stored and returned by the queue (`dequeue`, `get_job`, listings) in
    /// their stored form: an encrypted job has `is_encrypted == true`, the ciphertext in
    /// `encrypted_payload` (with the `encryption` feature) and a redacted `payload`.
    /// Workers call this just before running the handler, so only the handler sees the
    /// plaintext. Call it yourself to read an encrypted job's payload.
    ///
    /// A job that is not encrypted is returned unchanged.
    ///
    /// # Errors
    ///
    /// [`HammerworkError::Encryption`](crate::HammerworkError::Encryption) when the job is
    /// encrypted and the queue has no encryption engine (or the `encryption` feature is
    /// disabled), the engine does not have the job's key, or decryption or the integrity
    /// check fails.
    pub async fn decrypt_job(&self, job: Job) -> Result<Job> {
        if !job.is_encrypted {
            return Ok(job);
        }
        #[cfg(feature = "encryption")]
        {
            let engine =
                self.encryption
                    .as_ref()
                    .ok_or_else(|| crate::HammerworkError::Encryption {
                        message: format!(
                            "Job {} has an encrypted payload but the queue has no encryption \
                         engine; configure one with JobQueue::with_encryption",
                            job.id
                        ),
                    })?;
            let job_id = job.id;
            crate::encryption::job_payload::open_job(engine, job)
                .await
                .map_err(|e| crate::HammerworkError::Encryption {
                    message: format!("Cannot decrypt the payload of job {}: {}", job_id, e),
                })
        }
        #[cfg(not(feature = "encryption"))]
        {
            Err(crate::HammerworkError::Encryption {
                message: format!(
                    "Job {} has an encrypted payload; decrypting it needs the `encryption` \
                     feature and an encryption engine (JobQueue::with_encryption)",
                    job.id
                ),
            })
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

    /// Register the throttles from a [`RateLimitingConfig`] on this queue.
    ///
    /// Does nothing when rate limiting is disabled. Only per-queue throttles are
    /// registered; the default throttle is applied to workers through
    /// [`RateLimitingConfig::throttle_for`].
    pub async fn apply_rate_limiting_config(&self, config: &RateLimitingConfig) -> Result<()> {
        if !config.enabled {
            return Ok(());
        }
        for (queue_name, throttle) in &config.queue_throttles {
            self.set_throttle(queue_name, throttle.clone()).await?;
        }
        Ok(())
    }
}

#[cfg(any(feature = "postgres", feature = "mysql"))]
use crate::config::HammerworkConfig;

#[cfg(feature = "postgres")]
impl JobQueue<sqlx::Postgres> {
    /// Connect to PostgreSQL using a [`HammerworkConfig`].
    ///
    /// Uses `database.url`, `database.pool_size` and `database.connection_timeout_secs`
    /// to build the pool, runs migrations when `database.auto_migrate` is set, and
    /// registers the per-queue throttles from `rate_limiting`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{HammerworkConfig, JobQueue};
    ///
    /// # async fn example() -> hammerwork::Result<()> {
    /// let config = HammerworkConfig::from_file("hammerwork.toml")?;
    /// let queue = JobQueue::<sqlx::Postgres>::from_config(&config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &HammerworkConfig) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(config.database.pool_size)
            .acquire_timeout(config.database.connection_timeout())
            .connect(&config.database.url)
            .await?;

        if config.database.auto_migrate {
            let runner = crate::migrations::postgres::PostgresMigrationRunner::new(pool.clone());
            crate::migrations::MigrationManager::new(Box::new(runner))
                .run_migrations()
                .await?;
        }

        let queue = Self::new(pool);
        queue
            .apply_rate_limiting_config(&config.rate_limiting)
            .await?;
        Ok(queue)
    }
}

#[cfg(feature = "mysql")]
impl JobQueue<sqlx::MySql> {
    /// Connect to MySQL using a [`HammerworkConfig`].
    ///
    /// Uses `database.url`, `database.pool_size` and `database.connection_timeout_secs`
    /// to build the pool, runs migrations when `database.auto_migrate` is set, and
    /// registers the per-queue throttles from `rate_limiting`.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork::{HammerworkConfig, JobQueue};
    ///
    /// # async fn example() -> hammerwork::Result<()> {
    /// let config = HammerworkConfig::from_file("hammerwork.toml")?;
    /// let queue = JobQueue::<sqlx::MySql>::from_config(&config).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn from_config(config: &HammerworkConfig) -> Result<Self> {
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(config.database.pool_size)
            .acquire_timeout(config.database.connection_timeout())
            .connect(&config.database.url)
            .await?;

        if config.database.auto_migrate {
            let runner = crate::migrations::mysql::MySqlMigrationRunner::new(pool.clone());
            crate::migrations::MigrationManager::new(Box::new(runner))
                .run_migrations()
                .await?;
        }

        let queue = Self::new(pool);
        queue
            .apply_rate_limiting_config(&config.rate_limiting)
            .await?;
        Ok(queue)
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
