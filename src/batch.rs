//! Job batching and bulk operations for high-throughput scenarios.
//!
//! This module provides the [`JobBatch`] struct and related types for processing
//! multiple jobs as a single unit, enabling significant performance improvements
//! for high-volume job processing.

use crate::{Result, job::Job};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[cfg(any(feature = "postgres", feature = "mysql"))]
use sqlx::{Decode, Encode, Type};

#[cfg(feature = "postgres")]
use sqlx::Postgres;

#[cfg(feature = "mysql")]
use sqlx::MySql;

/// Unique identifier for a job batch.
pub type BatchId = Uuid;

/// Strategies for handling partial failures within a batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PartialFailureMode {
    /// Continue processing remaining jobs even if some fail.
    /// Failed jobs are tracked but don't stop batch processing.
    ContinueOnError,
    /// Stop processing immediately when the first job fails.
    /// Remaining jobs in the batch are not processed.
    FailFast,
    /// Process all jobs and collect all errors.
    /// Similar to ContinueOnError but provides detailed error reporting.
    CollectErrors,
}

impl Default for PartialFailureMode {
    fn default() -> Self {
        Self::ContinueOnError
    }
}

/// Current status of a job batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum BatchStatus {
    /// Batch is waiting to be processed.
    Pending,
    /// Batch is currently being processed.
    Processing,
    /// All jobs in the batch completed successfully.
    Completed,
    /// Some jobs failed but batch processing continued.
    PartiallyFailed,
    /// Batch processing failed (either FailFast mode or all jobs failed).
    Failed,
}

// SQLx implementations for BatchStatus
#[cfg(feature = "postgres")]
impl Type<Postgres> for BatchStatus {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }
}

#[cfg(feature = "postgres")]
impl Encode<'_, Postgres> for BatchStatus {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> std::result::Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>>
    {
        let status_str = match self {
            BatchStatus::Pending => "Pending",
            BatchStatus::Processing => "Processing",
            BatchStatus::Completed => "Completed",
            BatchStatus::PartiallyFailed => "PartiallyFailed",
            BatchStatus::Failed => "Failed",
        };
        <&str as Encode<'_, Postgres>>::encode_by_ref(&status_str, buf)
    }
}

#[cfg(feature = "postgres")]
impl Decode<'_, Postgres> for BatchStatus {
    fn decode(
        value: sqlx::postgres::PgValueRef<'_>,
    ) -> std::result::Result<Self, sqlx::error::BoxDynError> {
        let status_str = <String as Decode<Postgres>>::decode(value)?;
        // Handle both quoted (old format) and unquoted (new format) status values
        let cleaned_str = status_str.trim_matches('"');
        match cleaned_str {
            "Pending" => Ok(BatchStatus::Pending),
            "Processing" => Ok(BatchStatus::Processing),
            "Completed" => Ok(BatchStatus::Completed),
            "PartiallyFailed" => Ok(BatchStatus::PartiallyFailed),
            "Failed" => Ok(BatchStatus::Failed),
            _ => Err(format!("Unknown batch status: {}", status_str).into()),
        }
    }
}

#[cfg(feature = "mysql")]
impl Type<MySql> for BatchStatus {
    fn type_info() -> sqlx::mysql::MySqlTypeInfo {
        <String as Type<MySql>>::type_info()
    }
}

#[cfg(feature = "mysql")]
impl Encode<'_, MySql> for BatchStatus {
    fn encode_by_ref(
        &self,
        buf: &mut Vec<u8>,
    ) -> std::result::Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync + 'static>>
    {
        let status_str = match self {
            BatchStatus::Pending => "Pending",
            BatchStatus::Processing => "Processing",
            BatchStatus::Completed => "Completed",
            BatchStatus::PartiallyFailed => "PartiallyFailed",
            BatchStatus::Failed => "Failed",
        };
        <&str as Encode<'_, MySql>>::encode_by_ref(&status_str, buf)
    }
}

#[cfg(feature = "mysql")]
impl Decode<'_, MySql> for BatchStatus {
    fn decode(
        value: sqlx::mysql::MySqlValueRef<'_>,
    ) -> std::result::Result<Self, sqlx::error::BoxDynError> {
        let status_str = <String as Decode<MySql>>::decode(value)?;
        // Handle both quoted (old format) and unquoted (new format) status values
        let cleaned_str = status_str.trim_matches('"');
        match cleaned_str {
            "Pending" => Ok(BatchStatus::Pending),
            "Processing" => Ok(BatchStatus::Processing),
            "Completed" => Ok(BatchStatus::Completed),
            "PartiallyFailed" => Ok(BatchStatus::PartiallyFailed),
            "Failed" => Ok(BatchStatus::Failed),
            _ => Err(format!("Unknown batch status: {}", status_str).into()),
        }
    }
}

/// Summary of batch processing results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResult {
    /// Unique identifier for the batch.
    pub batch_id: BatchId,
    /// Total number of jobs in the batch.
    pub total_jobs: u32,
    /// Number of successfully completed jobs.
    pub completed_jobs: u32,
    /// Number of failed jobs.
    pub failed_jobs: u32,
    /// Number of jobs still pending processing.
    pub pending_jobs: u32,
    /// Current status of the batch.
    pub status: BatchStatus,
    /// Time when the batch was created.
    pub created_at: DateTime<Utc>,
    /// Time when the batch processing completed (if finished).
    pub completed_at: Option<DateTime<Utc>>,
    /// Summary of errors encountered during batch processing.
    pub error_summary: Option<String>,
    /// Detailed error information by job ID (for CollectErrors mode).
    pub job_errors: HashMap<Uuid, String>,
}

/// Configuration and container for a batch of jobs to be processed together.
#[derive(Debug, Clone)]
pub struct JobBatch {
    /// Unique identifier for this batch.
    pub id: BatchId,
    /// Human-readable name for the batch.
    pub name: String,
    /// Jobs to be processed in this batch.
    pub jobs: Vec<Job>,
    /// Maximum number of jobs to process in a single database operation.
    pub batch_size: Option<u32>,
    /// Strategy for handling partial failures.
    pub failure_mode: PartialFailureMode,
    /// Time when the batch was created.
    pub created_at: DateTime<Utc>,
    /// Optional metadata for the batch.
    pub metadata: HashMap<String, String>,
}

impl JobBatch {
    /// Creates a new job batch with the specified name.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::batch::JobBatch;
    ///
    /// let batch = JobBatch::new("email_notifications");
    /// assert_eq!(batch.name, "email_notifications");
    /// assert!(batch.jobs.is_empty());
    /// ```
    pub fn new<S: Into<String>>(name: S) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            jobs: Vec::new(),
            batch_size: None,
            failure_mode: PartialFailureMode::default(),
            created_at: Utc::now(),
            metadata: HashMap::new(),
        }
    }

    /// Adds jobs to the batch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, batch::JobBatch};
    /// use serde_json::json;
    ///
    /// let job1 = Job::new("email".to_string(), json!({"to": "user1@example.com"}));
    /// let job2 = Job::new("email".to_string(), json!({"to": "user2@example.com"}));
    ///
    /// let batch = JobBatch::new("email_batch")
    ///     .with_jobs(vec![job1, job2]);
    ///
    /// assert_eq!(batch.jobs.len(), 2);
    /// ```
    pub fn with_jobs(mut self, jobs: Vec<Job>) -> Self {
        self.jobs = jobs;
        self
    }

    /// Adds a single job to the batch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::{Job, batch::JobBatch};
    /// use serde_json::json;
    ///
    /// let job = Job::new("email".to_string(), json!({"to": "user@example.com"}));
    /// let batch = JobBatch::new("email_batch")
    ///     .add_job(job);
    ///
    /// assert_eq!(batch.jobs.len(), 1);
    /// ```
    pub fn add_job(mut self, job: Job) -> Self {
        self.jobs.push(job);
        self
    }

    /// Sets the maximum batch size for database operations.
    ///
    /// When enqueueing large batches, they will be split into chunks
    /// of this size for optimal database performance.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::batch::JobBatch;
    ///
    /// let batch = JobBatch::new("large_batch")
    ///     .with_batch_size(100);
    ///
    /// assert_eq!(batch.batch_size, Some(100));
    /// ```
    pub fn with_batch_size(mut self, size: u32) -> Self {
        self.batch_size = Some(size);
        self
    }

    /// Sets the partial failure handling mode.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::batch::{JobBatch, PartialFailureMode};
    ///
    /// let batch = JobBatch::new("critical_batch")
    ///     .with_partial_failure_handling(PartialFailureMode::FailFast);
    ///
    /// assert_eq!(batch.failure_mode, PartialFailureMode::FailFast);
    /// ```
    pub fn with_partial_failure_handling(mut self, mode: PartialFailureMode) -> Self {
        self.failure_mode = mode;
        self
    }

    /// Adds metadata to the batch.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::batch::JobBatch;
    ///
    /// let batch = JobBatch::new("user_notifications")
    ///     .with_metadata("user_id", "12345")
    ///     .with_metadata("campaign_id", "summer_2024");
    ///
    /// assert_eq!(batch.metadata.get("user_id"), Some(&"12345".to_string()));
    /// ```
    pub fn with_metadata<K, V>(mut self, key: K, value: V) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Returns the number of jobs in the batch.
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Returns true if the batch is empty.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// Validates the batch configuration.
    ///
    /// Returns an error if the batch is invalid (e.g., empty, too large, etc.).
    pub fn validate(&self) -> Result<()> {
        if self.jobs.is_empty() {
            return Err(crate::HammerworkError::Queue {
                message: "Batch cannot be empty".to_string(),
            });
        }

        // Check for reasonable batch size limits
        if self.jobs.len() > 10_000 {
            return Err(crate::HammerworkError::Queue {
                message: format!(
                    "Batch size {} exceeds maximum allowed size of 10,000 jobs",
                    self.jobs.len()
                ),
            });
        }

        // Validate that all jobs have the same queue name
        if let Some(first_queue) = self.jobs.first().map(|j| &j.queue_name) {
            for (i, job) in self.jobs.iter().enumerate() {
                if &job.queue_name != first_queue {
                    return Err(crate::HammerworkError::Queue {
                        message: format!(
                            "All jobs in a batch must have the same queue name. \
                             Job at index {} has queue '{}' but expected '{}'",
                            i, job.queue_name, first_queue
                        ),
                    });
                }
            }
        }

        Ok(())
    }

    /// Splits the batch into smaller chunks based on the configured batch size.
    ///
    /// This is useful for processing very large batches in manageable chunks.
    pub fn into_chunks(self) -> Vec<JobBatch> {
        let chunk_size = self.batch_size.unwrap_or(1000) as usize;

        if self.jobs.len() <= chunk_size {
            return vec![self];
        }

        self.jobs
            .chunks(chunk_size)
            .enumerate()
            .map(|(i, chunk)| JobBatch {
                id: Uuid::new_v4(),
                name: format!("{}_chunk_{}", self.name, i + 1),
                jobs: chunk.to_vec(),
                batch_size: self.batch_size,
                failure_mode: self.failure_mode.clone(),
                created_at: self.created_at,
                metadata: self.metadata.clone(),
            })
            .collect()
    }
}

impl BatchResult {
    /// Returns true if the batch completed successfully.
    pub fn is_successful(&self) -> bool {
        matches!(self.status, BatchStatus::Completed)
    }

    /// Returns true if the batch failed completely.
    pub fn is_failed(&self) -> bool {
        matches!(self.status, BatchStatus::Failed)
    }

    /// Returns true if the batch completed with some failures.
    pub fn is_partially_failed(&self) -> bool {
        matches!(self.status, BatchStatus::PartiallyFailed)
    }

    /// Returns the success rate as a percentage (0.0 to 100.0).
    pub fn success_rate(&self) -> f64 {
        if self.total_jobs == 0 {
            return 100.0;
        }
        (self.completed_jobs as f64 / self.total_jobs as f64) * 100.0
    }

    /// Returns the failure rate as a percentage (0.0 to 100.0).
    pub fn failure_rate(&self) -> f64 {
        if self.total_jobs == 0 {
            return 0.0;
        }
        (self.failed_jobs as f64 / self.total_jobs as f64) * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Job;
    use serde_json::json;

    #[test]
    fn test_job_batch_creation() {
        let batch = JobBatch::new("test_batch");
        assert_eq!(batch.name, "test_batch");
        assert!(batch.jobs.is_empty());
        assert_eq!(batch.failure_mode, PartialFailureMode::ContinueOnError);
        assert!(batch.batch_size.is_none());
    }

    #[test]
    fn test_job_batch_with_jobs() {
        let job1 = Job::new("queue1".to_string(), json!({"id": 1}));
        let job2 = Job::new("queue1".to_string(), json!({"id": 2}));

        let batch = JobBatch::new("test_batch").with_jobs(vec![job1, job2]);

        assert_eq!(batch.job_count(), 2);
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_job_batch_add_job() {
        let job = Job::new("queue1".to_string(), json!({"id": 1}));

        let batch = JobBatch::new("test_batch").add_job(job);

        assert_eq!(batch.job_count(), 1);
    }

    #[test]
    fn test_job_batch_configuration() {
        let batch = JobBatch::new("configured_batch")
            .with_batch_size(50)
            .with_partial_failure_handling(PartialFailureMode::FailFast)
            .with_metadata("user_id", "123")
            .with_metadata("campaign", "test");

        assert_eq!(batch.batch_size, Some(50));
        assert_eq!(batch.failure_mode, PartialFailureMode::FailFast);
        assert_eq!(batch.metadata.get("user_id"), Some(&"123".to_string()));
        assert_eq!(batch.metadata.get("campaign"), Some(&"test".to_string()));
    }

    #[test]
    fn test_batch_validation_empty() {
        let batch = JobBatch::new("empty_batch");
        let result = batch.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Batch cannot be empty")
        );
    }

    #[test]
    fn test_batch_validation_too_large() {
        let mut jobs = Vec::new();
        for i in 0..10_001 {
            jobs.push(Job::new("queue1".to_string(), json!({"id": i})));
        }

        let batch = JobBatch::new("large_batch").with_jobs(jobs);
        let result = batch.validate();
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum allowed size")
        );
    }

    #[test]
    fn test_batch_validation_mixed_queues() {
        let job1 = Job::new("queue1".to_string(), json!({"id": 1}));
        let job2 = Job::new("queue2".to_string(), json!({"id": 2}));

        let batch = JobBatch::new("mixed_batch").with_jobs(vec![job1, job2]);

        let result = batch.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("same queue name"));
    }

    #[test]
    fn test_batch_validation_success() {
        let job1 = Job::new("queue1".to_string(), json!({"id": 1}));
        let job2 = Job::new("queue1".to_string(), json!({"id": 2}));

        let batch = JobBatch::new("valid_batch").with_jobs(vec![job1, job2]);

        assert!(batch.validate().is_ok());
    }

    #[test]
    fn test_batch_into_chunks() {
        let mut jobs = Vec::new();
        for i in 0..250 {
            jobs.push(Job::new("queue1".to_string(), json!({"id": i})));
        }

        let batch = JobBatch::new("large_batch")
            .with_jobs(jobs)
            .with_batch_size(100);

        let chunks = batch.into_chunks();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].job_count(), 100);
        assert_eq!(chunks[1].job_count(), 100);
        assert_eq!(chunks[2].job_count(), 50);
    }

    #[test]
    fn test_batch_into_chunks_small_batch() {
        let job = Job::new("queue1".to_string(), json!({"id": 1}));
        let batch = JobBatch::new("small_batch")
            .with_jobs(vec![job])
            .with_batch_size(100);

        let chunks = batch.into_chunks();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].job_count(), 1);
    }

    #[test]
    fn test_batch_result_success_rate() {
        let result = BatchResult {
            batch_id: Uuid::new_v4(),
            total_jobs: 100,
            completed_jobs: 80,
            failed_jobs: 20,
            pending_jobs: 0,
            status: BatchStatus::PartiallyFailed,
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
            error_summary: None,
            job_errors: HashMap::new(),
        };

        assert_eq!(result.success_rate(), 80.0);
        assert_eq!(result.failure_rate(), 20.0);
        assert!(result.is_partially_failed());
        assert!(!result.is_successful());
        assert!(!result.is_failed());
    }

    #[test]
    fn test_batch_result_zero_jobs() {
        let result = BatchResult {
            batch_id: Uuid::new_v4(),
            total_jobs: 0,
            completed_jobs: 0,
            failed_jobs: 0,
            pending_jobs: 0,
            status: BatchStatus::Completed,
            created_at: Utc::now(),
            completed_at: Some(Utc::now()),
            error_summary: None,
            job_errors: HashMap::new(),
        };

        assert_eq!(result.success_rate(), 100.0);
        assert_eq!(result.failure_rate(), 0.0);
    }

    #[test]
    fn test_partial_failure_mode_default() {
        assert_eq!(
            PartialFailureMode::default(),
            PartialFailureMode::ContinueOnError
        );
    }

    #[test]
    fn test_batch_status_variants() {
        let statuses = [
            BatchStatus::Pending,
            BatchStatus::Processing,
            BatchStatus::Completed,
            BatchStatus::PartiallyFailed,
            BatchStatus::Failed,
        ];

        // Ensure all variants compile and are distinct
        for (i, status1) in statuses.iter().enumerate() {
            for (j, status2) in statuses.iter().enumerate() {
                if i != j {
                    assert_ne!(status1, status2);
                }
            }
        }
    }
}
