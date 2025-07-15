//! In-memory test implementation of the job queue for testing purposes.
//!
//! This module provides a `TestQueue` that implements the `DatabaseQueue` trait
//! entirely in memory, making it ideal for unit tests and development without
//! requiring a database connection.
//!
//! # Features
//!
//! - All operations are performed in-memory
//! - Thread-safe with async operations
//! - Supports time manipulation for testing delayed jobs
//! - Full compatibility with the `DatabaseQueue` trait
//! - Deterministic behavior for testing
//!
//! # Examples
//!
//! ```rust
//! use hammerwork::queue::test::TestQueue;
//! use hammerwork::{Job, JobStatus, queue::DatabaseQueue};
//! use serde_json::json;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let queue = TestQueue::new();
//!
//! // Enqueue a job
//! let job = Job::new("test_queue".to_string(), json!({"test": true}));
//! let job_id = queue.enqueue(job).await?;
//!
//! // Process the job
//! if let Some(mut job) = queue.dequeue("test_queue").await? {
//!     // Process the job...
//!     queue.complete_job(job.id).await?;
//! }
//! # Ok(())
//! # }
//! ```

use crate::{
    HammerworkError, Result,
    batch::{BatchId, BatchResult, BatchStatus, JobBatch},
    job::{Job, JobId, JobStatus},
    priority::{JobPriority, PriorityWeights},
    queue::{DatabaseQueue, QueuePauseInfo},
    rate_limit::ThrottleConfig,
    stats::{DeadJobSummary, QueueStats},
    workflow::{JobGroup, WorkflowId, WorkflowStatus},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::RwLock;
use uuid::Uuid;

// We need to use a real sqlx Database type for the trait bound
// This is just a marker - TestQueue doesn't actually use SQL
#[cfg(feature = "postgres")]
type TestDatabaseType = sqlx::Postgres;

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
type TestDatabaseType = sqlx::MySql;

#[cfg(all(not(feature = "postgres"), not(feature = "mysql")))]
type TestDatabaseType = sqlx::Any; // Fallback

/// Mock clock for controlling time in tests.
///
/// The `MockClock` allows you to control time flow in your tests, making it possible
/// to test time-dependent functionality like delayed jobs, cron schedules, and timeouts
/// in a deterministic and fast manner.
///
/// # Examples
///
/// ## Basic time manipulation
///
/// ```rust
/// use hammerwork::queue::test::MockClock;
/// use chrono::Duration;
///
/// let clock = MockClock::new();
/// let initial_time = clock.now();
///
/// // Advance time by 1 hour
/// clock.advance(Duration::hours(1));
///
/// let after_advance = clock.now();
/// assert_eq!((after_advance - initial_time).num_hours(), 1);
/// ```
///
/// ## Testing delayed jobs
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use hammerwork::queue::test::{MockClock, TestQueue};
/// use hammerwork::{Job, queue::DatabaseQueue};
/// use serde_json::json;
/// use chrono::Duration;
///
/// let clock = MockClock::new();
/// let queue = TestQueue::with_clock(clock.clone());
///
/// // Create a delayed job (runs in 2 hours)
/// let delayed_job = Job::with_delay(
///     "delayed_queue".to_string(),
///     json!({"message": "Hello future!"}),
///     Duration::hours(2)
/// );
///
/// let job_id = queue.enqueue(delayed_job).await?;
///
/// // Job should not be available immediately
/// assert!(queue.dequeue("delayed_queue").await?.is_none());
///
/// // Advance time by 2 hours
/// clock.advance(Duration::hours(2));
///
/// // Now the job should be available
/// let job = queue.dequeue("delayed_queue").await?.unwrap();
/// assert_eq!(job.id, job_id);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct MockClock {
    current_time: Arc<Mutex<DateTime<Utc>>>,
}

impl MockClock {
    /// Create a new mock clock starting at the current time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::MockClock;
    ///
    /// let clock = MockClock::new();
    /// let now = clock.now();
    /// println!("Mock clock started at: {}", now);
    /// ```
    pub fn new() -> Self {
        Self {
            current_time: Arc::new(Mutex::new(Utc::now())),
        }
    }

    /// Get the current mock time.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::MockClock;
    /// use chrono::Duration;
    ///
    /// let clock = MockClock::new();
    /// let time1 = clock.now();
    ///
    /// clock.advance(Duration::minutes(30));
    /// let time2 = clock.now();
    ///
    /// assert_eq!((time2 - time1).num_minutes(), 30);
    /// ```
    pub fn now(&self) -> DateTime<Utc> {
        *self.current_time.lock().unwrap()
    }

    /// Advance the mock time by the given duration.
    ///
    /// This is useful for testing time-dependent functionality without
    /// waiting for real time to pass.
    ///
    /// # Arguments
    ///
    /// * `duration` - The amount of time to advance the clock
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::MockClock;
    /// use chrono::Duration;
    ///
    /// let clock = MockClock::new();
    /// let start_time = clock.now();
    ///
    /// // Advance by 1 day, 2 hours, and 30 minutes
    /// clock.advance(Duration::days(1));
    /// clock.advance(Duration::hours(2));
    /// clock.advance(Duration::minutes(30));
    ///
    /// let end_time = clock.now();
    /// let total_duration = end_time - start_time;
    ///
    /// assert_eq!(total_duration.num_hours(), 26); // 24 + 2
    /// assert_eq!(total_duration.num_minutes(), 1590); // 26 * 60 + 30
    /// ```
    pub fn advance(&self, duration: chrono::Duration) {
        let mut time = self.current_time.lock().unwrap();
        *time += duration;
    }

    /// Set the mock time to a specific instant.
    ///
    /// This allows you to set the clock to any specific time, which can be
    /// useful for testing scenarios that depend on absolute times.
    ///
    /// # Arguments
    ///
    /// * `time` - The specific time to set the clock to
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::MockClock;
    /// use chrono::{Utc, TimeZone};
    ///
    /// let clock = MockClock::new();
    ///
    /// // Set to a specific date and time
    /// let specific_time = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
    /// clock.set_time(specific_time);
    ///
    /// assert_eq!(clock.now(), specific_time);
    /// ```
    pub fn set_time(&self, time: DateTime<Utc>) {
        *self.current_time.lock().unwrap() = time;
    }
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

/// In-memory storage for the test queue
#[derive(Debug)]
struct TestStorage {
    /// All jobs stored by ID
    jobs: HashMap<JobId, Job>,
    /// Jobs organized by queue name and status
    queues: HashMap<String, HashMap<JobStatus, Vec<JobId>>>,
    /// Batch information
    batches: HashMap<BatchId, JobBatch>,
    /// Batch job mappings
    batch_jobs: HashMap<BatchId, Vec<JobId>>,
    /// Workflow information
    workflows: HashMap<WorkflowId, JobGroup>,
    /// Job dependencies (job_id -> depends on these job_ids)
    dependencies: HashMap<JobId, Vec<JobId>>,
    /// Reverse dependencies (job_id -> these jobs depend on it)
    dependents: HashMap<JobId, Vec<JobId>>,
    /// Throttle configurations
    throttle_configs: HashMap<String, ThrottleConfig>,
    /// Job results storage
    job_results: HashMap<JobId, (serde_json::Value, Option<DateTime<Utc>>)>,
    /// Paused queues information
    paused_queues: HashMap<String, QueuePauseInfo>,
    /// Mock clock for time control
    clock: MockClock,
}

impl TestStorage {
    fn new(clock: MockClock) -> Self {
        Self {
            jobs: HashMap::new(),
            queues: HashMap::new(),
            batches: HashMap::new(),
            batch_jobs: HashMap::new(),
            workflows: HashMap::new(),
            dependencies: HashMap::new(),
            dependents: HashMap::new(),
            throttle_configs: HashMap::new(),
            job_results: HashMap::new(),
            paused_queues: HashMap::new(),
            clock,
        }
    }

    /// Add a job to the appropriate queue and status list
    fn add_job_to_queue(&mut self, job: &Job) {
        let queue_jobs = self.queues.entry(job.queue_name.clone()).or_default();
        let status_jobs = queue_jobs.entry(job.status).or_default();
        if !status_jobs.contains(&job.id) {
            status_jobs.push(job.id);
        }
    }

    /// Update a job's status
    fn update_job_status(&mut self, job_id: JobId, new_status: JobStatus) -> Result<()> {
        // Get old status first
        let old_status = self
            .jobs
            .get(&job_id)
            .map(|job| job.status)
            .ok_or_else(|| HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;

        // Get the queue name
        let queue_name = self
            .jobs
            .get(&job_id)
            .map(|job| job.queue_name.clone())
            .ok_or_else(|| HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;

        // Remove from old status
        if let Some(queue_jobs) = self.queues.get_mut(&queue_name) {
            if let Some(status_jobs) = queue_jobs.get_mut(&old_status) {
                status_jobs.retain(|id| *id != job_id);
            }
        }

        // Update the job status
        if let Some(job) = self.jobs.get_mut(&job_id) {
            job.status = new_status;
        }

        // Add to new status
        let queue_jobs = self.queues.entry(queue_name).or_default();
        let status_jobs = queue_jobs.entry(new_status).or_default();
        if !status_jobs.contains(&job_id) {
            status_jobs.push(job_id);
        }

        Ok(())
    }

    /// Get the next job to dequeue based on priority and scheduled time
    fn get_next_job(&self, queue_name: &str, weights: Option<&PriorityWeights>) -> Option<JobId> {
        let queue_jobs = self.queues.get(queue_name)?;
        let pending_jobs = queue_jobs.get(&JobStatus::Pending)?;

        let now = self.clock.now();
        let eligible_jobs: Vec<&Job> = pending_jobs
            .iter()
            .filter_map(|id| self.jobs.get(id))
            .filter(|job| job.scheduled_at <= now)
            .collect();

        if eligible_jobs.is_empty() {
            return None;
        }

        // If no weights provided, use strict priority ordering
        if weights.is_none() {
            return eligible_jobs
                .into_iter()
                .max_by_key(|job| (job.priority.as_i32(), std::cmp::Reverse(job.scheduled_at)))
                .map(|job| job.id);
        }

        // Use weighted selection
        let weights = weights.unwrap();

        // If strict priority, just get highest priority job
        if weights.is_strict() {
            return eligible_jobs
                .into_iter()
                .max_by_key(|job| (job.priority.as_i32(), std::cmp::Reverse(job.scheduled_at)))
                .map(|job| job.id);
        }

        // Group jobs by priority
        let mut candidates_by_priority: HashMap<JobPriority, Vec<&Job>> = HashMap::new();
        for job in eligible_jobs {
            candidates_by_priority
                .entry(job.priority)
                .or_default()
                .push(job);
        }

        // Build weighted selection pool
        let mut weighted_choices = Vec::new();
        for priority in candidates_by_priority.keys() {
            let weight = weights.get_weight(*priority);
            for _ in 0..weight {
                weighted_choices.push(*priority);
            }
        }

        if weighted_choices.is_empty() {
            return None;
        }

        // Use hash-based selection for deterministic behavior
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        queue_name.hash(&mut hasher);
        self.clock.now().timestamp_millis().hash(&mut hasher);
        let hash_value = hasher.finish();

        let selected_priority = weighted_choices[hash_value as usize % weighted_choices.len()];

        // Get the oldest job from the selected priority
        candidates_by_priority
            .get(&selected_priority)
            .and_then(|jobs| {
                jobs.iter()
                    .min_by_key(|job| job.scheduled_at)
                    .map(|job| job.id)
            })
    }
}

/// In-memory test implementation of the job queue.
///
/// `TestQueue` provides a complete implementation of the `DatabaseQueue` trait
/// that runs entirely in memory, making it perfect for unit testing your job
/// processing logic without requiring a database connection.
///
/// # Features
///
/// - **Full DatabaseQueue compatibility**: Drop-in replacement for testing
/// - **Time control**: MockClock integration for testing time-dependent features
/// - **Deterministic behavior**: Predictable ordering and timing for reliable tests
/// - **Complete feature support**: Batches, workflows, cron jobs, priorities, and more
/// - **Thread-safe**: Safe to use in concurrent test scenarios
///
/// # Examples
///
/// ## Basic job processing test
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use hammerwork::queue::test::TestQueue;
/// use hammerwork::{Job, JobStatus, queue::DatabaseQueue};
/// use serde_json::json;
///
/// let queue = TestQueue::new();
///
/// // Enqueue a job
/// let job = Job::new("test_queue".to_string(), json!({"task": "process_data"}));
/// let job_id = queue.enqueue(job).await?;
///
/// // Dequeue and process the job
/// let job = queue.dequeue("test_queue").await?.unwrap();
/// assert_eq!(job.status, JobStatus::Running);
///
/// // Mark job as completed
/// queue.complete_job(job_id).await?;
///
/// // Verify completion
/// let completed_job = queue.get_job(job_id).await?.unwrap();
/// assert_eq!(completed_job.status, JobStatus::Completed);
/// # Ok(())
/// # }
/// ```
///
/// ## Testing job priorities
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use hammerwork::queue::test::TestQueue;
/// use hammerwork::{Job, JobPriority, queue::DatabaseQueue};
/// use serde_json::json;
///
/// let queue = TestQueue::new();
///
/// // Enqueue jobs with different priorities
/// let low_job = Job::new("priority_queue".to_string(), json!({"priority": "low"}))
///     .as_low_priority();
/// let high_job = Job::new("priority_queue".to_string(), json!({"priority": "high"}))
///     .as_high_priority();
///
/// let low_id = queue.enqueue(low_job).await?;
/// let high_id = queue.enqueue(high_job).await?;
///
/// // High priority job should be dequeued first
/// let first_job = queue.dequeue("priority_queue").await?.unwrap();
/// assert_eq!(first_job.id, high_id);
/// assert_eq!(first_job.priority, JobPriority::High);
///
/// let second_job = queue.dequeue("priority_queue").await?.unwrap();
/// assert_eq!(second_job.id, low_id);
/// assert_eq!(second_job.priority, JobPriority::Low);
/// # Ok(())
/// # }
/// ```
///
/// ## Testing job failures and retries
///
/// ```rust
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use hammerwork::queue::test::TestQueue;
/// use hammerwork::{Job, JobStatus, queue::DatabaseQueue};
/// use serde_json::json;
///
/// let queue = TestQueue::new();
///
/// // Create a job with custom retry settings
/// let job = Job::new("retry_queue".to_string(), json!({"data": "test"}))
///     .with_max_attempts(3);
/// let job_id = queue.enqueue(job).await?;
///
/// // Dequeue and fail the job
/// let job = queue.dequeue("retry_queue").await?.unwrap();
/// queue.fail_job(job_id, "Temporary network error").await?;
///
/// // Job should be in retrying state
/// let retrying_job = queue.get_job(job_id).await?.unwrap();
/// assert_eq!(retrying_job.status, JobStatus::Retrying);
/// assert_eq!(retrying_job.attempts, 1);
///
/// // After 3 failures, job becomes dead
/// queue.fail_job(job_id, "Still failing").await?;
/// queue.fail_job(job_id, "Final failure").await?;
///
/// let dead_job = queue.get_job(job_id).await?.unwrap();
/// assert_eq!(dead_job.status, JobStatus::Dead);
/// assert_eq!(dead_job.attempts, 3);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct TestQueue {
    storage: Arc<RwLock<TestStorage>>,
    clock: MockClock,
}

impl TestQueue {
    /// Create a new test queue with a fresh MockClock.
    ///
    /// This is the most common way to create a TestQueue for testing.
    /// Each TestQueue gets its own isolated storage and time control.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::TestQueue;
    ///
    /// let queue = TestQueue::new();
    /// // Queue is ready to use for testing
    /// ```
    pub fn new() -> Self {
        let clock = MockClock::new();
        Self {
            storage: Arc::new(RwLock::new(TestStorage::new(clock.clone()))),
            clock,
        }
    }

    /// Create a new test queue with a custom MockClock.
    ///
    /// This allows you to share time control across multiple test components
    /// or start with a specific time state.
    ///
    /// # Arguments
    ///
    /// * `clock` - The MockClock instance to use for time control
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::{MockClock, TestQueue};
    /// use chrono::{Utc, TimeZone};
    ///
    /// // Create a clock set to a specific time
    /// let clock = MockClock::new();
    /// let specific_time = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    /// clock.set_time(specific_time);
    ///
    /// // Create queue with this clock
    /// let queue = TestQueue::with_clock(clock);
    ///
    /// // Queue will use the specified time
    /// assert_eq!(queue.clock().now(), specific_time);
    /// ```
    pub fn with_clock(clock: MockClock) -> Self {
        Self {
            storage: Arc::new(RwLock::new(TestStorage::new(clock.clone()))),
            clock,
        }
    }

    /// Get access to the mock clock for time manipulation.
    ///
    /// This allows you to control time flow in your tests, which is essential
    /// for testing time-dependent features like delayed jobs, cron schedules,
    /// and timeouts.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::queue::test::TestQueue;
    /// use chrono::Duration;
    ///
    /// let queue = TestQueue::new();
    /// let initial_time = queue.clock().now();
    ///
    /// // Advance time by 1 hour
    /// queue.clock().advance(Duration::hours(1));
    ///
    /// let later_time = queue.clock().now();
    /// assert_eq!((later_time - initial_time).num_hours(), 1);
    /// ```
    pub fn clock(&self) -> &MockClock {
        &self.clock
    }

    /// Get the number of jobs in a specific status for a queue.
    ///
    /// This is a testing utility that helps you verify the state of your
    /// job queue during tests.
    ///
    /// # Arguments
    ///
    /// * `queue_name` - The name of the queue to check
    /// * `status` - The job status to count
    ///
    /// # Returns
    ///
    /// The number of jobs in the specified status
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use hammerwork::queue::test::TestQueue;
    /// use hammerwork::{Job, JobStatus, queue::DatabaseQueue};
    /// use serde_json::json;
    ///
    /// let queue = TestQueue::new();
    ///
    /// // Initially no jobs
    /// assert_eq!(queue.get_job_count("test_queue", &JobStatus::Pending).await, 0);
    ///
    /// // Add some jobs
    /// for i in 0..5 {
    ///     let job = Job::new("test_queue".to_string(), json!({"index": i}));
    ///     queue.enqueue(job).await?;
    /// }
    ///
    /// // Should have 5 pending jobs
    /// assert_eq!(queue.get_job_count("test_queue", &JobStatus::Pending).await, 5);
    ///
    /// // Process one job
    /// let job = queue.dequeue("test_queue").await?.unwrap();
    /// queue.complete_job(job.id).await?;
    ///
    /// // Should have 4 pending and 1 completed
    /// assert_eq!(queue.get_job_count("test_queue", &JobStatus::Pending).await, 4);
    /// assert_eq!(queue.get_job_count("test_queue", &JobStatus::Completed).await, 1);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_job_count(&self, queue_name: &str, status: &JobStatus) -> usize {
        let storage = self.storage.read().await;
        storage
            .queues
            .get(queue_name)
            .and_then(|q| q.get(status))
            .map(|jobs| jobs.len())
            .unwrap_or(0)
    }

    /// Get all jobs for testing purposes.
    ///
    /// This returns all jobs stored in the TestQueue, regardless of their
    /// queue name or status. This is useful for comprehensive testing and
    /// debugging.
    ///
    /// # Returns
    ///
    /// A vector containing all jobs in the TestQueue
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use hammerwork::queue::test::TestQueue;
    /// use hammerwork::{Job, queue::DatabaseQueue};
    /// use serde_json::json;
    ///
    /// let queue = TestQueue::new();
    ///
    /// // Add jobs to different queues
    /// let job1 = Job::new("queue_a".to_string(), json!({"type": "a"}));
    /// let job2 = Job::new("queue_b".to_string(), json!({"type": "b"}));
    ///
    /// queue.enqueue(job1).await?;
    /// queue.enqueue(job2).await?;
    ///
    /// // Get all jobs
    /// let all_jobs = queue.get_all_jobs().await;
    /// assert_eq!(all_jobs.len(), 2);
    ///
    /// // Jobs from different queues are included
    /// let queue_names: Vec<String> = all_jobs.iter()
    ///     .map(|job| job.queue_name.clone())
    ///     .collect();
    /// assert!(queue_names.contains(&"queue_a".to_string()));
    /// assert!(queue_names.contains(&"queue_b".to_string()));
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_all_jobs(&self) -> Vec<Job> {
        let storage = self.storage.read().await;
        storage.jobs.values().cloned().collect()
    }
}

impl Default for TestQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseQueue for TestQueue {
    type Database = TestDatabaseType;

    async fn enqueue(&self, mut job: Job) -> Result<JobId> {
        let mut storage = self.storage.write().await;

        // Set timestamps using the mock clock
        let now = storage.clock.now();
        let original_scheduled_at = job.scheduled_at;
        let original_created_at = job.created_at;

        // Calculate the delay from the original creation
        let delay = original_scheduled_at - original_created_at;

        job.created_at = now;

        // For immediate jobs (where scheduled_at was set to created_at in Job::new()),
        // update scheduled_at to use the mock clock time
        if original_scheduled_at == original_created_at {
            job.scheduled_at = now;
        } else {
            // For delayed jobs, preserve the delay but use mock clock time as base
            job.scheduled_at = now + delay;
        }

        // Store the job
        storage.jobs.insert(job.id, job.clone());
        storage.add_job_to_queue(&job);

        Ok(job.id)
    }

    async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
        let mut storage = self.storage.write().await;

        if let Some(job_id) = storage.get_next_job(queue_name, None) {
            // Update job status to Running
            storage.update_job_status(job_id, JobStatus::Running)?;

            // Set started_at
            let now = storage.clock.now();
            if let Some(job) = storage.jobs.get_mut(&job_id) {
                job.started_at = Some(now);
                return Ok(Some(job.clone()));
            }
        }

        Ok(None)
    }

    async fn dequeue_with_priority_weights(
        &self,
        queue_name: &str,
        weights: &PriorityWeights,
    ) -> Result<Option<Job>> {
        let mut storage = self.storage.write().await;

        if let Some(job_id) = storage.get_next_job(queue_name, Some(weights)) {
            // Update job status to Running
            storage.update_job_status(job_id, JobStatus::Running)?;

            // Set started_at
            let now = storage.clock.now();
            if let Some(job) = storage.jobs.get_mut(&job_id) {
                job.started_at = Some(now);
                return Ok(Some(job.clone()));
            }
        }

        Ok(None)
    }

    async fn complete_job(&self, job_id: JobId) -> Result<()> {
        let mut storage = self.storage.write().await;

        storage.update_job_status(job_id, JobStatus::Completed)?;

        let now = storage.clock.now();
        if let Some(job) = storage.jobs.get_mut(&job_id) {
            job.completed_at = Some(now);

            // Handle cron jobs
            if job.recurring && job.cron_schedule.is_some() {
                // Calculate next run time
                if let Ok(schedule) = job
                    .cron_schedule
                    .as_ref()
                    .unwrap()
                    .parse::<cron::Schedule>()
                {
                    let timezone = job
                        .timezone
                        .as_ref()
                        .and_then(|tz| tz.parse::<chrono_tz::Tz>().ok())
                        .unwrap_or(chrono_tz::UTC);

                    let now_in_tz = now.with_timezone(&timezone);
                    if let Some(next) = schedule.after(&now_in_tz).next() {
                        job.next_run_at = Some(next.with_timezone(&Utc));
                    }
                }
            }
        }

        // Resolve dependencies
        if let Some(dependents) = storage.dependents.get(&job_id).cloned() {
            let now = storage.clock.now();
            for dependent_id in dependents {
                // Check if all dependencies are satisfied
                if let Some(deps) = storage.dependencies.get(&dependent_id) {
                    let all_complete = deps.iter().all(|dep_id| {
                        storage
                            .jobs
                            .get(dep_id)
                            .map(|j| j.status == JobStatus::Completed)
                            .unwrap_or(false)
                    });

                    if all_complete {
                        // Make the job eligible for execution
                        if let Some(dep_job) = storage.jobs.get_mut(&dependent_id) {
                            if dep_job.status == JobStatus::Pending {
                                dep_job.scheduled_at = now;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
        let mut storage = self.storage.write().await;

        // First increment attempts
        if let Some(job) = storage.jobs.get_mut(&job_id) {
            job.attempts += 1;
            job.error_message = Some(error_message.to_string());
        } else {
            return Err(HammerworkError::JobNotFound {
                id: job_id.to_string(),
            });
        }

        // Then check if we should retry
        let should_retry = if let Some(job) = storage.jobs.get(&job_id) {
            job.attempts < job.max_attempts
        } else {
            return Err(HammerworkError::JobNotFound {
                id: job_id.to_string(),
            });
        };

        let now = storage.clock.now();

        if should_retry {
            storage.update_job_status(job_id, JobStatus::Retrying)?;
        } else {
            storage.update_job_status(job_id, JobStatus::Dead)?;
            if let Some(job) = storage.jobs.get_mut(&job_id) {
                job.failed_at = Some(now);
            }

            // Check if this job belongs to a workflow with fail-fast policy
            let other_job_ids: Vec<JobId> = storage
                .workflows
                .values()
                .find(|workflow| {
                    workflow.failure_policy == crate::workflow::FailurePolicy::FailFast
                        && workflow.jobs.iter().any(|j| j.id == job_id)
                })
                .map(|workflow| {
                    workflow
                        .jobs
                        .iter()
                        .filter(|j| j.id != job_id)
                        .map(|j| j.id)
                        .collect()
                })
                .unwrap_or_default();

            // Fail all other pending jobs in the workflow
            for other_job_id in other_job_ids {
                if let Some(j) = storage.jobs.get(&other_job_id) {
                    if j.status == JobStatus::Pending || j.status == JobStatus::Retrying {
                        storage
                            .update_job_status(other_job_id, JobStatus::Failed)
                            .ok();
                        if let Some(j) = storage.jobs.get_mut(&other_job_id) {
                            j.error_message =
                                Some("Workflow failed (fail-fast policy)".to_string());
                            j.failed_at = Some(now);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
        let mut storage = self.storage.write().await;

        storage.update_job_status(job_id, JobStatus::Pending)?;

        if let Some(job) = storage.jobs.get_mut(&job_id) {
            job.scheduled_at = retry_at;
            job.started_at = None;
        }

        Ok(())
    }

    async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
        let storage = self.storage.read().await;
        Ok(storage.jobs.get(&job_id).cloned())
    }

    async fn delete_job(&self, job_id: JobId) -> Result<()> {
        let mut storage = self.storage.write().await;

        if let Some(job) = storage.jobs.remove(&job_id) {
            // Remove from queue
            if let Some(queue_jobs) = storage.queues.get_mut(&job.queue_name) {
                if let Some(status_jobs) = queue_jobs.get_mut(&job.status) {
                    status_jobs.retain(|id| *id != job_id);
                }
            }

            // Remove from batch if applicable
            if let Some(batch_id) = job.batch_id {
                if let Some(batch_jobs) = storage.batch_jobs.get_mut(&batch_id) {
                    batch_jobs.retain(|id| *id != job_id);
                }
            }

            // Clean up dependencies
            storage.dependencies.remove(&job_id);
            for deps in storage.dependents.values_mut() {
                deps.retain(|id| *id != job_id);
            }

            Ok(())
        } else {
            Err(HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })
        }
    }

    // Batch operations
    async fn enqueue_batch(&self, mut batch: JobBatch) -> Result<BatchId> {
        let mut storage = self.storage.write().await;

        batch.created_at = storage.clock.now();
        let batch_id = batch.id;

        // Enqueue all jobs in the batch
        let mut job_ids = Vec::new();
        for mut job in batch.jobs.iter().cloned() {
            job.batch_id = Some(batch_id);

            // Apply the same timestamp logic as regular enqueue
            let now = storage.clock.now();
            let original_scheduled_at = job.scheduled_at;
            let original_created_at = job.created_at;

            // Calculate the delay from the original creation
            let delay = original_scheduled_at - original_created_at;

            job.created_at = now;

            // For immediate jobs (where scheduled_at was set to created_at in Job::new()),
            // update scheduled_at to use the mock clock time
            if original_scheduled_at == original_created_at {
                job.scheduled_at = now;
            } else {
                // For delayed jobs, preserve the delay but use mock clock time as base
                job.scheduled_at = now + delay;
            }

            storage.jobs.insert(job.id, job.clone());
            storage.add_job_to_queue(&job);
            job_ids.push(job.id);
        }

        storage.batch_jobs.insert(batch_id, job_ids);
        storage.batches.insert(batch_id, batch);

        Ok(batch_id)
    }

    async fn get_batch_status(&self, batch_id: BatchId) -> Result<BatchResult> {
        let storage = self.storage.read().await;

        let batch = storage
            .batches
            .get(&batch_id)
            .ok_or_else(|| HammerworkError::Batch {
                message: format!("Batch {} not found", batch_id),
            })?;

        let job_ids = storage
            .batch_jobs
            .get(&batch_id)
            .ok_or_else(|| HammerworkError::Batch {
                message: format!("Batch jobs for {} not found", batch_id),
            })?;

        let mut completed_jobs = 0;
        let mut failed_jobs = 0;
        let mut pending_jobs = 0;
        let mut job_errors = HashMap::new();

        for job_id in job_ids {
            if let Some(job) = storage.jobs.get(job_id) {
                match &job.status {
                    JobStatus::Completed => completed_jobs += 1,
                    JobStatus::Failed | JobStatus::Dead | JobStatus::TimedOut => {
                        failed_jobs += 1;
                        if let Some(error) = &job.error_message {
                            job_errors.insert(job.id, error.clone());
                        }
                    }
                    JobStatus::Pending | JobStatus::Retrying => pending_jobs += 1,
                    JobStatus::Running => pending_jobs += 1, // Count running as pending
                    JobStatus::Archived => {} // Archived jobs don't count in workflow stats
                }
            }
        }

        let total_jobs = job_ids.len() as u32;
        let status = if failed_jobs > 0
            && batch.failure_mode == crate::batch::PartialFailureMode::FailFast
        {
            BatchStatus::Failed
        } else if completed_jobs == total_jobs {
            BatchStatus::Completed
        } else if completed_jobs + failed_jobs == total_jobs {
            BatchStatus::PartiallyFailed
        } else {
            BatchStatus::Processing
        };

        let completed_at = if status == BatchStatus::Completed || status == BatchStatus::Failed {
            Some(storage.clock.now())
        } else {
            None
        };

        Ok(BatchResult {
            batch_id,
            total_jobs,
            completed_jobs,
            failed_jobs,
            pending_jobs,
            status,
            created_at: batch.created_at,
            completed_at,
            error_summary: if !job_errors.is_empty() {
                Some(format!("{} jobs failed", job_errors.len()))
            } else {
                None
            },
            job_errors,
        })
    }

    async fn get_batch_jobs(&self, batch_id: BatchId) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;

        let job_ids = storage
            .batch_jobs
            .get(&batch_id)
            .ok_or_else(|| HammerworkError::Batch {
                message: format!("Batch {} not found", batch_id),
            })?;

        let jobs: Vec<Job> = job_ids
            .iter()
            .filter_map(|id| storage.jobs.get(id).cloned())
            .collect();

        Ok(jobs)
    }

    async fn delete_batch(&self, batch_id: BatchId) -> Result<()> {
        let mut storage = self.storage.write().await;

        // Delete all jobs in the batch
        if let Some(job_ids) = storage.batch_jobs.remove(&batch_id) {
            for job_id in job_ids {
                if let Some(job) = storage.jobs.remove(&job_id) {
                    if let Some(queue_jobs) = storage.queues.get_mut(&job.queue_name) {
                        if let Some(status_jobs) = queue_jobs.get_mut(&job.status) {
                            status_jobs.retain(|id| *id != job_id);
                        }
                    }
                }
            }
        }

        storage.batches.remove(&batch_id);
        Ok(())
    }

    // Dead job management
    async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()> {
        let mut storage = self.storage.write().await;

        storage.update_job_status(job_id, JobStatus::Dead)?;

        let now = storage.clock.now();
        if let Some(job) = storage.jobs.get_mut(&job_id) {
            job.failed_at = Some(now);
            job.error_message = Some(error_message.to_string());
        }

        Ok(())
    }

    async fn mark_job_timed_out(&self, job_id: JobId, error_message: &str) -> Result<()> {
        let mut storage = self.storage.write().await;

        storage.update_job_status(job_id, JobStatus::TimedOut)?;

        let now = storage.clock.now();
        if let Some(job) = storage.jobs.get_mut(&job_id) {
            job.timed_out_at = Some(now);
            job.error_message = Some(error_message.to_string());
        }

        Ok(())
    }

    async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;

        let mut dead_jobs: Vec<Job> = storage
            .jobs
            .values()
            .filter(|job| job.status == JobStatus::Dead || job.status == JobStatus::TimedOut)
            .cloned()
            .collect();

        // Sort by failed_at or timed_out_at (newest first)
        dead_jobs.sort_by(|a, b| {
            let a_time = a.failed_at.or(a.timed_out_at).unwrap_or(a.created_at);
            let b_time = b.failed_at.or(b.timed_out_at).unwrap_or(b.created_at);
            b_time.cmp(&a_time)
        });

        // Apply offset and limit
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(100) as usize;

        Ok(dead_jobs.into_iter().skip(offset).take(limit).collect())
    }

    async fn get_dead_jobs_by_queue(
        &self,
        queue_name: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;

        let mut dead_jobs: Vec<Job> = storage
            .jobs
            .values()
            .filter(|job| {
                job.queue_name == queue_name
                    && (job.status == JobStatus::Dead || job.status == JobStatus::TimedOut)
            })
            .cloned()
            .collect();

        // Sort by failed_at or timed_out_at (newest first)
        dead_jobs.sort_by(|a, b| {
            let a_time = a.failed_at.or(a.timed_out_at).unwrap_or(a.created_at);
            let b_time = b.failed_at.or(b.timed_out_at).unwrap_or(b.created_at);
            b_time.cmp(&a_time)
        });

        // Apply offset and limit
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(100) as usize;

        Ok(dead_jobs.into_iter().skip(offset).take(limit).collect())
    }

    async fn retry_dead_job(&self, job_id: JobId) -> Result<()> {
        let mut storage = self.storage.write().await;

        let job = storage
            .jobs
            .get(&job_id)
            .ok_or_else(|| HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;

        if job.status != JobStatus::Dead && job.status != JobStatus::TimedOut {
            return Err(HammerworkError::Queue {
                message: format!("Job {} is not dead", job_id),
            });
        }

        storage.update_job_status(job_id, JobStatus::Pending)?;

        let now = storage.clock.now();
        if let Some(job) = storage.jobs.get_mut(&job_id) {
            job.attempts = 0;
            job.error_message = None;
            job.failed_at = None;
            job.timed_out_at = None;
            job.scheduled_at = now;
        }

        Ok(())
    }

    async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let mut storage = self.storage.write().await;

        let dead_job_ids: Vec<JobId> = storage
            .jobs
            .iter()
            .filter(|(_, job)| {
                (job.status == JobStatus::Dead || job.status == JobStatus::TimedOut)
                    && job.failed_at.or(job.timed_out_at).unwrap_or(job.created_at) <= older_than
            })
            .map(|(id, _)| *id)
            .collect();

        let count = dead_job_ids.len() as u64;

        for job_id in dead_job_ids {
            if let Some(job) = storage.jobs.remove(&job_id) {
                if let Some(queue_jobs) = storage.queues.get_mut(&job.queue_name) {
                    if let Some(status_jobs) = queue_jobs.get_mut(&job.status) {
                        status_jobs.retain(|id| *id != job_id);
                    }
                }
            }
        }

        Ok(count)
    }

    async fn get_dead_job_summary(&self) -> Result<DeadJobSummary> {
        let storage = self.storage.read().await;

        let mut dead_jobs_by_queue: HashMap<String, u64> = HashMap::new();
        let mut error_patterns: HashMap<String, u64> = HashMap::new();
        let mut total_dead_jobs = 0;
        let mut oldest_dead_job: Option<DateTime<Utc>> = None;
        let mut newest_dead_job: Option<DateTime<Utc>> = None;

        for job in storage.jobs.values() {
            if job.status == JobStatus::Dead || job.status == JobStatus::TimedOut {
                total_dead_jobs += 1;

                *dead_jobs_by_queue
                    .entry(job.queue_name.clone())
                    .or_insert(0) += 1;

                if let Some(error) = &job.error_message {
                    let error_key = error.split('\n').next().unwrap_or(error).to_string();
                    *error_patterns.entry(error_key).or_insert(0) += 1;
                }

                let job_time = job.failed_at.or(job.timed_out_at).unwrap_or(job.created_at);

                if oldest_dead_job.is_none() || job_time < oldest_dead_job.unwrap() {
                    oldest_dead_job = Some(job_time);
                }

                if newest_dead_job.is_none() || job_time > newest_dead_job.unwrap() {
                    newest_dead_job = Some(job_time);
                }
            }
        }

        Ok(DeadJobSummary {
            total_dead_jobs,
            dead_jobs_by_queue,
            error_patterns,
            oldest_dead_job,
            newest_dead_job,
        })
    }

    // Statistics and monitoring
    async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
        let storage = self.storage.read().await;

        let queue_jobs = storage
            .queues
            .get(queue_name)
            .ok_or_else(|| HammerworkError::Queue {
                message: format!("Queue {} not found", queue_name),
            })?;

        let pending_count = queue_jobs
            .get(&JobStatus::Pending)
            .map(|j| j.len())
            .unwrap_or(0) as u64;
        let running_count = queue_jobs
            .get(&JobStatus::Running)
            .map(|j| j.len())
            .unwrap_or(0) as u64;
        let completed_count = queue_jobs
            .get(&JobStatus::Completed)
            .map(|j| j.len())
            .unwrap_or(0) as u64;
        let dead_count = queue_jobs
            .get(&JobStatus::Dead)
            .map(|j| j.len())
            .unwrap_or(0) as u64;
        let timed_out_count = queue_jobs
            .get(&JobStatus::TimedOut)
            .map(|j| j.len())
            .unwrap_or(0) as u64;

        // Calculate processing times for completed jobs
        let completed_jobs: Vec<&Job> = queue_jobs
            .get(&JobStatus::Completed)
            .map(|job_ids| {
                job_ids
                    .iter()
                    .filter_map(|id| storage.jobs.get(id))
                    .collect()
            })
            .unwrap_or_default();

        let processing_times: Vec<i64> = completed_jobs
            .iter()
            .filter_map(|job| match (job.started_at, job.completed_at) {
                (Some(start), Some(end)) => Some((end - start).num_milliseconds()),
                _ => None,
            })
            .collect();

        let avg_processing_time_ms = if !processing_times.is_empty() {
            processing_times.iter().sum::<i64>() as f64 / processing_times.len() as f64
        } else {
            0.0
        };

        let min_processing_time_ms = processing_times.iter().min().copied().unwrap_or(0) as u64;
        let max_processing_time_ms = processing_times.iter().max().copied().unwrap_or(0) as u64;

        // Get throughput (jobs completed in last minute)
        let one_minute_ago = storage.clock.now() - chrono::Duration::minutes(1);
        let jobs_per_minute = completed_jobs
            .iter()
            .filter(|job| job.completed_at.unwrap_or(job.created_at) > one_minute_ago)
            .count() as f64;

        let total_processed = completed_count + dead_count + timed_out_count;
        let failed_count = queue_jobs
            .get(&JobStatus::Failed)
            .map(|j| j.len())
            .unwrap_or(0) as u64
            + dead_count;
        let error_rate = if total_processed > 0 {
            (failed_count + timed_out_count) as f64 / total_processed as f64
        } else {
            0.0
        };

        let statistics = crate::stats::JobStatistics {
            total_processed,
            completed: completed_count,
            failed: failed_count,
            dead: dead_count,
            timed_out: timed_out_count,
            running: running_count,
            avg_processing_time_ms,
            min_processing_time_ms,
            max_processing_time_ms,
            throughput_per_minute: jobs_per_minute,
            error_rate,
            priority_stats: None,
            time_window: std::time::Duration::from_secs(3600),
            calculated_at: storage.clock.now(),
        };

        Ok(QueueStats {
            queue_name: queue_name.to_string(),
            pending_count,
            running_count,
            completed_count,
            dead_count,
            timed_out_count,
            statistics,
        })
    }

    async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>> {
        let storage = self.storage.read().await;
        let queue_names: Vec<String> = storage.queues.keys().cloned().collect();
        drop(storage);

        let mut stats = Vec::new();
        for queue_name in queue_names {
            if let Ok(queue_stats) = self.get_queue_stats(&queue_name).await {
                stats.push(queue_stats);
            }
        }

        Ok(stats)
    }

    async fn get_job_counts_by_status(&self, queue_name: &str) -> Result<HashMap<String, u64>> {
        let storage = self.storage.read().await;

        let mut counts = HashMap::new();

        if let Some(queue_jobs) = storage.queues.get(queue_name) {
            for (status, jobs) in queue_jobs {
                let status_str = match status {
                    JobStatus::Pending => "pending",
                    JobStatus::Running => "running",
                    JobStatus::Completed => "completed",
                    JobStatus::Failed => "failed",
                    JobStatus::Dead => "dead",
                    JobStatus::TimedOut => "timed_out",
                    JobStatus::Retrying => "retrying",
                    JobStatus::Archived => "archived",
                };
                counts.insert(status_str.to_string(), jobs.len() as u64);
            }
        }

        Ok(counts)
    }

    async fn get_processing_times(
        &self,
        queue_name: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<i64>> {
        let storage = self.storage.read().await;

        let processing_times: Vec<i64> = storage
            .jobs
            .values()
            .filter(|job| {
                job.queue_name == queue_name
                    && job.status == JobStatus::Completed
                    && job.completed_at.unwrap_or(job.created_at) >= since
            })
            .filter_map(|job| match (job.started_at, job.completed_at) {
                (Some(start), Some(end)) => Some((end - start).num_milliseconds()),
                _ => None,
            })
            .collect();

        Ok(processing_times)
    }

    async fn get_error_frequencies(
        &self,
        queue_name: Option<&str>,
        since: DateTime<Utc>,
    ) -> Result<HashMap<String, u64>> {
        let storage = self.storage.read().await;

        let mut frequencies = HashMap::new();

        for job in storage.jobs.values() {
            if let Some(qn) = queue_name {
                if job.queue_name != qn {
                    continue;
                }
            }

            if (job.status == JobStatus::Failed
                || job.status == JobStatus::Dead
                || job.status == JobStatus::TimedOut)
                && job.failed_at.or(job.timed_out_at).unwrap_or(job.created_at) >= since
            {
                if let Some(error) = &job.error_message {
                    let error_key = error.split('\n').next().unwrap_or(error).to_string();
                    *frequencies.entry(error_key).or_insert(0) += 1;
                }
            }
        }

        Ok(frequencies)
    }

    // Cron job management
    async fn enqueue_cron_job(&self, mut job: Job) -> Result<JobId> {
        if job.cron_schedule.is_none() {
            return Err(HammerworkError::Queue {
                message: "Job must have a cron schedule".to_string(),
            });
        }

        job.recurring = true;

        // Calculate initial next_run_at
        if let Ok(schedule) = job
            .cron_schedule
            .as_ref()
            .unwrap()
            .parse::<cron::Schedule>()
        {
            let timezone = job
                .timezone
                .as_ref()
                .and_then(|tz| tz.parse::<chrono_tz::Tz>().ok())
                .unwrap_or(chrono_tz::UTC);

            let now = self.clock.now();
            let now_in_tz = now.with_timezone(&timezone);

            if let Some(next) = schedule.after(&now_in_tz).next() {
                job.next_run_at = Some(next.with_timezone(&Utc));
                job.scheduled_at = job.next_run_at.unwrap();
            }
        } else {
            return Err(HammerworkError::Queue {
                message: format!("Invalid cron schedule: {:?}", job.cron_schedule),
            });
        }

        self.enqueue(job).await
    }

    async fn get_due_cron_jobs(&self, queue_name: Option<&str>) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;
        let now = storage.clock.now();

        let due_jobs: Vec<Job> = storage
            .jobs
            .values()
            .filter(|job| {
                job.recurring
                    && job.status == JobStatus::Completed
                    && job.next_run_at.map(|next| next <= now).unwrap_or(false)
                    && (queue_name.is_none() || job.queue_name == queue_name.unwrap())
            })
            .cloned()
            .collect();

        Ok(due_jobs)
    }

    async fn reschedule_cron_job(&self, job_id: JobId, next_run_at: DateTime<Utc>) -> Result<()> {
        let mut storage = self.storage.write().await;

        let job = storage
            .jobs
            .get_mut(&job_id)
            .ok_or_else(|| HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;

        if !job.recurring {
            return Err(HammerworkError::Queue {
                message: "Job is not a recurring job".to_string(),
            });
        }

        job.next_run_at = Some(next_run_at);

        // Create a new job instance for the next run
        let mut new_job = job.clone();
        new_job.id = Uuid::new_v4();
        new_job.status = JobStatus::Pending;
        new_job.attempts = 0;
        new_job.created_at = storage.clock.now();
        new_job.scheduled_at = next_run_at;
        new_job.started_at = None;
        new_job.completed_at = None;
        new_job.failed_at = None;
        new_job.timed_out_at = None;
        new_job.error_message = None;

        storage.jobs.insert(new_job.id, new_job.clone());
        storage.add_job_to_queue(&new_job);

        Ok(())
    }

    async fn get_recurring_jobs(&self, queue_name: &str) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;

        let recurring_jobs: Vec<Job> = storage
            .jobs
            .values()
            .filter(|job| job.queue_name == queue_name && job.recurring)
            .cloned()
            .collect();

        Ok(recurring_jobs)
    }

    async fn disable_recurring_job(&self, job_id: JobId) -> Result<()> {
        let mut storage = self.storage.write().await;

        let job = storage
            .jobs
            .get_mut(&job_id)
            .ok_or_else(|| HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;

        if !job.recurring {
            return Err(HammerworkError::Queue {
                message: "Job is not a recurring job".to_string(),
            });
        }

        job.recurring = false;
        job.next_run_at = None;

        Ok(())
    }

    async fn enable_recurring_job(&self, job_id: JobId) -> Result<()> {
        let mut storage = self.storage.write().await;

        // First, check if job exists and has cron schedule
        let cron_schedule = {
            let job = storage
                .jobs
                .get(&job_id)
                .ok_or_else(|| HammerworkError::JobNotFound {
                    id: job_id.to_string(),
                })?;

            if job.cron_schedule.is_none() {
                return Err(HammerworkError::Queue {
                    message: "Job does not have a cron schedule".to_string(),
                });
            }

            (job.cron_schedule.clone(), job.timezone.clone())
        };

        // Get current time
        let now = storage.clock.now();

        // Calculate next run time
        let next_run_at =
            if let Ok(schedule) = cron_schedule.0.as_ref().unwrap().parse::<cron::Schedule>() {
                let timezone = cron_schedule
                    .1
                    .as_ref()
                    .and_then(|tz| tz.parse::<chrono_tz::Tz>().ok())
                    .unwrap_or(chrono_tz::UTC);

                let now_in_tz = now.with_timezone(&timezone);
                schedule
                    .after(&now_in_tz)
                    .next()
                    .map(|next| next.with_timezone(&Utc))
            } else {
                None
            };

        // Now update the job
        let job = storage
            .jobs
            .get_mut(&job_id)
            .ok_or_else(|| HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;

        job.recurring = true;
        job.next_run_at = next_run_at;

        Ok(())
    }

    // Throttling configuration
    async fn set_throttle_config(&self, queue_name: &str, config: ThrottleConfig) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage
            .throttle_configs
            .insert(queue_name.to_string(), config);
        Ok(())
    }

    async fn get_throttle_config(&self, queue_name: &str) -> Result<Option<ThrottleConfig>> {
        let storage = self.storage.read().await;
        Ok(storage.throttle_configs.get(queue_name).cloned())
    }

    async fn remove_throttle_config(&self, queue_name: &str) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage.throttle_configs.remove(queue_name);
        Ok(())
    }

    async fn get_all_throttle_configs(&self) -> Result<HashMap<String, ThrottleConfig>> {
        let storage = self.storage.read().await;
        Ok(storage.throttle_configs.clone())
    }

    async fn get_queue_depth(&self, queue_name: &str) -> Result<u64> {
        let storage = self.storage.read().await;

        let depth = storage
            .queues
            .get(queue_name)
            .and_then(|q| q.get(&JobStatus::Pending))
            .map(|jobs| jobs.len() as u64)
            .unwrap_or(0);

        Ok(depth)
    }

    // Job result storage
    async fn store_job_result(
        &self,
        job_id: JobId,
        result_data: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let mut storage = self.storage.write().await;

        // Verify job exists
        if !storage.jobs.contains_key(&job_id) {
            return Err(HammerworkError::JobNotFound {
                id: job_id.to_string(),
            });
        }

        storage
            .job_results
            .insert(job_id, (result_data, expires_at));
        Ok(())
    }

    async fn get_job_result(&self, job_id: JobId) -> Result<Option<serde_json::Value>> {
        let storage = self.storage.read().await;

        if let Some((result, expires_at)) = storage.job_results.get(&job_id) {
            // Check if expired
            if let Some(exp) = expires_at {
                if *exp <= storage.clock.now() {
                    return Ok(None);
                }
            }
            Ok(Some(result.clone()))
        } else {
            Ok(None)
        }
    }

    async fn delete_job_result(&self, job_id: JobId) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage.job_results.remove(&job_id);
        Ok(())
    }

    async fn cleanup_expired_results(&self) -> Result<u64> {
        let mut storage = self.storage.write().await;
        let now = storage.clock.now();

        let expired_ids: Vec<JobId> = storage
            .job_results
            .iter()
            .filter(|(_, (_, expires_at))| expires_at.map(|exp| exp <= now).unwrap_or(false))
            .map(|(id, _)| *id)
            .collect();

        let count = expired_ids.len() as u64;

        for id in expired_ids {
            storage.job_results.remove(&id);
        }

        Ok(count)
    }

    // Workflow and dependency management
    async fn enqueue_workflow(&self, mut workflow: JobGroup) -> Result<WorkflowId> {
        let mut storage = self.storage.write().await;

        workflow.created_at = storage.clock.now();
        let workflow_id = workflow.id;

        // Store dependencies
        for (job_id, deps) in &workflow.dependencies {
            storage.dependencies.insert(*job_id, deps.clone());

            // Store reverse dependencies
            for dep_id in deps {
                storage.dependents.entry(*dep_id).or_default().push(*job_id);
            }
        }

        // Enqueue all jobs
        for mut job in workflow.jobs.iter().cloned() {
            // Apply the same timestamp logic as regular enqueue
            let now = storage.clock.now();
            let original_scheduled_at = job.scheduled_at;
            let original_created_at = job.created_at;

            // Calculate the delay from the original creation
            let delay = original_scheduled_at - original_created_at;

            job.created_at = now;

            // Check if job has dependencies
            let has_deps = workflow
                .dependencies
                .get(&job.id)
                .map(|deps| !deps.is_empty())
                .unwrap_or(false);

            // If job has dependencies, don't make it immediately available
            if has_deps {
                job.scheduled_at = DateTime::<Utc>::MAX_UTC; // Far future
            } else {
                // For jobs without dependencies, apply normal timestamp logic
                if original_scheduled_at == original_created_at {
                    job.scheduled_at = now;
                } else {
                    // For delayed jobs, preserve the delay but use mock clock time as base
                    job.scheduled_at = now + delay;
                }
            }

            storage.jobs.insert(job.id, job.clone());
            storage.add_job_to_queue(&job);
        }

        storage.workflows.insert(workflow_id, workflow);
        Ok(workflow_id)
    }

    async fn get_workflow_status(&self, workflow_id: WorkflowId) -> Result<Option<JobGroup>> {
        let storage = self.storage.read().await;

        if let Some(mut workflow) = storage.workflows.get(&workflow_id).cloned() {
            // Update workflow status based on job statuses
            let mut completed = 0;
            let mut failed = 0;

            for job_id in workflow.jobs.iter().map(|j| j.id) {
                if let Some(job) = storage.jobs.get(&job_id) {
                    match &job.status {
                        JobStatus::Completed => completed += 1,
                        JobStatus::Failed | JobStatus::Dead | JobStatus::TimedOut => failed += 1,
                        _ => {}
                    }
                }
            }

            workflow.completed_jobs = completed;
            workflow.failed_jobs = failed;

            // Only update status if it's not already set to a terminal state
            if workflow.status != WorkflowStatus::Cancelled {
                if failed > 0 {
                    workflow.status = WorkflowStatus::Failed;
                    workflow.failed_at = Some(storage.clock.now());
                } else if completed == workflow.total_jobs {
                    workflow.status = WorkflowStatus::Completed;
                    workflow.completed_at = Some(storage.clock.now());
                } else {
                    workflow.status = WorkflowStatus::Running;
                }
            }

            Ok(Some(workflow))
        } else {
            Ok(None)
        }
    }

    async fn resolve_job_dependencies(&self, completed_job_id: JobId) -> Result<Vec<JobId>> {
        let mut storage = self.storage.write().await;
        let mut resolved_jobs = Vec::new();

        if let Some(dependents) = storage.dependents.get(&completed_job_id).cloned() {
            for dependent_id in dependents {
                // Check if all dependencies are satisfied
                if let Some(deps) = storage.dependencies.get(&dependent_id) {
                    let all_complete = deps.iter().all(|dep_id| {
                        storage
                            .jobs
                            .get(dep_id)
                            .map(|j| j.status == JobStatus::Completed)
                            .unwrap_or(false)
                    });

                    if all_complete {
                        // Make the job eligible for execution
                        let now = storage.clock.now();
                        if let Some(job) = storage.jobs.get_mut(&dependent_id) {
                            if job.status == JobStatus::Pending
                                && job.scheduled_at == DateTime::<Utc>::MAX_UTC
                            {
                                job.scheduled_at = now;
                                resolved_jobs.push(dependent_id);
                            }
                        }
                    }
                }
            }
        }

        Ok(resolved_jobs)
    }

    async fn get_ready_jobs(&self, queue_name: &str, limit: u32) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;
        let now = storage.clock.now();

        let ready_jobs: Vec<Job> = storage
            .jobs
            .values()
            .filter(|job| {
                job.queue_name == queue_name &&
                job.status == JobStatus::Pending &&
                job.scheduled_at <= now &&
                // Check that all dependencies are satisfied
                storage.dependencies.get(&job.id)
                    .map(|deps| deps.is_empty() || deps.iter().all(|dep_id| {
                        storage.jobs.get(dep_id)
                            .map(|j| j.status == JobStatus::Completed)
                            .unwrap_or(false)
                    }))
                    .unwrap_or(true)
            })
            .take(limit as usize)
            .cloned()
            .collect();

        Ok(ready_jobs)
    }

    async fn fail_job_dependencies(&self, failed_job_id: JobId) -> Result<Vec<JobId>> {
        let mut storage = self.storage.write().await;
        let mut failed_jobs = Vec::new();

        // Get the workflow this job belongs to
        let workflow = storage
            .workflows
            .values()
            .find(|w| w.jobs.iter().any(|j| j.id == failed_job_id))
            .cloned();

        if let Some(workflow) = workflow {
            // Check failure policy
            match workflow.failure_policy {
                crate::workflow::FailurePolicy::ContinueOnFailure => {
                    // Don't fail dependencies, continue with independent jobs
                }
                crate::workflow::FailurePolicy::Manual => {
                    // Don't automatically fail dependencies, wait for manual intervention
                }
                crate::workflow::FailurePolicy::FailFast => {
                    // Fail all pending jobs in the workflow
                    let current_time = storage.clock.now();
                    for job in &workflow.jobs {
                        if let Some(j) = storage.jobs.get(&job.id) {
                            if j.status == JobStatus::Pending {
                                storage.update_job_status(job.id, JobStatus::Failed).ok();
                                if let Some(j) = storage.jobs.get_mut(&job.id) {
                                    j.error_message = Some("Dependency failed".to_string());
                                    j.failed_at = Some(current_time);
                                }
                                failed_jobs.push(job.id);
                            }
                        }
                    }
                }
            }
        }

        Ok(failed_jobs)
    }

    async fn get_workflow_jobs(&self, workflow_id: WorkflowId) -> Result<Vec<Job>> {
        let storage = self.storage.read().await;

        let workflow =
            storage
                .workflows
                .get(&workflow_id)
                .ok_or_else(|| HammerworkError::Workflow {
                    message: format!("Workflow {} not found", workflow_id),
                })?;

        let jobs: Vec<Job> = workflow
            .jobs
            .iter()
            .filter_map(|job| storage.jobs.get(&job.id).cloned())
            .collect();

        Ok(jobs)
    }

    async fn cancel_workflow(&self, workflow_id: WorkflowId) -> Result<()> {
        let mut storage = self.storage.write().await;

        let workflow = storage
            .workflows
            .get(&workflow_id)
            .ok_or_else(|| HammerworkError::Workflow {
                message: format!("Workflow {} not found", workflow_id),
            })?
            .clone();

        // Cancel all pending jobs
        let now = storage.clock.now();
        for job in &workflow.jobs {
            if let Some(j) = storage.jobs.get(&job.id) {
                if j.status == JobStatus::Pending || j.status == JobStatus::Running {
                    storage.update_job_status(job.id, JobStatus::Failed).ok();
                    if let Some(j) = storage.jobs.get_mut(&job.id) {
                        j.error_message = Some("Workflow cancelled".to_string());
                        j.failed_at = Some(now);
                    }
                }
            }
        }

        // Update workflow status
        if let Some(workflow) = storage.workflows.get_mut(&workflow_id) {
            workflow.status = WorkflowStatus::Cancelled;
            workflow.failed_at = Some(now);
        }

        Ok(())
    }

    // Archival methods
    async fn archive_jobs(
        &self,
        queue_name: Option<&str>,
        policy: &crate::archive::ArchivalPolicy,
        _config: &crate::archive::ArchivalConfig,
        _reason: crate::archive::ArchivalReason,
        _archived_by: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats> {
        let mut storage = self.storage.write().await;
        let now = storage.clock.now();
        let mut jobs_archived = 0u64;
        let mut bytes_archived = 0u64;

        // Find jobs to archive based on policy
        let mut jobs_to_archive = Vec::new();

        for job in storage.jobs.values() {
            // Skip if queue_name filter doesn't match
            if let Some(queue) = queue_name {
                if job.queue_name != queue {
                    continue;
                }
            }

            // Check if job should be archived based on policy and status
            let age = match job.status {
                JobStatus::Completed => job.completed_at.map(|t| now - t),
                JobStatus::Failed => job.failed_at.map(|t| now - t),
                JobStatus::Dead => job.failed_at.map(|t| now - t), // Use failed_at for dead jobs
                JobStatus::TimedOut => job.timed_out_at.map(|t| now - t),
                _ => None,
            };

            if let Some(age) = age {
                if policy.should_archive(&job.status, age) {
                    jobs_to_archive.push(job.id);
                }
            }
        }

        // Archive jobs
        for job_id in jobs_to_archive.iter().take(policy.batch_size) {
            if let Some(job) = storage.jobs.get_mut(job_id) {
                // Calculate payload size for stats
                let payload_size = serde_json::to_string(&job.payload)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0);

                // Update job status
                job.status = JobStatus::Archived;
                // In a real implementation, we'd move to archive table and compress

                jobs_archived += 1;
                bytes_archived += payload_size;
            }
        }

        // Update queue structure separately to avoid borrow checker issues
        let queue_names: Vec<_> = jobs_to_archive
            .iter()
            .take(policy.batch_size)
            .filter_map(|job_id| {
                storage
                    .jobs
                    .get(job_id)
                    .map(|job| (*job_id, job.queue_name.clone()))
            })
            .collect();

        for (job_id, queue_name) in queue_names {
            if let Some(queue_jobs) = storage.queues.get_mut(&queue_name) {
                // Remove from old status list
                for jobs_list in queue_jobs.values_mut() {
                    jobs_list.retain(|&id| id != job_id);
                }
                // Add to archived list
                queue_jobs
                    .entry(JobStatus::Archived)
                    .or_default()
                    .push(job_id);
            }
        }

        // Calculate compression ratio (mock)
        let compression_ratio = if policy.compress_payloads { 0.7 } else { 1.0 };

        Ok(crate::archive::ArchivalStats {
            jobs_archived,
            jobs_purged: 0,
            bytes_archived,
            bytes_purged: 0,
            compression_ratio,
            last_run_at: now,
            operation_duration: std::time::Duration::from_millis(100), // Mock duration
        })
    }

    async fn restore_archived_job(&self, job_id: JobId) -> Result<Job> {
        let mut storage = self.storage.write().await;

        if let Some(job) = storage.jobs.get_mut(&job_id) {
            if job.status == JobStatus::Archived {
                let queue_name = job.queue_name.clone(); // Clone to avoid borrow checker issues

                // Restore job to pending status
                job.status = JobStatus::Pending;
                let restored_job = job.clone();

                // Update queue structure
                if let Some(queue_jobs) = storage.queues.get_mut(&queue_name) {
                    // Remove from archived list
                    if let Some(archived_jobs) = queue_jobs.get_mut(&JobStatus::Archived) {
                        archived_jobs.retain(|&id| id != job_id);
                    }
                    // Add to pending list
                    queue_jobs
                        .entry(JobStatus::Pending)
                        .or_default()
                        .push(job_id);
                }

                Ok(restored_job)
            } else {
                Err(crate::HammerworkError::Queue {
                    message: format!("Job {} is not archived", job_id),
                })
            }
        } else {
            Err(crate::HammerworkError::Queue {
                message: format!("Job {} not found", job_id),
            })
        }
    }

    async fn list_archived_jobs(
        &self,
        queue_name: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<crate::archive::ArchivedJob>> {
        let storage = self.storage.read().await;
        let now = storage.clock.now();

        let mut archived_jobs = Vec::new();

        for job in storage.jobs.values() {
            if job.status != JobStatus::Archived {
                continue;
            }

            // Filter by queue if specified
            if let Some(queue) = queue_name {
                if job.queue_name != queue {
                    continue;
                }
            }

            let payload_size = serde_json::to_string(&job.payload)
                .map(|s| s.len())
                .unwrap_or(0);

            archived_jobs.push(crate::archive::ArchivedJob {
                id: job.id,
                queue_name: job.queue_name.clone(),
                status: job.status,
                created_at: job.created_at,
                archived_at: now, // Mock archived time
                archival_reason: crate::archive::ArchivalReason::Manual, // Mock reason
                original_payload_size: Some(payload_size),
                payload_compressed: false,             // Mock
                archived_by: Some("test".to_string()), // Mock user
            });
        }

        // Apply pagination
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(100) as usize;

        archived_jobs.sort_by(|a, b| b.archived_at.cmp(&a.archived_at));

        if offset < archived_jobs.len() {
            let end = std::cmp::min(offset + limit, archived_jobs.len());
            Ok(archived_jobs[offset..end].to_vec())
        } else {
            Ok(Vec::new())
        }
    }

    async fn purge_archived_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let mut storage = self.storage.write().await;
        let mut purged_count = 0u64;

        // Find archived jobs older than cutoff
        let jobs_to_purge: Vec<JobId> = storage
            .jobs
            .values()
            .filter(|job| {
                job.status == JobStatus::Archived && job.created_at < older_than // Mock: use created_at as archived_at
            })
            .map(|job| job.id)
            .collect();

        // Remove jobs
        for job_id in jobs_to_purge {
            storage.jobs.remove(&job_id);
            purged_count += 1;

            // Clean up from queue structure
            for queue_jobs in storage.queues.values_mut() {
                for jobs_list in queue_jobs.values_mut() {
                    jobs_list.retain(|&id| id != job_id);
                }
            }
        }

        Ok(purged_count)
    }

    async fn get_archival_stats(
        &self,
        queue_name: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats> {
        let storage = self.storage.read().await;
        let now = storage.clock.now();

        let mut jobs_archived = 0u64;
        let mut bytes_archived = 0u64;

        for job in storage.jobs.values() {
            if job.status != JobStatus::Archived {
                continue;
            }

            // Filter by queue if specified
            if let Some(queue) = queue_name {
                if job.queue_name != queue {
                    continue;
                }
            }

            jobs_archived += 1;
            bytes_archived += serde_json::to_string(&job.payload)
                .map(|s| s.len() as u64)
                .unwrap_or(0);
        }

        Ok(crate::archive::ArchivalStats {
            jobs_archived,
            jobs_purged: 0,
            bytes_archived,
            bytes_purged: 0,
            compression_ratio: 0.8, // Mock compression ratio
            last_run_at: now,
            operation_duration: std::time::Duration::from_millis(50),
        })
    }

    async fn pause_queue(&self, queue_name: &str, paused_by: Option<&str>) -> Result<()> {
        let mut storage = self.storage.write().await;
        let now = storage.clock.now();
        
        let pause_info = QueuePauseInfo {
            queue_name: queue_name.to_string(),
            paused_at: now,
            paused_by: paused_by.map(|s| s.to_string()),
            reason: None,
        };
        
        storage.paused_queues.insert(queue_name.to_string(), pause_info);
        Ok(())
    }

    async fn resume_queue(&self, queue_name: &str, _resumed_by: Option<&str>) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage.paused_queues.remove(queue_name);
        Ok(())
    }

    async fn is_queue_paused(&self, queue_name: &str) -> Result<bool> {
        let storage = self.storage.read().await;
        Ok(storage.paused_queues.contains_key(queue_name))
    }

    async fn get_queue_pause_info(&self, queue_name: &str) -> Result<Option<QueuePauseInfo>> {
        let storage = self.storage.read().await;
        Ok(storage.paused_queues.get(queue_name).cloned())
    }

    async fn get_paused_queues(&self) -> Result<Vec<QueuePauseInfo>> {
        let storage = self.storage.read().await;
        Ok(storage.paused_queues.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_basic_enqueue_dequeue() {
        let queue = TestQueue::new();

        // Enqueue a job
        let job = Job::new("test_queue".to_string(), json!({"test": true}));
        let job_id = queue.enqueue(job).await.unwrap();

        // Verify job count
        assert_eq!(
            queue.get_job_count("test_queue", &JobStatus::Pending).await,
            1
        );

        // Dequeue the job
        let dequeued = queue.dequeue("test_queue").await.unwrap();
        assert!(dequeued.is_some());
        assert_eq!(dequeued.unwrap().id, job_id);

        // Verify job is now running
        assert_eq!(
            queue.get_job_count("test_queue", &JobStatus::Running).await,
            1
        );
        assert_eq!(
            queue.get_job_count("test_queue", &JobStatus::Pending).await,
            0
        );
    }

    #[tokio::test]
    async fn test_priority_dequeue() {
        let queue = TestQueue::new();

        // Enqueue jobs with different priorities
        let low_job =
            Job::new("test_queue".to_string(), json!({"priority": "low"})).as_low_priority();
        let high_job =
            Job::new("test_queue".to_string(), json!({"priority": "high"})).as_high_priority();
        let normal_job = Job::new("test_queue".to_string(), json!({"priority": "normal"}));

        let _low_id = queue.enqueue(low_job).await.unwrap();
        let high_id = queue.enqueue(high_job).await.unwrap();
        let _normal_id = queue.enqueue(normal_job).await.unwrap();

        // Dequeue should get the high priority job first
        let dequeued = queue.dequeue("test_queue").await.unwrap();
        assert_eq!(dequeued.unwrap().id, high_id);
    }

    #[tokio::test]
    async fn test_delayed_job() {
        let queue = TestQueue::new();
        let clock = queue.clock();

        // Enqueue a delayed job
        let delay = chrono::Duration::hours(1);
        let job = Job::with_delay("test_queue".to_string(), json!({"delayed": true}), delay);
        let job_id = queue.enqueue(job).await.unwrap();

        // Should not be able to dequeue immediately
        let dequeued = queue.dequeue("test_queue").await.unwrap();
        assert!(dequeued.is_none());

        // Advance time
        clock.advance(chrono::Duration::hours(2));

        // Now should be able to dequeue
        let dequeued = queue.dequeue("test_queue").await.unwrap();
        assert_eq!(dequeued.unwrap().id, job_id);
    }

    #[tokio::test]
    async fn test_job_completion() {
        let queue = TestQueue::new();

        // Enqueue and dequeue a job
        let job = Job::new("test_queue".to_string(), json!({"test": true}));
        let job_id = queue.enqueue(job).await.unwrap();
        let _dequeued = queue.dequeue("test_queue").await.unwrap();

        // Complete the job
        queue.complete_job(job_id).await.unwrap();

        // Verify job is completed
        let completed_job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(completed_job.status, JobStatus::Completed);
        assert!(completed_job.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_job_failure_and_retry() {
        let queue = TestQueue::new();

        // Enqueue a job with multiple retry attempts
        let job = Job::new("test_queue".to_string(), json!({"test": true})).with_max_attempts(3);
        let job_id = queue.enqueue(job).await.unwrap();

        // Dequeue and fail the job
        let _dequeued = queue.dequeue("test_queue").await.unwrap();
        queue.fail_job(job_id, "Test error").await.unwrap();

        // Job should be in retrying status
        let job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Retrying);
        assert_eq!(job.attempts, 1);

        // Retry the job
        let retry_at = queue.clock().now() + chrono::Duration::minutes(5);
        queue.retry_job(job_id, retry_at).await.unwrap();

        // Job should be pending again
        let job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.scheduled_at, retry_at);
    }

    #[tokio::test]
    async fn test_batch_operations() {
        let queue = TestQueue::new();

        // Create a batch of jobs
        let jobs = vec![
            Job::new("batch_queue".to_string(), json!({"batch": 1})),
            Job::new("batch_queue".to_string(), json!({"batch": 2})),
            Job::new("batch_queue".to_string(), json!({"batch": 3})),
        ];

        let batch = JobBatch::new("test_batch".to_string()).with_jobs(jobs);
        let batch_id = queue.enqueue_batch(batch).await.unwrap();

        // Get batch status
        let status = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(status.total_jobs, 3);
        assert_eq!(status.pending_jobs, 3);
        assert_eq!(status.status, BatchStatus::Processing);

        // Process all jobs
        for _ in 0..3 {
            if let Some(job) = queue.dequeue("batch_queue").await.unwrap() {
                queue.complete_job(job.id).await.unwrap();
            }
        }

        // Check batch is completed
        let status = queue.get_batch_status(batch_id).await.unwrap();
        assert_eq!(status.completed_jobs, 3);
        assert_eq!(status.status, BatchStatus::Completed);
    }

    #[tokio::test]
    async fn test_workflow_with_dependencies() {
        let queue = TestQueue::new();

        // Create jobs
        let job1 = Job::new("workflow_queue".to_string(), json!({"step": 1}));
        let job2 = Job::new("workflow_queue".to_string(), json!({"step": 2}));
        let job3 = Job::new("workflow_queue".to_string(), json!({"step": 3}));

        let job1_id = job1.id;
        let job2_id = job2.id;
        let job3_id = job3.id;

        // Create workflow with dependencies: job2 depends on job1, job3 depends on job2
        let mut dependencies = HashMap::new();
        dependencies.insert(job2_id, vec![job1_id]);
        dependencies.insert(job3_id, vec![job2_id]);

        let mut workflow = JobGroup::new("test_workflow".to_string());
        workflow.jobs = vec![job1, job2, job3];
        workflow.total_jobs = 3;
        workflow.dependencies = dependencies;

        let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();

        // Only job1 should be available initially
        let dequeued = queue.dequeue("workflow_queue").await.unwrap();
        assert_eq!(dequeued.unwrap().id, job1_id);

        // Job2 and job3 should not be available
        assert!(queue.dequeue("workflow_queue").await.unwrap().is_none());

        // Complete job1
        queue.complete_job(job1_id).await.unwrap();

        // Now job2 should be available
        let dequeued = queue.dequeue("workflow_queue").await.unwrap();
        assert_eq!(dequeued.unwrap().id, job2_id);

        // Complete job2
        queue.complete_job(job2_id).await.unwrap();

        // Now job3 should be available
        let dequeued = queue.dequeue("workflow_queue").await.unwrap();
        assert_eq!(dequeued.unwrap().id, job3_id);

        // Complete job3
        queue.complete_job(job3_id).await.unwrap();

        // Check workflow status
        let workflow_status = queue
            .get_workflow_status(workflow_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(workflow_status.status, WorkflowStatus::Completed);
        assert_eq!(workflow_status.completed_jobs, 3);
    }

    #[tokio::test]
    async fn test_cron_job() {
        let queue = TestQueue::new();
        let clock = queue.clock();

        // Create a cron job that runs every hour
        let mut job = Job::new("cron_queue".to_string(), json!({"cron": true}));
        job.cron_schedule = Some("0 0 * * * *".to_string());

        let job_id = queue.enqueue_cron_job(job).await.unwrap();

        // Get the job
        let job = queue.get_job(job_id).await.unwrap().unwrap();
        assert!(job.recurring);
        assert!(job.next_run_at.is_some());

        // Advance clock to make the cron job ready for execution
        // Since the cron is "0 0 * * * *" (every hour at minute 0),
        // advance to the next hour boundary
        clock.advance(chrono::Duration::hours(1));

        // Now the job should be available for dequeue
        let dequeued = queue.dequeue("cron_queue").await.unwrap();
        assert!(dequeued.is_some());
        queue.complete_job(job_id).await.unwrap();

        // Job should have next_run_at updated
        let job = queue.get_job(job_id).await.unwrap().unwrap();
        assert!(job.next_run_at.is_some());

        // Advance time to next run
        clock.advance(chrono::Duration::hours(2));

        // Get due cron jobs
        let due_jobs = queue.get_due_cron_jobs(Some("cron_queue")).await.unwrap();
        assert_eq!(due_jobs.len(), 1);
        assert_eq!(due_jobs[0].id, job_id);

        // Reschedule for next run
        let next_run = clock.now() + chrono::Duration::hours(1);
        queue.reschedule_cron_job(job_id, next_run).await.unwrap();

        // Should have created a new job instance
        assert_eq!(
            queue.get_job_count("cron_queue", &JobStatus::Pending).await,
            1
        );
    }

    #[tokio::test]
    async fn test_job_result_storage() {
        let queue = TestQueue::new();
        let clock = queue.clock();

        // Create and enqueue a job
        let job = Job::new("result_queue".to_string(), json!({"test": true}));
        let job_id = queue.enqueue(job).await.unwrap();

        // Store a result
        let result = json!({"success": true, "data": "test result"});
        let expires_at = clock.now() + chrono::Duration::hours(1);
        queue
            .store_job_result(job_id, result.clone(), Some(expires_at))
            .await
            .unwrap();

        // Retrieve the result
        let retrieved = queue.get_job_result(job_id).await.unwrap();
        assert_eq!(retrieved, Some(result));

        // Advance time past expiration
        clock.advance(chrono::Duration::hours(2));

        // Result should be expired
        let retrieved = queue.get_job_result(job_id).await.unwrap();
        assert_eq!(retrieved, None);

        // Cleanup expired results
        let cleaned = queue.cleanup_expired_results().await.unwrap();
        assert_eq!(cleaned, 1);
    }

    #[tokio::test]
    async fn test_queue_stats() {
        let queue = TestQueue::new();

        // Enqueue various jobs
        for i in 0..5 {
            let job = Job::new("stats_queue".to_string(), json!({"index": i}));
            queue.enqueue(job).await.unwrap();
        }

        // Process some jobs
        for _ in 0..2 {
            if let Some(job) = queue.dequeue("stats_queue").await.unwrap() {
                queue.complete_job(job.id).await.unwrap();
            }
        }

        // Fail one job
        if let Some(job) = queue.dequeue("stats_queue").await.unwrap() {
            queue.fail_job(job.id, "Test failure").await.unwrap();
            // Exhaust retries
            queue.fail_job(job.id, "Test failure").await.unwrap();
            queue.fail_job(job.id, "Test failure").await.unwrap();
        }

        // Get stats
        let stats = queue.get_queue_stats("stats_queue").await.unwrap();

        assert_eq!(stats.pending_count, 2);
        assert_eq!(stats.statistics.completed, 2);
        assert_eq!(stats.statistics.dead, 1); // One job marked as dead
        assert_eq!(stats.statistics.total_processed, 3); // 2 completed + 1 dead
    }
}
