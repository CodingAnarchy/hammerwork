use anyhow::Result;
use clap::Subcommand;
use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobPriority, JobStatus};
use sqlx::Row;
use tracing::info;

use crate::config::Config;
use crate::utils::database::{DatabasePool, JobQueueWrapper};
use crate::utils::display::JobTable;
use crate::utils::job_ops::{
    JobSelector, cancel_many, cancel_one, retry_many, retry_one, select_job_ids,
};
use crate::utils::sql::{
    Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg, execute_binds,
};
use crate::utils::validation::{validate_json_payload, validate_priority, validate_status};

/// Statuses `job retry` re-runs (stored capitalized).
const RETRYABLE_STATUSES: [JobStatus; 3] =
    [JobStatus::Failed, JobStatus::Dead, JobStatus::TimedOut];

#[derive(Subcommand)]
pub enum JobCommand {
    #[command(about = "List jobs in the queue")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name to filter by")]
        queue: Option<String>,
        #[arg(short = 't', long, help = "Job status to filter by")]
        status: Option<String>,
        #[arg(short = 'r', long, help = "Job priority to filter by")]
        priority: Option<String>,
        #[arg(short, long, help = "Maximum number of jobs to display")]
        limit: Option<u32>,
        #[arg(long, help = "Show only failed jobs")]
        failed: bool,
        #[arg(long, help = "Show only completed jobs")]
        completed: bool,
        #[arg(long, help = "Show jobs from last N hours")]
        last_hours: Option<u32>,
    },
    #[command(about = "Show details of a specific job")]
    Show {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
    },
    #[command(about = "Enqueue a new job")]
    Enqueue {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name")]
        queue: String,
        #[arg(short = 'j', long, help = "Job payload as JSON")]
        payload: String,
        #[arg(short = 'r', long, help = "Job priority")]
        priority: Option<String>,
        #[arg(long, help = "Delay in seconds before job becomes available")]
        delay: Option<u64>,
        #[arg(long, help = "Maximum number of retry attempts")]
        max_attempts: Option<u32>,
        #[arg(long, help = "Timeout in seconds")]
        timeout: Option<u32>,
        #[arg(
            long,
            help = "Encrypt the payload with the application's encryption key (see the \
                    encryption_config setting), even if the queue is not one of its \
                    encrypted_queues"
        )]
        encrypt: bool,
        #[arg(
            long = "pii-field",
            value_name = "FIELD",
            help = "Encrypt only this payload field (repeatable; implies --encrypt)"
        )]
        pii_fields: Vec<String>,
    },
    #[command(
        about = "Retry failed, dead and timed-out jobs",
        long_about = "Retry jobs that are Failed, Dead or TimedOut.\n\n\
            Pass a job ID (positionally or with --job-id) to retry one job, or use \
            --queue and/or --all to retry many. Jobs in any other status are left alone."
    )]
    Retry {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            value_name = "JOB_ID",
            conflicts_with = "job_id",
            help = "Specific job ID to retry (failed, dead or timed-out jobs)"
        )]
        id: Option<String>,
        #[arg(
            long,
            help = "Specific job ID to retry (same as the positional JOB_ID)"
        )]
        job_id: Option<String>,
        #[arg(
            short = 'n',
            short_alias = 'Q',
            long,
            help = "Queue name to retry all failed jobs"
        )]
        queue: Option<String>,
        #[arg(long, help = "Retry all failed jobs")]
        all: bool,
    },
    #[command(about = "Cancel/delete jobs")]
    Cancel {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            value_name = "JOB_ID",
            conflicts_with = "job_id",
            help = "Specific job ID to cancel"
        )]
        id: Option<String>,
        #[arg(
            long,
            help = "Specific job ID to cancel (same as the positional JOB_ID)"
        )]
        job_id: Option<String>,
        #[arg(
            short = 'n',
            short_alias = 'Q',
            long,
            help = "Queue name to cancel pending jobs"
        )]
        queue: Option<String>,
        #[arg(long, help = "Cancel all pending jobs")]
        all_pending: bool,
    },
    #[command(about = "Purge completed or dead jobs")]
    Purge {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name to filter by")]
        queue: Option<String>,
        #[arg(long, help = "Only purge completed jobs")]
        completed: bool,
        #[arg(long, help = "Only purge dead jobs")]
        dead: bool,
        #[arg(long, help = "Only purge failed jobs")]
        failed: bool,
        #[arg(long, help = "Purge jobs older than N days")]
        older_than_days: Option<u32>,
        #[arg(long, help = "Confirm the purge operation")]
        confirm: bool,
    },
    #[command(
        about = "Reclaim jobs stuck in Running after a worker crashed",
        long_about = "Reclaim jobs left in Running by workers that crashed or were killed.\n\n\
            A Running job is stale when its lease has expired. Workers take the lease \
            when they claim a job and renew it with heartbeats, so a job whose worker \
            is alive is never reclaimed, whatever --older-than-secs is. Only a job \
            without a lease (claimed by an older Hammerwork version) is reclaimed by \
            age, when it started more than --older-than-secs ago. Stale jobs with \
            attempts left go back to Pending; \
            the rest are marked Dead. Safe to run while workers and other reapers are \
            running. Requires migration 015_add_job_leases.\n\n\
            Example: cargo hammerwork job requeue-stale --older-than-secs 3600"
    )]
    RequeueStale {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            long,
            default_value_t = 3600,
            help = "For jobs without a lease (claimed by older versions): reclaim if started more than N seconds ago"
        )]
        older_than_secs: u64,
    },
}

impl JobCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect_with_config(&db_url, config).await?;

        match self {
            JobCommand::List {
                queue,
                status,
                priority,
                limit,
                failed,
                completed,
                last_hours,
                ..
            } => {
                list_jobs(
                    pool,
                    queue.clone(),
                    status.clone(),
                    priority.clone(),
                    limit.unwrap_or(config.get_default_limit()),
                    *failed,
                    *completed,
                    *last_hours,
                )
                .await?;
            }
            JobCommand::Show { job_id, .. } => {
                show_job_details(pool, job_id).await?;
            }
            JobCommand::Enqueue {
                queue,
                payload,
                priority,
                delay,
                max_attempts,
                timeout,
                encrypt,
                pii_fields,
                ..
            } => {
                let job_queue = pool
                    .create_enqueue_queue(&config.encryption_settings()?)
                    .await?;
                enqueue_job(
                    job_queue,
                    queue,
                    payload,
                    priority,
                    *delay,
                    *max_attempts,
                    *timeout,
                    EncryptionRequest {
                        encrypt: *encrypt,
                        pii_fields: pii_fields.clone(),
                    },
                )
                .await?;
            }
            JobCommand::Retry {
                id,
                job_id,
                queue,
                all,
                ..
            } => {
                let target = id.clone().or_else(|| job_id.clone());
                retry_jobs(pool, target, queue.clone(), *all).await?;
            }
            JobCommand::Cancel {
                id,
                job_id,
                queue,
                all_pending,
                ..
            } => {
                let target = id.clone().or_else(|| job_id.clone());
                cancel_jobs(pool, target, queue.clone(), *all_pending).await?;
            }
            JobCommand::Purge {
                queue,
                completed,
                dead,
                failed,
                older_than_days,
                confirm,
                ..
            } => {
                purge_jobs(
                    pool,
                    queue.clone(),
                    *completed,
                    *dead,
                    *failed,
                    *older_than_days,
                    *confirm,
                )
                .await?;
            }
            JobCommand::RequeueStale {
                older_than_secs, ..
            } => {
                requeue_stale_jobs(pool, *older_than_secs).await?;
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url_option = match self {
            JobCommand::List { database_url, .. } => database_url,
            JobCommand::Show { database_url, .. } => database_url,
            JobCommand::Enqueue { database_url, .. } => database_url,
            JobCommand::Retry { database_url, .. } => database_url,
            JobCommand::Cancel { database_url, .. } => database_url,
            JobCommand::Purge { database_url, .. } => database_url,
            JobCommand::RequeueStale { database_url, .. } => database_url,
        };

        url_option
            .as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
    }
}

/// The value the library stores in the `status` column for a CLI status name
/// (`pending`, `timed_out`, ...). Expects a name accepted by `validate_status`.
pub(crate) fn status_db_value(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "pending" => "Pending",
        "running" => "Running",
        "completed" => "Completed",
        "failed" => "Failed",
        "dead" => "Dead",
        "retrying" => "Retrying",
        "timed_out" | "timedout" => "TimedOut",
        _ => "Pending",
    }
}

/// The display name of a stored integer priority.
pub fn priority_display(priority: i32) -> String {
    JobPriority::from_i32(priority)
        .map(|p| p.to_string())
        .unwrap_or_else(|_| priority.to_string())
}

/// Builds the parameterized `job list` query.
///
/// `postgres` selects `$n` placeholders and PostgreSQL interval syntax, otherwise `?`
/// placeholders and MySQL syntax. Status names are mapped to the capitalized values the
/// library stores and priorities to their integer column values.
#[allow(clippy::too_many_arguments)]
pub fn build_list_jobs_query(
    postgres: bool,
    queue: Option<&str>,
    status: Option<&str>,
    priority: Option<JobPriority>,
    limit: u32,
    failed: bool,
    completed: bool,
    last_hours: Option<u32>,
) -> (String, Vec<Bind>) {
    let mut conditions = Vec::new();
    let mut params = SqlParams::new(if postgres {
        Backend::Postgres
    } else {
        Backend::MySql
    });

    if let Some(queue_name) = queue {
        let p = params.text(queue_name);
        conditions.push(format!("queue_name = {p}"));
    }
    if let Some(status) = status {
        let p = params.text(status_db_value(status));
        conditions.push(format!("status = {p}"));
    }
    if let Some(priority) = priority {
        let p = params.int(i64::from(priority.as_i32()));
        conditions.push(format!("priority = {p}"));
    }
    if failed {
        conditions.push("status IN ('Failed', 'Dead')".to_string());
    }
    if completed {
        conditions.push("status = 'Completed'".to_string());
    }
    if let Some(hours) = last_hours {
        let since = params.ago(hours, IntervalUnit::Hour);
        conditions.push(format!("created_at > {since}"));
    }

    let mut query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs".to_string();
    if !conditions.is_empty() {
        query.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }
    let limit_clause = params.limit(limit);
    query.push_str(&format!(" ORDER BY created_at DESC {limit_clause}"));
    (query, params.into_binds())
}

/// The `DELETE` behind `job purge`. `status_condition` must be built from constant status
/// names; the queue name and age are bound.
pub fn build_purge_query(
    backend: Backend,
    status_condition: &str,
    queue: Option<&str>,
    older_than_days: Option<u32>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let mut query = format!("DELETE FROM hammerwork_jobs WHERE {status_condition}");
    if let Some(queue_name) = queue {
        query.push_str(&format!(" AND queue_name = {}", params.text(queue_name)));
    }
    if let Some(days) = older_than_days {
        query.push_str(&format!(
            " AND created_at < {}",
            params.ago(days, IntervalUnit::Day)
        ));
    }
    (query, params.into_binds())
}

#[allow(clippy::too_many_arguments)]
async fn list_jobs(
    pool: DatabasePool,
    queue: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    limit: u32,
    failed: bool,
    completed: bool,
    last_hours: Option<u32>,
) -> Result<()> {
    // Validate inputs
    if let Some(ref s) = status {
        validate_status(s)?;
    }
    let priority = priority.as_deref().map(validate_priority).transpose()?;

    let mut job_table = JobTable::new();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let (query, params) = build_list_jobs_query(
                true,
                queue.as_deref(),
                status.as_deref(),
                priority,
                limit,
                failed,
                completed,
                last_hours,
            );
            let rows = bind_pg(sqlx::query(&query), &params)
                .fetch_all(&pg_pool)
                .await?;
            for row in rows {
                let id: uuid::Uuid = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority: i32 = row.try_get("priority")?;
                let attempts: i32 = row.try_get("attempts")?;
                let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                let scheduled_at: chrono::DateTime<chrono::Utc> = row.try_get("scheduled_at")?;

                job_table.add_job_row(
                    &id.to_string(),
                    &queue_name,
                    &status,
                    &priority_display(priority),
                    attempts,
                    &created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    &scheduled_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                );
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let (query, params) = build_list_jobs_query(
                false,
                queue.as_deref(),
                status.as_deref(),
                priority,
                limit,
                failed,
                completed,
                last_hours,
            );
            let rows = bind_mysql(sqlx::query(&query), &params)
                .fetch_all(&mysql_pool)
                .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority: i32 = row.try_get("priority")?;
                let attempts: i32 = row.try_get("attempts")?;
                let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                let scheduled_at: chrono::DateTime<chrono::Utc> = row.try_get("scheduled_at")?;

                job_table.add_job_row(
                    &id,
                    &queue_name,
                    &status,
                    &priority_display(priority),
                    attempts,
                    &created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    &scheduled_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                );
            }
        }
    }

    println!("{}", job_table);
    Ok(())
}

/// A job as shown by `job show`.
#[derive(Debug, Clone, PartialEq)]
pub struct JobDetails {
    pub id: String,
    pub queue: String,
    pub status: String,
    pub priority: i32,
    pub attempts: i32,
    pub max_attempts: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub scheduled_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub failed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
    pub payload: serde_json::Value,
    encryption: EncryptionDetails,
}

macro_rules! job_details {
    ($row:expr, $id:expr) => {{
        let row = $row;
        JobDetails {
            id: $id,
            queue: row.try_get("queue_name")?,
            status: row.try_get("status")?,
            priority: row.try_get("priority")?,
            attempts: row.try_get("attempts")?,
            max_attempts: row.try_get("max_attempts")?,
            created_at: row.try_get("created_at")?,
            scheduled_at: row.try_get("scheduled_at")?,
            started_at: row.try_get("started_at")?,
            completed_at: row.try_get("completed_at")?,
            failed_at: row.try_get("failed_at")?,
            error_message: row.try_get("error_message")?,
            payload: row.try_get("payload")?,
            encryption: EncryptionDetails {
                is_encrypted: row.try_get("is_encrypted")?,
                key_id: row.try_get("encryption_key_id")?,
                algorithm: row.try_get("encryption_algorithm")?,
                retention_policy: row.try_get("retention_policy")?,
                retention_delete_at: row.try_get("retention_delete_at")?,
            },
        }
    }};
}

/// Look a job up by id; `None` if there is no such job.
pub async fn fetch_job_details(pool: &DatabasePool, job_id: &str) -> Result<Option<JobDetails>> {
    let job_uuid = uuid::Uuid::parse_str(job_id)
        .map_err(|e| anyhow::anyhow!("Invalid job ID '{}': {}", job_id, e))?;
    Ok(match pool {
        DatabasePool::Postgres(pg_pool) => {
            sqlx::query("SELECT * FROM hammerwork_jobs WHERE id = $1")
                .bind(job_uuid)
                .fetch_optional(pg_pool)
                .await?
                .map(|row| -> Result<JobDetails> {
                    let id = row.try_get::<uuid::Uuid, _>("id")?.to_string();
                    Ok(job_details!(&row, id))
                })
                .transpose()?
        }
        DatabasePool::MySQL(mysql_pool) => {
            sqlx::query("SELECT * FROM hammerwork_jobs WHERE id = ?")
                .bind(job_uuid.to_string())
                .fetch_optional(mysql_pool)
                .await?
                .map(|row| -> Result<JobDetails> {
                    let id = row.try_get::<String, _>("id")?;
                    Ok(job_details!(&row, id))
                })
                .transpose()?
        }
    })
}

/// The text `job show` prints.
pub fn render_job_details(job: &JobDetails) -> String {
    let time = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%d %H:%M:%S UTC").to_string();
    let mut lines = vec![
        "📋 Job Details".to_string(),
        "═══════════════".to_string(),
        format!("ID: {}", job.id),
        format!("Queue: {}", job.queue),
        format!("Status: {}", job.status),
        format!("Priority: {}", priority_display(job.priority)),
        format!("Attempts: {}/{}", job.attempts, job.max_attempts),
        format!("Created: {}", time(job.created_at)),
        format!("Scheduled: {}", time(job.scheduled_at)),
    ];
    if let Some(started) = job.started_at {
        lines.push(format!("Started: {}", time(started)));
    }
    if let Some(completed) = job.completed_at {
        lines.push(format!("Completed: {}", time(completed)));
    }
    if let Some(failed) = job.failed_at {
        lines.push(format!("Failed: {}", time(failed)));
    }
    if let Some(error) = &job.error_message {
        lines.push(format!("Error: {}", error));
    }
    lines.extend(encryption_detail_lines(&job.encryption));
    lines.push(format!(
        "Payload: {}",
        serde_json::to_string_pretty(&job.payload).unwrap_or_else(|_| job.payload.to_string())
    ));
    lines.join("\n")
}

async fn show_job_details(pool: DatabasePool, job_id: &str) -> Result<()> {
    match fetch_job_details(&pool, job_id).await? {
        Some(job) => {
            println!("{}", render_job_details(&job));
            Ok(())
        }
        None => Err(anyhow::anyhow!("Job not found: {}", job_id)),
    }
}

/// Encryption columns of a job row, as shown by `job show`.
#[derive(Debug, Clone, PartialEq)]
struct EncryptionDetails {
    is_encrypted: bool,
    key_id: Option<String>,
    algorithm: Option<String>,
    retention_policy: Option<String>,
    retention_delete_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Lines describing a job's payload encryption (empty for a plaintext job). The CLI
/// never decrypts: an encrypted job's payload is shown in its redacted stored form.
fn encryption_detail_lines(details: &EncryptionDetails) -> Vec<String> {
    if !details.is_encrypted {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "Encrypted: yes ({}, key {}); the payload below is redacted",
        details.algorithm.as_deref().unwrap_or("unknown algorithm"),
        details.key_id.as_deref().unwrap_or("unknown")
    )];
    match (&details.retention_policy, details.retention_delete_at) {
        (Some(policy), Some(delete_at)) => lines.push(format!(
            "Retention: {} (delete after {})",
            policy,
            delete_at.format("%Y-%m-%d %H:%M:%S UTC")
        )),
        (Some(policy), None) => lines.push(format!("Retention: {}", policy)),
        _ => {}
    }
    lines
}

/// `job enqueue --encrypt` / `--pii-field`.
#[derive(Debug, Clone, Default)]
struct EncryptionRequest {
    encrypt: bool,
    pii_fields: Vec<String>,
}

/// Ask for `job` to be encrypted with `job_queue`'s engine (the application's key), as
/// requested by `--encrypt` / `--pii-field`. Fails if the queue has no engine.
fn request_encryption(
    job: Job,
    job_queue: &JobQueueWrapper,
    request: &EncryptionRequest,
) -> Result<Job> {
    if !request.encrypt && request.pii_fields.is_empty() {
        return Ok(job);
    }
    let engine = job_queue.encryption_engine().ok_or_else(|| {
        anyhow::anyhow!(
            "--encrypt and --pii-field need the application's encryption settings with \
             encryption enabled: set encryption_config (`cargo hammerwork config set \
             encryption_config <path to hammerwork.toml>`) or the HAMMERWORK_ENCRYPTION_* \
             environment variables, and make its key available"
        )
    })?;
    let mut config = hammerwork::encryption::EncryptionConfig::new(engine.algorithm().clone())
        .with_key_id(engine.key_id());
    config.compression_enabled = engine.config().compression_enabled;
    let job = job.with_encryption(config);
    Ok(if request.pii_fields.is_empty() {
        job
    } else {
        job.with_pii_fields(request.pii_fields.clone())
    })
}

#[allow(clippy::too_many_arguments)]
async fn enqueue_job(
    job_queue: JobQueueWrapper,
    queue: &str,
    payload: &str,
    priority: &Option<String>,
    delay: Option<u64>,
    max_attempts: Option<u32>,
    timeout: Option<u32>,
    encryption: EncryptionRequest,
) -> Result<()> {
    let payload_value = validate_json_payload(payload)?;
    let job_priority = if let Some(p) = priority {
        validate_priority(p)?
    } else {
        JobPriority::Normal
    };

    let mut job = request_encryption(
        Job::new(queue.to_string(), payload_value),
        &job_queue,
        &encryption,
    )?;
    job.priority = job_priority;

    if let Some(max_att) = max_attempts {
        job.max_attempts = i32::try_from(max_att)
            .map_err(|_| anyhow::anyhow!("--max-attempts {} is too large", max_att))?;
    }

    if let Some(timeout_secs) = timeout {
        job.timeout = Some(std::time::Duration::from_secs(timeout_secs as u64));
    }

    if let Some(delay_secs) = delay {
        job.scheduled_at = delayed_schedule(chrono::Utc::now(), delay_secs)?;
    }

    match job_queue {
        crate::utils::database::JobQueueWrapper::Postgres(queue) => {
            let job_id = queue.enqueue(job).await?;
            info!("✅ Job enqueued successfully: {}", job_id);
        }
        crate::utils::database::JobQueueWrapper::MySQL(queue) => {
            let job_id = queue.enqueue(job).await?;
            info!("✅ Job enqueued successfully: {}", job_id);
        }
    }

    Ok(())
}

/// `now + delay_secs`, rejecting delays that overflow instead of wrapping or panicking.
fn delayed_schedule(
    now: chrono::DateTime<chrono::Utc>,
    delay_secs: u64,
) -> Result<chrono::DateTime<chrono::Utc>> {
    i64::try_from(delay_secs)
        .ok()
        .and_then(chrono::Duration::try_seconds)
        .and_then(|delay| now.checked_add_signed(delay))
        .ok_or_else(|| anyhow::anyhow!("--delay {} seconds is out of range", delay_secs))
}

async fn requeue_stale_jobs(pool: DatabasePool, older_than_secs: u64) -> Result<()> {
    let older_than = std::time::Duration::from_secs(older_than_secs);
    let recovery = match pool.create_job_queue() {
        crate::utils::database::JobQueueWrapper::Postgres(queue) => {
            queue.requeue_stale_jobs(older_than).await?
        }
        crate::utils::database::JobQueueWrapper::MySQL(queue) => {
            queue.requeue_stale_jobs(older_than).await?
        }
    };

    for id in &recovery.requeued {
        info!("↩️  Requeued stale job {}", id);
    }
    for id in &recovery.dead {
        info!("💀 Marked stale job {} dead (no attempts left)", id);
    }
    info!(
        "✅ Reclaimed {} stale jobs ({} requeued, {} dead)",
        recovery.total(),
        recovery.requeued.len(),
        recovery.dead.len()
    );
    Ok(())
}

async fn retry_jobs(
    pool: DatabasePool,
    job_id: Option<String>,
    queue: Option<String>,
    all: bool,
) -> Result<()> {
    if !all && job_id.is_none() && queue.is_none() {
        return Err(anyhow::anyhow!("Must specify --job-id, --queue, or --all"));
    }

    let wrapper = pool.clone().create_job_queue();

    // A single job: report the library's verdict (not found / invalid transition).
    if let Some(id) = job_id {
        let job_uuid = uuid::Uuid::parse_str(&id)?;
        match &wrapper {
            JobQueueWrapper::Postgres(q) => retry_one(q, job_uuid).await?,
            JobQueueWrapper::MySQL(q) => retry_one(q, job_uuid).await?,
        }
        info!("✅ Retried job {}", job_uuid);
        return Ok(());
    }

    let (scope, selector) = match queue {
        Some(queue_name) => (
            format!("queue '{}'", queue_name),
            JobSelector {
                statuses: RETRYABLE_STATUSES.to_vec(),
                queue: Some(queue_name),
                ..Default::default()
            },
        ),
        None => (
            "all failed jobs".to_string(),
            JobSelector {
                statuses: RETRYABLE_STATUSES.to_vec(),
                ..Default::default()
            },
        ),
    };

    let ids = select_job_ids(&pool, &selector).await?;
    let result = retry_many(&wrapper, &ids).await;

    info!("✅ Retried {} jobs for {}", result.succeeded, scope);
    if !result.skipped.is_empty() {
        info!(
            "⚠️  Skipped {} jobs that changed state or could not be retried",
            result.skipped.len()
        );
    }
    Ok(())
}

async fn cancel_jobs(
    pool: DatabasePool,
    job_id: Option<String>,
    queue: Option<String>,
    all_pending: bool,
) -> Result<()> {
    if !all_pending && job_id.is_none() && queue.is_none() {
        return Err(anyhow::anyhow!(
            "Must specify --job-id, --queue, or --all-pending"
        ));
    }

    let wrapper = pool.clone().create_job_queue();
    let allowed = [JobStatus::Pending];

    if let Some(id) = job_id {
        let job_uuid = uuid::Uuid::parse_str(&id)?;
        match &wrapper {
            JobQueueWrapper::Postgres(q) => cancel_one(q, job_uuid, &allowed).await?,
            JobQueueWrapper::MySQL(q) => cancel_one(q, job_uuid, &allowed).await?,
        }
        info!("✅ Cancelled job {}", job_uuid);
        return Ok(());
    }

    let (scope, selector) = match queue {
        Some(queue_name) => (
            format!("queue '{}'", queue_name),
            JobSelector {
                statuses: vec![JobStatus::Pending],
                queue: Some(queue_name),
                ..Default::default()
            },
        ),
        None => (
            "all pending jobs".to_string(),
            JobSelector {
                statuses: vec![JobStatus::Pending],
                ..Default::default()
            },
        ),
    };

    let ids = select_job_ids(&pool, &selector).await?;
    let result = cancel_many(&wrapper, &ids, &allowed).await;

    info!("✅ Cancelled {} jobs for {}", result.succeeded, scope);
    Ok(())
}

async fn purge_jobs(
    pool: DatabasePool,
    queue: Option<String>,
    completed: bool,
    dead: bool,
    failed: bool,
    older_than_days: Option<u32>,
    confirm: bool,
) -> Result<()> {
    if !completed && !dead && !failed {
        return Err(anyhow::anyhow!(
            "Must specify at least one of: --completed, --dead, --failed"
        ));
    }

    if !confirm {
        println!("⚠️  This will permanently delete jobs. Use --confirm to proceed.");
        return Ok(());
    }

    // Statuses are stored capitalized; the queue name is bound, never interpolated.
    let mut statuses = Vec::new();
    if completed {
        statuses.push("'Completed'");
    }
    if dead {
        statuses.push("'Dead'");
    }
    if failed {
        statuses.push("'Failed'");
    }
    let status_condition = format!("status IN ({})", statuses.join(", "));

    let (query, binds) = build_purge_query(
        pool.backend(),
        &status_condition,
        queue.as_deref(),
        older_than_days,
    );
    let affected = execute_binds(&pool, &query, &binds).await?;

    info!("✅ Purged {} jobs", affected);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::sql::fetch_i64;
    use crate::utils::test_support::*;
    use clap::Parser;

    #[test]
    fn test_delayed_schedule_adds_delay() {
        let now = chrono::Utc::now();
        assert_eq!(
            delayed_schedule(now, 90).unwrap(),
            now + chrono::Duration::seconds(90)
        );
    }

    #[test]
    fn test_delayed_schedule_rejects_overflow() {
        let now = chrono::Utc::now();
        assert!(delayed_schedule(now, u64::MAX).is_err());
        assert!(delayed_schedule(now, i64::MAX as u64).is_err());
    }

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: JobCommand,
    }

    #[test]
    fn test_requeue_stale_parses_with_default_threshold() {
        let cli = TestCli::try_parse_from(["test", "requeue-stale"]).unwrap();
        match cli.command {
            JobCommand::RequeueStale {
                database_url,
                older_than_secs,
            } => {
                assert!(database_url.is_none());
                assert_eq!(older_than_secs, 3600);
            }
            _ => panic!("expected RequeueStale"),
        }

        let cli = TestCli::try_parse_from([
            "test",
            "requeue-stale",
            "--older-than-secs",
            "120",
            "-u",
            "postgres://localhost/db",
        ])
        .unwrap();
        match cli.command {
            JobCommand::RequeueStale {
                database_url,
                older_than_secs,
            } => {
                assert_eq!(database_url.as_deref(), Some("postgres://localhost/db"));
                assert_eq!(older_than_secs, 120);
            }
            _ => panic!("expected RequeueStale"),
        }

        assert!(
            TestCli::try_parse_from(["test", "requeue-stale", "--older-than-secs", "-5"]).is_err()
        );
    }

    #[test]
    fn test_list_jobs_query_postgres_binds_typed_params() {
        let (query, params) = build_list_jobs_query(
            true,
            Some("emails"),
            Some("timed_out"),
            Some(JobPriority::High),
            25,
            false,
            false,
            Some(2),
        );
        assert_eq!(
            query,
            "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at \
             FROM hammerwork_jobs WHERE queue_name = $1 AND status = $2 AND priority = $3 \
             AND created_at > NOW() - make_interval(hours => $4::int) \
             ORDER BY created_at DESC LIMIT $5"
        );
        assert_eq!(
            params,
            vec![
                Bind::Text("emails".into()),
                Bind::Text("TimedOut".into()),
                Bind::Int(3),
                Bind::Int(2),
                Bind::Int(25),
            ]
        );
    }

    #[test]
    fn test_list_jobs_query_mysql_and_flags() {
        let (query, params) = build_list_jobs_query(
            false,
            Some("q'; DROP"),
            None,
            None,
            10,
            true,
            false,
            Some(1),
        );
        assert!(query.contains("queue_name = ?"));
        assert!(query.contains("status IN ('Failed', 'Dead')"));
        assert!(query.contains("DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? HOUR)"));
        // User input is bound, never interpolated
        assert!(!query.contains("DROP"));
        assert_eq!(
            params,
            vec![Bind::Text("q'; DROP".into()), Bind::Int(1), Bind::Int(10)]
        );

        let (query, params) = build_list_jobs_query(false, None, None, None, 5, false, true, None);
        assert!(query.ends_with("WHERE status = 'Completed' ORDER BY created_at DESC LIMIT ?"));
        assert_eq!(params, vec![Bind::Int(5)]);
    }

    #[test]
    fn test_purge_query_binds_queue_and_age() {
        let (query, binds) = build_purge_query(
            Backend::Postgres,
            "status IN ('Completed')",
            Some(HOSTILE_QUEUE),
            Some(7),
        );
        assert_eq!(
            query,
            "DELETE FROM hammerwork_jobs WHERE status IN ('Completed') AND queue_name = $1 \
             AND created_at < NOW() - make_interval(days => $2::int)"
        );
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into()), Bind::Int(7)]);

        let (query, binds) = build_purge_query(
            Backend::MySql,
            "status IN ('Dead')",
            Some(HOSTILE_QUEUE),
            Some(3),
        );
        assert_eq!(
            query,
            "DELETE FROM hammerwork_jobs WHERE status IN ('Dead') AND queue_name = ? \
             AND created_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? DAY)"
        );
        assert_eq!(binds.len(), 2);

        let (query, binds) = build_purge_query(Backend::MySql, "status IN ('Dead')", None, None);
        assert_eq!(
            query,
            "DELETE FROM hammerwork_jobs WHERE status IN ('Dead')"
        );
        assert!(binds.is_empty());
    }

    async fn job_roundtrip(pool: DatabasePool) {
        let hostile = hostile_queue();
        let other = format!("other_{}", uuid::Uuid::new_v4().simple());
        seed(&pool, &SeedJob::new(&hostile, "Pending")).await;
        seed(&pool, &SeedJob::new(&hostile, "Completed")).await;
        seed(&pool, &SeedJob::new(&hostile, "Completed")).await;
        seed(&pool, &SeedJob::new(&other, "Completed")).await;

        let backend = pool.backend();
        let (query, binds) = build_list_jobs_query(
            backend == Backend::Postgres,
            Some(&hostile),
            Some("completed"),
            None,
            1,
            false,
            false,
            Some(1),
        );
        let queues = column_strings(&pool, &query, &binds, "queue_name").await;
        assert_eq!(queues, vec![hostile.clone()]); // LIMIT 1, only the hostile queue
        list_jobs(
            pool.clone(),
            Some(hostile.clone()),
            None,
            None,
            10,
            false,
            false,
            Some(1),
        )
        .await
        .unwrap();

        // Nothing is a day old yet: the age filter keeps every job
        purge_jobs(
            pool.clone(),
            Some(hostile.clone()),
            true,
            false,
            false,
            Some(1),
            true,
        )
        .await
        .unwrap();
        let (sql, binds) = crate::commands::queue::build_queue_total_query(backend, Some(&hostile));
        assert_eq!(fetch_i64(&pool, &sql, &binds, "total").await.unwrap(), 3);
        // Purge only touches the hostile queue's completed jobs
        purge_jobs(
            pool.clone(),
            Some(hostile.clone()),
            true,
            false,
            false,
            None,
            true,
        )
        .await
        .unwrap();
        assert_eq!(fetch_i64(&pool, &sql, &binds, "total").await.unwrap(), 1);
        let (sql, binds) = crate::commands::queue::build_queue_total_query(backend, Some(&other));
        assert_eq!(fetch_i64(&pool, &sql, &binds, "total").await.unwrap(), 1);

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_job_list_and_purge_are_injection_safe_postgres() {
        job_roundtrip(pg_pool().await).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_job_list_and_purge_are_injection_safe_mysql() {
        job_roundtrip(mysql_pool().await).await;
    }

    #[test]
    fn test_status_and_priority_mapping() {
        assert_eq!(status_db_value("pending"), "Pending");
        assert_eq!(status_db_value("DEAD"), "Dead");
        assert_eq!(status_db_value("timed_out"), "TimedOut");
        assert_eq!(priority_display(2), "normal");
        assert_eq!(priority_display(4), "critical");
        assert_eq!(priority_display(42), "42");
    }

    #[test]
    fn test_encryption_detail_lines() {
        let plain = EncryptionDetails {
            is_encrypted: false,
            key_id: None,
            algorithm: None,
            retention_policy: None,
            retention_delete_at: None,
        };
        assert!(encryption_detail_lines(&plain).is_empty());

        let delete_at = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let encrypted = EncryptionDetails {
            is_encrypted: true,
            key_id: Some("key-1".into()),
            algorithm: Some("AES256GCM".into()),
            retention_policy: Some("DeleteAfter".into()),
            retention_delete_at: Some(delete_at),
        };
        let lines = encryption_detail_lines(&encrypted);
        assert_eq!(
            lines,
            vec![
                "Encrypted: yes (AES256GCM, key key-1); the payload below is redacted",
                "Retention: DeleteAfter (delete after 2026-01-02 03:04:05 UTC)",
            ]
        );
    }

    fn parse(args: &[&str]) -> JobCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&[
            "list",
            "-n",
            "q",
            "-t",
            "failed",
            "-r",
            "high",
            "-l",
            "5",
            "--failed",
            "--completed",
            "--last-hours",
            "3",
        ]) {
            JobCommand::List {
                queue,
                status,
                priority,
                limit,
                failed,
                completed,
                last_hours,
                ..
            } => {
                assert_eq!(queue.as_deref(), Some("q"));
                assert_eq!(status.as_deref(), Some("failed"));
                assert_eq!(priority.as_deref(), Some("high"));
                assert_eq!(limit, Some(5));
                assert!(failed && completed);
                assert_eq!(last_hours, Some(3));
            }
            _ => panic!("expected List"),
        }
        assert!(matches!(
            parse(&["show", "abc"]),
            JobCommand::Show { job_id, .. } if job_id == "abc"
        ));
        match parse(&[
            "enqueue",
            "-n",
            "q",
            "-j",
            "{}",
            "-r",
            "low",
            "--delay",
            "30",
            "--max-attempts",
            "7",
            "--timeout",
            "60",
        ]) {
            JobCommand::Enqueue {
                queue,
                payload,
                priority,
                delay,
                max_attempts,
                timeout,
                ..
            } => {
                assert_eq!((queue.as_str(), payload.as_str()), ("q", "{}"));
                assert_eq!(priority.as_deref(), Some("low"));
                assert_eq!(
                    (delay, max_attempts, timeout),
                    (Some(30), Some(7), Some(60))
                );
            }
            _ => panic!("expected Enqueue"),
        }
        match parse(&["retry", "--job-id", "x", "-n", "q", "--all"]) {
            JobCommand::Retry {
                job_id, queue, all, ..
            } => assert_eq!(
                (job_id.as_deref(), queue.as_deref(), all),
                (Some("x"), Some("q"), true)
            ),
            _ => panic!("expected Retry"),
        }
        match parse(&["cancel", "--job-id", "x", "-n", "q", "--all-pending"]) {
            JobCommand::Cancel {
                job_id,
                queue,
                all_pending,
                ..
            } => assert_eq!(
                (job_id.as_deref(), queue.as_deref(), all_pending),
                (Some("x"), Some("q"), true)
            ),
            _ => panic!("expected Cancel"),
        }
        match parse(&[
            "purge",
            "-Q",
            "q",
            "--completed",
            "--dead",
            "--failed",
            "--older-than-days",
            "4",
            "--confirm",
        ]) {
            JobCommand::Purge {
                queue,
                completed,
                dead,
                failed,
                older_than_days,
                confirm,
                ..
            } => {
                assert_eq!(queue.as_deref(), Some("q"));
                assert!(completed && dead && failed && confirm);
                assert_eq!(older_than_days, Some(4));
            }
            _ => panic!("expected Purge"),
        }
        assert!(
            TestCli::try_parse_from(["test", "enqueue", "-n", "q"]).is_err(),
            "payload is required"
        );
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        for args in [
            &["list"][..],
            &["show", "x"],
            &["retry"],
            &["cancel"],
            &["requeue-stale"],
        ] {
            assert_eq!(
                parse(args).get_database_url(&config).unwrap(),
                "postgres://config/db"
            );
            assert!(parse(args).get_database_url(&Config::default()).is_err());
        }
        assert_eq!(
            parse(&["list", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
    }

    fn sample_details() -> JobDetails {
        let at = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        JobDetails {
            id: "11111111-1111-1111-1111-111111111111".into(),
            queue: "emails".into(),
            status: "Failed".into(),
            priority: 3,
            attempts: 2,
            max_attempts: 5,
            created_at: at,
            scheduled_at: at,
            started_at: Some(at),
            completed_at: None,
            failed_at: Some(at),
            error_message: Some("smtp down".into()),
            payload: serde_json::json!({"to": "a@example.com"}),
            encryption: EncryptionDetails {
                is_encrypted: true,
                key_id: Some("k1".into()),
                algorithm: Some("AES256GCM".into()),
                retention_policy: Some("DeleteAfter".into()),
                retention_delete_at: None,
            },
        }
    }

    #[test]
    fn job_details_render_every_present_field() {
        let out = render_job_details(&sample_details());
        for expected in [
            "ID: 11111111-1111-1111-1111-111111111111",
            "Queue: emails",
            "Status: Failed",
            "Priority: high",
            "Attempts: 2/5",
            "Created: 2030-01-02 03:04:05 UTC",
            "Started: 2030-01-02 03:04:05 UTC",
            "Failed: 2030-01-02 03:04:05 UTC",
            "Error: smtp down",
            "Encrypted: yes (AES256GCM, key k1)",
            "Retention: DeleteAfter",
            "\"to\": \"a@example.com\"",
        ] {
            assert!(out.contains(expected), "{expected} in {out}");
        }
        assert!(!out.contains("Completed:"));

        let mut plain = sample_details();
        plain.started_at = None;
        plain.failed_at = None;
        plain.error_message = None;
        plain.encryption.is_encrypted = false;
        plain.completed_at = Some(plain.created_at);
        let out = render_job_details(&plain);
        assert!(out.contains("Completed:") && !out.contains("Started:") && !out.contains("Error:"));
        assert!(!out.contains("Encrypted"));
    }

    async fn job_commands(base_url: String) {
        // retry --all, cancel --all-pending and requeue-stale act on every queue
        let db = ScratchDb::create(&base_url).await;
        let pool = &db.pool;
        let config = db.config();
        let run = |args: &[&str]| {
            let cmd = parse(args);
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let q = hostile_queue();
        let other = unique_queue("job_other");

        // --- enqueue stores what was asked for
        run(&[
            "enqueue",
            "-n",
            &q,
            "-j",
            r#"{"to": "a@example.com"}"#,
            "-r",
            "critical",
            "--max-attempts",
            "7",
            "--timeout",
            "90",
            "--delay",
            "3600",
        ])
        .await
        .unwrap();
        run(&["enqueue", "-n", &q, "-j", "[1, 2]"]).await.unwrap();
        assert_eq!(count_jobs(pool, &q, Some("Pending")).await, 2);
        let jobs = fetch_job_ids(pool, &q).await;
        let delayed = &jobs[0];
        assert_eq!(
            job_column(pool, delayed, "priority").await.as_deref(),
            Some("4")
        );
        assert_eq!(
            job_column(pool, delayed, "max_attempts").await.as_deref(),
            Some("7")
        );
        assert_eq!(
            job_column(pool, delayed, "timeout_seconds")
                .await
                .as_deref(),
            Some("90")
        );
        let payload = job_column(pool, delayed, "payload").await.unwrap();
        assert!(payload.contains("a@example.com"), "{payload}");
        let later = fetch_i64_sql(
            pool,
            &format!(
                "SELECT COUNT(*) AS count FROM hammerwork_jobs WHERE id = '{delayed}' AND scheduled_at > {}",
                match pool.backend() {
                    Backend::Postgres => "NOW() + INTERVAL '30 minutes'",
                    Backend::MySql => "DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 MINUTE)",
                }
            ),
        )
        .await;
        assert_eq!(later, 1, "--delay pushes scheduled_at into the future");
        for (args, expected) in [
            (vec!["enqueue", "-n", "q", "-j", "{broken"], "Invalid JSON"),
            (
                vec!["enqueue", "-n", "q", "-j", "{}", "-r", "urgent"],
                "Invalid priority",
            ),
            (
                vec![
                    "enqueue",
                    "-n",
                    "q",
                    "-j",
                    "{}",
                    "--delay",
                    "18446744073709551615",
                ],
                "out of range",
            ),
        ] {
            let err = run(&args).await.unwrap_err().to_string();
            assert!(err.contains(expected), "{args:?}: {err}");
        }

        // --- show works on both backends and reports unknown/malformed ids
        run(&["show", delayed]).await.unwrap();
        let details = fetch_job_details(pool, delayed).await.unwrap().unwrap();
        assert_eq!(&details.id, delayed);
        assert_eq!(details.queue, q);
        assert_eq!(
            (
                details.status.as_str(),
                details.priority,
                details.max_attempts
            ),
            ("Pending", 4, 7)
        );
        assert_eq!(details.payload["to"], "a@example.com");
        let missing = uuid::Uuid::new_v4().to_string();
        let err = run(&["show", &missing]).await.unwrap_err().to_string();
        assert!(err.contains("Job not found"), "{err}");
        let err = run(&["show", "nope"]).await.unwrap_err().to_string();
        assert!(err.contains("Invalid job ID"), "{err}");

        // --- list with every filter
        let mut failed = SeedJob::new(&q, "Failed");
        failed.failed_now = true;
        let failed_id = seed(pool, &failed).await;
        let dead_id = seed(pool, &SeedJob::new(&q, "Dead")).await;
        let mut completed = SeedJob::new(&q, "Completed");
        completed.completed_now = true;
        let completed_id = seed(pool, &completed).await;
        seed(pool, &SeedJob::new(&other, "Pending")).await;
        for args in [
            vec!["list"],
            vec!["list", "-n", &q],
            vec!["list", "-n", &q, "-t", "failed", "-l", "1"],
            vec!["list", "-t", "timed_out"],
            vec!["list", "-r", "critical", "--last-hours", "1"],
            vec!["list", "--failed"],
            vec!["list", "--completed", "-n", &q],
        ] {
            run(&args).await.unwrap();
        }
        assert!(run(&["list", "-t", "bogus"]).await.is_err());
        assert!(run(&["list", "-r", "urgent"]).await.is_err());

        // --- retry: a single job, by queue, everything; refuses non-retryable and unknown jobs
        assert!(run(&["retry"]).await.is_err(), "needs a target");
        run(&["retry", "--job-id", &failed_id]).await.unwrap();
        assert_eq!(job_status(pool, &failed_id).await, "Pending");
        assert!(
            run(&["retry", "--job-id", &failed_id]).await.is_err(),
            "a pending job cannot be retried"
        );
        assert!(run(&["retry", "--job-id", &missing]).await.is_err());
        assert!(run(&["retry", "--job-id", "nope"]).await.is_err());
        run(&["retry", "-n", &q]).await.unwrap();
        assert_eq!(job_status(pool, &dead_id).await, "Pending");
        let foreign_dead = seed(pool, &SeedJob::new(&other, "Dead")).await;
        let foreign_failed = seed(pool, &SeedJob::new(&other, "Failed")).await;
        run(&["retry", "--all"]).await.unwrap();
        assert_eq!(job_status(pool, &foreign_dead).await, "Pending");
        assert_eq!(job_status(pool, &foreign_failed).await, "Pending");

        // --- cancel: only pending jobs can be cancelled
        assert!(run(&["cancel"]).await.is_err(), "needs a target");
        assert!(run(&["cancel", "--job-id", &completed_id]).await.is_err());
        assert_eq!(job_status(pool, &completed_id).await, "Completed");
        run(&["cancel", "--job-id", &failed_id]).await.unwrap();
        assert!(job_column(pool, &failed_id, "status").await.is_none());
        assert!(run(&["cancel", "--job-id", &missing]).await.is_err());
        run(&["cancel", "-n", &q]).await.unwrap();
        assert_eq!(
            count_jobs(pool, &q, None).await,
            1,
            "only the completed job is left"
        );
        run(&["cancel", "--all-pending"]).await.unwrap();
        assert_eq!(count_jobs(pool, &other, None).await, 0);

        // --- purge: needs a status flag and --confirm; scoped by queue and age
        assert!(run(&["purge", "-Q", &q]).await.is_err());
        run(&["purge", "-Q", &q, "--completed"]).await.unwrap(); // no --confirm
        assert_eq!(count_jobs(pool, &q, None).await, 1);
        run(&[
            "purge",
            "-Q",
            &q,
            "--completed",
            "--older-than-days",
            "1",
            "--confirm",
        ])
        .await
        .unwrap();
        assert_eq!(count_jobs(pool, &q, None).await, 1, "too recent to purge");
        let dead_old = seed(pool, &SeedJob::new(&q, "Dead")).await;
        backdate(pool, &dead_old, "created_at", 3).await;
        run(&[
            "purge",
            "-Q",
            &q,
            "--dead",
            "--older-than-days",
            "1",
            "--confirm",
        ])
        .await
        .unwrap();
        assert!(job_column(pool, &dead_old, "status").await.is_none());
        run(&["purge", "-Q", &q, "--completed", "--failed", "--confirm"])
            .await
            .unwrap();
        assert_eq!(count_jobs(pool, &q, None).await, 0);

        // --- requeue-stale: a stale job with attempts left goes back to Pending, an exhausted one dies
        let mut stale = SeedJob::new(&q, "Running");
        stale.started_long_ago = true;
        let retryable = seed(pool, &stale).await;
        let exhausted = seed(pool, &stale).await;
        exec_sql(
            pool,
            &format!("UPDATE hammerwork_jobs SET attempts = max_attempts WHERE id = '{exhausted}'"),
        )
        .await;
        let fresh = seed(pool, &SeedJob::new(&q, "Running")).await;
        backdate(pool, &fresh, "started_at", 0).await;
        run(&["requeue-stale", "--older-than-secs", "3600"])
            .await
            .unwrap();
        assert_eq!(job_status(pool, &retryable).await, "Pending");
        assert_eq!(job_status(pool, &exhausted).await, "Dead");
        assert_eq!(job_status(pool, &fresh).await, "Running");
        run(&["requeue-stale"]).await.unwrap();

        assert!(table_exists(pool).await);
        db.drop_db().await;
    }

    async fn fetch_job_ids(pool: &DatabasePool, queue: &str) -> Vec<String> {
        let mut params = SqlParams::new(pool.backend());
        let sql = format!(
            "SELECT {} AS id FROM hammerwork_jobs WHERE queue_name = {} ORDER BY priority DESC, id",
            match pool.backend() {
                Backend::Postgres => "CAST(id AS TEXT)",
                Backend::MySql => "id",
            },
            params.text(queue)
        );
        column_strings(pool, &sql, params.binds(), "id").await
    }

    async fn fetch_i64_sql(pool: &DatabasePool, sql: &str) -> i64 {
        fetch_i64(pool, sql, &[], "count").await.unwrap()
    }

    db_tests!(
        job_commands,
        test_job_commands_postgres,
        test_job_commands_mysql
    );

    /// `job enqueue` encrypts like the application (its `[encryption]` settings), and
    /// without them refuses to write plaintext to a queue that holds encrypted jobs
    /// (#64 H1).
    async fn enqueue_follows_the_application_encryption(url: String) {
        use base64::Engine as _;

        let _guard = serial().await;
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let encrypted_q = unique_queue("cli_enc");
        let plain_q = unique_queue("cli_plain");
        let key_var = format!("HW_CLI_TEST_KEY_{}", uuid::Uuid::new_v4().simple());
        let key = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        // SAFETY: tests touching the environment hold the `serial` lock.
        unsafe { std::env::set_var(&key_var, &key) };

        let dir = tempfile::tempdir().unwrap();
        let app_config = dir.path().join("hammerwork.toml");
        std::fs::write(
            &app_config,
            format!(
                "[database]\nurl = \"ignored\"\n\n[encryption]\nenabled = true\n\
                 key_source = \"env://{key_var}\"\nkey_id = \"cli-key\"\n\
                 encrypted_queues = [\"{encrypted_q}\"]\n"
            ),
        )
        .unwrap();
        let with_app = Config {
            encryption_config: Some(app_config.to_string_lossy().into_owned()),
            ..config_for(&url)
        };
        let without_app = config_for(&url);
        let run = |config: &Config, args: &[&str]| {
            let cmd = parse(args);
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let ids = |queue: String| {
            let pool = pool.clone();
            async move { fetch_job_ids(&pool, &queue).await }
        };
        let secret = format!("4111-{}", uuid::Uuid::new_v4().simple());
        let payload = format!(r#"{{"card": "{secret}", "amount": 5}}"#);

        // An encrypted queue of the application: encrypted without asking
        run(&with_app, &["enqueue", "-n", &encrypted_q, "-j", &payload])
            .await
            .unwrap();
        let id = ids(encrypted_q.clone()).await.pop().unwrap();
        let is_encrypted = job_column(&pool, &id, "is_encrypted").await.unwrap();
        assert!(
            is_encrypted == "true" || is_encrypted == "1",
            "{is_encrypted}"
        );
        assert_eq!(
            job_column(&pool, &id, "encryption_key_id").await.as_deref(),
            Some("cli-key")
        );
        let stored = job_column(&pool, &id, "payload").await.unwrap();
        assert!(!stored.contains(&secret), "{stored}");

        // --encrypt / --pii-field encrypt jobs for any queue
        run(
            &with_app,
            &[
                "enqueue",
                "-n",
                &plain_q,
                "-j",
                &payload,
                "--pii-field",
                "card",
            ],
        )
        .await
        .unwrap();
        let id = ids(plain_q.clone()).await.pop().unwrap();
        let stored = job_column(&pool, &id, "payload").await.unwrap();
        assert!(
            !stored.contains(&secret) && stored.contains("amount"),
            "{stored}"
        );
        assert!(
            job_column(&pool, &id, "pii_fields")
                .await
                .unwrap()
                .contains("card")
        );

        // The application's engine decrypts what the CLI wrote
        let app_queue = match pool.clone() {
            DatabasePool::Postgres(p) => JobQueueWrapper::Postgres(hammerwork::JobQueue::new(p)),
            DatabasePool::MySQL(p) => JobQueueWrapper::MySQL(hammerwork::JobQueue::new(p)),
        };
        let engine = hammerwork::encryption::EncryptionEngine::new(
            hammerwork::encryption::EncryptionConfig::new(
                hammerwork::encryption::EncryptionAlgorithm::AES256GCM,
            )
            .with_key_id("cli-key")
            .with_key_source(hammerwork::encryption::KeySource::Static(key.clone())),
        )
        .await
        .unwrap();
        let job_id = uuid::Uuid::parse_str(&id).unwrap();
        let opened = match app_queue {
            JobQueueWrapper::Postgres(q) => {
                let q = q.with_encryption(engine);
                q.decrypt_job(q.get_job(job_id).await.unwrap().unwrap())
                    .await
            }
            JobQueueWrapper::MySQL(q) => {
                let q = q.with_encryption(engine);
                q.decrypt_job(q.get_job(job_id).await.unwrap().unwrap())
                    .await
            }
        }
        .unwrap();
        assert_eq!(opened.payload["card"], secret.as_str());

        // Without the application's settings: refused on the queue holding encrypted
        // jobs, for jobs, cron jobs and batches; other queues are unaffected.
        let before = count_jobs(&pool, &encrypted_q, None).await;
        let err = run(
            &without_app,
            &["enqueue", "-n", &encrypted_q, "-j", &payload],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("holds encrypted jobs"), "{err}");
        let cron = crate::commands::cron::CronCommand::Create {
            database_url: None,
            queue: encrypted_q.clone(),
            payload: payload.clone(),
            schedule: "0 0 0 1 1 *".to_string(),
            timezone: None,
            priority: None,
        };
        assert!(cron.execute(&without_app).await.is_err());
        assert_eq!(count_jobs(&pool, &encrypted_q, None).await, before);
        let err = run(
            &without_app,
            &["enqueue", "-n", &plain_q, "-j", "{}", "--encrypt"],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--encrypt"), "{err}");
        let fresh_q = unique_queue("cli_fresh");
        run(&without_app, &["enqueue", "-n", &fresh_q, "-j", "{}"])
            .await
            .unwrap();

        // Settings whose key is unavailable: nothing is written
        // SAFETY: as above.
        unsafe { std::env::remove_var(&key_var) };
        let err = run(&with_app, &["enqueue", "-n", &fresh_q, "-j", "{}"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refusing to write jobs"), "{err}");
        assert_eq!(count_jobs(&pool, &fresh_q, None).await, 1);

        cleanup(&pool, &[&encrypted_q, &plain_q, &fresh_q]).await;
    }

    db_tests!(
        enqueue_follows_the_application_encryption,
        test_enqueue_follows_the_application_encryption_postgres,
        test_enqueue_follows_the_application_encryption_mysql
    );
}
