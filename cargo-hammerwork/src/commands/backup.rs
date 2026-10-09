use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use sqlx::Row;
use std::fs::File;
use std::io::{BufWriter, Write};
use tracing::info;

use crate::commands::job::priority_display;
use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::job_rows::{self, Column, Param, RowInsert};
use crate::utils::sql::{Backend, Bind, SqlParams, bind_mysql, bind_pg};
use crate::utils::validation::validate_priority;

/// Version of the JSON backup format written by `backup create`.
///
/// - `2.0`: every column of `hammerwork_jobs` (see [`job_rows`]), including encrypted
///   payloads and their encryption metadata.
/// - `1.0`: the 13 columns of [`JobData`]. Still restored.
pub const BACKUP_VERSION: &str = "2.0";

#[derive(Subcommand)]
pub enum BackupCommand {
    #[command(
        about = "Create a backup of job data",
        long_about = "Create a backup of job data.\n\n\
            The JSON format (the default) holds every column of every selected job, \
            including encrypted payloads (still encrypted) and their encryption metadata, \
            cron schedules, timeouts, retry strategies, dependencies, workflows, results, \
            tracing and retention, so `backup restore` recreates the jobs exactly. The CSV \
            format is an export of the main columns only and cannot be restored."
    )]
    Create {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'o', long, help = "Output file path")]
        output: String,
        #[arg(
            short = 'n',
            short_alias = 'Q',
            long,
            help = "Include only specific queue"
        )]
        queue: Option<String>,
        #[arg(long, help = "Include completed jobs")]
        include_completed: bool,
        #[arg(long, help = "Include failed jobs")]
        include_failed: bool,
        #[arg(
            long,
            help = "Backup format: json (complete, restorable; default) or csv (export only)"
        )]
        format: Option<String>,
    },
    #[command(
        about = "Restore job data from backup",
        long_about = "Restore jobs from a JSON backup made by `backup create`.\n\n\
            Every column in the backup is restored as it was. Jobs whose id already exists \
            in the database are never overwritten or duplicated: they are skipped and \
            counted. The backup is checked before anything is written, and the jobs are \
            inserted in one transaction, so a bad backup restores nothing. A backup taken \
            from a newer schema must be restored into a database migrated at least as far."
    )]
    Restore {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'i', long, help = "Input file path")]
        input: String,
        #[arg(long, help = "Confirm the restore operation")]
        confirm: bool,
        #[arg(
            long,
            help = "Skip existing jobs (always the case: existing jobs are never overwritten)"
        )]
        skip_existing: bool,
    },
    #[command(about = "List available backups")]
    List {
        #[arg(short = 'p', long, help = "Backup directory path")]
        path: Option<String>,
    },
}

/// A job as written to backups and exports.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub(crate) struct JobData {
    pub id: String,
    pub queue_name: String,
    pub payload: Value,
    pub status: String,
    pub priority: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub scheduled_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub failed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
}

/// The columns `JobData` is read from.
pub(crate) const JOB_DATA_COLUMNS: &str = "id, queue_name, payload, status, priority, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message";

impl BackupCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        match self {
            BackupCommand::Create {
                database_url,
                output,
                queue,
                include_completed,
                include_failed,
                format,
            } => {
                let db_url = database_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;

                create_backup(
                    db_url,
                    output,
                    queue.clone(),
                    *include_completed,
                    *include_failed,
                    format.as_deref().unwrap_or("json"),
                    config.get_connection_pool_size(),
                )
                .await?;
            }
            BackupCommand::Restore {
                database_url,
                input,
                confirm,
                skip_existing,
            } => {
                let db_url = database_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;

                restore_backup(
                    db_url,
                    input,
                    *confirm,
                    *skip_existing,
                    config.get_connection_pool_size(),
                )
                .await?;
            }
            BackupCommand::List { path } => {
                list_backups(path.clone()).await?;
            }
        }
        Ok(())
    }
}

/// The `SELECT` of [`JOB_DATA_COLUMNS`] that gathers the jobs to back up (as CSV). The
/// queue name is bound, never interpolated.
pub fn build_backup_query(
    backend: Backend,
    queue: Option<&str>,
    include_completed: bool,
    include_failed: bool,
) -> (String, Vec<Bind>) {
    build_backup_select(
        backend,
        JOB_DATA_COLUMNS,
        queue,
        include_completed,
        include_failed,
    )
}

/// The `SELECT` of `select_list` that gathers the jobs to back up.
pub fn build_backup_select(
    backend: Backend,
    select_list: &str,
    queue: Option<&str>,
    include_completed: bool,
    include_failed: bool,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let mut query = format!("SELECT {select_list} FROM hammerwork_jobs WHERE 1=1");
    let mut conditions = Vec::new();

    if let Some(queue_name) = queue {
        conditions.push(format!("queue_name = {}", params.text(queue_name)));
    }
    if !include_completed {
        conditions.push("status != 'Completed'".to_string());
    }
    if !include_failed {
        conditions.push("status != 'Failed'".to_string());
    }
    if !conditions.is_empty() {
        query.push_str(&format!(" AND {}", conditions.join(" AND ")));
    }
    query.push_str(" ORDER BY created_at ASC");
    (query, params.into_binds())
}

/// Every column of the jobs to back up, as backup objects (see [`job_rows`]).
async fn fetch_backup_rows(
    pool: &DatabasePool,
    columns: &[Column],
    queue: Option<&str>,
    include_completed: bool,
    include_failed: bool,
) -> Result<Vec<serde_json::Map<String, Value>>> {
    let select = job_rows::select_list(pool.backend(), columns);
    let (query, binds) = build_backup_select(
        pool.backend(),
        &select,
        queue,
        include_completed,
        include_failed,
    );
    match pool {
        DatabasePool::Postgres(pg_pool) => bind_pg(sqlx::query(&query), &binds)
            .fetch_all(pg_pool)
            .await?
            .iter()
            .map(|row| job_rows::pg_row_to_json(row, columns))
            .collect(),
        DatabasePool::MySQL(mysql_pool) => bind_mysql(sqlx::query(&query), &binds)
            .fetch_all(mysql_pool)
            .await?
            .iter()
            .map(|row| job_rows::mysql_row_to_json(row, columns))
            .collect(),
    }
}

async fn fetch_backup_jobs(
    pool: &DatabasePool,
    queue: Option<&str>,
    include_completed: bool,
    include_failed: bool,
) -> Result<Vec<JobData>> {
    let (query, binds) =
        build_backup_query(pool.backend(), queue, include_completed, include_failed);
    info!("Creating backup with query: {}", query);
    fetch_job_data(pool, &query, &binds).await
}

/// Run a query selecting [`JOB_DATA_COLUMNS`] and read the rows as [`JobData`].
pub(crate) async fn fetch_job_data(
    pool: &DatabasePool,
    query: &str,
    binds: &[Bind],
) -> Result<Vec<JobData>> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = bind_pg(sqlx::query(query), binds)
                .fetch_all(pg_pool)
                .await?;
            rows.into_iter()
                .map(|row| extract_job_data_postgres(&row))
                .collect()
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = bind_mysql(sqlx::query(query), binds)
                .fetch_all(mysql_pool)
                .await?;
            rows.into_iter()
                .map(|row| extract_job_data_mysql(&row))
                .collect()
        }
    }
}

pub(crate) const CSV_HEADER: &str = "id,queue_name,payload,status,priority,attempts,max_attempts,created_at,scheduled_at,started_at,completed_at,failed_at,error_message";

/// One CSV field, quoted when it contains a comma, quote or line break (RFC 4180).
pub(crate) fn csv_field(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub(crate) fn csv_row(job: &JobData) -> String {
    let time =
        |t: Option<chrono::DateTime<chrono::Utc>>| t.map(|t| t.to_rfc3339()).unwrap_or_default();
    [
        csv_field(&job.id),
        csv_field(&job.queue_name),
        csv_field(&job.payload.to_string()),
        csv_field(&job.status),
        csv_field(&job.priority),
        job.attempts.to_string(),
        job.max_attempts.to_string(),
        job.created_at.to_rfc3339(),
        job.scheduled_at.to_rfc3339(),
        time(job.started_at),
        time(job.completed_at),
        time(job.failed_at),
        csv_field(job.error_message.as_deref().unwrap_or("")),
    ]
    .join(",")
}

async fn create_backup(
    database_url: &str,
    output: &str,
    queue: Option<String>,
    include_completed: bool,
    include_failed: bool,
    format: &str,
    pool_size: u32,
) -> Result<()> {
    if !matches!(format, "json" | "csv") {
        anyhow::bail!("Unknown backup format '{}': use json or csv", format);
    }
    let pool = DatabasePool::connect(database_url, pool_size).await?;

    // Read everything before creating the file, so a failure leaves no partial backup.
    let (content, total) = if format == "csv" {
        let job_data =
            fetch_backup_jobs(&pool, queue.as_deref(), include_completed, include_failed).await?;
        let mut csv = format!("{}\n", CSV_HEADER);
        for job in &job_data {
            csv.push_str(&csv_row(job));
            csv.push('\n');
        }
        (csv, job_data.len())
    } else {
        let columns = job_rows::job_columns(&pool).await?;
        let rows = fetch_backup_rows(
            &pool,
            &columns,
            queue.as_deref(),
            include_completed,
            include_failed,
        )
        .await?;
        let backup_data = serde_json::json!({
            "version": BACKUP_VERSION,
            "created_at": chrono::Utc::now(),
            "backend": match pool.backend() {
                Backend::Postgres => "postgres",
                Backend::MySql => "mysql",
            },
            "total_jobs": rows.len(),
            "filters": {
                "queue": queue,
                "include_completed": include_completed,
                "include_failed": include_failed
            },
            "columns": columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            "jobs": rows
        });
        (serde_json::to_string_pretty(&backup_data)?, rows.len())
    };
    info!("Found {} jobs to backup", total);

    let file = File::create(output)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(content.as_bytes())?;
    writer.flush()?;
    info!("✅ Backup created successfully: {}", output);
    println!("💾 Backup saved to: {}", output);
    println!("📊 Total jobs backed up: {}", total);

    Ok(())
}

fn extract_job_data_postgres(row: &sqlx::postgres::PgRow) -> Result<JobData> {
    Ok(JobData {
        id: row.try_get::<uuid::Uuid, _>("id")?.to_string(),
        queue_name: row.try_get("queue_name")?,
        payload: row.try_get("payload")?,
        status: row.try_get("status")?,
        priority: priority_display(row.try_get("priority")?),
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        created_at: row.try_get("created_at")?,
        scheduled_at: row.try_get("scheduled_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        failed_at: row.try_get("failed_at")?,
        error_message: row.try_get("error_message")?,
    })
}

fn extract_job_data_mysql(row: &sqlx::mysql::MySqlRow) -> Result<JobData> {
    Ok(JobData {
        id: row.try_get("id")?,
        queue_name: row.try_get("queue_name")?,
        payload: row.try_get("payload")?,
        status: row.try_get("status")?,
        priority: priority_display(row.try_get("priority")?),
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        created_at: row.try_get("created_at")?,
        scheduled_at: row.try_get("scheduled_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        failed_at: row.try_get("failed_at")?,
        error_message: row.try_get("error_message")?,
    })
}

async fn restore_backup(
    database_url: &str,
    input: &str,
    confirm: bool,
    skip_existing: bool,
    pool_size: u32,
) -> Result<()> {
    if !confirm {
        println!("⚠️  This will restore jobs from backup. Use --confirm to proceed.");
        return Ok(());
    }

    let pool = DatabasePool::connect(database_url, pool_size).await?;

    // Read backup file
    let backup_content = std::fs::read_to_string(input)
        .map_err(|e| anyhow::anyhow!("Cannot read backup file {}: {}", input, e))?;
    let backup_data: Value = serde_json::from_str(&backup_content).map_err(|e| {
        anyhow::anyhow!(
            "{} is not a JSON backup ({}); only backups created with --format json can be restored",
            input,
            e
        )
    })?;

    match backup_data.get("version").and_then(Value::as_str) {
        None | Some("1.0") | Some("2.0") => {}
        Some(other) => anyhow::bail!(
            "{} is a version {} backup, which this cargo-hammerwork cannot restore; \
             upgrade cargo-hammerwork",
            input,
            other
        ),
    }
    let jobs = backup_data["jobs"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid backup format: missing jobs array"))?;

    info!("Restoring {} jobs from backup", jobs.len());

    // Check every job before writing anything.
    let target = job_rows::job_columns(&pool).await?;
    let mut rows = Vec::with_capacity(jobs.len());
    let mut skipped = 0;
    for job in jobs {
        let row = restore_row(job, &target)?;
        if skip_existing && check_job_exists(&pool, &row.id).await? {
            skipped += 1;
            continue;
        }
        rows.push(row);
    }

    // Jobs whose id exists (now, or by the time they are inserted) are left alone.
    let restored = job_rows::insert_rows(&pool, &rows).await?;
    skipped += rows.len() - restored;

    info!(
        "✅ Restore completed: {} jobs restored, {} skipped",
        restored, skipped
    );
    println!("📥 Restore completed successfully");
    println!("   Restored: {} jobs", restored);
    if skipped > 0 {
        println!("   Skipped: {} existing jobs", skipped);
    }

    Ok(())
}

async fn check_job_exists(pool: &DatabasePool, id: &str) -> Result<bool> {
    let mut params = SqlParams::new(pool.backend());
    let id = params.uuid(id);
    let sql = format!("SELECT 1 AS present FROM hammerwork_jobs WHERE id = {id}");
    Ok(match pool {
        DatabasePool::Postgres(pg_pool) => bind_pg(sqlx::query(&sql), params.binds())
            .fetch_optional(pg_pool)
            .await?
            .is_some(),
        DatabasePool::MySQL(mysql_pool) => bind_mysql(sqlx::query(&sql), params.binds())
            .fetch_optional(mysql_pool)
            .await?
            .is_some(),
    })
}

/// Fields of a backed-up job, validated and converted for insertion.
#[derive(Debug)]
struct BackupJobFields<'a> {
    id: uuid::Uuid,
    queue_name: &'a str,
    payload: &'a Value,
    status: &'a str,
    priority: i32,
    attempts: i32,
    max_attempts: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    scheduled_at: chrono::DateTime<chrono::Utc>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    failed_at: Option<chrono::DateTime<chrono::Utc>>,
    error_message: Option<&'a str>,
}

const JOB_STATUSES: [&str; 8] = [
    "Pending",
    "Running",
    "Completed",
    "Failed",
    "Dead",
    "TimedOut",
    "Retrying",
    "Archived",
];

/// Read an optional string field; a present but non-string value is an error.
fn backup_str<'a>(job: &'a Value, field: &str, default: &'a str) -> Result<&'a str> {
    match job.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::String(s)) => Ok(s),
        Some(other) => Err(anyhow::anyhow!(
            "backup field '{}' must be a string, got {}",
            field,
            other
        )),
    }
}

/// Read an optional i32 field; a present but non-integer or out-of-range value is an error.
fn backup_i32(job: &Value, field: &str, default: i32) -> Result<i32> {
    match job.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "backup field '{}' must be a 32-bit integer, got {}",
                    field,
                    value
                )
            }),
    }
}

/// Read an optional RFC 3339 timestamp. A missing field falls back to now;
/// a present but unparseable one is an error rather than silently becoming now.
fn backup_timestamp(job: &Value, field: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    match job.get(field) {
        None | Some(Value::Null) => Ok(chrono::Utc::now()),
        Some(Value::String(s)) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .map_err(|e| {
                anyhow::anyhow!("backup field '{}' is not RFC 3339 ({}): {}", field, s, e)
            }),
        Some(other) => Err(anyhow::anyhow!(
            "backup field '{}' must be a timestamp string, got {}",
            field,
            other
        )),
    }
}

/// Read an optional timestamp; absent or null is `None`.
fn backup_optional_timestamp(
    job: &Value,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    match job.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => backup_timestamp(job, field).map(Some),
    }
}

/// The priority of a backed-up job: the name written by `backup create` ("high") or the
/// numeric level stored in the database.
fn backup_priority(job: &Value) -> Result<i32> {
    match job.get("priority") {
        None | Some(Value::Null) => Ok(hammerwork::JobPriority::Normal.as_i32()),
        Some(Value::String(name)) => Ok(validate_priority(name)?.as_i32()),
        Some(value) => value
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .filter(|n| hammerwork::JobPriority::from_i32(*n).is_ok())
            .ok_or_else(|| {
                anyhow::anyhow!("backup field 'priority' is not a priority, got {}", value)
            }),
    }
}

fn parse_backup_job(job: &Value) -> Result<BackupJobFields<'_>> {
    let id = backup_str(job, "id", "")?;
    if id.is_empty() {
        anyhow::bail!("backup job is missing required field 'id'");
    }
    let id = uuid::Uuid::parse_str(id)
        .map_err(|e| anyhow::anyhow!("backup job id '{}' is not a UUID: {}", id, e))?;
    let queue_name = backup_str(job, "queue_name", "")?;
    if queue_name.is_empty() {
        anyhow::bail!("backup job {} is missing required field 'queue_name'", id);
    }
    let status = backup_str(job, "status", "Pending")?;
    if !JOB_STATUSES.contains(&status) {
        anyhow::bail!(
            "backup job {} has unknown status '{}' (expected one of {})",
            id,
            status,
            JOB_STATUSES.join(", ")
        );
    }
    Ok(BackupJobFields {
        id,
        queue_name,
        payload: &job["payload"],
        status,
        priority: backup_priority(job)?,
        attempts: backup_i32(job, "attempts", 0)?,
        max_attempts: backup_i32(job, "max_attempts", 3)?,
        created_at: backup_timestamp(job, "created_at")?,
        scheduled_at: backup_timestamp(job, "scheduled_at")?,
        started_at: backup_optional_timestamp(job, "started_at")?,
        completed_at: backup_optional_timestamp(job, "completed_at")?,
        failed_at: backup_optional_timestamp(job, "failed_at")?,
        error_message: match job.get("error_message") {
            None | Some(Value::Null) => None,
            Some(_) => Some(backup_str(job, "error_message", "")?),
        },
    })
}

/// A backed-up job with its core fields validated and normalized ([`parse_backup_job`]:
/// priority names become levels, missing timestamps become now, a missing payload
/// becomes `{}`), and every other column as it was backed up.
fn normalized_backup_job(job: &Value) -> Result<serde_json::Map<String, Value>> {
    let mut object = job
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("backup job must be a JSON object, got {}", job))?;
    let fields = parse_backup_job(job)?;
    let time = |t: chrono::DateTime<chrono::Utc>| {
        Value::String(t.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
    };
    let optional_time = |t: Option<chrono::DateTime<chrono::Utc>>| t.map_or(Value::Null, time);
    let payload = if fields.payload.is_null() {
        Value::Object(Default::default())
    } else {
        fields.payload.clone()
    };
    for (column, value) in [
        ("id", Value::String(fields.id.to_string())),
        ("queue_name", Value::String(fields.queue_name.to_string())),
        ("payload", payload),
        ("status", Value::String(fields.status.to_string())),
        ("priority", Value::from(fields.priority)),
        ("attempts", Value::from(fields.attempts)),
        ("max_attempts", Value::from(fields.max_attempts)),
        ("created_at", time(fields.created_at)),
        ("scheduled_at", time(fields.scheduled_at)),
        ("started_at", optional_time(fields.started_at)),
        ("completed_at", optional_time(fields.completed_at)),
        ("failed_at", optional_time(fields.failed_at)),
        (
            "error_message",
            fields
                .error_message
                .map_or(Value::Null, |m| Value::String(m.to_string())),
        ),
    ] {
        object.insert(column.to_string(), value);
    }
    Ok(object)
}

/// The insert of one backed-up job into a `hammerwork_jobs` table with `target` columns.
///
/// Every column in the backup is restored. A non-null value for a column the table does
/// not have is an error (the backup comes from a newer schema), so nothing is lost
/// silently; columns missing from the backup get their defaults.
fn restore_row(job: &Value, target: &[Column]) -> Result<RowInsert> {
    let object = normalized_backup_job(job)?;
    let id = object["id"].as_str().unwrap_or_default().to_string();
    let mut columns = Vec::new();
    let mut params = Vec::new();
    for (name, value) in &object {
        match target.iter().find(|column| column.name == *name) {
            Some(column) => {
                params.push(
                    Param::from_backup(column, value)
                        .map_err(|e| anyhow::anyhow!("backup job {}: {}", id, e))?,
                );
                columns.push(column.clone());
            }
            None if value.is_null() => {}
            None => anyhow::bail!(
                "backup job {} has a value for column '{}', which this database's \
                 hammerwork_jobs table does not have; run `cargo hammerwork migration run` \
                 on it first",
                id,
                name
            ),
        }
    }
    Ok(RowInsert {
        id,
        columns,
        params,
    })
}

async fn list_backups(path: Option<String>) -> Result<()> {
    let backup_dir = path.unwrap_or_else(|| "./backups".to_string());

    if !std::path::Path::new(&backup_dir).exists() {
        println!("📂 No backup directory found at: {}", backup_dir);
        return Ok(());
    }

    let entries = std::fs::read_dir(&backup_dir)?;
    let mut backups = Vec::new();

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_file()
            && let Some(ext) = path.extension()
            && (ext == "json" || ext == "csv")
        {
            let metadata = entry.metadata()?;
            let size = metadata.len();
            let modified = metadata.modified()?;
            let modified_time = chrono::DateTime::<chrono::Utc>::from(modified);

            backups.push((
                path.file_name().unwrap().to_string_lossy().to_string(),
                size,
                modified_time,
            ));
        }
    }

    if backups.is_empty() {
        println!("📂 No backups found in: {}", backup_dir);
        return Ok(());
    }

    backups.sort_by_key(|b| std::cmp::Reverse(b.2)); // Sort by modified time, newest first

    println!("📋 Available Backups");
    println!("════════════════════");

    for (name, size, modified) in backups {
        let size_str = if size > 1024 * 1024 {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        } else if size > 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else {
            format!("{} bytes", size)
        };

        println!(
            "📄 {} ({}) - {}",
            name,
            size_str,
            modified.format("%Y-%m-%d %H:%M:%S UTC")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::*;
    use clap::Parser;

    const ID: &str = "11111111-1111-1111-1111-111111111111";

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: BackupCommand,
    }

    fn parse(args: &[&str]) -> BackupCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&[
            "create",
            "-o",
            "out.json",
            "-n",
            "q",
            "--include-completed",
            "--include-failed",
            "--format",
            "csv",
        ]) {
            BackupCommand::Create {
                output,
                queue,
                include_completed,
                include_failed,
                format,
                ..
            } => {
                assert_eq!(output, "out.json");
                assert_eq!(queue.as_deref(), Some("q"));
                assert!(include_completed && include_failed);
                assert_eq!(format.as_deref(), Some("csv"));
            }
            _ => panic!("expected Create"),
        }
        match parse(&["restore", "-i", "in.json", "--confirm", "--skip-existing"]) {
            BackupCommand::Restore {
                input,
                confirm,
                skip_existing,
                ..
            } => assert_eq!(
                (input.as_str(), confirm, skip_existing),
                ("in.json", true, true)
            ),
            _ => panic!("expected Restore"),
        }
        assert!(matches!(
            parse(&["list", "-p", "/tmp/b"]),
            BackupCommand::List { path: Some(p) } if p == "/tmp/b"
        ));
        assert!(
            TestCli::try_parse_from(["test", "create"]).is_err(),
            "--output is required"
        );
        assert!(
            TestCli::try_parse_from(["test", "restore"]).is_err(),
            "--input is required"
        );
    }

    #[tokio::test]
    async fn create_and_restore_need_a_database_url() {
        let config = Config::default();
        for cmd in [
            parse(&["create", "-o", "x.json"]),
            parse(&["restore", "-i", "x.json", "--confirm"]),
        ] {
            let err = cmd.execute(&config).await.unwrap_err();
            assert!(
                err.to_string().contains("Database URL is required"),
                "{err}"
            );
        }
    }

    #[test]
    fn parse_backup_job_defaults_and_values() {
        let job = serde_json::json!({
            "id": ID, "queue_name": "q", "payload": {"a": 1},
            "attempts": 2, "created_at": "2024-01-02T03:04:05Z",
            "priority": "critical", "status": "Failed",
            "failed_at": "2024-01-02T04:00:00Z", "error_message": "boom"
        });
        let parsed = parse_backup_job(&job).unwrap();
        assert_eq!(parsed.id.to_string(), ID);
        assert_eq!(parsed.attempts, 2);
        assert_eq!(parsed.max_attempts, 3);
        assert_eq!(parsed.status, "Failed");
        assert_eq!(parsed.priority, hammerwork::JobPriority::Critical.as_i32());
        assert_eq!(parsed.created_at.to_rfc3339(), "2024-01-02T03:04:05+00:00");
        assert_eq!(
            parsed.failed_at.unwrap().to_rfc3339(),
            "2024-01-02T04:00:00+00:00"
        );
        assert_eq!(parsed.error_message, Some("boom"));
        assert!(parsed.started_at.is_none() && parsed.completed_at.is_none());

        let minimal_job = serde_json::json!({"id": ID, "queue_name": "q"});
        let minimal = parse_backup_job(&minimal_job).unwrap();
        assert_eq!(minimal.status, "Pending");
        assert_eq!(minimal.priority, hammerwork::JobPriority::Normal.as_i32());
        assert!(minimal.payload.is_null());
    }

    #[test]
    fn parse_backup_job_accepts_numeric_priorities_only_in_range() {
        let with = |p: serde_json::Value| {
            parse_backup_job(&serde_json::json!({"id": ID, "queue_name": "q", "priority": p}))
                .map(|f| f.priority)
        };
        assert_eq!(with(serde_json::json!(4)).unwrap(), 4);
        assert_eq!(with(serde_json::json!("Background")).unwrap(), 0);
        assert!(with(serde_json::json!(9)).is_err());
        assert!(with(serde_json::json!("urgent")).is_err());
        assert!(with(serde_json::json!(true)).is_err());
    }

    #[test]
    fn parse_backup_job_rejects_bad_input() {
        let err = parse_backup_job(
            &serde_json::json!({"id": ID, "queue_name": "q", "created_at": "yesterday"}),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("created_at"), "{err}");
        let err = parse_backup_job(
            &serde_json::json!({"id": ID, "queue_name": "q", "attempts": 3_000_000_000i64}),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("attempts"), "{err}");
        assert!(
            parse_backup_job(
                &serde_json::json!({"id": ID, "queue_name": "q", "max_attempts": "many"})
            )
            .is_err()
        );
        assert!(parse_backup_job(&serde_json::json!({"queue_name": "q"})).is_err());
        assert!(parse_backup_job(&serde_json::json!({"id": ID})).is_err());
        let err = parse_backup_job(&serde_json::json!({"id": "abc", "queue_name": "q"}))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a UUID"), "{err}");
        let err =
            parse_backup_job(&serde_json::json!({"id": ID, "queue_name": "q", "status": "bogus"}))
                .unwrap_err()
                .to_string();
        assert!(err.contains("unknown status 'bogus'"), "{err}");
        assert!(parse_backup_job(&serde_json::json!({"id": 5, "queue_name": "q"})).is_err());
        assert!(
            parse_backup_job(&serde_json::json!({"id": ID, "queue_name": "q", "error_message": 5}))
                .is_err()
        );
    }

    #[test]
    fn csv_fields_are_quoted_when_needed() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");

        let job = JobData {
            id: ID.into(),
            queue_name: "q,1".into(),
            payload: serde_json::json!({"a": 1, "b": "x,y"}),
            status: "Failed".into(),
            priority: "high".into(),
            attempts: 1,
            max_attempts: 3,
            created_at: chrono::DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z")
                .unwrap()
                .into(),
            scheduled_at: chrono::DateTime::parse_from_rfc3339("2024-01-02T03:04:05Z")
                .unwrap()
                .into(),
            started_at: None,
            completed_at: None,
            failed_at: None,
            error_message: Some("bad, \"worse\"".into()),
        };
        let row = csv_row(&job);
        assert!(row.starts_with(&format!(
            "{ID},\"q,1\",\"{{\"\"a\"\":1,\"\"b\"\":\"\"x,y\"\"}}\",Failed,high,1,3,"
        )));
        assert!(row.ends_with(",,,,\"bad, \"\"worse\"\"\""), "{row}");
        assert_eq!(CSV_HEADER.split(',').count(), 13);
    }

    #[test]
    fn test_backup_query_binds_queue_name() {
        let (sql, binds) = build_backup_query(Backend::Postgres, Some(HOSTILE_QUEUE), false, true);
        assert!(sql.contains("AND queue_name = $1 AND status != 'Completed' ORDER BY"));
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into())]);
        assert!(!sql.contains("DROP"));

        let (sql, binds) = build_backup_query(Backend::MySql, Some(HOSTILE_QUEUE), true, true);
        assert!(sql.contains("WHERE 1=1 AND queue_name = ? ORDER BY created_at ASC"));
        assert_eq!(binds.len(), 1);

        let (sql, binds) = build_backup_query(Backend::MySql, None, true, true);
        assert!(!sql.contains('?') && binds.is_empty());
    }

    #[tokio::test]
    async fn list_reports_missing_empty_and_populated_directories() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        list_backups(Some(missing.to_str().unwrap().into()))
            .await
            .unwrap();
        list_backups(Some(dir.path().to_str().unwrap().into()))
            .await
            .unwrap();

        std::fs::write(dir.path().join("a.json"), "{}").unwrap();
        std::fs::write(dir.path().join("b.csv"), vec![b'x'; 2048]).unwrap();
        std::fs::write(dir.path().join("big.json"), vec![b'x'; 2 * 1024 * 1024]).unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();
        std::fs::create_dir(dir.path().join("sub.json")).unwrap();
        parse(&["list", "-p", dir.path().to_str().unwrap()])
            .execute(&Config::default())
            .await
            .unwrap();
    }

    async fn backup_roundtrip(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let hostile = hostile_queue();
        let other = unique_queue("backup_other");
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str| dir.path().join(name).to_str().unwrap().to_string();

        let mut high = SeedJob::new(&hostile, "Pending");
        high.payload = r#"{"to": "a@example.com", "note": "x,y \"q\""}"#;
        let pending = seed(&pool, &high).await;
        exec_sql(
            &pool,
            &format!("UPDATE hammerwork_jobs SET priority = 4 WHERE id = '{pending}'"),
        )
        .await;
        let mut failed = SeedJob::new(&hostile, "Failed");
        failed.failed_now = true;
        let failed_id = seed(&pool, &failed).await;
        exec_sql(
            &pool,
            &format!("UPDATE hammerwork_jobs SET error_message = 'it broke', attempts = 3 WHERE id = '{failed_id}'"),
        )
        .await;
        let completed = seed(&pool, &SeedJob::new(&hostile, "Completed")).await;
        seed(&pool, &SeedJob::new(&other, "Pending")).await;

        let backup = |args: Vec<String>| {
            let cmd = parse(&args.iter().map(String::as_str).collect::<Vec<_>>());
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let s = String::from;

        // JSON backup of one queue: completed and failed jobs only with the include flags
        let default_out = file("default.json");
        backup(vec![
            s("create"),
            s("-o"),
            default_out.clone(),
            s("-n"),
            hostile.clone(),
        ])
        .await
        .unwrap();
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(&default_out).unwrap()).unwrap();
        assert_eq!(doc["version"], BACKUP_VERSION);
        assert_eq!(doc["total_jobs"], 1);
        assert_eq!(doc["filters"]["queue"], hostile.as_str());
        assert_eq!(doc["jobs"][0]["id"], pending.as_str());
        assert_eq!(doc["jobs"][0]["priority"], 4);
        assert_eq!(doc["jobs"][0]["payload"]["to"], "a@example.com");

        let full_out = file("full.json");
        backup(vec![
            s("create"),
            s("-o"),
            full_out.clone(),
            s("-n"),
            hostile.clone(),
            s("--include-completed"),
            s("--include-failed"),
        ])
        .await
        .unwrap();
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(&full_out).unwrap()).unwrap();
        assert_eq!(doc["total_jobs"], 3);
        let failed_json = doc["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == failed_id.as_str())
            .unwrap();
        assert_eq!(failed_json["error_message"], "it broke");
        assert!(failed_json["failed_at"].is_string());

        // CSV backup quotes embedded commas and quotes
        let csv_out = file("full.csv");
        backup(vec![
            s("create"),
            s("-o"),
            csv_out.clone(),
            s("-n"),
            hostile.clone(),
            s("--include-completed"),
            s("--include-failed"),
            s("--format"),
            s("csv"),
        ])
        .await
        .unwrap();
        let csv = std::fs::read_to_string(&csv_out).unwrap();
        assert!(csv.starts_with(CSV_HEADER));
        assert!(
            csv.contains(&format!("{pending},{hostile},\"")),
            "plain fields stay unquoted: {csv}"
        );
        assert!(
            csv.contains(r#""{""note"":""x,y \""q\"""",""to"":""a@example.com""}""#),
            "payload is one quoted field: {csv}"
        );
        assert_eq!(csv.matches(&pending).count(), 1);

        // restore: refuses without --confirm, then recreates deleted jobs with their data
        cleanup(&pool, &[&hostile]).await;
        backup(vec![s("restore"), s("-i"), full_out.clone()])
            .await
            .unwrap();
        assert_eq!(
            count_jobs(&pool, &hostile, None).await,
            0,
            "no --confirm, no restore"
        );
        backup(vec![
            s("restore"),
            s("-i"),
            full_out.clone(),
            s("--confirm"),
        ])
        .await
        .unwrap();
        assert_eq!(count_jobs(&pool, &hostile, None).await, 3);
        assert_eq!(job_status(&pool, &pending).await, "Pending");
        assert_eq!(
            job_column(&pool, &pending, "priority").await.as_deref(),
            Some("4")
        );
        assert_eq!(job_status(&pool, &failed_id).await, "Failed");
        assert_eq!(
            job_column(&pool, &failed_id, "error_message")
                .await
                .as_deref(),
            Some("it broke")
        );
        assert!(job_column(&pool, &failed_id, "failed_at").await.is_some());
        assert_eq!(
            job_column(&pool, &failed_id, "attempts").await.as_deref(),
            Some("3")
        );
        assert_eq!(job_status(&pool, &completed).await, "Completed");
        let payload = job_column(&pool, &pending, "payload").await.unwrap();
        assert!(payload.contains("a@example.com"), "{payload}");

        // restoring again: existing rows are skipped (with and without --skip-existing)
        backup(vec![
            s("restore"),
            s("-i"),
            full_out.clone(),
            s("--confirm"),
            s("--skip-existing"),
        ])
        .await
        .unwrap();
        backup(vec![
            s("restore"),
            s("-i"),
            full_out.clone(),
            s("--confirm"),
        ])
        .await
        .unwrap();
        assert_eq!(count_jobs(&pool, &hostile, None).await, 3);

        // bad input is reported, and nothing is half-written
        let err = backup(vec![s("restore"), s("-i"), csv_out.clone(), s("--confirm")])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a JSON backup"), "{err}");
        let err = backup(vec![
            s("restore"),
            s("-i"),
            file("missing.json"),
            s("--confirm"),
        ])
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("Cannot read backup file"), "{err}");
        let no_jobs = file("nojobs.json");
        std::fs::write(&no_jobs, "{}").unwrap();
        let err = backup(vec![s("restore"), s("-i"), no_jobs, s("--confirm")])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing jobs array"), "{err}");
        let bad = file("bad.json");
        std::fs::write(
            &bad,
            r#"{"jobs": [{"id": "not-a-uuid", "queue_name": "q"}]}"#,
        )
        .unwrap();
        assert!(
            backup(vec![s("restore"), s("-i"), bad, s("--confirm")])
                .await
                .is_err()
        );

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    db_tests!(
        backup_roundtrip,
        test_backup_roundtrip_postgres,
        test_backup_roundtrip_mysql
    );

    use hammerwork::queue::DatabaseQueue;
    use hammerwork::{Job, JobPriority};

    const TEST_KEY: &str = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=";

    fn test_engine_config() -> hammerwork::encryption::EncryptionConfig {
        hammerwork::encryption::EncryptionConfig::new(
            hammerwork::encryption::EncryptionAlgorithm::AES256GCM,
        )
        .with_key_id("backup-key")
        .with_key_source(hammerwork::encryption::KeySource::Static(
            TEST_KEY.to_string(),
        ))
    }

    /// Jobs on `queue_name` that set every kind of attribute: encrypted (whole payload
    /// and PII fields, with retention), recurring with a timezone, timeouts, retry
    /// strategies, dependencies in a workflow, a batch, a stored result, tracing, and a
    /// dead job. Returns their ids.
    async fn seed_every_kind<DB>(
        queue: &hammerwork::JobQueue<DB>,
        queue_name: &str,
        secret: &str,
    ) -> Vec<uuid::Uuid>
    where
        DB: sqlx::Database,
        hammerwork::JobQueue<DB>: hammerwork::queue::DatabaseQueue,
    {
        use hammerwork::encryption::RetentionPolicy;
        use std::time::Duration;

        let q = queue_name.to_string();
        let thirty_days = RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 3600));
        let mut ids = Vec::new();

        let whole = Job::new(q.clone(), serde_json::json!({"card": secret}))
            .with_encryption(test_engine_config())
            .with_retention_policy(thirty_days.clone())
            .with_priority(JobPriority::High)
            .with_max_attempts(7)
            .with_timeout(Duration::from_secs(90));
        ids.push(queue.enqueue(whole).await.unwrap());

        let pii = Job::new(q.clone(), serde_json::json!({"card": secret, "amount": 3}))
            .with_encryption(test_engine_config())
            .with_pii_fields(vec!["card"])
            .with_retention_policy(thirty_days)
            .with_trace_id("trace-1")
            .with_correlation_id("corr-1")
            .with_parent_span_id("span-1")
            .with_span_context("ctx");
        ids.push(queue.enqueue(pii).await.unwrap());

        let cron = Job::new(q.clone(), serde_json::json!({"report": "daily"}))
            .with_cron(
                hammerwork::cron::CronSchedule::with_timezone("0 0 9 * * *", "America/New_York")
                    .unwrap(),
            )
            .unwrap()
            .with_retry_strategy(hammerwork::retry::RetryStrategy::exponential(
                Duration::from_secs(2),
                2.0,
                Some(Duration::from_secs(600)),
            ));
        ids.push(queue.enqueue_cron_job(cron).await.unwrap());

        let first = Job::new(q.clone(), serde_json::json!({"step": 1}));
        let second = Job::new(q.clone(), serde_json::json!({"step": 2})).depends_on(&first.id);
        ids.extend([first.id, second.id]);
        queue
            .enqueue_workflow(
                hammerwork::workflow::JobGroup::new("backup workflow")
                    .add_job(first)
                    .add_job(second),
            )
            .await
            .unwrap();

        let batched = Job::new(q.clone(), serde_json::json!({"in": "batch"}));
        ids.push(batched.id);
        queue
            .enqueue_batch(
                hammerwork::batch::JobBatch::new("backup batch").with_jobs(vec![batched]),
            )
            .await
            .unwrap();

        let done = queue
            .enqueue(
                Job::new(q.clone(), serde_json::json!({"done": true}))
                    .with_result_ttl(Duration::from_secs(3600)),
            )
            .await
            .unwrap();
        queue.complete_job(done).await.unwrap();
        queue
            .store_job_result(
                done,
                serde_json::json!({"rows": 42}),
                Some(chrono::Utc::now() + chrono::Duration::hours(1)),
            )
            .await
            .unwrap();
        ids.push(done);

        let dead = queue
            .enqueue(Job::new(q, serde_json::json!({"doomed": true})))
            .await
            .unwrap();
        queue.mark_job_dead(dead, "gave up").await.unwrap();
        ids.push(dead);
        ids
    }

    /// Every column of `id`, as text computed by the database (independent of the
    /// backup code).
    async fn row_fingerprint(pool: &DatabasePool, id: &str) -> String {
        match pool {
            DatabasePool::Postgres(p) => {
                sqlx::query_scalar("SELECT j::text FROM hammerwork_jobs j WHERE id = $1::uuid")
                    .bind(id)
                    .fetch_one(p)
                    .await
                    .unwrap()
            }
            DatabasePool::MySQL(p) => {
                let columns: Vec<(String, String)> = sqlx::query_as(
                    "SELECT CAST(COLUMN_NAME AS CHAR), CAST(DATA_TYPE AS CHAR) \
                     FROM information_schema.columns WHERE TABLE_SCHEMA = DATABASE() \
                     AND TABLE_NAME = 'hammerwork_jobs' ORDER BY ORDINAL_POSITION",
                )
                .fetch_all(p)
                .await
                .unwrap();
                let parts = columns
                    .iter()
                    .map(|(name, data_type)| {
                        if data_type.contains("blob") || data_type.contains("binary") {
                            format!("COALESCE(HEX(`{name}`), 'NULL')")
                        } else {
                            format!("COALESCE(CAST(`{name}` AS CHAR), 'NULL')")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                sqlx::query_scalar(&format!(
                    "SELECT CONCAT_WS('|', {parts}) FROM hammerwork_jobs WHERE id = ?"
                ))
                .bind(id)
                .fetch_one(p)
                .await
                .unwrap()
            }
        }
    }

    /// The names of every column of hammerwork_jobs, straight from the database.
    async fn table_column_names(pool: &DatabasePool) -> Vec<String> {
        let sql = match pool.backend() {
            Backend::Postgres => {
                "SELECT column_name::text AS name FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = 'hammerwork_jobs' \
                 ORDER BY ordinal_position"
            }
            Backend::MySql => {
                "SELECT CAST(COLUMN_NAME AS CHAR) AS name FROM information_schema.columns \
                 WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'hammerwork_jobs' \
                 ORDER BY ORDINAL_POSITION"
            }
        };
        column_strings(pool, sql, &[], "name").await
    }

    /// `backup create` + `backup restore` recreate every column of every job exactly,
    /// including encrypted payloads, which still decrypt afterwards (#64 C3).
    async fn backup_restores_every_column(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let queue_name = unique_queue("backup_full");
        let secret = format!("4111-{}", uuid::Uuid::new_v4().simple());
        let engine = || async {
            hammerwork::encryption::EncryptionEngine::new(test_engine_config())
                .await
                .unwrap()
        };
        let ids: Vec<String> = match pool.clone() {
            DatabasePool::Postgres(p) => {
                let queue = hammerwork::JobQueue::new(p).with_encryption(engine().await);
                seed_every_kind(&queue, &queue_name, &secret).await
            }
            DatabasePool::MySQL(p) => {
                let queue = hammerwork::JobQueue::new(p).with_encryption(engine().await);
                seed_every_kind(&queue, &queue_name, &secret).await
            }
        }
        .iter()
        .map(uuid::Uuid::to_string)
        .collect();
        // Columns no builder sets
        exec_sql(
            &pool,
            &format!(
                "UPDATE hammerwork_jobs SET last_heartbeat_at = created_at, \
                 lease_expires_at = scheduled_at WHERE queue_name = '{queue_name}'"
            ),
        )
        .await;

        let mut before = Vec::new();
        for id in &ids {
            before.push(row_fingerprint(&pool, id).await);
        }
        let columns = job_rows::job_columns(&pool).await.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("full.json").to_string_lossy().into_owned();
        let run = |args: &[&str]| {
            let cmd = parse(args);
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        run(&[
            "create",
            "-o",
            &file,
            "-n",
            &queue_name,
            "--include-completed",
            "--include-failed",
        ])
        .await
        .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains(&secret), "the backup holds only ciphertext");
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["version"], BACKUP_VERSION);
        assert_eq!(doc["total_jobs"], ids.len());
        // Every column of the table is in the backup: a column added by a migration
        // that backups could not copy fails here.
        let backed_up: Vec<String> = doc["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        assert_eq!(backed_up, table_column_names(&pool).await);
        for job in doc["jobs"].as_array().unwrap() {
            assert_eq!(job.as_object().unwrap().len(), backed_up.len());
        }

        // Wipe and restore
        cleanup(&pool, &[&queue_name]).await;
        run(&["restore", "-i", &file, "--confirm"]).await.unwrap();
        for (id, fingerprint) in ids.iter().zip(&before) {
            assert_eq!(
                &row_fingerprint(&pool, id).await,
                fingerprint,
                "job {id} differs after restore"
            );
        }
        // The reader agrees column by column (clearer failures than the fingerprint)
        let restored = fetch_backup_rows(&pool, &columns, Some(&queue_name), true, true)
            .await
            .unwrap();
        assert_eq!(
            Value::from(restored.into_iter().map(Value::Object).collect::<Vec<_>>()),
            doc["jobs"]
        );

        // Encrypted jobs decrypt after the restore
        let opened = match pool.clone() {
            DatabasePool::Postgres(p) => {
                let queue = hammerwork::JobQueue::new(p).with_encryption(engine().await);
                let mut payloads = Vec::new();
                for id in &ids[..2] {
                    let job = queue
                        .get_job(uuid::Uuid::parse_str(id).unwrap())
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(job.is_encrypted);
                    payloads.push(queue.decrypt_job(job).await.unwrap().payload);
                }
                payloads
            }
            DatabasePool::MySQL(p) => {
                let queue = hammerwork::JobQueue::new(p).with_encryption(engine().await);
                let mut payloads = Vec::new();
                for id in &ids[..2] {
                    let job = queue
                        .get_job(uuid::Uuid::parse_str(id).unwrap())
                        .await
                        .unwrap()
                        .unwrap();
                    assert!(job.is_encrypted);
                    payloads.push(queue.decrypt_job(job).await.unwrap().payload);
                }
                payloads
            }
        };
        assert_eq!(opened[0], serde_json::json!({"card": secret}));
        assert_eq!(opened[1], serde_json::json!({"card": secret, "amount": 3}));

        // Restoring again changes nothing: existing ids are skipped, never duplicated
        run(&["restore", "-i", &file, "--confirm"]).await.unwrap();
        assert_eq!(count_jobs(&pool, &queue_name, None).await, ids.len() as i64);
        for (id, fingerprint) in ids.iter().zip(&before) {
            assert_eq!(&row_fingerprint(&pool, id).await, fingerprint);
        }

        // A backup with a column this database lacks restores nothing
        cleanup(&pool, &[&queue_name]).await;
        let mut newer = doc.clone();
        newer["jobs"][ids.len() - 1]["column_from_the_future"] = Value::from("x");
        let newer_file = dir.path().join("newer.json").to_string_lossy().into_owned();
        std::fs::write(&newer_file, newer.to_string()).unwrap();
        let err = run(&["restore", "-i", &newer_file, "--confirm"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("column_from_the_future"), "{err}");
        assert_eq!(count_jobs(&pool, &queue_name, None).await, 0);
    }

    db_tests!(
        backup_restores_every_column,
        test_backup_restores_every_column_postgres,
        test_backup_restores_every_column_mysql
    );
}
