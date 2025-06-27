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
use sqlx::{Database, Pool};
use std::{collections::HashMap, marker::PhantomData, sync::Arc};
use tokio::sync::RwLock;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(feature = "mysql")]
pub mod mysql;

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
    /// let result_data = json!({"status": "success", "count": 42});
    /// let expires_at = Some(Utc::now() + Duration::hours(24));
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
/// use hammerwork::{JobQueue, Job};
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
    pub(crate) pool: Pool<DB>,
    pub(crate) _phantom: PhantomData<DB>,
    pub(crate) throttle_configs: Arc<RwLock<HashMap<String, ThrottleConfig>>>,
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
}