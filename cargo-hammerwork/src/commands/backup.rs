use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use sqlx::Row;
use std::fs::File;
use std::io::{BufWriter, Write};
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;

#[derive(Subcommand)]
pub enum BackupCommand {
    #[command(about = "Create a backup of job data")]
    Create {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'o', long, help = "Output file path")]
        output: String,
        #[arg(short = 'n', long, help = "Include only specific queue")]
        queue: Option<String>,
        #[arg(long, help = "Include completed jobs")]
        include_completed: bool,
        #[arg(long, help = "Include failed jobs")]
        include_failed: bool,
        #[arg(long, help = "Backup format (json, csv)")]
        format: Option<String>,
    },
    #[command(about = "Restore job data from backup")]
    Restore {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'i', long, help = "Input file path")]
        input: String,
        #[arg(long, help = "Confirm the restore operation")]
        confirm: bool,
        #[arg(long, help = "Skip existing jobs")]
        skip_existing: bool,
    },
    #[command(about = "List available backups")]
    List {
        #[arg(short = 'p', long, help = "Backup directory path")]
        path: Option<String>,
    },
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct JobData {
    id: String,
    queue_name: String,
    payload: Value,
    status: String,
    priority: String,
    attempts: i32,
    max_attempts: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    scheduled_at: chrono::DateTime<chrono::Utc>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    failed_at: Option<chrono::DateTime<chrono::Utc>>,
    error_message: Option<String>,
}

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

async fn create_backup(
    database_url: &str,
    output: &str,
    queue: Option<String>,
    include_completed: bool,
    include_failed: bool,
    format: &str,
    pool_size: u32,
) -> Result<()> {
    let pool = DatabasePool::connect(database_url, pool_size).await?;

    // Build query based on filters
    let mut query = "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, error_message FROM hammerwork_jobs WHERE 1=1".to_string();
    let mut conditions = Vec::new();

    if let Some(queue_name) = &queue {
        conditions.push(format!("queue_name = '{}'", queue_name));
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

    info!("Creating backup with query: {}", query);

    // Execute query and extract data based on database type
    let job_data = match &pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = sqlx::query(&query).fetch_all(pg_pool).await?;
            rows.into_iter()
                .map(|row| extract_job_data_postgres(&row))
                .collect::<Result<Vec<_>>>()?
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = sqlx::query(&query).fetch_all(mysql_pool).await?;
            rows.into_iter()
                .map(|row| extract_job_data_mysql(&row))
                .collect::<Result<Vec<_>>>()?
        }
    };

    info!("Found {} jobs to backup", job_data.len());

    // Create output file
    let file = File::create(output)?;
    let mut writer = BufWriter::new(file);

    match format {
        "csv" => {
            // CSV format
            writeln!(
                writer,
                "id,queue_name,payload,status,priority,attempts,max_attempts,created_at,scheduled_at,started_at,completed_at,failed_at,error_message"
            )?;

            for job in &job_data {
                writeln!(
                    writer,
                    "{},{},{},{},{},{},{},{},{},{},{},{},{}",
                    job.id,
                    job.queue_name,
                    job.payload.to_string().replace(',', ";"),
                    job.status,
                    job.priority,
                    job.attempts,
                    job.max_attempts,
                    job.created_at.to_rfc3339(),
                    job.scheduled_at.to_rfc3339(),
                    job.started_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                    job.completed_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                    job.failed_at.map(|t| t.to_rfc3339()).unwrap_or_default(),
                    job.error_message.as_deref().unwrap_or("").replace(',', ";")
                )?;
            }
        }
        _ => {
            // JSON format (default)
            let backup_data = serde_json::json!({
                "version": "1.0",
                "created_at": chrono::Utc::now(),
                "total_jobs": job_data.len(),
                "filters": {
                    "queue": queue,
                    "include_completed": include_completed,
                    "include_failed": include_failed
                },
                "jobs": job_data
            });

            serde_json::to_writer_pretty(&mut writer, &backup_data)?;
        }
    }

    writer.flush()?;
    info!("✅ Backup created successfully: {}", output);
    println!("💾 Backup saved to: {}", output);
    println!("📊 Total jobs backed up: {}", job_data.len());

    Ok(())
}

fn extract_job_data_postgres(row: &sqlx::postgres::PgRow) -> Result<JobData> {
    Ok(JobData {
        id: row.try_get("id")?,
        queue_name: row.try_get("queue_name")?,
        payload: row.try_get("payload")?,
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
    })
}

fn extract_job_data_mysql(row: &sqlx::mysql::MySqlRow) -> Result<JobData> {
    Ok(JobData {
        id: row.try_get("id")?,
        queue_name: row.try_get("queue_name")?,
        payload: row.try_get("payload")?,
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
    let backup_content = std::fs::read_to_string(input)?;
    let backup_data: Value = serde_json::from_str(&backup_content)?;

    let jobs = backup_data["jobs"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Invalid backup format: missing jobs array"))?;

    info!("Restoring {} jobs from backup", jobs.len());

    let mut restored = 0;
    let mut skipped = 0;

    for job in jobs {
        let id = job["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid job: missing id"))?;

        // Check if job already exists
        if skip_existing {
            let exists = check_job_exists(&pool, id).await?;
            if exists {
                skipped += 1;
                continue;
            }
        }

        // Insert job
        insert_job_from_backup(&pool, job).await?;
        restored += 1;
    }

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
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let result = sqlx::query("SELECT 1 FROM hammerwork_jobs WHERE id = $1")
                .bind(id)
                .fetch_optional(pg_pool)
                .await?;
            Ok(result.is_some())
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query("SELECT 1 FROM hammerwork_jobs WHERE id = ?")
                .bind(id)
                .fetch_optional(mysql_pool)
                .await?;
            Ok(result.is_some())
        }
    }
}

async fn insert_job_from_backup(pool: &DatabasePool, job: &Value) -> Result<()> {
    let id = job["id"].as_str().unwrap_or("");
    let queue_name = job["queue_name"].as_str().unwrap_or("");
    let payload = &job["payload"];
    let status = job["status"].as_str().unwrap_or("Pending");
    let priority = job["priority"].as_str().unwrap_or("normal");
    let attempts = job["attempts"].as_i64().unwrap_or(0) as i32;
    let max_attempts = job["max_attempts"].as_i64().unwrap_or(3) as i32;

    let created_at = job["created_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    let scheduled_at = job["scheduled_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs (
                    id, queue_name, payload, status, priority, attempts, max_attempts,
                    created_at, scheduled_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                ON CONFLICT (id) DO NOTHING
            "#,
            )
            .bind(id)
            .bind(queue_name)
            .bind(payload)
            .bind(status)
            .bind(priority)
            .bind(attempts)
            .bind(max_attempts)
            .bind(created_at)
            .bind(scheduled_at)
            .execute(pg_pool)
            .await?;
        }
        DatabasePool::MySQL(mysql_pool) => {
            sqlx::query(
                r#"
                INSERT IGNORE INTO hammerwork_jobs (
                    id, queue_name, payload, status, priority, attempts, max_attempts,
                    created_at, scheduled_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            )
            .bind(id)
            .bind(queue_name)
            .bind(payload)
            .bind(status)
            .bind(priority)
            .bind(attempts)
            .bind(max_attempts)
            .bind(created_at)
            .bind(scheduled_at)
            .execute(mysql_pool)
            .await?;
        }
    }

    Ok(())
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

        if path.is_file() {
            if let Some(ext) = path.extension() {
                if ext == "json" || ext == "csv" {
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
        }
    }

    if backups.is_empty() {
        println!("📂 No backups found in: {}", backup_dir);
        return Ok(());
    }

    backups.sort_by(|a, b| b.2.cmp(&a.2)); // Sort by modified time, newest first

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
