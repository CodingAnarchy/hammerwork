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
    /// if found. The spawned jobs are enqueued one at a time, outside any transaction;
    /// [`Worker`](crate::Worker)s instead use [`prepare_spawn`](Self::prepare_spawn) and
    /// enqueue the children together with the parent's completion
    /// ([`DatabaseQueue::complete_job_run_with_children`]).
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
        let Some(child_jobs) = self.prepare_spawn(&job, &config, queue.clone()).await? else {
            return Ok(None);
        };

        // Enqueue child jobs
        let mut spawned_job_ids = Vec::new();
        for child_job in child_jobs {
            let job_id = queue.enqueue(child_job).await?;
            spawned_job_ids.push(job_id);
        }

        let spawn_result = SpawnResult {
            parent_job_id: job.id,
            spawned_jobs: spawned_job_ids,
            spawned_at: Utc::now(),
            spawn_operation_id: config.operation_id.clone(),
        };
        self.notify_spawn_complete(&job, &spawn_result).await?;
        Ok(Some(spawn_result))
    }

    /// Build the child jobs `job` spawns, without enqueueing them.
    ///
    /// Runs the job type's handler (`validate_spawn`, then `spawn_jobs`), enforces
    /// `config.max_spawn_count` and applies the inheritance settings. Every child
    /// depends on the parent and joins its workflow. Returns `None` when no handler is
    /// registered for the job's queue.
    ///
    /// Workers call this before recording the parent's completion and then enqueue the
    /// children in the same transaction as the completion
    /// ([`DatabaseQueue::complete_job_run_with_children`]); an error here fails the
    /// parent's run, so it is retried instead of completing without its children.
    pub async fn prepare_spawn(
        &self,
        job: &Job,
        config: &SpawnConfig,
        queue: Arc<dyn DatabaseQueue<Database = DB> + Send + Sync>,
    ) -> Result<Option<Vec<Job>>> {
        let Some(handler) = self.handlers.get(&job.queue_name) else {
            return Ok(None);
        };
        // Validate spawn operation
        handler.validate_spawn(job, config).await?;

        // Generate child jobs
        let context = SpawnContext {
            parent_job: job.clone(),
            config: config.clone(),
            queue,
        };
        let mut child_jobs = handler.spawn_jobs(context).await?;

        // Check spawn limits
        if let Some(max_count) = config.max_spawn_count
            && child_jobs.len() > max_count
        {
            return Err(HammerworkError::SpawnError(
                SpawnError::SpawnLimitExceeded {
                    attempted: child_jobs.len(),
                    limit: max_count,
                },
            ));
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

        Ok(Some(child_jobs))
    }

    /// Call the post-spawn callback (`on_spawn_complete`) of `parent`'s handler, once
    /// its children were enqueued.
    pub async fn notify_spawn_complete(&self, parent: &Job, result: &SpawnResult) -> Result<()> {
        match self.handlers.get(&parent.queue_name) {
            Some(handler) => handler.on_spawn_complete(result).await,
            None => Ok(()),
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
///     .with_spawning()?;
///
/// // Check that spawn config was added to payload
/// assert!(job.payload.get("_spawn_config").is_some());
/// # Ok::<(), hammerwork::HammerworkError>(())
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
///     .with_spawn_config(spawn_config)?;
///
/// // Verify the configuration is stored
/// let stored_config = job.payload.get("_spawn_config").unwrap();
/// let parsed_config: SpawnConfig = serde_json::from_value(stored_config.clone()).unwrap();
/// assert_eq!(parsed_config.max_spawn_count, Some(10));
/// # Ok::<(), hammerwork::HammerworkError>(())
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
/// .with_spawning()?; // Uses default spawn configuration
///
/// // When this job completes, the spawn handler will create
/// // individual notification jobs for each user
/// # Ok::<(), hammerwork::HammerworkError>(())
/// ```
///
/// ## Payload requirement
///
/// The configuration lives in the payload, so a non-object payload is rejected
/// instead of silently dropping the configuration:
///
/// ```rust
/// use hammerwork::{Job, spawn::JobSpawnExt};
/// use serde_json::json;
///
/// let job = Job::new("queue".to_string(), json!([1, 2, 3]));
/// assert!(job.with_spawning().is_err());
/// ```
pub trait JobSpawnExt {
    /// Enable spawning for this job with the given configuration.
    ///
    /// The configuration is stored in the job payload under
    /// [`SPAWN_CONFIG_KEY`] (`"_spawn_config"`), so the payload must be a JSON object.
    ///
    /// # Errors
    ///
    /// Returns [`HammerworkError::InvalidJobPayload`] if the payload is not a JSON object
    /// (the configuration would have nowhere to live), or a serialization error if the
    /// configuration cannot be serialized. The job is never silently left without its
    /// spawn configuration.
    fn with_spawn_config(self, config: SpawnConfig) -> Result<Self>
    where
        Self: Sized;

    /// Enable spawning for this job with default configuration.
    ///
    /// # Errors
    ///
    /// Same as [`JobSpawnExt::with_spawn_config`].
    fn with_spawning(self) -> Result<Self>
    where
        Self: Sized;
}

impl JobSpawnExt for Job {
    fn with_spawn_config(mut self, config: SpawnConfig) -> Result<Self> {
        let value = serde_json::to_value(config)?;
        let kind = json_kind(&self.payload);
        let payload_obj =
            self.payload
                .as_object_mut()
                .ok_or_else(|| HammerworkError::InvalidJobPayload {
                    message: format!(
                        "a job's spawn configuration is stored in its payload under \
                         `{SPAWN_CONFIG_KEY}`, so the payload must be a JSON object (got {kind})"
                    ),
                })?;
        payload_obj.insert(SPAWN_CONFIG_KEY.to_string(), value);
        Ok(self)
    }

    fn with_spawning(self) -> Result<Self> {
        self.with_spawn_config(SpawnConfig::default())
    }
}

/// The payload key under which a job's [`SpawnConfig`] is stored.
///
/// The job schema has no dedicated spawn column, so the configuration travels in the
/// payload. A worker reads it from the job's *decrypted* payload, so spawning works for
/// encrypted jobs too; the stored (encrypted) row, however, does not expose the key to SQL,
/// which means `cargo hammerwork spawn` queries only see jobs whose payload is stored in
/// the clear.
pub const SPAWN_CONFIG_KEY: &str = "_spawn_config";

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Read the spawn configuration out of a job payload.
///
/// Returns `Ok(None)` when the payload carries no [`SPAWN_CONFIG_KEY`], and an error when the
/// key is present but is not a valid [`SpawnConfig`], so a misconfigured job is reported
/// instead of silently never spawning.
pub fn spawn_config_from_payload(payload: &serde_json::Value) -> Result<Option<SpawnConfig>> {
    match payload.get(SPAWN_CONFIG_KEY) {
        None => Ok(None),
        Some(value) => serde_json::from_value::<SpawnConfig>(value.clone())
            .map(Some)
            .map_err(|e| HammerworkError::InvalidJobPayload {
                message: format!("invalid `{SPAWN_CONFIG_KEY}` in job payload: {e}"),
            }),
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
        let _handler = TestSpawnHandler;
        let _parent_job = Job::new("parent_task".to_string(), json!({"spawn_count": 3}));

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

    #[cfg(feature = "postgres")]
    #[test]
    fn test_spawn_manager_registration() {
        let mut manager: SpawnManager<sqlx::Postgres> = SpawnManager::new();
        assert!(!manager.has_handler("test_job"));

        manager.register_handler("test_job", TestSpawnHandler);
        assert!(manager.has_handler("test_job"));

        let types = manager.registered_types();
        assert!(types.contains(&"test_job".to_string()));
    }

    #[test]
    fn test_with_spawn_config_stores_config_in_object_payload() {
        let job = Job::new("q".to_string(), json!({"a": 1}))
            .with_spawn_config(SpawnConfig {
                max_spawn_count: Some(7),
                operation_id: Some("op".to_string()),
                ..SpawnConfig::default()
            })
            .unwrap();
        assert_eq!(job.payload["a"], 1);
        let config = spawn_config_from_payload(&job.payload).unwrap().unwrap();
        assert_eq!(config.max_spawn_count, Some(7));
        assert_eq!(config.operation_id.as_deref(), Some("op"));

        let job = Job::new("q".to_string(), json!({}))
            .with_spawning()
            .unwrap();
        assert!(job.payload.get(SPAWN_CONFIG_KEY).is_some());
    }

    #[test]
    fn test_with_spawn_config_rejects_non_object_payloads() {
        for payload in [
            json!(null),
            json!(3),
            json!("text"),
            json!([1, 2]),
            json!(true),
        ] {
            let job = Job::new("q".to_string(), payload.clone());
            let err = job.with_spawning().unwrap_err();
            assert!(
                matches!(err, HammerworkError::InvalidJobPayload { .. }),
                "{payload}: {err}"
            );
            assert!(err.to_string().contains("must be a JSON object"), "{err}");
        }
    }

    #[test]
    fn test_spawn_config_from_payload() {
        assert!(spawn_config_from_payload(&json!({})).unwrap().is_none());
        assert!(spawn_config_from_payload(&json!("x")).unwrap().is_none());
        let err = spawn_config_from_payload(&json!({"_spawn_config": {"max_spawn_count": "many"}}))
            .unwrap_err();
        assert!(matches!(err, HammerworkError::InvalidJobPayload { .. }));
        assert!(err.to_string().contains("_spawn_config"));
    }
}
