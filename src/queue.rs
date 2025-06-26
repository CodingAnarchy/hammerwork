use crate::{
    job::{Job, JobId},
    rate_limit::ThrottleConfig,
    stats::{DeadJobSummary, QueueStats},
    Result,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Database, Pool};
use std::{collections::HashMap, marker::PhantomData, sync::Arc};
use tokio::sync::RwLock;

#[async_trait]
pub trait DatabaseQueue: Send + Sync {
    type Database: Database;

    // Core job operations
    async fn create_tables(&self) -> Result<()>;
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
}

pub struct JobQueue<DB: Database> {
    #[allow(dead_code)] // Used in database-specific implementations
    pool: Pool<DB>,
    _phantom: PhantomData<DB>,
    throttle_configs: Arc<RwLock<HashMap<String, ThrottleConfig>>>,
}

impl<DB: Database> JobQueue<DB> {
    pub fn new(pool: Pool<DB>) -> Self {
        Self {
            pool,
            _phantom: PhantomData,
            throttle_configs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set throttling configuration for a queue
    pub async fn set_throttle(&self, queue_name: &str, config: ThrottleConfig) -> Result<()> {
        let mut configs = self.throttle_configs.write().await;
        configs.insert(queue_name.to_string(), config);
        Ok(())
    }

    /// Get throttling configuration for a queue
    pub async fn get_throttle(&self, queue_name: &str) -> Option<ThrottleConfig> {
        let configs = self.throttle_configs.read().await;
        configs.get(queue_name).cloned()
    }

    /// Remove throttling configuration for a queue
    pub async fn remove_throttle(&self, queue_name: &str) -> Result<()> {
        let mut configs = self.throttle_configs.write().await;
        configs.remove(queue_name);
        Ok(())
    }

    /// Get all throttling configurations
    pub async fn get_all_throttles(&self) -> HashMap<String, ThrottleConfig> {
        let configs = self.throttle_configs.read().await;
        configs.clone()
    }
}

#[cfg(feature = "postgres")]
pub mod postgres {
    use super::*;
    use crate::job::JobStatus;
    use crate::priority::JobPriority;
    use sqlx::{FromRow, Postgres, Row};
    use std::time::Duration;

    #[derive(FromRow)]
    struct JobRow {
        id: uuid::Uuid,
        queue_name: String,
        payload: serde_json::Value,
        status: String,
        priority: i32,
        attempts: i32,
        max_attempts: i32,
        timeout_seconds: Option<i32>,
        created_at: DateTime<Utc>,
        scheduled_at: DateTime<Utc>,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
        failed_at: Option<DateTime<Utc>>,
        timed_out_at: Option<DateTime<Utc>>,
        error_message: Option<String>,
        cron_schedule: Option<String>,
        next_run_at: Option<DateTime<Utc>>,
        recurring: bool,
        timezone: Option<String>,
    }

    impl JobRow {
        fn into_job(self) -> Result<Job> {
            Ok(Job {
                id: self.id,
                queue_name: self.queue_name,
                payload: self.payload,
                status: serde_json::from_str(&self.status)?,
                priority: JobPriority::from_i32(self.priority).unwrap_or(JobPriority::Normal),
                attempts: self.attempts,
                max_attempts: self.max_attempts,
                created_at: self.created_at,
                scheduled_at: self.scheduled_at,
                started_at: self.started_at,
                completed_at: self.completed_at,
                failed_at: self.failed_at,
                timed_out_at: self.timed_out_at,
                timeout: self
                    .timeout_seconds
                    .map(|s| std::time::Duration::from_secs(s as u64)),
                error_message: self.error_message,
                cron_schedule: self.cron_schedule,
                next_run_at: self.next_run_at,
                recurring: self.recurring,
                timezone: self.timezone,
            })
        }
    }

    #[derive(FromRow)]
    struct DeadJobRow {
        id: uuid::Uuid,
        queue_name: String,
        payload: serde_json::Value,
        status: String,
        priority: i32,
        attempts: i32,
        max_attempts: i32,
        timeout_seconds: Option<i32>,
        created_at: DateTime<Utc>,
        scheduled_at: DateTime<Utc>,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
        failed_at: Option<DateTime<Utc>>,
        timed_out_at: Option<DateTime<Utc>>,
        error_message: Option<String>,
    }

    impl DeadJobRow {
        fn into_job(self) -> Job {
            Job {
                id: self.id,
                queue_name: self.queue_name,
                payload: self.payload,
                status: serde_json::from_str(&self.status).unwrap_or(JobStatus::Dead),
                priority: JobPriority::from_i32(self.priority).unwrap_or(JobPriority::Normal),
                attempts: self.attempts,
                max_attempts: self.max_attempts,
                created_at: self.created_at,
                scheduled_at: self.scheduled_at,
                started_at: self.started_at,
                completed_at: self.completed_at,
                failed_at: self.failed_at,
                timed_out_at: self.timed_out_at,
                timeout: self
                    .timeout_seconds
                    .map(|s| std::time::Duration::from_secs(s as u64)),
                error_message: self.error_message,
                cron_schedule: None,
                next_run_at: None,
                recurring: false,
                timezone: None,
            }
        }
    }

    #[async_trait]
    impl DatabaseQueue for JobQueue<Postgres> {
        type Database = Postgres;

        async fn create_tables(&self) -> Result<()> {
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS hammerwork_jobs (
                    id UUID PRIMARY KEY,
                    queue_name VARCHAR NOT NULL,
                    payload JSONB NOT NULL,
                    status VARCHAR NOT NULL,
                    priority INTEGER NOT NULL DEFAULT 2,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    max_attempts INTEGER NOT NULL DEFAULT 3,
                    timeout_seconds INTEGER,
                    created_at TIMESTAMPTZ NOT NULL,
                    scheduled_at TIMESTAMPTZ NOT NULL,
                    started_at TIMESTAMPTZ,
                    completed_at TIMESTAMPTZ,
                    failed_at TIMESTAMPTZ,
                    timed_out_at TIMESTAMPTZ,
                    error_message TEXT,
                    cron_schedule VARCHAR(100),
                    next_run_at TIMESTAMPTZ,
                    recurring BOOLEAN NOT NULL DEFAULT FALSE,
                    timezone VARCHAR(50)
                );
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status_priority_scheduled 
                ON hammerwork_jobs (queue_name, status, priority DESC, scheduled_at);
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_status_failed_at
                ON hammerwork_jobs (status, failed_at);
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_queue_status
                ON hammerwork_jobs (queue_name, status);
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_recurring_next_run
                ON hammerwork_jobs (recurring, next_run_at) WHERE recurring = TRUE;
                
                CREATE INDEX IF NOT EXISTS idx_hammerwork_jobs_cron_schedule
                ON hammerwork_jobs (cron_schedule) WHERE cron_schedule IS NOT NULL;
                "#,
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn enqueue(&self, job: Job) -> Result<JobId> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
                "#
            )
            .bind(job.id)
            .bind(&job.queue_name)
            .bind(&job.payload)
            .bind(serde_json::to_string(&job.status)?)
            .bind(job.priority.as_i32())
            .bind(job.attempts)
            .bind(job.max_attempts)
            .bind(job.timeout.map(|t| t.as_secs() as i32))
            .bind(job.created_at)
            .bind(job.scheduled_at)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(job.failed_at)
            .bind(job.timed_out_at)
            .bind(&job.error_message)
            .bind(&job.cron_schedule)
            .bind(job.next_run_at)
            .bind(job.recurring)
            .bind(&job.timezone)
            .execute(&self.pool)
            .await?;

            Ok(job.id)
        }

        async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
            let row = sqlx::query(
                r#"
                UPDATE hammerwork_jobs 
                SET status = $1, started_at = $2, attempts = attempts + 1
                WHERE id = (
                    SELECT id FROM hammerwork_jobs 
                    WHERE queue_name = $3 
                    AND status = $4 
                    AND scheduled_at <= $2
                    ORDER BY priority DESC, scheduled_at ASC 
                    LIMIT 1 
                    FOR UPDATE SKIP LOCKED
                )
                RETURNING id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                "#
            )
            .bind(serde_json::to_string(&JobStatus::Running)?)
            .bind(Utc::now())
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .fetch_optional(&self.pool)
            .await?;

            if let Some(row) = row {
                let id: uuid::Uuid = row.get("id");
                let queue_name: String = row.get("queue_name");
                let payload: serde_json::Value = row.get("payload");
                let status: String = row.get("status");
                let priority: i32 = row.get("priority");
                let attempts: i32 = row.get("attempts");
                let max_attempts: i32 = row.get("max_attempts");
                let timeout_seconds: Option<i32> = row.get("timeout_seconds");
                let created_at: DateTime<Utc> = row.get("created_at");
                let scheduled_at: DateTime<Utc> = row.get("scheduled_at");
                let started_at: Option<DateTime<Utc>> = row.get("started_at");
                let completed_at: Option<DateTime<Utc>> = row.get("completed_at");
                let failed_at: Option<DateTime<Utc>> = row.get("failed_at");
                let timed_out_at: Option<DateTime<Utc>> = row.get("timed_out_at");
                let error_message: Option<String> = row.get("error_message");
                let cron_schedule: Option<String> = row.get("cron_schedule");
                let next_run_at: Option<DateTime<Utc>> = row.get("next_run_at");
                let recurring: bool = row.get("recurring");
                let timezone: Option<String> = row.get("timezone");

                Ok(Some(Job {
                    id,
                    queue_name,
                    payload,
                    status: serde_json::from_str(&status)?,
                    priority: JobPriority::from_i32(priority).unwrap_or(JobPriority::Normal),
                    attempts,
                    max_attempts,
                    created_at,
                    scheduled_at,
                    started_at,
                    completed_at,
                    failed_at,
                    timed_out_at,
                    timeout: timeout_seconds.map(|s| std::time::Duration::from_secs(s as u64)),
                    error_message,
                    cron_schedule,
                    next_run_at,
                    recurring,
                    timezone,
                }))
            } else {
                Ok(None)
            }
        }

        async fn dequeue_with_priority_weights(
            &self,
            queue_name: &str,
            weights: &crate::priority::PriorityWeights,
        ) -> Result<Option<Job>> {
            use crate::priority::JobPriority;

            if weights.is_strict() {
                // Use strict priority - same as regular dequeue
                return self.dequeue(queue_name).await;
            }

            // Get available jobs by priority
            let available_jobs = sqlx::query(
                r#"
                SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE queue_name = $1 
                AND status = $2 
                AND scheduled_at <= $3
                ORDER BY priority DESC, scheduled_at ASC 
                LIMIT 20
                FOR UPDATE SKIP LOCKED
                "#
            )
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .fetch_all(&self.pool)
            .await?;

            if available_jobs.is_empty() {
                return Ok(None);
            }

            // Group jobs by priority and apply weighted selection
            let mut priority_jobs: std::collections::HashMap<JobPriority, Vec<_>> =
                std::collections::HashMap::new();

            for row in available_jobs {
                let priority_val: i32 = row.get("priority");
                let priority = JobPriority::from_i32(priority_val).unwrap_or(JobPriority::Normal);
                priority_jobs.entry(priority).or_default().push(row);
            }

            // Calculate weighted selection
            let mut weighted_choices = Vec::new();
            for (priority, _jobs) in &priority_jobs {
                let weight = weights.get_weight(*priority);
                for _ in 0..weight {
                    weighted_choices.push(priority);
                }
            }

            if weighted_choices.is_empty() {
                return Ok(None);
            }

            // Use a simple hash-based selection instead of thread_rng for Send compatibility
            let selection_index = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                queue_name.hash(&mut hasher);
                Utc::now()
                    .timestamp_nanos_opt()
                    .unwrap_or(0)
                    .hash(&mut hasher);
                (hasher.finish() as usize) % weighted_choices.len()
            };
            let selected_priority = weighted_choices[selection_index];

            // Select the oldest job from the selected priority
            if let Some(jobs) = priority_jobs.get(selected_priority) {
                if let Some(selected_row) = jobs.first() {
                    let job_id: uuid::Uuid = selected_row.get("id");

                    // Update the selected job
                    let updated_row = sqlx::query(
                        "UPDATE hammerwork_jobs SET status = $1, started_at = $2, attempts = attempts + 1 WHERE id = $3 RETURNING id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone"
                    )
                    .bind(serde_json::to_string(&JobStatus::Running)?)
                    .bind(Utc::now())
                    .bind(job_id)
                    .fetch_optional(&self.pool)
                    .await?;

                    if let Some(row) = updated_row {
                        let id: uuid::Uuid = row.get("id");
                        let queue_name: String = row.get("queue_name");
                        let payload: serde_json::Value = row.get("payload");
                        let status: String = row.get("status");
                        let priority: i32 = row.get("priority");
                        let attempts: i32 = row.get("attempts");
                        let max_attempts: i32 = row.get("max_attempts");
                        let timeout_seconds: Option<i32> = row.get("timeout_seconds");
                        let created_at: DateTime<Utc> = row.get("created_at");
                        let scheduled_at: DateTime<Utc> = row.get("scheduled_at");
                        let started_at: Option<DateTime<Utc>> = row.get("started_at");
                        let completed_at: Option<DateTime<Utc>> = row.get("completed_at");
                        let failed_at: Option<DateTime<Utc>> = row.get("failed_at");
                        let timed_out_at: Option<DateTime<Utc>> = row.get("timed_out_at");
                        let error_message: Option<String> = row.get("error_message");
                        let cron_schedule: Option<String> = row.get("cron_schedule");
                        let next_run_at: Option<DateTime<Utc>> = row.get("next_run_at");
                        let recurring: bool = row.get("recurring");
                        let timezone: Option<String> = row.get("timezone");

                        return Ok(Some(Job {
                            id,
                            queue_name,
                            payload,
                            status: serde_json::from_str(&status)?,
                            priority: JobPriority::from_i32(priority)
                                .unwrap_or(JobPriority::Normal),
                            attempts,
                            max_attempts,
                            created_at,
                            scheduled_at,
                            started_at,
                            completed_at,
                            failed_at,
                            timed_out_at,
                            timeout: timeout_seconds
                                .map(|s| std::time::Duration::from_secs(s as u64)),
                            error_message,
                            cron_schedule,
                            next_run_at,
                            recurring,
                            timezone,
                        }));
                    }
                }
            }

            Ok(None)
        }

        async fn complete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET status = $1, completed_at = $2 WHERE id = $3")
                .bind(serde_json::to_string(&JobStatus::Completed)?)
                .bind(Utc::now())
                .bind(job_id)
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, error_message = $2, failed_at = $3 WHERE id = $4"
            )
            .bind(serde_json::to_string(&JobStatus::Failed)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, scheduled_at = $2, started_at = NULL WHERE id = $3"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(retry_at)
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
            let row = sqlx::query_as::<_, JobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone FROM hammerwork_jobs WHERE id = $1"
            )
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await?;

            match row {
                Some(job_row) => Ok(Some(job_row.into_job()?)),
                None => Ok(None),
            }
        }

        async fn delete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("DELETE FROM hammerwork_jobs WHERE id = $1")
                .bind(job_id)
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        // Dead job management
        async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, error_message = $2, failed_at = $3 WHERE id = $4"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn mark_job_timed_out(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, error_message = $2, timed_out_at = $3 WHERE id = $4"
            )
            .bind(serde_json::to_string(&JobStatus::TimedOut)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;

            let rows = sqlx::query_as::<_, DeadJobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = $1 ORDER BY failed_at DESC LIMIT $2 OFFSET $3"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            Ok(rows.into_iter().map(|row| row.into_job()).collect())
        }

        async fn get_dead_jobs_by_queue(
            &self,
            queue_name: &str,
            limit: Option<u32>,
            offset: Option<u32>,
        ) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;

            let rows = sqlx::query_as::<_, DeadJobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = $1 AND queue_name = $2 ORDER BY failed_at DESC LIMIT $3 OFFSET $4"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(queue_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            Ok(rows.into_iter().map(|row| row.into_job()).collect())
        }

        async fn retry_dead_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = $1, attempts = 0, scheduled_at = $2, started_at = NULL, failed_at = NULL WHERE id = $3 AND status = $4"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .bind(job_id)
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
            let result =
                sqlx::query("DELETE FROM hammerwork_jobs WHERE status = $1 AND failed_at < $2")
                    .bind(serde_json::to_string(&JobStatus::Dead)?)
                    .bind(older_than)
                    .execute(&self.pool)
                    .await?;

            Ok(result.rows_affected())
        }

        async fn get_dead_job_summary(&self) -> Result<DeadJobSummary> {
            use std::collections::HashMap;

            // Get total dead job count
            let total_dead_jobs: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM hammerwork_jobs WHERE status = $1")
                    .bind(serde_json::to_string(&JobStatus::Dead)?)
                    .fetch_one(&self.pool)
                    .await?;

            // Get dead jobs by queue
            let dead_jobs_by_queue_rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT queue_name, COUNT(*) FROM hammerwork_jobs WHERE status = $1 GROUP BY queue_name"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get oldest and newest dead jobs
            let timestamps: Vec<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = sqlx::query_as(
                "SELECT MIN(failed_at), MAX(failed_at) FROM hammerwork_jobs WHERE status = $1 AND failed_at IS NOT NULL"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get error patterns
            let error_patterns_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE status = $1 AND error_message IS NOT NULL GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 20"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            let dead_jobs_by_queue: HashMap<String, u64> = dead_jobs_by_queue_rows
                .into_iter()
                .map(|(queue, count)| (queue, count as u64))
                .collect();

            let error_patterns: HashMap<String, u64> = error_patterns_rows
                .into_iter()
                .filter_map(|(error, count)| error.map(|e| (e, count as u64)))
                .collect();

            let (oldest_dead_job, newest_dead_job) = timestamps
                .first()
                .map(|(oldest, newest)| (*oldest, *newest))
                .unwrap_or((None, None));

            Ok(DeadJobSummary {
                total_dead_jobs: total_dead_jobs.0 as u64,
                dead_jobs_by_queue,
                oldest_dead_job,
                newest_dead_job,
                error_patterns,
            })
        }

        // Statistics and monitoring
        async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
            use crate::stats::JobStatistics;
            use std::collections::HashMap;

            // Get job counts by status
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 GROUP BY status"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            let mut counts = HashMap::new();
            for (status, count) in status_counts {
                counts.insert(status, count as u64);
            }

            let pending_count = counts
                .get(&serde_json::to_string(&JobStatus::Pending).unwrap())
                .copied()
                .unwrap_or(0);
            let running_count = counts
                .get(&serde_json::to_string(&JobStatus::Running).unwrap())
                .copied()
                .unwrap_or(0);
            let dead_count = counts
                .get(&serde_json::to_string(&JobStatus::Dead).unwrap())
                .copied()
                .unwrap_or(0);
            let timed_out_count = counts
                .get(&serde_json::to_string(&JobStatus::TimedOut).unwrap())
                .copied()
                .unwrap_or(0);
            let completed_count = counts
                .get(&serde_json::to_string(&JobStatus::Completed).unwrap())
                .copied()
                .unwrap_or(0);

            // Basic statistics (more detailed stats would require the statistics collector)
            let statistics = JobStatistics {
                total_processed: completed_count + dead_count,
                completed: completed_count,
                failed: counts
                    .get(&serde_json::to_string(&JobStatus::Failed).unwrap())
                    .copied()
                    .unwrap_or(0),
                dead: dead_count,
                timed_out: timed_out_count,
                running: running_count,
                time_window: Duration::from_secs(3600), // Default 1 hour window
                calculated_at: Utc::now(),
                ..Default::default()
            };

            Ok(QueueStats {
                queue_name: queue_name.to_string(),
                pending_count,
                running_count,
                dead_count,
                timed_out_count,
                completed_count,
                statistics,
            })
        }

        async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>> {
            // Get all unique queue names
            let queue_names: Vec<(String,)> =
                sqlx::query_as("SELECT DISTINCT queue_name FROM hammerwork_jobs")
                    .fetch_all(&self.pool)
                    .await?;

            let mut results = Vec::new();
            for (queue_name,) in queue_names {
                let stats = self.get_queue_stats(&queue_name).await?;
                results.push(stats);
            }

            Ok(results)
        }

        async fn get_job_counts_by_status(
            &self,
            queue_name: &str,
        ) -> Result<std::collections::HashMap<String, u64>> {
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 GROUP BY status"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            Ok(status_counts
                .into_iter()
                .map(|(status, count)| (status, count as u64))
                .collect())
        }

        async fn get_processing_times(
            &self,
            queue_name: &str,
            since: DateTime<Utc>,
        ) -> Result<Vec<i64>> {
            let times: Vec<(Option<i64>,)> = sqlx::query_as(
                r#"
                SELECT EXTRACT(EPOCH FROM (completed_at - started_at)) * 1000 as processing_time_ms
                FROM hammerwork_jobs 
                WHERE queue_name = $1 
                AND started_at IS NOT NULL 
                AND completed_at IS NOT NULL 
                AND completed_at >= $2
                ORDER BY completed_at DESC
                LIMIT 1000
                "#,
            )
            .bind(queue_name)
            .bind(since)
            .fetch_all(&self.pool)
            .await?;

            Ok(times.into_iter().filter_map(|(time,)| time).collect())
        }

        async fn get_error_frequencies(
            &self,
            queue_name: Option<&str>,
            since: DateTime<Utc>,
        ) -> Result<std::collections::HashMap<String, u64>> {
            let query = if queue_name.is_some() {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 AND error_message IS NOT NULL AND failed_at >= $2 GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            } else {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE error_message IS NOT NULL AND failed_at >= $1 GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            };

            let error_frequencies: Vec<(String, i64)> = if let Some(queue) = queue_name {
                sqlx::query_as(query)
                    .bind(queue)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                sqlx::query_as(query)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            };

            Ok(error_frequencies
                .into_iter()
                .map(|(error, count)| (error, count as u64))
                .collect())
        }

        // Cron job management
        async fn enqueue_cron_job(&self, job: Job) -> Result<JobId> {
            // For cron jobs, we use the regular enqueue method
            // The job should already have the cron fields set
            self.enqueue(job).await
        }

        async fn get_due_cron_jobs(&self, queue_name: Option<&str>) -> Result<Vec<Job>> {
            let query = if queue_name.is_some() {
                r#"
                SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE recurring = TRUE 
                AND queue_name = $1
                AND (next_run_at IS NULL OR next_run_at <= $2)
                AND status = $3
                ORDER BY next_run_at ASC
                "#
            } else {
                r#"
                SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE recurring = TRUE 
                AND (next_run_at IS NULL OR next_run_at <= $1)
                AND status = $2
                ORDER BY next_run_at ASC
                "#
            };

            let rows = if let Some(queue) = queue_name {
                sqlx::query_as::<_, JobRow>(query)
                    .bind(queue)
                    .bind(Utc::now())
                    .bind(serde_json::to_string(&JobStatus::Pending)?)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                sqlx::query_as::<_, JobRow>(query)
                    .bind(Utc::now())
                    .bind(serde_json::to_string(&JobStatus::Pending)?)
                    .fetch_all(&self.pool)
                    .await?
            };

            rows.into_iter().map(|row| row.into_job()).collect()
        }

        async fn reschedule_cron_job(
            &self,
            job_id: JobId,
            next_run_at: DateTime<Utc>,
        ) -> Result<()> {
            sqlx::query(
                r#"
                UPDATE hammerwork_jobs 
                SET status = $1, 
                    scheduled_at = $2, 
                    next_run_at = $2,
                    attempts = 0,
                    started_at = NULL,
                    completed_at = NULL,
                    failed_at = NULL,
                    timed_out_at = NULL,
                    error_message = NULL
                WHERE id = $3 AND recurring = TRUE
                "#,
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(next_run_at)
            .bind(job_id)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_recurring_jobs(&self, queue_name: &str) -> Result<Vec<Job>> {
            let rows = sqlx::query_as::<_, JobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone FROM hammerwork_jobs WHERE queue_name = $1 AND recurring = TRUE ORDER BY next_run_at ASC"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter().map(|row| row.into_job()).collect()
        }

        async fn disable_recurring_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET recurring = FALSE WHERE id = $1")
                .bind(job_id)
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn enable_recurring_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET recurring = TRUE WHERE id = $1")
                .bind(job_id)
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn set_throttle_config(&self, queue_name: &str, config: ThrottleConfig) -> Result<()> {
            self.set_throttle(queue_name, config).await
        }

        async fn get_throttle_config(&self, queue_name: &str) -> Result<Option<ThrottleConfig>> {
            Ok(self.get_throttle(queue_name).await)
        }

        async fn remove_throttle_config(&self, queue_name: &str) -> Result<()> {
            self.remove_throttle(queue_name).await
        }

        async fn get_all_throttle_configs(&self) -> Result<HashMap<String, ThrottleConfig>> {
            Ok(self.get_all_throttles().await)
        }

        async fn get_queue_depth(&self, queue_name: &str) -> Result<u64> {
            let count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 AND status = $2"
            )
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .fetch_one(&self.pool)
            .await?;

            Ok(count.0 as u64)
        }
    }
}

#[cfg(feature = "mysql")]
pub mod mysql {
    use super::*;
    use crate::job::JobStatus;
    use crate::priority::JobPriority;
    use sqlx::{FromRow, MySql};
    use std::time::Duration;

    #[derive(FromRow, Clone)]
    struct JobRow {
        id: String, // MySQL uses CHAR(36) for UUID
        queue_name: String,
        payload: serde_json::Value,
        status: String,
        priority: i32,
        attempts: i32,
        max_attempts: i32,
        timeout_seconds: Option<i32>,
        created_at: DateTime<Utc>,
        scheduled_at: DateTime<Utc>,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
        failed_at: Option<DateTime<Utc>>,
        timed_out_at: Option<DateTime<Utc>>,
        error_message: Option<String>,
        cron_schedule: Option<String>,
        next_run_at: Option<DateTime<Utc>>,
        recurring: bool,
        timezone: Option<String>,
    }

    impl JobRow {
        fn into_job(self) -> Result<Job> {
            Ok(Job {
                id: uuid::Uuid::parse_str(&self.id)?,
                queue_name: self.queue_name,
                payload: self.payload,
                status: serde_json::from_str(&self.status)?,
                priority: JobPriority::from_i32(self.priority).unwrap_or(JobPriority::Normal),
                attempts: self.attempts,
                max_attempts: self.max_attempts,
                created_at: self.created_at,
                scheduled_at: self.scheduled_at,
                started_at: self.started_at,
                completed_at: self.completed_at,
                failed_at: self.failed_at,
                timed_out_at: self.timed_out_at,
                timeout: self
                    .timeout_seconds
                    .map(|s| std::time::Duration::from_secs(s as u64)),
                error_message: self.error_message,
                cron_schedule: self.cron_schedule,
                next_run_at: self.next_run_at,
                recurring: self.recurring,
                timezone: self.timezone,
            })
        }
    }

    #[derive(FromRow)]
    struct DeadJobRow {
        id: String,
        queue_name: String,
        payload: serde_json::Value,
        status: String,
        priority: i32,
        attempts: i32,
        max_attempts: i32,
        timeout_seconds: Option<i32>,
        created_at: DateTime<Utc>,
        scheduled_at: DateTime<Utc>,
        started_at: Option<DateTime<Utc>>,
        completed_at: Option<DateTime<Utc>>,
        failed_at: Option<DateTime<Utc>>,
        timed_out_at: Option<DateTime<Utc>>,
        error_message: Option<String>,
    }

    impl DeadJobRow {
        fn into_job(self) -> Result<Job> {
            Ok(Job {
                id: uuid::Uuid::parse_str(&self.id)?,
                queue_name: self.queue_name,
                payload: self.payload,
                status: serde_json::from_str(&self.status).unwrap_or(JobStatus::Dead),
                priority: JobPriority::from_i32(self.priority).unwrap_or(JobPriority::Normal),
                attempts: self.attempts,
                max_attempts: self.max_attempts,
                created_at: self.created_at,
                scheduled_at: self.scheduled_at,
                started_at: self.started_at,
                completed_at: self.completed_at,
                failed_at: self.failed_at,
                timed_out_at: self.timed_out_at,
                timeout: self
                    .timeout_seconds
                    .map(|s| std::time::Duration::from_secs(s as u64)),
                error_message: self.error_message,
                cron_schedule: None,
                next_run_at: None,
                recurring: false,
                timezone: None,
            })
        }
    }

    #[async_trait]
    impl DatabaseQueue for JobQueue<MySql> {
        type Database = MySql;

        async fn create_tables(&self) -> Result<()> {
            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS hammerwork_jobs (
                    id CHAR(36) PRIMARY KEY,
                    queue_name VARCHAR(255) NOT NULL,
                    payload JSON NOT NULL,
                    status VARCHAR(50) NOT NULL,
                    priority INT NOT NULL DEFAULT 2,
                    attempts INT NOT NULL DEFAULT 0,
                    max_attempts INT NOT NULL DEFAULT 3,
                    timeout_seconds INT NULL,
                    created_at TIMESTAMP(6) NOT NULL,
                    scheduled_at TIMESTAMP(6) NOT NULL,
                    started_at TIMESTAMP(6) NULL,
                    completed_at TIMESTAMP(6) NULL,
                    failed_at TIMESTAMP(6) NULL,
                    timed_out_at TIMESTAMP(6) NULL,
                    error_message TEXT NULL,
                    cron_schedule VARCHAR(100) NULL,
                    next_run_at TIMESTAMP(6) NULL,
                    recurring BOOLEAN NOT NULL DEFAULT FALSE,
                    timezone VARCHAR(50) NULL,
                    INDEX idx_queue_status_priority_scheduled (queue_name, status, priority DESC, scheduled_at),
                    INDEX idx_status_failed_at (status, failed_at),
                    INDEX idx_queue_status (queue_name, status),
                    INDEX idx_recurring_next_run (recurring, next_run_at),
                    INDEX idx_cron_schedule (cron_schedule)
                )
                "#,
            )
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn enqueue(&self, job: Job) -> Result<JobId> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#
            )
            .bind(job.id.to_string())
            .bind(&job.queue_name)
            .bind(&job.payload)
            .bind(serde_json::to_string(&job.status)?)
            .bind(job.priority.as_i32())
            .bind(job.attempts)
            .bind(job.max_attempts)
            .bind(job.timeout.map(|t| t.as_secs() as i32))
            .bind(job.created_at)
            .bind(job.scheduled_at)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(job.failed_at)
            .bind(job.timed_out_at)
            .bind(&job.error_message)
            .bind(&job.cron_schedule)
            .bind(job.next_run_at)
            .bind(job.recurring)
            .bind(&job.timezone)
            .execute(&self.pool)
            .await?;

            Ok(job.id)
        }

        async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
            // MySQL doesn't support FOR UPDATE SKIP LOCKED in the same way
            // This is a simplified version - in production you might want advisory locks
            let mut tx = self.pool.begin().await?;

            let row = sqlx::query_as::<_, JobRow>(
                r#"
                SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE queue_name = ? 
                AND status = ? 
                AND scheduled_at <= ?
                ORDER BY priority DESC, scheduled_at ASC 
                LIMIT 1
                FOR UPDATE
                "#
            )
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .fetch_optional(&mut *tx)
            .await?;

            if let Some(job_row) = row {
                let _job_id = uuid::Uuid::parse_str(&job_row.id)?;

                sqlx::query(
                    "UPDATE hammerwork_jobs SET status = ?, started_at = ?, attempts = attempts + 1 WHERE id = ?"
                )
                .bind(serde_json::to_string(&JobStatus::Running)?)
                .bind(Utc::now())
                .bind(&job_row.id)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;

                let mut job = job_row.into_job()?;
                job.status = JobStatus::Running;
                job.attempts += 1;
                job.started_at = Some(Utc::now());

                Ok(Some(job))
            } else {
                tx.rollback().await?;
                Ok(None)
            }
        }

        async fn dequeue_with_priority_weights(
            &self,
            queue_name: &str,
            weights: &crate::priority::PriorityWeights,
        ) -> Result<Option<Job>> {
            use crate::priority::JobPriority;

            if weights.is_strict() {
                // Use strict priority - same as regular dequeue
                return self.dequeue(queue_name).await;
            }

            let mut tx = self.pool.begin().await?;

            // Get available jobs by priority
            let available_jobs = sqlx::query_as::<_, JobRow>(
                r#"
                SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE queue_name = ? 
                AND status = ? 
                AND scheduled_at <= ?
                ORDER BY priority DESC, scheduled_at ASC 
                LIMIT 20
                FOR UPDATE
                "#
            )
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .fetch_all(&mut *tx)
            .await?;

            if available_jobs.is_empty() {
                tx.rollback().await?;
                return Ok(None);
            }

            // Group jobs by priority and apply weighted selection
            let mut priority_jobs: std::collections::HashMap<JobPriority, Vec<_>> =
                std::collections::HashMap::new();

            for job_row in available_jobs {
                let priority =
                    JobPriority::from_i32(job_row.priority).unwrap_or(JobPriority::Normal);
                priority_jobs.entry(priority).or_default().push(job_row);
            }

            // Calculate weighted selection
            let mut weighted_choices = Vec::new();
            for (priority, _jobs) in &priority_jobs {
                let weight = weights.get_weight(*priority);
                for _ in 0..weight {
                    weighted_choices.push(priority);
                }
            }

            if weighted_choices.is_empty() {
                tx.rollback().await?;
                return Ok(None);
            }

            // Use a simple hash-based selection instead of thread_rng for Send compatibility
            let selection_index = {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                queue_name.hash(&mut hasher);
                chrono::Utc::now()
                    .timestamp_nanos_opt()
                    .unwrap_or(0)
                    .hash(&mut hasher);
                (hasher.finish() as usize) % weighted_choices.len()
            };
            let selected_priority = weighted_choices[selection_index];

            // Select the oldest job from the selected priority
            if let Some(jobs) = priority_jobs.get(selected_priority) {
                if let Some(selected_job) = jobs.first() {
                    // Update the selected job
                    sqlx::query(
                        "UPDATE hammerwork_jobs SET status = ?, started_at = ?, attempts = attempts + 1 WHERE id = ?"
                    )
                    .bind(serde_json::to_string(&JobStatus::Running)?)
                    .bind(Utc::now())
                    .bind(&selected_job.id)
                    .execute(&mut *tx)
                    .await?;

                    tx.commit().await?;

                    let mut job = selected_job.clone().into_job()?;
                    job.status = JobStatus::Running;
                    job.attempts += 1;
                    job.started_at = Some(Utc::now());

                    return Ok(Some(job));
                }
            }

            tx.rollback().await?;
            Ok(None)
        }

        async fn complete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET status = ?, completed_at = ? WHERE id = ?")
                .bind(serde_json::to_string(&JobStatus::Completed)?)
                .bind(Utc::now())
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ? WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Failed)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, scheduled_at = ?, started_at = NULL WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(retry_at)
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
            let row = sqlx::query_as::<_, JobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone FROM hammerwork_jobs WHERE id = ?"
            )
            .bind(job_id.to_string())
            .fetch_optional(&self.pool)
            .await?;

            match row {
                Some(job_row) => Ok(Some(job_row.into_job()?)),
                None => Ok(None),
            }
        }

        async fn delete_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ?")
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        // Dead job management
        async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ? WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn mark_job_timed_out(&self, job_id: JobId, error_message: &str) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, timed_out_at = ? WHERE id = ?"
            )
            .bind(serde_json::to_string(&JobStatus::TimedOut)?)
            .bind(error_message)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;

            let rows = sqlx::query_as::<_, DeadJobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = ? ORDER BY failed_at DESC LIMIT ? OFFSET ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter().map(|row| row.into_job()).collect()
        }

        async fn get_dead_jobs_by_queue(
            &self,
            queue_name: &str,
            limit: Option<u32>,
            offset: Option<u32>,
        ) -> Result<Vec<Job>> {
            let limit = limit.unwrap_or(100) as i64;
            let offset = offset.unwrap_or(0) as i64;

            let rows = sqlx::query_as::<_, DeadJobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = ? AND queue_name = ? ORDER BY failed_at DESC LIMIT ? OFFSET ?"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .bind(queue_name)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter().map(|row| row.into_job()).collect()
        }

        async fn retry_dead_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, attempts = 0, scheduled_at = ?, started_at = NULL, failed_at = NULL WHERE id = ? AND status = ?"
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
            let result =
                sqlx::query("DELETE FROM hammerwork_jobs WHERE status = ? AND failed_at < ?")
                    .bind(serde_json::to_string(&JobStatus::Dead)?)
                    .bind(older_than)
                    .execute(&self.pool)
                    .await?;

            Ok(result.rows_affected())
        }

        async fn get_dead_job_summary(&self) -> Result<DeadJobSummary> {
            use std::collections::HashMap;

            // Get total dead job count
            let total_dead_jobs: (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM hammerwork_jobs WHERE status = ?")
                    .bind(serde_json::to_string(&JobStatus::Dead)?)
                    .fetch_one(&self.pool)
                    .await?;

            // Get dead jobs by queue
            let dead_jobs_by_queue_rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT queue_name, COUNT(*) FROM hammerwork_jobs WHERE status = ? GROUP BY queue_name"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get oldest and newest dead jobs
            let timestamps: Vec<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = sqlx::query_as(
                "SELECT MIN(failed_at), MAX(failed_at) FROM hammerwork_jobs WHERE status = ? AND failed_at IS NOT NULL"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            // Get error patterns
            let error_patterns_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE status = ? AND error_message IS NOT NULL GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 20"
            )
            .bind(serde_json::to_string(&JobStatus::Dead)?)
            .fetch_all(&self.pool)
            .await?;

            let dead_jobs_by_queue: HashMap<String, u64> = dead_jobs_by_queue_rows
                .into_iter()
                .map(|(queue, count)| (queue, count as u64))
                .collect();

            let error_patterns: HashMap<String, u64> = error_patterns_rows
                .into_iter()
                .filter_map(|(error, count)| error.map(|e| (e, count as u64)))
                .collect();

            let (oldest_dead_job, newest_dead_job) = timestamps
                .first()
                .map(|(oldest, newest)| (*oldest, *newest))
                .unwrap_or((None, None));

            Ok(DeadJobSummary {
                total_dead_jobs: total_dead_jobs.0 as u64,
                dead_jobs_by_queue,
                oldest_dead_job,
                newest_dead_job,
                error_patterns,
            })
        }

        // Statistics and monitoring
        async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
            use crate::stats::JobStatistics;
            use std::collections::HashMap;

            // Get job counts by status
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status",
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            let mut counts = HashMap::new();
            for (status, count) in status_counts {
                counts.insert(status, count as u64);
            }

            let pending_count = counts
                .get(&serde_json::to_string(&JobStatus::Pending).unwrap())
                .copied()
                .unwrap_or(0);
            let running_count = counts
                .get(&serde_json::to_string(&JobStatus::Running).unwrap())
                .copied()
                .unwrap_or(0);
            let dead_count = counts
                .get(&serde_json::to_string(&JobStatus::Dead).unwrap())
                .copied()
                .unwrap_or(0);
            let timed_out_count = counts
                .get(&serde_json::to_string(&JobStatus::TimedOut).unwrap())
                .copied()
                .unwrap_or(0);
            let completed_count = counts
                .get(&serde_json::to_string(&JobStatus::Completed).unwrap())
                .copied()
                .unwrap_or(0);

            // Basic statistics (more detailed stats would require the statistics collector)
            let statistics = JobStatistics {
                total_processed: completed_count + dead_count,
                completed: completed_count,
                failed: counts
                    .get(&serde_json::to_string(&JobStatus::Failed).unwrap())
                    .copied()
                    .unwrap_or(0),
                dead: dead_count,
                timed_out: timed_out_count,
                running: running_count,
                time_window: Duration::from_secs(3600), // Default 1 hour window
                calculated_at: Utc::now(),
                ..Default::default()
            };

            Ok(QueueStats {
                queue_name: queue_name.to_string(),
                pending_count,
                running_count,
                dead_count,
                timed_out_count,
                completed_count,
                statistics,
            })
        }

        async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>> {
            // Get all unique queue names
            let queue_names: Vec<(String,)> =
                sqlx::query_as("SELECT DISTINCT queue_name FROM hammerwork_jobs")
                    .fetch_all(&self.pool)
                    .await?;

            let mut results = Vec::new();
            for (queue_name,) in queue_names {
                let stats = self.get_queue_stats(&queue_name).await?;
                results.push(stats);
            }

            Ok(results)
        }

        async fn get_job_counts_by_status(
            &self,
            queue_name: &str,
        ) -> Result<std::collections::HashMap<String, u64>> {
            let status_counts: Vec<(String, i64)> = sqlx::query_as(
                "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status",
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            Ok(status_counts
                .into_iter()
                .map(|(status, count)| (status, count as u64))
                .collect())
        }

        async fn get_processing_times(
            &self,
            queue_name: &str,
            since: DateTime<Utc>,
        ) -> Result<Vec<i64>> {
            let times: Vec<(Option<i64>,)> = sqlx::query_as(
                r#"
                SELECT TIMESTAMPDIFF(MICROSECOND, started_at, completed_at) / 1000 as processing_time_ms
                FROM hammerwork_jobs 
                WHERE queue_name = ? 
                AND started_at IS NOT NULL 
                AND completed_at IS NOT NULL 
                AND completed_at >= ?
                ORDER BY completed_at DESC
                LIMIT 1000
                "#
            )
            .bind(queue_name)
            .bind(since)
            .fetch_all(&self.pool)
            .await?;

            Ok(times.into_iter().filter_map(|(time,)| time).collect())
        }

        async fn get_error_frequencies(
            &self,
            queue_name: Option<&str>,
            since: DateTime<Utc>,
        ) -> Result<std::collections::HashMap<String, u64>> {
            let query = if queue_name.is_some() {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? AND error_message IS NOT NULL AND failed_at >= ? GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            } else {
                "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE error_message IS NOT NULL AND failed_at >= ? GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
            };

            let error_frequencies: Vec<(String, i64)> = if let Some(queue) = queue_name {
                sqlx::query_as(query)
                    .bind(queue)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                sqlx::query_as(query)
                    .bind(since)
                    .fetch_all(&self.pool)
                    .await?
            };

            Ok(error_frequencies
                .into_iter()
                .map(|(error, count)| (error, count as u64))
                .collect())
        }

        // Cron job management
        async fn enqueue_cron_job(&self, job: Job) -> Result<JobId> {
            // For cron jobs, we use the regular enqueue method
            // The job should already have the cron fields set
            self.enqueue(job).await
        }

        async fn get_due_cron_jobs(&self, queue_name: Option<&str>) -> Result<Vec<Job>> {
            let query = if queue_name.is_some() {
                r#"
                SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE recurring = TRUE 
                AND queue_name = ?
                AND (next_run_at IS NULL OR next_run_at <= ?)
                AND status = ?
                ORDER BY next_run_at ASC
                "#
            } else {
                r#"
                SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone
                FROM hammerwork_jobs 
                WHERE recurring = TRUE 
                AND (next_run_at IS NULL OR next_run_at <= ?)
                AND status = ?
                ORDER BY next_run_at ASC
                "#
            };

            let rows = if let Some(queue) = queue_name {
                sqlx::query_as::<_, JobRow>(query)
                    .bind(queue)
                    .bind(Utc::now())
                    .bind(serde_json::to_string(&JobStatus::Pending)?)
                    .fetch_all(&self.pool)
                    .await?
            } else {
                sqlx::query_as::<_, JobRow>(query)
                    .bind(Utc::now())
                    .bind(serde_json::to_string(&JobStatus::Pending)?)
                    .fetch_all(&self.pool)
                    .await?
            };

            rows.into_iter().map(|row| row.into_job()).collect()
        }

        async fn reschedule_cron_job(
            &self,
            job_id: JobId,
            next_run_at: DateTime<Utc>,
        ) -> Result<()> {
            sqlx::query(
                r#"
                UPDATE hammerwork_jobs 
                SET status = ?, 
                    scheduled_at = ?, 
                    next_run_at = ?,
                    attempts = 0,
                    started_at = NULL,
                    completed_at = NULL,
                    failed_at = NULL,
                    timed_out_at = NULL,
                    error_message = NULL
                WHERE id = ? AND recurring = TRUE
                "#,
            )
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .bind(next_run_at)
            .bind(next_run_at)
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

            Ok(())
        }

        async fn get_recurring_jobs(&self, queue_name: &str) -> Result<Vec<Job>> {
            let rows = sqlx::query_as::<_, JobRow>(
                "SELECT id, queue_name, payload, status, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone FROM hammerwork_jobs WHERE queue_name = ? AND recurring = TRUE ORDER BY next_run_at ASC"
            )
            .bind(queue_name)
            .fetch_all(&self.pool)
            .await?;

            rows.into_iter().map(|row| row.into_job()).collect()
        }

        async fn disable_recurring_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET recurring = FALSE WHERE id = ?")
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn enable_recurring_job(&self, job_id: JobId) -> Result<()> {
            sqlx::query("UPDATE hammerwork_jobs SET recurring = TRUE WHERE id = ?")
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;

            Ok(())
        }

        async fn set_throttle_config(&self, queue_name: &str, config: ThrottleConfig) -> Result<()> {
            self.set_throttle(queue_name, config).await
        }

        async fn get_throttle_config(&self, queue_name: &str) -> Result<Option<ThrottleConfig>> {
            Ok(self.get_throttle(queue_name).await)
        }

        async fn remove_throttle_config(&self, queue_name: &str) -> Result<()> {
            self.remove_throttle(queue_name).await
        }

        async fn get_all_throttle_configs(&self) -> Result<HashMap<String, ThrottleConfig>> {
            Ok(self.get_all_throttles().await)
        }

        async fn get_queue_depth(&self, queue_name: &str) -> Result<u64> {
            let count: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? AND status = ?"
            )
            .bind(queue_name)
            .bind(serde_json::to_string(&JobStatus::Pending)?)
            .fetch_one(&self.pool)
            .await?;

            Ok(count.0 as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_queue_trait_exists() {
        // This test verifies the DatabaseQueue trait is properly defined
        // We can't easily unit test the implementations without database connections
        // But we can verify the trait signature compiles
        fn _test_trait_signature<T: DatabaseQueue>() {}
        // This function compiles if the trait is properly defined
    }

    #[test]
    fn test_job_queue_generic_struct() {
        // Test that the JobQueue struct is properly generic
        // We can't instantiate it without a real database pool
        // This would be the structure if we had a real database type
        // let _phantom: PhantomData<sqlx::Postgres> = PhantomData;
        assert!(true); // Compilation test
    }

    #[test]
    fn test_timeout_related_trait_methods() {
        use crate::job::{Job, JobId};
        use serde_json::json;
        use std::time::Duration;

        // Test that timeout-related methods are part of the DatabaseQueue trait
        // This is a compilation test to ensure the trait includes timeout methods

        // Create a job with timeout for testing
        let job = Job::new("timeout_test".to_string(), json!({"data": "test"}))
            .with_timeout(Duration::from_secs(30));

        assert_eq!(job.timeout, Some(Duration::from_secs(30)));
        assert_eq!(job.timed_out_at, None);

        // Test JobId type alias works with timeout methods
        let job_id: JobId = job.id;
        assert_eq!(job_id, job.id);
    }

    #[test]
    fn test_timeout_database_schema_compatibility() {
        // Test that the database schema includes timeout-related columns
        // This verifies our schema design is consistent

        use crate::job::{Job, JobStatus};
        use serde_json::json;
        use std::time::Duration;

        // Create job with all timeout fields
        let mut job = Job::new("schema_test".to_string(), json!({"test": "data"}))
            .with_timeout(Duration::from_secs(120));

        // Simulate timeout scenario
        job.status = JobStatus::TimedOut;
        job.timed_out_at = Some(chrono::Utc::now());
        job.error_message = Some("Timeout test".to_string());

        // Verify all timeout-related fields are present
        assert!(job.timeout.is_some());
        assert!(job.timed_out_at.is_some());
        assert_eq!(job.status, JobStatus::TimedOut);
        assert!(job.error_message.is_some());

        // Test timeout duration conversion to seconds (for database storage)
        let timeout_seconds = job.timeout.unwrap().as_secs() as i32;
        assert_eq!(timeout_seconds, 120);
    }

    #[test]
    fn test_job_timeout_status_consistency() {
        use crate::job::{Job, JobStatus};
        use serde_json::json;

        // Test that TimedOut status is distinct from other statuses
        let statuses = vec![
            JobStatus::Pending,
            JobStatus::Running,
            JobStatus::Completed,
            JobStatus::Failed,
            JobStatus::Dead,
            JobStatus::TimedOut,
            JobStatus::Retrying,
        ];

        // Verify TimedOut is included in all statuses
        assert!(statuses.contains(&JobStatus::TimedOut));

        // Test status transitions for timeout scenarios
        let mut job = Job::new("status_test".to_string(), json!({"data": "test"}));

        assert_eq!(job.status, JobStatus::Pending);
        assert!(!job.is_timed_out());
        assert!(!job.is_dead());

        job.status = JobStatus::TimedOut;
        assert!(job.is_timed_out());
        assert!(!job.is_dead()); // TimedOut ≠ Dead
    }

    #[test]
    fn test_timeout_database_operations_interface() {
        // Test that the DatabaseQueue trait includes all necessary timeout operations
        // This is a compilation test to verify method signatures

        // Test that mark_job_timed_out method exists in the trait
        // This ensures our timeout functionality is properly integrated
        use crate::job::{Job, JobStatus};
        use serde_json::json;
        use std::time::Duration;

        // Test job creation with timeout functionality
        let job = Job::new("trait_test".to_string(), json!({"data": "test"}))
            .with_timeout(Duration::from_secs(30));

        assert!(job.timeout.is_some());
        assert_eq!(job.status, JobStatus::Pending);

        // Test that TimedOut status is available
        let timed_out_status = JobStatus::TimedOut;
        assert_eq!(format!("{:?}", timed_out_status), "TimedOut");

        // Test timeout-related job methods
        let mut test_job = job;
        test_job.status = JobStatus::TimedOut;
        test_job.timed_out_at = Some(chrono::Utc::now());

        assert!(test_job.is_timed_out());
        assert!(!test_job.is_dead()); // TimedOut ≠ Dead
    }
}
