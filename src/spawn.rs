//! Dynamic job spawning functionality for creating child jobs from parent jobs.
//!
//! This module provides the [`SpawnHandler`] trait and related types that enable jobs
//! to dynamically create other jobs during their execution. This is particularly useful
//! for fan-out processing patterns where a single job needs to spawn multiple child jobs.

use crate::{
    Result,
    error::HammerworkError,
    job::{Job, JobId},
    queue::DatabaseQueue,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Result of a spawn operation containing information about spawned jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnResult {
    /// The parent job that performed the spawn operation
    pub parent_job_id: JobId,
    /// List of spawned child jobs
    pub spawned_jobs: Vec<JobId>,
    /// When the spawn operation occurred
    pub spawned_at: DateTime<Utc>,
    /// Optional spawn operation identifier for tracking
    pub spawn_operation_id: Option<String>,
}

/// Configuration for spawn operations.
///
/// Controls how jobs are spawned, including limits, inheritance settings,
/// and operational metadata.
///
/// # Examples
///
/// ## Basic Configuration
///
/// ```rust
/// use hammerwork::spawn::SpawnConfig;
///
/// // Use default configuration
/// let config = SpawnConfig::default();
/// assert_eq!(config.max_spawn_count, Some(100));
/// assert!(config.inherit_priority);
/// assert!(config.inherit_retry_strategy);
/// assert!(!config.inherit_timeout);
/// assert!(config.inherit_trace_context);
/// ```
///
/// ## Custom Configuration for File Processing
///
/// ```rust
/// use hammerwork::spawn::SpawnConfig;
///
/// let config = SpawnConfig {
///     max_spawn_count: Some(50),        // Limit to 50 files
///     inherit_priority: true,           // Inherit parent priority
///     inherit_retry_strategy: true,     // Inherit retry settings
///     inherit_timeout: false,           // Each file has own timeout
///     inherit_trace_context: true,      // Maintain tracing
///     operation_id: Some("file_batch_001".to_string()),
/// };
/// ```
///
/// ## Configuration for Large Data Processing
///
/// ```rust
/// use hammerwork::spawn::SpawnConfig;
///
/// let config = SpawnConfig {
///     max_spawn_count: Some(1000),      // Allow many chunks
///     inherit_priority: false,          // Let chunks be normal priority
///     inherit_retry_strategy: true,     // Inherit retry logic
///     inherit_timeout: true,            // Inherit timeout settings
///     inherit_trace_context: true,      // Maintain trace correlation
///     operation_id: Some("data_processing_2024_01".to_string()),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnConfig {
    /// Maximum number of child jobs that can be spawned by a single parent
    pub max_spawn_count: Option<usize>,
    /// Whether spawned jobs should inherit the parent's priority
    pub inherit_priority: bool,
    /// Whether spawned jobs should inherit the parent's retry strategy
    pub inherit_retry_strategy: bool,
    /// Whether spawned jobs should inherit the parent's timeout
    pub inherit_timeout: bool,
    /// Whether spawned jobs should inherit the parent's trace context
    pub inherit_trace_context: bool,
    /// Custom spawn operation identifier
    pub operation_id: Option<String>,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            max_spawn_count: Some(100), // Reasonable default limit
            inherit_priority: true,
            inherit_retry_strategy: true,
            inherit_timeout: false, // Timeout is usually job-specific
            inherit_trace_context: true,
            operation_id: None,
        }
    }
}

/// Context provided to spawn handlers with information about the parent job.
pub struct SpawnContext<DB: sqlx::Database> {
    /// The parent job that is spawning child jobs
    pub parent_job: Job,
    /// Configuration for the spawn operation
    pub config: SpawnConfig,
    /// Reference to the job queue for enqueuing spawned jobs
    pub queue: Arc<dyn DatabaseQueue<Database = DB> + Send + Sync>,
}

/// Error types specific to spawn operations.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("Spawn limit exceeded: attempted to spawn {attempted} jobs, limit is {limit}")]
    SpawnLimitExceeded { attempted: usize, limit: usize },

    #[error("Invalid spawn configuration: {message}")]
    InvalidConfig { message: String },

    #[error("Parent job {parent_id} is not eligible for spawning")]
    ParentNotEligible { parent_id: JobId },

    #[error("Spawn operation failed: {message}")]
    SpawnOperationFailed { message: String },
}

/// Trait for handling dynamic job spawning logic.
///
/// Implement this trait to define how a job should spawn child jobs during execution.
/// The spawn handler receives the parent job's context and returns a list of child jobs
/// to be enqueued.
///
/// # Examples
///
/// ## Basic File Processing Spawner
///
/// ```rust,no_run
/// use async_trait::async_trait;
/// use serde_json::json;
/// use hammerwork::{Job, spawn::{SpawnHandler, SpawnContext}};
///
/// struct FileProcessingSpawner;
///
/// #[async_trait]
/// impl<DB: sqlx::Database + Send + Sync> SpawnHandler<DB> for FileProcessingSpawner {
///     async fn spawn_jobs(&self, context: SpawnContext<DB>) -> hammerwork::Result<Vec<Job>> {
///         let files = context.parent_job.payload["files"].as_array()
///             .ok_or_else(|| hammerwork::error::HammerworkError::InvalidJobPayload {
///                 message: "Missing files array in payload".to_string()
///             })?;
///
///         let mut child_jobs = Vec::new();
///         for file in files {
///             let child_job = Job::new(
///                 "process_file".to_string(),
///                 json!({
///                     "file_path": file,
///                     "parent_job_id": context.parent_job.id
///                 })
///             );
///             child_jobs.push(child_job);
///         }
///
///         Ok(child_jobs)
///     }
/// }
/// ```
///
/// ## Data Chunking Spawner with Validation
///
/// This example shows how to split large datasets into smaller processing chunks:
///
/// ```rust,no_run
/// use async_trait::async_trait;
/// use serde_json::json;
/// use hammerwork::{Job, spawn::{SpawnHandler, SpawnContext, SpawnConfig}};
///
/// struct DataChunkingSpawner;
///
/// #[async_trait]
/// impl<DB: sqlx::Database + Send + Sync> SpawnHandler<DB> for DataChunkingSpawner {
///     async fn spawn_jobs(&self, context: SpawnContext<DB>) -> hammerwork::Result<Vec<Job>> {
///         // Break large data processing into manageable chunks
///         let total_records = context.parent_job.payload["total_records"].as_u64().unwrap_or(0) as usize;
///         let chunk_size = context.parent_job.payload["chunk_size"].as_u64().unwrap_or(1000) as usize;
///
///         let mut child_jobs = Vec::new();
///         let mut offset = 0;
///
///         while offset < total_records {
///             let limit = std::cmp::min(chunk_size, total_records - offset);
///             
///             let child_job = Job::new(
///                 "process_chunk".to_string(),
///                 json!({
///                     "offset": offset,
///                     "limit": limit,
///                     "parent_job_id": context.parent_job.id
///                 })
///             );
///             child_jobs.push(child_job);
///             offset += chunk_size;
///         }
///         
///         Ok(child_jobs)
///     }
/// }
/// ```
#[async_trait]
pub trait SpawnHandler<DB: sqlx::Database>: Send + Sync {
    /// Generate child jobs based on the parent job's context.
    ///
    /// This method is called when a job with spawn capabilities completes successfully.
    /// It should analyze the parent job's payload and create appropriate child jobs.
    ///
    /// # Arguments
    ///
    /// * `context` - Context containing the parent job and spawn configuration
    ///
    /// # Returns
    ///
    /// A vector of child jobs to be enqueued, or an error if spawning fails.
    async fn spawn_jobs(&self, context: SpawnContext<DB>) -> Result<Vec<Job>>;

    /// Optional validation method called before spawning jobs.
    ///
    /// This method can be used to validate the parent job's payload or perform
    /// any pre-spawn checks. The default implementation always returns `Ok(())`.
    ///
    /// # Arguments
    ///
    /// * `parent_job` - The parent job that wants to spawn children
    /// * `config` - The spawn configuration
    ///
    /// # Returns
    ///
    /// `Ok(())` if validation passes, or an error if spawning should be prevented.
    async fn validate_spawn(&self, _parent_job: &Job, _config: &SpawnConfig) -> Result<()> {
        Ok(())
    }

    /// Optional post-spawn callback called after child jobs are enqueued.
    ///
    /// This method can be used to perform cleanup or additional processing
    /// after the spawn operation completes. The default implementation does nothing.
    ///
    /// # Arguments
    ///
    /// * `result` - Information about the completed spawn operation
    async fn on_spawn_complete(&self, _result: &SpawnResult) -> Result<()> {
        Ok(())
    }
}

/// A spawn handler that creates jobs based on a simple closure.
pub struct ClosureSpawnHandler<F, DB: sqlx::Database> {
    handler: F,
    _phantom: std::marker::PhantomData<DB>,
}

impl<F, DB> ClosureSpawnHandler<F, DB>
where
    F: Fn(SpawnContext<DB>) -> Result<Vec<Job>> + Send + Sync,
    DB: sqlx::Database + Send + Sync,
{
    /// Create a new closure-based spawn handler.
    pub fn new(handler: F) -> Self {
        Self {
            handler,
            _phantom: std::marker::PhantomData,
        }
    }
}

#[async_trait]
impl<F, DB> SpawnHandler<DB> for ClosureSpawnHandler<F, DB>
where
    F: Fn(SpawnContext<DB>) -> Result<Vec<Job>> + Send + Sync,
    DB: sqlx::Database + Send + Sync,
{
    async fn spawn_jobs(&self, context: SpawnContext<DB>) -> Result<Vec<Job>> {
        (self.handler)(context)
    }
}

/// Manager for registering and executing spawn handlers.
///
/// The SpawnManager coordinates job spawning by mapping job types to their corresponding
/// spawn handlers. It handles the registration of handlers, validation, and execution
/// of spawn operations.
///
/// # Examples
///
/// ## Basic Usage
///
/// ```rust,no_run
/// use hammerwork::spawn::SpawnManager;
/// use async_trait::async_trait;
/// use serde_json::json;
/// use hammerwork::{Job, spawn::{SpawnHandler, SpawnContext}};
/// use std::sync::Arc;
///
/// struct SimpleSpawner;
///
/// #[async_trait]
/// impl<DB: sqlx::Database + Send + Sync> SpawnHandler<DB> for SimpleSpawner {
///     async fn spawn_jobs(&self, context: SpawnContext<DB>) -> hammerwork::Result<Vec<Job>> {
///         let count = context.parent_job.payload["count"].as_u64().unwrap_or(1) as usize;
///         let mut jobs = Vec::new();
///         for i in 0..count {
///             jobs.push(Job::new("child_task".to_string(), json!({"index": i, "parent": context.parent_job.id})));
///         }
///         Ok(jobs)
///     }
/// }
///
/// // Create and configure spawn manager
/// # #[cfg(feature = "postgres")]
/// # {
/// let mut spawn_manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
/// spawn_manager.register_handler("parent_task", SimpleSpawner);
///
/// // Check if handler is registered
/// assert!(spawn_manager.has_handler("parent_task"));
/// assert!(!spawn_manager.has_handler("other_task"));
///
/// // Get registered types
/// let types = spawn_manager.registered_types();
/// assert!(types.contains(&"parent_task".to_string()));
/// # }
/// ```
///
/// ## Integration with Worker
///
/// ```rust,no_run
/// use hammerwork::{spawn::SpawnManager, Worker};
/// use async_trait::async_trait;
/// use hammerwork::spawn::{SpawnHandler, SpawnContext};
/// use std::sync::Arc;
///
/// struct TaskSpawner;
///
/// #[async_trait]
/// impl<DB: sqlx::Database + Send + Sync> SpawnHandler<DB> for TaskSpawner {
///     async fn spawn_jobs(&self, context: SpawnContext<DB>) -> hammerwork::Result<Vec<hammerwork::Job>> {
///         // Implementation here
///         Ok(vec![])
///     }
/// }
///
/// // Set up spawn manager with handlers
/// # #[cfg(feature = "postgres")]
/// # {
/// let mut spawn_manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
/// spawn_manager.register_handler("spawning_task", TaskSpawner);
/// # }
///
/// // In real usage, integrate with Worker:
/// // let worker = Worker::new(queue, "spawning_task".to_string(), handler)
/// //     .with_spawn_manager(Arc::new(spawn_manager));
/// ```
pub struct SpawnManager<DB: sqlx::Database> {
    handlers: std::collections::HashMap<String, Arc<dyn SpawnHandler<DB>>>,
    _phantom: std::marker::PhantomData<DB>,
}

impl<DB: sqlx::Database> SpawnManager<DB> {
    /// Create a new spawn manager.
    pub fn new() -> Self {
        Self {
            handlers: std::collections::HashMap::new(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Register a spawn handler for a specific job type.
    ///
    /// # Arguments
    ///
    /// * `job_type` - The job type identifier (typically the queue name)
    /// * `handler` - The spawn handler implementation
    pub fn register_handler<H>(&mut self, job_type: impl Into<String>, handler: H)
    where
        H: SpawnHandler<DB> + 'static,
    {
        self.handlers.insert(job_type.into(), Arc::new(handler));
    }

    /// Execute spawn logic for a completed job.
    ///
    /// This method checks if the job has a registered spawn handler and executes it
    /// if found. The spawned jobs are automatically enqueued and tracked.
    ///
    /// # Arguments
    ///
    /// * `job` - The parent job that completed successfully
    /// * `config` - Spawn configuration
    /// * `queue` - Reference to the job queue for enqueuing child jobs
    ///
    /// # Returns
    ///
    /// `Some(SpawnResult)` if jobs were spawned, `None` if no spawn handler was found.
    pub async fn execute_spawn(
        &self,
        job: Job,
        config: SpawnConfig,
        queue: Arc<dyn DatabaseQueue<Database = DB> + Send + Sync>,
    ) -> Result<Option<SpawnResult>> {
        if let Some(handler) = self.handlers.get(&job.queue_name) {
            // Validate spawn operation
            handler.validate_spawn(&job, &config).await?;

            // Create spawn context
            let context = SpawnContext {
                parent_job: job.clone(),
                config: config.clone(),
                queue: queue.clone(),
            };

            // Generate child jobs
            let mut child_jobs = handler.spawn_jobs(context).await?;

            // Check spawn limits
            if let Some(max_count) = config.max_spawn_count {
                if child_jobs.len() > max_count {
                    return Err(HammerworkError::SpawnError(
                        SpawnError::SpawnLimitExceeded {
                            attempted: child_jobs.len(),
                            limit: max_count,
                        },
                    ));
                }
            }

            // Apply inheritance settings
            for child_job in &mut child_jobs {
                if config.inherit_priority {
                    child_job.priority = job.priority;
                }
                if config.inherit_retry_strategy {
                    child_job.retry_strategy = job.retry_strategy.clone();
                }
                if config.inherit_timeout {
                    child_job.timeout = job.timeout;
                }
                if config.inherit_trace_context {
                    child_job.trace_id = job.trace_id.clone();
                    child_job.correlation_id = job.correlation_id.clone();
                    child_job.parent_span_id = job.parent_span_id.clone();
                    child_job.span_context = job.span_context.clone();
                }

                // Set up parent-child relationship
                child_job.depends_on = vec![job.id];
                child_job.workflow_id = job.workflow_id;
                child_job.workflow_name = job.workflow_name.clone();
            }

            // Enqueue child jobs
            let mut spawned_job_ids = Vec::new();
            for child_job in child_jobs {
                let job_id = queue.enqueue(child_job).await?;
                spawned_job_ids.push(job_id);
            }

            // Create spawn result
            let spawn_result = SpawnResult {
                parent_job_id: job.id,
                spawned_jobs: spawned_job_ids,
                spawned_at: Utc::now(),
                spawn_operation_id: config.operation_id.clone(),
            };

            // Call post-spawn callback
            handler.on_spawn_complete(&spawn_result).await?;

            Ok(Some(spawn_result))
        } else {
            Ok(None)
        }
    }

    /// Check if a job type has a registered spawn handler.
    pub fn has_handler(&self, job_type: &str) -> bool {
        self.handlers.contains_key(job_type)
    }

    /// Get the list of registered job types with spawn handlers.
    pub fn registered_types(&self) -> Vec<String> {
        self.handlers.keys().cloned().collect()
    }
}

impl<DB: sqlx::Database> Default for SpawnManager<DB> {
    fn default() -> Self {
        Self::new()
    }
}

/// Extension trait for adding spawn capabilities to jobs.
///
/// This trait provides convenience methods for configuring jobs to spawn
/// child jobs when they complete successfully.
///
/// # Examples
///
/// ## Basic Job Spawning
///
/// ```rust
/// use hammerwork::{Job, spawn::{JobSpawnExt, SpawnConfig}};
/// use serde_json::json;
///
/// // Create a job that will spawn children with default config
/// let job = Job::new("file_batch".to_string(), json!({"files": ["a.txt", "b.txt"]}))
///     .with_spawning();
///
/// // Check that spawn config was added to payload
/// assert!(job.payload.get("_spawn_config").is_some());
/// ```
///
/// ## Custom Spawn Configuration
///
/// ```rust
/// use hammerwork::{Job, spawn::{JobSpawnExt, SpawnConfig}};
/// use serde_json::json;
///
/// let spawn_config = SpawnConfig {
///     max_spawn_count: Some(10),
///     inherit_priority: true,
///     inherit_retry_strategy: false,
///     inherit_timeout: true,
///     inherit_trace_context: true,
///     operation_id: Some("batch_001".to_string()),
/// };
///
/// let job = Job::new("data_processing".to_string(), json!({"records": 5000}))
///     .as_high_priority()
///     .with_spawn_config(spawn_config);
///
/// // Verify the configuration is stored
/// let stored_config = job.payload.get("_spawn_config").unwrap();
/// let parsed_config: SpawnConfig = serde_json::from_value(stored_config.clone()).unwrap();
/// assert_eq!(parsed_config.max_spawn_count, Some(10));
/// ```
///
/// ## Fan-out Processing Pattern
///
/// ```rust
/// use hammerwork::{Job, spawn::JobSpawnExt};
/// use serde_json::json;
///
/// // Parent job that will spawn one child job per user
/// let user_ids = vec![1, 2, 3, 4, 5];
/// let notification_job = Job::new(
///     "send_notifications".to_string(),
///     json!({"user_ids": user_ids})
/// )
/// .as_high_priority()
/// .with_spawning(); // Uses default spawn configuration
///
/// // When this job completes, the spawn handler will create
/// // individual notification jobs for each user
/// ```
pub trait JobSpawnExt {
    /// Enable spawning for this job with the given configuration.
    fn with_spawn_config(self, config: SpawnConfig) -> Self;

    /// Enable spawning for this job with default configuration.
    fn with_spawning(self) -> Self;
}

impl JobSpawnExt for Job {
    fn with_spawn_config(mut self, config: SpawnConfig) -> Self {
        // Store spawn config in job metadata (we'll need to add this field to Job)
        // For now, we'll use a convention in the payload
        if let Some(payload_obj) = self.payload.as_object_mut() {
            payload_obj.insert(
                "_spawn_config".to_string(),
                serde_json::to_value(config).unwrap(),
            );
        }
        self
    }

    fn with_spawning(self) -> Self {
        self.with_spawn_config(SpawnConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestSpawnHandler;

    #[async_trait]
    impl<DB: sqlx::Database> SpawnHandler<DB> for TestSpawnHandler {
        async fn spawn_jobs(&self, context: SpawnContext<DB>) -> Result<Vec<Job>> {
            let count = context.parent_job.payload["spawn_count"]
                .as_u64()
                .unwrap_or(1) as usize;
            let mut jobs = Vec::new();

            for i in 0..count {
                let job = Job::new(
                    "child_task".to_string(),
                    json!({
                        "index": i,
                        "parent_id": context.parent_job.id
                    }),
                );
                jobs.push(job);
            }

            Ok(jobs)
        }
    }

    #[tokio::test]
    async fn test_spawn_handler_basic() {
        let handler = TestSpawnHandler;
        let parent_job = Job::new("parent_task".to_string(), json!({"spawn_count": 3}));

        // Note: This test would need a mock queue implementation
        // We'll implement proper tests when we add the queue integration
    }

    #[test]
    fn test_spawn_config_defaults() {
        let config = SpawnConfig::default();
        assert_eq!(config.max_spawn_count, Some(100));
        assert!(config.inherit_priority);
        assert!(config.inherit_retry_strategy);
        assert!(!config.inherit_timeout);
        assert!(config.inherit_trace_context);
    }

    #[test]
    fn test_spawn_manager_registration() {
        let mut manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
        assert!(!manager.has_handler("test_job"));

        manager.register_handler("test_job", TestSpawnHandler);
        assert!(manager.has_handler("test_job"));

        let types = manager.registered_types();
        assert!(types.contains(&"test_job".to_string()));
    }
}
