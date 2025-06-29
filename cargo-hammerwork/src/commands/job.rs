use anyhow::Result;
use clap::Subcommand;
use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobPriority};
use sqlx::Row;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::display::JobTable;
use crate::utils::validation::{validate_json_payload, validate_priority, validate_status};

#[derive(Subcommand)]
pub enum JobCommand {
    #[command(about = "List jobs in the queue")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name to filter by")]
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
        #[arg(short = 'n', long, help = "Queue name")]
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
    },
    #[command(about = "Retry failed jobs")]
    Retry {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Specific job ID to retry")]
        job_id: Option<String>,
        #[arg(short = 'n', long, help = "Queue name to retry all failed jobs")]
        queue: Option<String>,
        #[arg(long, help = "Retry all failed jobs")]
        all: bool,
    },
    #[command(about = "Cancel/delete jobs")]
    Cancel {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Specific job ID to cancel")]
        job_id: Option<String>,
        #[arg(short = 'n', long, help = "Queue name to cancel pending jobs")]
        queue: Option<String>,
        #[arg(long, help = "Cancel all pending jobs")]
        all_pending: bool,
    },
    #[command(about = "Purge completed or dead jobs")]
    Purge {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short, long, help = "Queue name to filter by")]
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
}

impl JobCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

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
                ..
            } => {
                enqueue_job(
                    pool,
                    queue,
                    payload,
                    priority,
                    *delay,
                    *max_attempts,
                    *timeout,
                )
                .await?;
            }
            JobCommand::Retry {
                job_id, queue, all, ..
            } => {
                retry_jobs(pool, job_id.clone(), queue.clone(), *all).await?;
            }
            JobCommand::Cancel {
                job_id,
                queue,
                all_pending,
                ..
            } => {
                cancel_jobs(pool, job_id.clone(), queue.clone(), *all_pending).await?;
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
        };

        url_option
            .as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
    }
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
    if let Some(ref p) = priority {
        validate_priority(p)?;
    }

    // Build dynamic query conditions
    let mut conditions = Vec::new();
    // Dynamic query building for complex filtering

    if let Some(queue_name) = &queue {
        // Escape single quotes to prevent SQL injection
        let escaped_queue = queue_name.replace("'", "''");
        conditions.push(format!("queue_name = '{}'", escaped_queue));
    }

    if let Some(status_str) = &status {
        let escaped_status = status_str.replace("'", "''");
        conditions.push(format!("status = '{}'", escaped_status));
    }

    if let Some(priority_str) = &priority {
        let escaped_priority = priority_str.replace("'", "''");
        conditions.push(format!("priority = '{}'", escaped_priority));
    }

    if failed {
        conditions.push("status IN ('failed', 'dead')".to_string());
    }

    if completed {
        conditions.push("status = 'completed'".to_string());
    }

    let mut job_table = JobTable::new();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            // Build PostgreSQL query
            let mut query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs".to_string();

            if !conditions.is_empty() {
                query.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
            }

            if let Some(hours) = last_hours {
                if conditions.is_empty() {
                    query.push_str(&format!(
                        " WHERE created_at > NOW() - INTERVAL '{} hours'",
                        hours
                    ));
                } else {
                    query.push_str(&format!(
                        " AND created_at > NOW() - INTERVAL '{} hours'",
                        hours
                    ));
                }
            }

            query.push_str(" ORDER BY created_at DESC");
            query.push_str(&format!(" LIMIT {}", limit));

            let rows = sqlx::query(&query).fetch_all(&pg_pool).await?;
            for row in rows {
                let id: uuid::Uuid = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority: String = row.try_get("priority")?;
                let attempts: i32 = row.try_get("attempts")?;
                let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                let scheduled_at: chrono::DateTime<chrono::Utc> = row.try_get("scheduled_at")?;

                job_table.add_job_row(
                    &id.to_string(),
                    &queue_name,
                    &status,
                    &priority,
                    attempts,
                    &created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                    &scheduled_at.format("%Y-%m-%d %H:%M:%S").to_string(),
                );
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            // Build MySQL query with different interval syntax
            let mut query = "SELECT id, queue_name, status, priority, attempts, created_at, scheduled_at FROM hammerwork_jobs".to_string();

            if !conditions.is_empty() {
                query.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
            }

            if let Some(hours) = last_hours {
                if conditions.is_empty() {
                    query.push_str(&format!(
                        " WHERE created_at > DATE_SUB(NOW(), INTERVAL {} HOUR)",
                        hours
                    ));
                } else {
                    query.push_str(&format!(
                        " AND created_at > DATE_SUB(NOW(), INTERVAL {} HOUR)",
                        hours
                    ));
                }
            }

            query.push_str(" ORDER BY created_at DESC");
            query.push_str(&format!(" LIMIT {}", limit));

            let rows = sqlx::query(&query).fetch_all(&mysql_pool).await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority: String = row.try_get("priority")?;
                let attempts: i32 = row.try_get("attempts")?;
                let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
                let scheduled_at: chrono::DateTime<chrono::Utc> = row.try_get("scheduled_at")?;

                job_table.add_job_row(
                    &id,
                    &queue_name,
                    &status,
                    &priority,
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

async fn show_job_details(pool: DatabasePool, job_id: &str) -> Result<()> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let job_uuid = uuid::Uuid::parse_str(job_id)?;
            let row = sqlx::query("SELECT * FROM hammerwork_jobs WHERE id = $1")
                .bind(job_uuid)
                .fetch_optional(&pg_pool)
                .await?;

            if let Some(row) = row {
                print_job_details_postgres(&row)?;
            } else {
                println!("❌ Job not found: {}", job_id);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let row = sqlx::query("SELECT * FROM hammerwork_jobs WHERE id = ?")
                .bind(job_id)
                .fetch_optional(&mysql_pool)
                .await?;

            if let Some(row) = row {
                print_job_details_mysql(&row)?;
            } else {
                println!("❌ Job not found: {}", job_id);
            }
        }
    }

    Ok(())
}

fn print_job_details_postgres(row: &sqlx::postgres::PgRow) -> Result<()> {
    println!("📋 Job Details");
    println!("═══════════════");
    println!("ID: {}", row.try_get::<String, _>("id")?);
    println!("Queue: {}", row.try_get::<String, _>("queue_name")?);
    println!("Status: {}", row.try_get::<String, _>("status")?);
    println!("Priority: {}", row.try_get::<String, _>("priority")?);
    println!(
        "Attempts: {}/{}",
        row.try_get::<i32, _>("attempts")?,
        row.try_get::<i32, _>("max_attempts")?
    );

    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
    let scheduled_at: chrono::DateTime<chrono::Utc> = row.try_get("scheduled_at")?;

    println!("Created: {}", created_at.format("%Y-%m-%d %H:%M:%S UTC"));
    println!(
        "Scheduled: {}",
        scheduled_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    if let Ok(Some(started)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("started_at")
    {
        println!("Started: {}", started.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    if let Ok(Some(completed)) =
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("completed_at")
    {
        println!("Completed: {}", completed.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    if let Ok(Some(failed)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("failed_at") {
        println!("Failed: {}", failed.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    if let Ok(Some(error)) = row.try_get::<Option<String>, _>("error_message") {
        println!("Error: {}", error);
    }

    let payload: serde_json::Value = row.try_get("payload")?;
    println!("Payload: {}", serde_json::to_string_pretty(&payload)?);

    Ok(())
}

fn print_job_details_mysql(row: &sqlx::mysql::MySqlRow) -> Result<()> {
    println!("📋 Job Details");
    println!("═══════════════");
    println!("ID: {}", row.try_get::<String, _>("id")?);
    println!("Queue: {}", row.try_get::<String, _>("queue_name")?);
    println!("Status: {}", row.try_get::<String, _>("status")?);
    println!("Priority: {}", row.try_get::<String, _>("priority")?);
    println!(
        "Attempts: {}/{}",
        row.try_get::<i32, _>("attempts")?,
        row.try_get::<i32, _>("max_attempts")?
    );

    let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;
    let scheduled_at: chrono::DateTime<chrono::Utc> = row.try_get("scheduled_at")?;

    println!("Created: {}", created_at.format("%Y-%m-%d %H:%M:%S UTC"));
    println!(
        "Scheduled: {}",
        scheduled_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    if let Ok(Some(started)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("started_at")
    {
        println!("Started: {}", started.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    if let Ok(Some(completed)) =
        row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("completed_at")
    {
        println!("Completed: {}", completed.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    if let Ok(Some(failed)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("failed_at") {
        println!("Failed: {}", failed.format("%Y-%m-%d %H:%M:%S UTC"));
    }

    if let Ok(Some(error)) = row.try_get::<Option<String>, _>("error_message") {
        println!("Error: {}", error);
    }

    let payload: serde_json::Value = row.try_get("payload")?;
    println!("Payload: {}", serde_json::to_string_pretty(&payload)?);

    Ok(())
}

async fn enqueue_job(
    pool: DatabasePool,
    queue: &str,
    payload: &str,
    priority: &Option<String>,
    delay: Option<u64>,
    max_attempts: Option<u32>,
    timeout: Option<u32>,
) -> Result<()> {
    let payload_value = validate_json_payload(payload)?;
    let job_priority = if let Some(p) = priority {
        validate_priority(p)?
    } else {
        JobPriority::Normal
    };

    let job_queue = pool.create_job_queue();
    let mut job = Job::new(queue.to_string(), payload_value);
    job.priority = job_priority;

    if let Some(max_att) = max_attempts {
        job.max_attempts = max_att as i32;
    }

    if let Some(timeout_secs) = timeout {
        job.timeout = Some(std::time::Duration::from_secs(timeout_secs as u64));
    }

    if let Some(delay_secs) = delay {
        let scheduled_at = chrono::Utc::now() + chrono::Duration::seconds(delay_secs as i64);
        job.scheduled_at = scheduled_at;
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

async fn retry_jobs(
    pool: DatabasePool,
    job_id: Option<String>,
    queue: Option<String>,
    all: bool,
) -> Result<()> {
    if !all && job_id.is_none() && queue.is_none() {
        return Err(anyhow::anyhow!("Must specify --job-id, --queue, or --all"));
    }

    let (query, affected) = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            if let Some(id) = job_id {
                let job_uuid = uuid::Uuid::parse_str(&id)?;
                let result = sqlx::query(
                    "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() 
                     WHERE id = $1 AND status IN ('failed', 'dead')"
                )
                .bind(job_uuid)
                .execute(pg_pool).await?;
                ("single job".to_string(), result.rows_affected())
            } else if let Some(queue_name) = queue {
                let result = sqlx::query(
                    "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() 
                     WHERE queue_name = $1 AND status IN ('failed', 'dead')"
                )
                .bind(&queue_name)
                .execute(pg_pool).await?;
                (format!("queue '{}'", queue_name), result.rows_affected())
            } else {
                let result = sqlx::query(
                    "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() 
                     WHERE status IN ('failed', 'dead')"
                )
                .execute(pg_pool).await?;
                ("all failed jobs".to_string(), result.rows_affected())
            }
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            if let Some(id) = job_id {
                let result = sqlx::query(
                    "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() 
                     WHERE id = ? AND status IN ('failed', 'dead')"
                )
                .bind(id)
                .execute(mysql_pool).await?;
                ("single job".to_string(), result.rows_affected())
            } else if let Some(queue_name) = queue {
                let result = sqlx::query(
                    "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() 
                     WHERE queue_name = ? AND status IN ('failed', 'dead')"
                )
                .bind(&queue_name)
                .execute(mysql_pool).await?;
                (format!("queue '{}'", queue_name), result.rows_affected())
            } else {
                let result = sqlx::query(
                    "UPDATE hammerwork_jobs SET status = 'pending', attempts = 0, scheduled_at = NOW() 
                     WHERE status IN ('failed', 'dead')"
                )
                .execute(mysql_pool).await?;
                ("all failed jobs".to_string(), result.rows_affected())
            }
        }
    };

    info!("✅ Retried {} jobs for {}", affected, query);
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

    let (query, affected) = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            if let Some(id) = job_id {
                let job_uuid = uuid::Uuid::parse_str(&id)?;
                let result =
                    sqlx::query("DELETE FROM hammerwork_jobs WHERE id = $1 AND status = 'pending'")
                        .bind(job_uuid)
                        .execute(pg_pool)
                        .await?;
                ("single job".to_string(), result.rows_affected())
            } else if let Some(queue_name) = queue {
                let result = sqlx::query(
                    "DELETE FROM hammerwork_jobs WHERE queue_name = $1 AND status = 'pending'",
                )
                .bind(&queue_name)
                .execute(pg_pool)
                .await?;
                (format!("queue '{}'", queue_name), result.rows_affected())
            } else {
                let result = sqlx::query("DELETE FROM hammerwork_jobs WHERE status = 'pending'")
                    .execute(pg_pool)
                    .await?;
                ("all pending jobs".to_string(), result.rows_affected())
            }
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            if let Some(id) = job_id {
                let result =
                    sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ? AND status = 'pending'")
                        .bind(id)
                        .execute(mysql_pool)
                        .await?;
                ("single job".to_string(), result.rows_affected())
            } else if let Some(queue_name) = queue {
                let result = sqlx::query(
                    "DELETE FROM hammerwork_jobs WHERE queue_name = ? AND status = 'pending'",
                )
                .bind(&queue_name)
                .execute(mysql_pool)
                .await?;
                (format!("queue '{}'", queue_name), result.rows_affected())
            } else {
                let result = sqlx::query("DELETE FROM hammerwork_jobs WHERE status = 'pending'")
                    .execute(mysql_pool)
                    .await?;
                ("all pending jobs".to_string(), result.rows_affected())
            }
        }
    };

    info!("✅ Cancelled {} jobs for {}", affected, query);
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

    let mut conditions = Vec::new();

    if completed {
        conditions.push("status = 'completed'");
    }
    if dead {
        conditions.push("status = 'dead'");
    }
    if failed {
        conditions.push("status = 'failed'");
    }

    let status_condition = format!("({})", conditions.join(" OR "));

    let affected = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            let mut query = format!("DELETE FROM hammerwork_jobs WHERE {}", status_condition);

            if let Some(queue_name) = queue {
                query.push_str(&format!(" AND queue_name = '{}'", queue_name));
            }

            if let Some(days) = older_than_days {
                query.push_str(&format!(
                    " AND created_at < NOW() - INTERVAL '{} days'",
                    days
                ));
            }

            let result = sqlx::query(&query).execute(pg_pool).await?;
            result.rows_affected()
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let mut query = format!("DELETE FROM hammerwork_jobs WHERE {}", status_condition);

            if let Some(queue_name) = queue {
                query.push_str(&format!(" AND queue_name = '{}'", queue_name));
            }

            if let Some(days) = older_than_days {
                query.push_str(&format!(
                    " AND created_at < DATE_SUB(NOW(), INTERVAL {} DAY)",
                    days
                ));
            }

            let result = sqlx::query(&query).execute(mysql_pool).await?;
            result.rows_affected()
        }
    };

    info!("✅ Purged {} jobs", affected);
    Ok(())
}
