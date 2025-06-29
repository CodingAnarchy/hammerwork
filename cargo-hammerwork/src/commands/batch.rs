use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::db_helpers::*;

#[derive(Subcommand)]
pub enum BatchCommand {
    #[command(about = "Enqueue multiple jobs from a file or stdin")]
    Enqueue {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'f', long, help = "Input file path (JSON lines format)")]
        file: Option<String>,
        #[arg(short = 'n', long, help = "Default queue name")]
        queue: String,
        #[arg(short = 'r', long, help = "Default priority")]
        priority: Option<String>,
        #[arg(long, help = "Batch size for bulk inserts")]
        batch_size: Option<u32>,
        #[arg(long, help = "Continue on errors")]
        continue_on_error: bool,
    },
    #[command(about = "Retry multiple jobs by criteria")]
    Retry {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(short = 't', long, help = "Status filter (failed, dead)")]
        status: Option<String>,
        #[arg(long, help = "Hours since last failure")]
        failed_since_hours: Option<u32>,
        #[arg(long, help = "Maximum attempts filter")]
        max_attempts_reached: bool,
        #[arg(long, help = "Confirm the batch retry operation")]
        confirm: bool,
        #[arg(long, help = "Dry run - show what would be retried")]
        dry_run: bool,
    },
    #[command(about = "Cancel multiple jobs by criteria")]
    Cancel {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(short = 't', long, help = "Status filter (pending, running)")]
        status: Option<String>,
        #[arg(long, help = "Jobs older than N hours")]
        older_than_hours: Option<u32>,
        #[arg(long, help = "Confirm the batch cancel operation")]
        confirm: bool,
        #[arg(long, help = "Dry run - show what would be cancelled")]
        dry_run: bool,
    },
    #[command(about = "Export job data to CSV/JSON")]
    Export {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'o', long, help = "Output file path")]
        output: String,
        #[arg(short = 'n', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(short = 't', long, help = "Status filter")]
        status: Option<String>,
        #[arg(long, help = "Export format (csv, json, jsonl)")]
        format: Option<String>,
        #[arg(long, help = "Include job payload in export")]
        include_payload: bool,
        #[arg(long, help = "Maximum number of jobs to export")]
        limit: Option<u32>,
    },
}

impl BatchCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

        match self {
            BatchCommand::Enqueue {
                file,
                queue,
                priority,
                batch_size,
                continue_on_error,
                ..
            } => {
                batch_enqueue(
                    pool,
                    file.clone(),
                    queue,
                    priority.clone(),
                    batch_size.unwrap_or(100),
                    *continue_on_error,
                )
                .await?;
            }
            BatchCommand::Retry {
                queue,
                status,
                failed_since_hours,
                max_attempts_reached,
                confirm,
                dry_run,
                ..
            } => {
                batch_retry(
                    pool,
                    queue.clone(),
                    status.clone(),
                    *failed_since_hours,
                    *max_attempts_reached,
                    *confirm,
                    *dry_run,
                )
                .await?;
            }
            BatchCommand::Cancel {
                queue,
                status,
                older_than_hours,
                confirm,
                dry_run,
                ..
            } => {
                batch_cancel(
                    pool,
                    queue.clone(),
                    status.clone(),
                    *older_than_hours,
                    *confirm,
                    *dry_run,
                )
                .await?;
            }
            BatchCommand::Export {
                output,
                queue,
                status,
                format,
                include_payload,
                limit,
                ..
            } => {
                println!("📤 Export functionality coming soon!");
                println!("   Output: {}", output);
                if let Some(q) = queue {
                    println!("   Queue filter: {}", q);
                }
                if let Some(s) = status {
                    println!("   Status filter: {}", s);
                }
                if let Some(f) = format {
                    println!("   Format: {}", f);
                }
                println!("   Include payload: {}", include_payload);
                if let Some(l) = limit {
                    println!("   Limit: {}", l);
                }
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url = match self {
            BatchCommand::Enqueue { database_url, .. } => database_url,
            BatchCommand::Retry { database_url, .. } => database_url,
            BatchCommand::Cancel { database_url, .. } => database_url,
            BatchCommand::Export { database_url, .. } => database_url,
        };

        url.as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
            .map(|s| s.to_string())
    }
}

async fn batch_enqueue(
    pool: DatabasePool,
    file: Option<String>,
    default_queue: &str,
    default_priority: Option<String>,
    batch_size: u32,
    continue_on_error: bool,
) -> Result<()> {
    info!("Starting batch enqueue operation");

    let reader: Box<dyn BufRead> = if let Some(file_path) = file {
        Box::new(BufReader::new(File::open(file_path)?))
    } else {
        println!("📥 Reading from stdin (provide JSON lines format)...");
        Box::new(BufReader::new(std::io::stdin()))
    };

    let mut total_processed = 0;
    let mut total_errors = 0;

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        // Parse JSON line
        let job_data: Value = match serde_json::from_str(&line) {
            Ok(data) => data,
            Err(e) => {
                total_errors += 1;
                eprintln!("❌ Line {}: Invalid JSON - {}", line_num + 1, e);
                if !continue_on_error {
                    return Err(anyhow::anyhow!(
                        "JSON parsing failed at line {}",
                        line_num + 1
                    ));
                }
                continue;
            }
        };

        // Extract job fields with defaults
        let queue = job_data["queue"]
            .as_str()
            .unwrap_or(default_queue)
            .to_string();
        let payload = job_data["payload"].clone();
        let priority = job_data["priority"]
            .as_str()
            .or(default_priority.as_deref())
            .unwrap_or("normal")
            .to_string();

        // Insert single job (simplified)
        let result = insert_single_job(&pool, &queue, &payload, &priority).await;
        match result {
            Ok(_) => {
                total_processed += 1;
                if total_processed % batch_size == 0 {
                    println!("✅ Processed {} jobs", total_processed);
                }
            }
            Err(e) => {
                total_errors += 1;
                eprintln!("❌ Job insert failed: {}", e);
                if !continue_on_error {
                    return Err(e);
                }
            }
        }
    }

    println!("📊 Batch Enqueue Summary");
    println!("════════════════════════");
    println!("Total jobs processed: {}", total_processed);
    if total_errors > 0 {
        println!("Total errors: {}", total_errors);
    }

    info!(
        "Batch enqueue completed: {} processed, {} errors",
        total_processed, total_errors
    );
    Ok(())
}

async fn insert_single_job(
    pool: &DatabasePool,
    queue: &str,
    payload: &Value,
    priority: &str,
) -> Result<()> {
    let job_id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs (
                    id, queue_name, payload, status, priority, attempts, max_attempts,
                    created_at, scheduled_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
            )
            .bind(&job_id)
            .bind(queue)
            .bind(payload)
            .bind("Pending")
            .bind(priority)
            .bind(0)
            .bind(3)
            .bind(now)
            .bind(now)
            .execute(pg_pool)
            .await?;
        }
        DatabasePool::MySQL(mysql_pool) => {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs (
                    id, queue_name, payload, status, priority, attempts, max_attempts,
                    created_at, scheduled_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            )
            .bind(&job_id)
            .bind(queue)
            .bind(payload)
            .bind("Pending")
            .bind(priority)
            .bind(0)
            .bind(3)
            .bind(now)
            .bind(now)
            .execute(mysql_pool)
            .await?;
        }
    }

    Ok(())
}

async fn batch_retry(
    pool: DatabasePool,
    queue: Option<String>,
    status: Option<String>,
    failed_since_hours: Option<u32>,
    max_attempts_reached: bool,
    confirm: bool,
    dry_run: bool,
) -> Result<()> {
    if !confirm && !dry_run {
        println!(
            "⚠️  This will retry multiple jobs. Use --confirm to proceed or --dry-run to preview."
        );
        return Ok(());
    }

    // Build filter conditions
    let mut conditions = Vec::new();

    if let Some(queue_name) = &queue {
        conditions.push(format!("queue_name = '{}'", queue_name));
    }

    match status.as_deref() {
        Some("failed") => conditions.push("status = 'Failed'".to_string()),
        Some("dead") => conditions.push("status = 'Dead'".to_string()),
        None => conditions.push("status IN ('Failed', 'Dead')".to_string()),
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid status filter. Use 'failed' or 'dead'"
            ));
        }
    }

    if let Some(hours) = failed_since_hours {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
        conditions.push(format!(
            "failed_at > '{}'",
            cutoff.format("%Y-%m-%d %H:%M:%S")
        ));
    }

    if max_attempts_reached {
        conditions.push("attempts >= max_attempts".to_string());
    }

    let where_clause = if conditions.is_empty() {
        "1=1".to_string()
    } else {
        conditions.join(" AND ")
    };

    // Count jobs to retry
    let count_query = format!(
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE {}",
        where_clause
    );
    let job_count = execute_count_query(&pool, &count_query).await?;

    println!("🔄 Batch Retry Analysis");
    println!("═══════════════════════");
    println!("Jobs matching criteria: {}", job_count);
    if let Some(q) = &queue {
        println!("Queue filter: {}", q);
    }
    if let Some(s) = &status {
        println!("Status filter: {}", s);
    }

    if dry_run {
        println!("\n💡 This was a dry run. Use --confirm to actually retry these jobs.");
        return Ok(());
    }

    if job_count == 0 {
        println!("✨ No jobs found matching the criteria.");
        return Ok(());
    }

    info!("Retrying {} jobs", job_count);

    // Update jobs to retry
    let update_query = format!(
        "UPDATE hammerwork_jobs SET status = 'Pending', attempts = 0, scheduled_at = '{}', error_message = NULL WHERE {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S"),
        where_clause
    );

    let updated = execute_update_query(&pool, &update_query).await?;

    println!("✅ Batch retry completed");
    println!("   Retried {} jobs", updated);

    Ok(())
}

async fn batch_cancel(
    pool: DatabasePool,
    queue: Option<String>,
    status: Option<String>,
    older_than_hours: Option<u32>,
    confirm: bool,
    dry_run: bool,
) -> Result<()> {
    if !confirm && !dry_run {
        println!(
            "⚠️  This will cancel multiple jobs. Use --confirm to proceed or --dry-run to preview."
        );
        return Ok(());
    }

    // Build filter conditions
    let mut conditions = Vec::new();

    if let Some(queue_name) = &queue {
        conditions.push(format!("queue_name = '{}'", queue_name));
    }

    match status.as_deref() {
        Some("pending") => conditions.push("status = 'Pending'".to_string()),
        Some("running") => conditions.push("status = 'Running'".to_string()),
        None => conditions.push("status IN ('Pending', 'Running')".to_string()),
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid status filter. Use 'pending' or 'running'"
            ));
        }
    }

    if let Some(hours) = older_than_hours {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
        conditions.push(format!(
            "created_at < '{}'",
            cutoff.format("%Y-%m-%d %H:%M:%S")
        ));
    }

    let where_clause = if conditions.is_empty() {
        "1=1".to_string()
    } else {
        conditions.join(" AND ")
    };

    // Count jobs to cancel
    let count_query = format!(
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE {}",
        where_clause
    );
    let job_count = execute_count_query(&pool, &count_query).await?;

    println!("🚫 Batch Cancel Analysis");
    println!("═══════════════════════");
    println!("Jobs matching criteria: {}", job_count);

    if dry_run {
        println!("\n💡 This was a dry run. Use --confirm to actually cancel these jobs.");
        return Ok(());
    }

    if job_count == 0 {
        println!("✨ No jobs found matching the criteria.");
        return Ok(());
    }

    info!("Cancelling {} jobs", job_count);

    // Delete jobs
    let delete_query = format!("DELETE FROM hammerwork_jobs WHERE {}", where_clause);
    let deleted = execute_update_query(&pool, &delete_query).await?;

    println!("✅ Batch cancel completed");
    println!("   Cancelled {} jobs", deleted);

    Ok(())
}
