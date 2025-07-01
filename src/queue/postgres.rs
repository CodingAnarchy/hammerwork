//! PostgreSQL implementation of the job queue.
//!
//! This module provides the PostgreSQL-specific implementation of the `DatabaseQueue` trait,
//! optimized for PostgreSQL's JSONB and advanced querying capabilities.

use super::{DatabaseQueue, DeadJobSummary, QueueStats};
use crate::{
    Result,
    job::{Job, JobId, JobStatus},
    priority::JobPriority,
    rate_limit::ThrottleConfig,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, Postgres, Row};
use std::collections::HashMap;

#[derive(FromRow, Clone)]
pub(crate) struct JobRow {
    pub id: uuid::Uuid,
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i32,
    pub attempts: i32,
    pub max_attempts: i32,
    pub timeout_seconds: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub timed_out_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub cron_schedule: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub recurring: bool,
    pub timezone: Option<String>,
    pub batch_id: Option<uuid::Uuid>,
    pub result_data: Option<serde_json::Value>,
    pub result_stored_at: Option<DateTime<Utc>>,
    pub result_expires_at: Option<DateTime<Utc>>,
    pub result_storage_type: Option<String>,
    pub result_ttl_seconds: Option<i64>,
    pub result_max_size_bytes: Option<i64>,
    pub depends_on: Option<serde_json::Value>,
    pub dependents: Option<serde_json::Value>,
    pub dependency_status: Option<String>,
    pub workflow_id: Option<uuid::Uuid>,
    pub workflow_name: Option<String>,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub span_context: Option<String>,
}

impl JobRow {
    pub fn into_job(self) -> Result<Job> {
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
            batch_id: self.batch_id,
            result_config: crate::job::ResultConfig {
                storage: self
                    .result_storage_type
                    .as_ref()
                    .map(|s| match s.as_str() {
                        "database" => crate::job::ResultStorage::Database,
                        "memory" => crate::job::ResultStorage::Memory,
                        "none" => crate::job::ResultStorage::None,
                        _ => crate::job::ResultStorage::None,
                    })
                    .unwrap_or(crate::job::ResultStorage::None),
                ttl: self
                    .result_ttl_seconds
                    .map(|s| std::time::Duration::from_secs(s as u64)),
                max_size_bytes: self.result_max_size_bytes.map(|b| b as usize),
            },
            result_data: self.result_data,
            result_stored_at: self.result_stored_at,
            result_expires_at: self.result_expires_at,
            retry_strategy: None, // Will be populated from job data when needed
            depends_on: self
                .depends_on
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            dependents: self
                .dependents
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            dependency_status: self
                .dependency_status
                .as_ref()
                .and_then(|s| crate::workflow::DependencyStatus::parse_from_db(s).ok())
                .unwrap_or(crate::workflow::DependencyStatus::None),
            workflow_id: self.workflow_id,
            workflow_name: self.workflow_name,
            trace_id: self.trace_id,
            correlation_id: self.correlation_id,
            parent_span_id: self.parent_span_id,
            span_context: self.span_context,
        })
    }
}

#[derive(FromRow)]
pub(crate) struct DeadJobRow {
    pub id: uuid::Uuid,
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i32,
    pub attempts: i32,
    pub max_attempts: i32,
    pub timeout_seconds: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub timed_out_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

impl DeadJobRow {
    pub fn into_job(self) -> Job {
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
            batch_id: None, // DeadJobRow doesn't track batch_id
            result_config: crate::job::ResultConfig::default(),
            result_data: None,
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: None,
            depends_on: Vec::new(),
            dependents: Vec::new(),
            dependency_status: crate::workflow::DependencyStatus::None,
            workflow_id: None,
            workflow_name: None,
            trace_id: None,
            correlation_id: None,
            parent_span_id: None,
            span_context: None,
        }
    }
}

#[async_trait]
impl DatabaseQueue for crate::queue::JobQueue<Postgres> {
    type Database = Postgres;

    async fn enqueue(&self, job: Job) -> Result<JobId> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_jobs (
                id, queue_name, payload, status, priority, attempts, max_attempts, 
                timeout_seconds, created_at, scheduled_at, error_message, 
                cron_schedule, next_run_at, recurring, timezone, batch_id,
                result_storage_type, result_ttl_seconds, result_max_size_bytes,
                depends_on, dependents, dependency_status, workflow_id, workflow_name,
                trace_id, correlation_id, parent_span_id, span_context
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
            "#,
        )
        .bind(job.id)
        .bind(&job.queue_name)
        .bind(&job.payload)
        .bind(serde_json::to_string(&job.status)?)
        .bind(job.priority.as_i32())
        .bind(job.attempts)
        .bind(job.max_attempts)
        .bind(job.timeout.map(|d| d.as_secs() as i32))
        .bind(job.created_at)
        .bind(job.scheduled_at)
        .bind(&job.error_message)
        .bind(&job.cron_schedule)
        .bind(job.next_run_at)
        .bind(job.recurring)
        .bind(&job.timezone)
        .bind(job.batch_id)
        .bind(match job.result_config.storage {
            crate::job::ResultStorage::Database => "database",
            crate::job::ResultStorage::Memory => "memory", 
            crate::job::ResultStorage::None => "none",
        })
        .bind(job.result_config.ttl.map(|d| d.as_secs() as i64))
        .bind(job.result_config.max_size_bytes.map(|s| s as i64))
        .bind(serde_json::to_value(&job.depends_on)?)
        .bind(serde_json::to_value(&job.dependents)?)
        .bind(job.dependency_status.as_str())
        .bind(job.workflow_id)
        .bind(&job.workflow_name)
        .bind(&job.trace_id)
        .bind(&job.correlation_id)
        .bind(&job.parent_span_id)
        .bind(&job.span_context)
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
                WHERE queue_name = $3 AND status = $4 AND scheduled_at <= $5
                  AND (dependency_status = 'none' OR dependency_status = 'satisfied')
                ORDER BY priority DESC, scheduled_at ASC 
                FOR UPDATE SKIP LOCKED 
                LIMIT 1
            )
            RETURNING id, queue_name, payload, status, priority, attempts, max_attempts, 
                     timeout_seconds, created_at, scheduled_at, started_at, completed_at, 
                     failed_at, timed_out_at, error_message, cron_schedule, next_run_at, 
                     recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at,
                     result_storage_type, result_ttl_seconds, result_max_size_bytes,
                     depends_on, dependents, dependency_status, workflow_id, workflow_name,
                     trace_id, correlation_id, parent_span_id, span_context
            "#,
        )
        .bind(serde_json::to_string(&JobStatus::Running)?)
        .bind(Utc::now())
        .bind(queue_name)
        .bind(serde_json::to_string(&JobStatus::Pending)?)
        .bind(Utc::now())
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
            let batch_id: Option<uuid::Uuid> = row.get("batch_id");
            let result_data: Option<serde_json::Value> = row.get("result_data");
            let result_stored_at: Option<DateTime<Utc>> = row.get("result_stored_at");
            let result_expires_at: Option<DateTime<Utc>> = row.get("result_expires_at");
            let result_storage_type: Option<String> = row.get("result_storage_type");
            let result_ttl_seconds: Option<i64> = row.get("result_ttl_seconds");
            let result_max_size_bytes: Option<i64> = row.get("result_max_size_bytes");
            let depends_on: Option<serde_json::Value> = row.get("depends_on");
            let dependents: Option<serde_json::Value> = row.get("dependents");
            let dependency_status: Option<String> = row.get("dependency_status");
            let workflow_id: Option<uuid::Uuid> = row.get("workflow_id");
            let workflow_name: Option<String> = row.get("workflow_name");
            let trace_id: Option<String> = row.get("trace_id");
            let correlation_id: Option<String> = row.get("correlation_id");
            let parent_span_id: Option<String> = row.get("parent_span_id");
            let span_context: Option<String> = row.get("span_context");

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
                batch_id,
                result_config: crate::job::ResultConfig {
                    storage: result_storage_type
                        .as_ref()
                        .map(|s| match s.as_str() {
                            "database" => crate::job::ResultStorage::Database,
                            "memory" => crate::job::ResultStorage::Memory,
                            "none" => crate::job::ResultStorage::None,
                            _ => crate::job::ResultStorage::None,
                        })
                        .unwrap_or(crate::job::ResultStorage::None),
                    ttl: result_ttl_seconds.map(|s| std::time::Duration::from_secs(s as u64)),
                    max_size_bytes: result_max_size_bytes.map(|b| b as usize),
                },
                result_data,
                result_stored_at,
                result_expires_at,
                retry_strategy: None,
                depends_on: depends_on
                    .map(|v| serde_json::from_value(v).unwrap_or_default())
                    .unwrap_or_default(),
                dependents: dependents
                    .map(|v| serde_json::from_value(v).unwrap_or_default())
                    .unwrap_or_default(),
                dependency_status: dependency_status
                    .as_ref()
                    .and_then(|s| crate::workflow::DependencyStatus::parse_from_db(s).ok())
                    .unwrap_or(crate::workflow::DependencyStatus::None),
                workflow_id,
                workflow_name,
                trace_id,
                correlation_id,
                parent_span_id,
                span_context,
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
            SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name
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
        for priority in priority_jobs.keys() {
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
                    "UPDATE hammerwork_jobs SET status = $1, started_at = $2, attempts = attempts + 1 WHERE id = $3 RETURNING id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name"
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
                    let batch_id: Option<uuid::Uuid> = row.get("batch_id");
                    let result_data: Option<serde_json::Value> = row.get("result_data");
                    let result_stored_at: Option<DateTime<Utc>> = row.get("result_stored_at");
                    let result_expires_at: Option<DateTime<Utc>> = row.get("result_expires_at");

                    return Ok(Some(Job {
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
                        batch_id,
                        result_config: crate::job::ResultConfig::default(),
                        result_data,
                        result_stored_at,
                        result_expires_at,
                        retry_strategy: None,
                        depends_on: Vec::new(),
                        dependents: Vec::new(),
                        dependency_status: crate::workflow::DependencyStatus::None,
                        workflow_id: None,
                        workflow_name: None,
                        trace_id: None,
                        correlation_id: None,
                        parent_span_id: None,
                        span_context: None,
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
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context FROM hammerwork_jobs WHERE id = $1"
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

    async fn enqueue_batch(&self, batch: crate::batch::JobBatch) -> Result<crate::batch::BatchId> {
        use crate::batch::BatchStatus;

        // Validate the batch first
        batch.validate()?;

        let mut tx = self.pool.begin().await?;

        // Insert batch metadata
        sqlx::query(
            r#"
            INSERT INTO hammerwork_batches 
            (id, batch_name, total_jobs, completed_jobs, failed_jobs, pending_jobs, status, failure_mode, created_at, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#
        )
        .bind(batch.id)
        .bind(&batch.name)
        .bind(batch.jobs.len() as i32)
        .bind(0i32) // completed_jobs
        .bind(0i32) // failed_jobs  
        .bind(batch.jobs.len() as i32) // pending_jobs
        .bind(serde_json::to_string(&BatchStatus::Pending)?)
        .bind(serde_json::to_string(&batch.failure_mode)?)
        .bind(batch.created_at)
        .bind(serde_json::to_value(&batch.metadata)?)
        .execute(&mut *tx)
        .await?;

        // Bulk insert jobs using UNNEST for optimal performance
        if !batch.jobs.is_empty() {
            let mut job_ids = Vec::new();
            let mut queue_names = Vec::new();
            let mut payloads = Vec::new();
            let mut statuses = Vec::new();
            let mut priorities = Vec::new();
            let mut attempts = Vec::new();
            let mut max_attempts = Vec::new();
            let mut timeout_seconds = Vec::new();
            let mut created_ats = Vec::new();
            let mut scheduled_ats = Vec::new();
            let mut started_ats = Vec::new();
            let mut completed_ats = Vec::new();
            let mut failed_ats = Vec::new();
            let mut timed_out_ats = Vec::new();
            let mut error_messages = Vec::new();
            let mut cron_schedules = Vec::new();
            let mut next_run_ats = Vec::new();
            let mut recurrings = Vec::new();
            let mut timezones = Vec::new();
            let mut batch_ids = Vec::new();

            for job in &batch.jobs {
                job_ids.push(job.id);
                queue_names.push(&job.queue_name);
                payloads.push(&job.payload);
                statuses.push(serde_json::to_string(&job.status)?);
                priorities.push(job.priority.as_i32());
                attempts.push(job.attempts);
                max_attempts.push(job.max_attempts);
                timeout_seconds.push(job.timeout.map(|t| t.as_secs() as i32));
                created_ats.push(job.created_at);
                scheduled_ats.push(job.scheduled_at);
                started_ats.push(job.started_at);
                completed_ats.push(job.completed_at);
                failed_ats.push(job.failed_at);
                timed_out_ats.push(job.timed_out_at);
                error_messages.push(&job.error_message);
                cron_schedules.push(&job.cron_schedule);
                next_run_ats.push(job.next_run_at);
                recurrings.push(job.recurring);
                timezones.push(&job.timezone);
                batch_ids.push(Some(batch.id));
            }

            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs 
                (id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id)
                SELECT * FROM UNNEST(
                    $1::uuid[], $2::varchar[], $3::jsonb[], $4::varchar[], $5::integer[], $6::integer[], $7::integer[], $8::integer[], 
                    $9::timestamptz[], $10::timestamptz[], $11::timestamptz[], $12::timestamptz[], $13::timestamptz[], $14::timestamptz[], 
                    $15::text[], $16::varchar[], $17::timestamptz[], $18::boolean[], $19::varchar[], $20::uuid[]
                )
                "#
            )
            .bind(&job_ids)
            .bind(&queue_names)
            .bind(&payloads)
            .bind(&statuses)
            .bind(&priorities)
            .bind(&attempts)
            .bind(&max_attempts)
            .bind(&timeout_seconds)
            .bind(&created_ats)
            .bind(&scheduled_ats)
            .bind(&started_ats)
            .bind(&completed_ats)
            .bind(&failed_ats)
            .bind(&timed_out_ats)
            .bind(&error_messages)
            .bind(&cron_schedules)
            .bind(&next_run_ats)
            .bind(&recurrings)
            .bind(&timezones)
            .bind(&batch_ids)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(batch.id)
    }

    async fn get_batch_status(
        &self,
        batch_id: crate::batch::BatchId,
    ) -> Result<crate::batch::BatchResult> {
        use crate::batch::BatchResult;
        use std::collections::HashMap;

        // Get batch metadata
        let batch_row = sqlx::query(
            "SELECT batch_name, total_jobs, completed_jobs, failed_jobs, pending_jobs, status, failure_mode, created_at, completed_at, error_summary, metadata FROM hammerwork_batches WHERE id = $1"
        )
        .bind(batch_id)
        .fetch_optional(&self.pool)
        .await?;

        let batch_row = batch_row.ok_or_else(|| crate::HammerworkError::JobNotFound {
            id: batch_id.to_string(),
        })?;

        let total_jobs: i32 = batch_row.get("total_jobs");
        let completed_jobs: i32 = batch_row.get("completed_jobs");
        let failed_jobs: i32 = batch_row.get("failed_jobs");
        let pending_jobs: i32 = batch_row.get("pending_jobs");
        let status: String = batch_row.get("status");
        let created_at: DateTime<Utc> = batch_row.get("created_at");
        let completed_at: Option<DateTime<Utc>> = batch_row.get("completed_at");
        let error_summary: Option<String> = batch_row.get("error_summary");

        // Get job errors for CollectErrors mode
        let job_errors: Vec<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, error_message FROM hammerwork_jobs WHERE batch_id = $1 AND error_message IS NOT NULL"
        )
        .bind(batch_id)
        .fetch_all(&self.pool)
        .await?;

        let job_errors_map: HashMap<uuid::Uuid, String> = job_errors.into_iter().collect();

        Ok(BatchResult {
            batch_id,
            total_jobs: total_jobs as u32,
            completed_jobs: completed_jobs as u32,
            failed_jobs: failed_jobs as u32,
            pending_jobs: pending_jobs as u32,
            status: serde_json::from_str(&status)?,
            created_at,
            completed_at,
            error_summary,
            job_errors: job_errors_map,
        })
    }

    async fn get_batch_jobs(&self, batch_id: crate::batch::BatchId) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context FROM hammerwork_jobs WHERE batch_id = $1 ORDER BY created_at ASC"
        )
        .bind(batch_id)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn delete_batch(&self, batch_id: crate::batch::BatchId) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Delete all jobs in the batch
        sqlx::query("DELETE FROM hammerwork_jobs WHERE batch_id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;

        // Delete the batch metadata
        sqlx::query("DELETE FROM hammerwork_batches WHERE id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

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
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = $1 ORDER BY failed_at DESC LIMIT $2 OFFSET $3"
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
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = $1 AND queue_name = $2 ORDER BY failed_at DESC LIMIT $3 OFFSET $4"
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

    async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
        use crate::stats::JobStatistics;
        use std::collections::HashMap;
        use std::time::Duration;

        // Get job counts by status
        let status_counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 GROUP BY status",
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
            "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 GROUP BY status",
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

    async fn enqueue_cron_job(&self, job: Job) -> Result<JobId> {
        // For cron jobs, we use the regular enqueue method
        // The job should already have the cron fields set
        self.enqueue(job).await
    }

    async fn get_due_cron_jobs(&self, queue_name: Option<&str>) -> Result<Vec<Job>> {
        let query = if queue_name.is_some() {
            r#"
            SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name
            FROM hammerwork_jobs 
            WHERE recurring = TRUE 
            AND queue_name = $1
            AND (next_run_at IS NULL OR next_run_at <= $2)
            AND status = $3
            ORDER BY next_run_at ASC
            "#
        } else {
            r#"
            SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name
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

    async fn reschedule_cron_job(&self, job_id: JobId, next_run_at: DateTime<Utc>) -> Result<()> {
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
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context FROM hammerwork_jobs WHERE queue_name = $1 AND recurring = TRUE ORDER BY next_run_at ASC"
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
            "SELECT COUNT(*) FROM hammerwork_jobs WHERE queue_name = $1 AND status = $2",
        )
        .bind(queue_name)
        .bind(serde_json::to_string(&JobStatus::Pending)?)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 as u64)
    }

    // Job result storage and retrieval operations
    async fn store_job_result(
        &self,
        job_id: JobId,
        result_data: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = $1, result_stored_at = $2, result_expires_at = $3 WHERE id = $4"
        )
        .bind(result_data)
        .bind(Utc::now())
        .bind(expires_at)
        .bind(job_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_job_result(&self, job_id: JobId) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            "SELECT result_data FROM hammerwork_jobs WHERE id = $1 AND result_data IS NOT NULL AND (result_expires_at IS NULL OR result_expires_at > $2)"
        )
        .bind(job_id)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let result_data: Option<serde_json::Value> = row.get("result_data");
                Ok(result_data)
            }
            None => Ok(None),
        }
    }

    async fn delete_job_result(&self, job_id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = NULL, result_stored_at = NULL, result_expires_at = NULL WHERE id = $1"
        )
        .bind(job_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn cleanup_expired_results(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = NULL, result_stored_at = NULL, result_expires_at = NULL WHERE result_expires_at IS NOT NULL AND result_expires_at <= $1"
        )
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    // Workflow and dependency management methods
    async fn enqueue_workflow(
        &self,
        _workflow: crate::workflow::JobGroup,
    ) -> Result<crate::workflow::WorkflowId> {
        todo!("Workflow enqueuing will be implemented next")
    }

    async fn get_workflow_status(
        &self,
        _workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Option<crate::workflow::JobGroup>> {
        todo!("Workflow status retrieval will be implemented next")
    }

    async fn resolve_job_dependencies(&self, _completed_job_id: JobId) -> Result<Vec<JobId>> {
        todo!("Dependency resolution will be implemented next")
    }

    async fn get_ready_jobs(&self, _queue_name: &str, _limit: u32) -> Result<Vec<Job>> {
        todo!("Ready jobs query will be implemented next")
    }

    async fn fail_job_dependencies(&self, _failed_job_id: JobId) -> Result<Vec<JobId>> {
        todo!("Dependency failure propagation will be implemented next")
    }

    async fn get_workflow_jobs(
        &self,
        _workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Vec<Job>> {
        todo!("Workflow jobs query will be implemented next")
    }

    async fn cancel_workflow(&self, _workflow_id: crate::workflow::WorkflowId) -> Result<()> {
        todo!("Workflow cancellation will be implemented next")
    }

    // Job archival operations
    async fn archive_jobs(
        &self,
        queue_name: Option<&str>,
        policy: &crate::archive::ArchivalPolicy,
        config: &crate::archive::ArchivalConfig,
        reason: crate::archive::ArchivalReason,
        archived_by: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats> {
        use crate::archive::ArchivalStats;
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        if !policy.enabled {
            return Ok(ArchivalStats::default());
        }

        let start_time = Utc::now();
        let mut jobs_archived = 0u64;
        let mut bytes_archived = 0u64;
        let mut total_compression_ratio = 0.0;

        // Execute transaction to archive jobs
        let mut tx = self.pool.begin().await?;

        // Build a simpler query for eligible jobs
        let mut jobs_to_archive = Vec::new();

        // Check each policy condition separately and combine results
        if let Some(completed_after) = policy.archive_completed_after {
            let threshold = Utc::now() - completed_after;
            let mut query = sqlx::query_as::<_, JobRow>(
                "SELECT * FROM hammerwork_jobs 
                WHERE archived_at IS NULL 
                AND status = 'Completed' 
                AND completed_at IS NOT NULL 
                AND completed_at <= $1
                ORDER BY created_at ASC 
                LIMIT $2",
            )
            .bind(threshold)
            .bind(policy.batch_size as i64);

            if let Some(queue) = queue_name {
                query = sqlx::query_as::<_, JobRow>(
                    "SELECT * FROM hammerwork_jobs 
                    WHERE archived_at IS NULL 
                    AND queue_name = $1
                    AND status = 'Completed' 
                    AND completed_at IS NOT NULL 
                    AND completed_at <= $2
                    ORDER BY created_at ASC 
                    LIMIT $3",
                )
                .bind(queue)
                .bind(threshold)
                .bind(policy.batch_size as i64);
            }

            jobs_to_archive.extend(query.fetch_all(&mut *tx).await?);
        }

        if let Some(failed_after) = policy.archive_failed_after {
            let threshold = Utc::now() - failed_after;
            let mut query = sqlx::query_as::<_, JobRow>(
                "SELECT * FROM hammerwork_jobs 
                WHERE archived_at IS NULL 
                AND status = 'Failed' 
                AND failed_at IS NOT NULL 
                AND failed_at <= $1
                ORDER BY created_at ASC 
                LIMIT $2",
            )
            .bind(threshold)
            .bind(policy.batch_size as i64);

            if let Some(queue) = queue_name {
                query = sqlx::query_as::<_, JobRow>(
                    "SELECT * FROM hammerwork_jobs 
                    WHERE archived_at IS NULL 
                    AND queue_name = $1
                    AND status = 'Failed' 
                    AND failed_at IS NOT NULL 
                    AND failed_at <= $2
                    ORDER BY created_at ASC 
                    LIMIT $3",
                )
                .bind(queue)
                .bind(threshold)
                .bind(policy.batch_size as i64);
            }

            jobs_to_archive.extend(query.fetch_all(&mut *tx).await?);
        }

        if let Some(dead_after) = policy.archive_dead_after {
            let threshold = Utc::now() - dead_after;
            let mut query = sqlx::query_as::<_, JobRow>(
                "SELECT * FROM hammerwork_jobs 
                WHERE archived_at IS NULL 
                AND status = 'Dead' 
                AND failed_at IS NOT NULL 
                AND failed_at <= $1
                ORDER BY created_at ASC 
                LIMIT $2",
            )
            .bind(threshold)
            .bind(policy.batch_size as i64);

            if let Some(queue) = queue_name {
                query = sqlx::query_as::<_, JobRow>(
                    "SELECT * FROM hammerwork_jobs 
                    WHERE archived_at IS NULL 
                    AND queue_name = $1
                    AND status = 'Dead' 
                    AND failed_at IS NOT NULL 
                    AND failed_at <= $2
                    ORDER BY created_at ASC 
                    LIMIT $3",
                )
                .bind(queue)
                .bind(threshold)
                .bind(policy.batch_size as i64);
            }

            jobs_to_archive.extend(query.fetch_all(&mut *tx).await?);
        }

        if let Some(timed_out_after) = policy.archive_timed_out_after {
            let threshold = Utc::now() - timed_out_after;
            let mut query = sqlx::query_as::<_, JobRow>(
                "SELECT * FROM hammerwork_jobs 
                WHERE archived_at IS NULL 
                AND status = 'TimedOut' 
                AND timed_out_at IS NOT NULL 
                AND timed_out_at <= $1
                ORDER BY created_at ASC 
                LIMIT $2",
            )
            .bind(threshold)
            .bind(policy.batch_size as i64);

            if let Some(queue) = queue_name {
                query = sqlx::query_as::<_, JobRow>(
                    "SELECT * FROM hammerwork_jobs 
                    WHERE archived_at IS NULL 
                    AND queue_name = $1
                    AND status = 'TimedOut' 
                    AND timed_out_at IS NOT NULL 
                    AND timed_out_at <= $2
                    ORDER BY created_at ASC 
                    LIMIT $3",
                )
                .bind(queue)
                .bind(threshold)
                .bind(policy.batch_size as i64);
            }

            jobs_to_archive.extend(query.fetch_all(&mut *tx).await?);
        }

        // Remove duplicates (if jobs match multiple criteria) and limit to batch size
        jobs_to_archive.sort_by_key(|job| job.created_at);
        jobs_to_archive.dedup_by_key(|job| job.id);
        jobs_to_archive.truncate(policy.batch_size);

        for job_row in jobs_to_archive {
            let job = job_row.into_job()?;
            let payload_json = serde_json::to_vec(&job.payload)?;
            let original_size = payload_json.len();

            let (final_payload, is_compressed) = if policy.compress_payloads {
                let mut encoder =
                    GzEncoder::new(Vec::new(), Compression::new(config.compression_level));
                encoder.write_all(&payload_json)?;
                let compressed = encoder.finish()?;

                if compressed.len() < original_size {
                    total_compression_ratio += original_size as f64 / compressed.len() as f64;
                    (compressed, true)
                } else {
                    (payload_json, false)
                }
            } else {
                (payload_json, false)
            };

            // Insert into archive table
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs_archive (
                    id, queue_name, payload, payload_compressed, original_payload_size,
                    status, priority, attempts, max_attempts, created_at, scheduled_at,
                    started_at, completed_at, failed_at, timed_out_at, archived_at,
                    error_message, result, result_ttl, retry_strategy, timeout_seconds,
                    priority_weight, cron_schedule, next_run_at, recurring, timezone,
                    batch_id, depends_on, dependency_status, result_config,
                    trace_id, correlation_id, parent_span_id, span_context,
                    archival_reason, archived_by, original_table
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                    $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30,
                    $31, $32, $33, $34, $35, $36, $37
                )
            "#,
            )
            .bind(job.id)
            .bind(&job.queue_name)
            .bind(&final_payload)
            .bind(is_compressed)
            .bind(original_size as i32)
            .bind(serde_json::to_string(&job.status)?)
            .bind(job.priority.to_string())
            .bind(job.attempts)
            .bind(job.max_attempts)
            .bind(job.created_at)
            .bind(job.scheduled_at)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(job.failed_at)
            .bind(job.timed_out_at)
            .bind(Utc::now())
            .bind(job.error_message)
            .bind(job.result_data)
            .bind(job.result_expires_at)
            .bind(
                job.retry_strategy
                    .map(|rs| serde_json::to_string(&rs).unwrap_or_default()),
            )
            .bind(job.timeout.map(|t| t.as_secs() as i32))
            .bind(job.priority.weight() as i32)
            .bind(job.cron_schedule)
            .bind(job.next_run_at)
            .bind(job.recurring)
            .bind(job.timezone)
            .bind(job.batch_id)
            .bind(if job.depends_on.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&job.depends_on)?)
            })
            .bind(serde_json::to_string(&job.dependency_status)?)
            .bind(serde_json::to_value(&job.result_config)?)
            .bind(job.trace_id)
            .bind(job.correlation_id)
            .bind(job.parent_span_id)
            .bind(job.span_context)
            .bind(reason.to_string())
            .bind(archived_by)
            .bind("hammerwork_jobs")
            .execute(&mut *tx)
            .await?;

            // Update main table to mark as archived
            sqlx::query(
                "UPDATE hammerwork_jobs SET archived_at = $1, archival_reason = $2, archival_policy_applied = $3 WHERE id = $4"
            )
            .bind(Utc::now())
            .bind(reason.to_string())
            .bind(archived_by)
            .bind(job.id)
            .execute(&mut *tx)
            .await?;

            jobs_archived += 1;
            bytes_archived += final_payload.len() as u64;
        }

        tx.commit().await?;

        let operation_duration = Utc::now() - start_time;
        let compression_ratio = if jobs_archived > 0 && total_compression_ratio > 0.0 {
            total_compression_ratio / jobs_archived as f64
        } else {
            1.0
        };

        Ok(ArchivalStats {
            jobs_archived,
            jobs_purged: 0,
            bytes_archived,
            bytes_purged: 0,
            compression_ratio,
            operation_duration: operation_duration
                .to_std()
                .unwrap_or(std::time::Duration::from_secs(0)),
            last_run_at: Utc::now(),
        })
    }

    async fn restore_archived_job(&self, job_id: JobId) -> Result<Job> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut tx = self.pool.begin().await?;

        // Get archived job
        let archived_row = sqlx::query(
            r#"
            SELECT 
                id, queue_name, payload, payload_compressed, original_payload_size,
                status, priority, attempts, max_attempts, created_at, scheduled_at,
                started_at, completed_at, failed_at, timed_out_at,
                error_message, result, result_ttl, retry_strategy, timeout_seconds,
                cron_schedule, next_run_at, recurring, timezone, batch_id,
                depends_on, dependency_status, result_config,
                trace_id, correlation_id, parent_span_id, span_context
            FROM hammerwork_jobs_archive 
            WHERE id = $1
        "#,
        )
        .bind(job_id)
        .fetch_one(&mut *tx)
        .await?;

        // Decompress payload if needed
        let payload_bytes: Vec<u8> = archived_row.get("payload");
        let is_compressed: bool = archived_row.get("payload_compressed");

        let payload_json = if is_compressed {
            let mut decoder = GzDecoder::new(&payload_bytes[..]);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            decompressed
        } else {
            payload_bytes
        };

        let payload: serde_json::Value = serde_json::from_slice(&payload_json)?;

        // Create job object
        let job = Job {
            id: archived_row.get("id"),
            queue_name: archived_row.get("queue_name"),
            payload,
            status: JobStatus::Pending, // Reset to pending for re-processing
            priority: archived_row
                .get::<String, _>("priority")
                .parse()
                .unwrap_or(JobPriority::Normal),
            attempts: 0, // Reset attempts
            max_attempts: archived_row.get("max_attempts"),
            created_at: archived_row.get("created_at"),
            scheduled_at: Utc::now(), // Schedule for immediate processing
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: archived_row
                .get::<Option<i32>, _>("timeout_seconds")
                .map(|s| std::time::Duration::from_secs(s as u64)),
            error_message: None, // Clear error message
            cron_schedule: archived_row.get("cron_schedule"),
            next_run_at: archived_row.get("next_run_at"),
            recurring: archived_row.get("recurring"),
            timezone: archived_row.get("timezone"),
            batch_id: archived_row.get("batch_id"),
            result_config: archived_row
                .get::<Option<serde_json::Value>, _>("result_config")
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            result_data: None, // Clear result data
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: archived_row
                .get::<Option<String>, _>("retry_strategy")
                .and_then(|s| serde_json::from_str(&s).ok()),
            depends_on: archived_row
                .get::<Option<serde_json::Value>, _>("depends_on")
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            dependents: Vec::new(),
            dependency_status: archived_row
                .get::<Option<String>, _>("dependency_status")
                .map(|s| serde_json::from_str(&s).unwrap_or_default())
                .unwrap_or_default(),
            workflow_id: None,
            workflow_name: None,
            trace_id: archived_row.get("trace_id"),
            correlation_id: archived_row.get("correlation_id"),
            parent_span_id: archived_row.get("parent_span_id"),
            span_context: archived_row.get("span_context"),
        };

        // Insert back into main table
        self.enqueue_with_tx(&mut tx, job.clone()).await?;

        // Remove from archive table
        sqlx::query("DELETE FROM hammerwork_jobs_archive WHERE id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;

        // Clear archival metadata from main table
        sqlx::query("UPDATE hammerwork_jobs SET archived_at = NULL, archival_reason = NULL, archival_policy_applied = NULL WHERE id = $1")
            .bind(job_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(job)
    }

    async fn list_archived_jobs(
        &self,
        queue_name: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<crate::archive::ArchivedJob>> {
        use crate::archive::ArchivedJob;

        let mut query = "SELECT id, queue_name, status, created_at, archived_at, archival_reason, original_payload_size, payload_compressed, archived_by FROM hammerwork_jobs_archive".to_string();
        let mut conditions = Vec::new();

        if let Some(_queue) = queue_name {
            conditions.push("queue_name = $1".to_string());
        }

        if !conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(" ORDER BY archived_at DESC");

        if let Some(_limit) = limit {
            let param_num = if queue_name.is_some() { 2 } else { 1 };
            query.push_str(&format!(" LIMIT ${}", param_num));
        }

        if let Some(_offset) = offset {
            let param_num = if queue_name.is_some() && limit.is_some() {
                3
            } else if queue_name.is_some() || limit.is_some() {
                2
            } else {
                1
            };
            query.push_str(&format!(" OFFSET ${}", param_num));
        }

        let mut sql_query = sqlx::query(&query);

        if let Some(queue) = queue_name {
            sql_query = sql_query.bind(queue);
        }
        if let Some(limit) = limit {
            sql_query = sql_query.bind(limit as i64);
        }
        if let Some(offset) = offset {
            sql_query = sql_query.bind(offset as i64);
        }

        let rows = sql_query.fetch_all(&self.pool).await?;

        let mut archived_jobs = Vec::new();
        for row in rows {
            archived_jobs.push(ArchivedJob {
                id: row.get("id"),
                queue_name: row.get("queue_name"),
                status: serde_json::from_str::<JobStatus>(&row.get::<String, _>("status"))
                    .unwrap_or(JobStatus::Dead),
                created_at: row.get("created_at"),
                archived_at: row.get("archived_at"),
                archival_reason: serde_json::from_str(&row.get::<String, _>("archival_reason"))
                    .unwrap_or(crate::archive::ArchivalReason::Automatic),
                original_payload_size: row
                    .get::<Option<i32>, _>("original_payload_size")
                    .map(|s| s as usize),
                payload_compressed: row.get("payload_compressed"),
                archived_by: row.get("archived_by"),
            });
        }

        Ok(archived_jobs)
    }

    async fn purge_archived_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query("DELETE FROM hammerwork_jobs_archive WHERE archived_at <= $1")
            .bind(older_than)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    async fn get_archival_stats(
        &self,
        queue_name: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats> {
        use crate::archive::ArchivalStats;

        let mut base_query = "SELECT 
            COUNT(*) as job_count,
            COALESCE(SUM(original_payload_size), 0) as total_original_size,
            COALESCE(SUM(LENGTH(payload)), 0) as total_compressed_size,
            MAX(archived_at) as last_archived_at
            FROM hammerwork_jobs_archive"
            .to_string();

        if let Some(_queue) = queue_name {
            base_query.push_str(" WHERE queue_name = $1");
        }

        let mut query = sqlx::query(&base_query);
        if let Some(queue) = queue_name {
            query = query.bind(queue);
        }

        let row = query.fetch_one(&self.pool).await?;

        let job_count: i64 = row.get("job_count");
        let total_original_size: i64 = row.get("total_original_size");
        let total_compressed_size: i64 = row.get("total_compressed_size");
        let last_archived_at: Option<DateTime<Utc>> = row.get("last_archived_at");

        let compression_ratio = if total_compressed_size > 0 {
            total_original_size as f64 / total_compressed_size as f64
        } else {
            1.0
        };

        Ok(ArchivalStats {
            jobs_archived: job_count as u64,
            jobs_purged: 0, // This would need separate tracking
            bytes_archived: total_compressed_size as u64,
            bytes_purged: 0,
            compression_ratio,
            operation_duration: std::time::Duration::from_secs(0),
            last_run_at: last_archived_at.unwrap_or(Utc::now()),
        })
    }
}

// Helper method for enqueueing with an existing transaction
impl crate::queue::JobQueue<Postgres> {
    async fn enqueue_with_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, Postgres>,
        job: Job,
    ) -> Result<JobId> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_jobs (
                id, queue_name, payload, status, priority, attempts, max_attempts, 
                timeout_seconds, created_at, scheduled_at, error_message, 
                cron_schedule, next_run_at, recurring, timezone, batch_id,
                result_storage_type, result_ttl_seconds, result_max_size_bytes,
                depends_on, dependents, dependency_status, workflow_id, workflow_name,
                trace_id, correlation_id, parent_span_id, span_context
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28
            )
            "#,
        )
        .bind(job.id)
        .bind(&job.queue_name)
        .bind(&job.payload)
        .bind(serde_json::to_string(&job.status)?)
        .bind(job.priority as i32)
        .bind(job.attempts)
        .bind(job.max_attempts)
        .bind(job.timeout.map(|d| d.as_secs() as i32))
        .bind(job.created_at)
        .bind(job.scheduled_at)
        .bind(job.error_message)
        .bind(job.cron_schedule)
        .bind(job.next_run_at)
        .bind(job.recurring)
        .bind(job.timezone)
        .bind(job.batch_id)
        .bind(match job.result_config.storage {
            crate::job::ResultStorage::Database => Some("database"),
            crate::job::ResultStorage::Memory => Some("memory"),
            crate::job::ResultStorage::None => Some("none"),
        })
        .bind(job.result_config.ttl.map(|t| t.as_secs() as i64))
        .bind(job.result_config.max_size_bytes.map(|s| s as i64))
        .bind(if job.depends_on.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&job.depends_on)?)
        })
        .bind(if job.dependents.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&job.dependents)?)
        })
        .bind(serde_json::to_string(&job.dependency_status)?)
        .bind(job.workflow_id)
        .bind(job.workflow_name)
        .bind(job.trace_id)
        .bind(job.correlation_id)
        .bind(job.parent_span_id)
        .bind(job.span_context)
        .execute(&mut **tx)
        .await?;

        Ok(job.id)
    }
}
