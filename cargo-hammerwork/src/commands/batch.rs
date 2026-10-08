use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use tracing::info;

use crate::config::Config;
use hammerwork::JobStatus;

use crate::utils::database::DatabasePool;
use crate::utils::job_ops::{JobSelector, cancel_many, retry_many, select_job_ids};

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

/// Build the selector for `batch retry` from its CLI filters.
fn retry_selector(
    queue: &Option<String>,
    status: Option<&str>,
    failed_since_hours: Option<u32>,
    max_attempts_reached: bool,
) -> Result<JobSelector> {
    let statuses = match status {
        Some("failed") => vec![JobStatus::Failed],
        Some("dead") => vec![JobStatus::Dead],
        None => vec![JobStatus::Failed, JobStatus::Dead],
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid status filter. Use 'failed' or 'dead'"
            ));
        }
    };

    let mut extra = Vec::new();
    if let Some(hours) = failed_since_hours {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
        extra.push(format!(
            "failed_at > '{}'",
            cutoff.format("%Y-%m-%d %H:%M:%S")
        ));
    }
    if max_attempts_reached {
        extra.push("attempts >= max_attempts".to_string());
    }

    Ok(JobSelector {
        statuses,
        queue: queue.clone(),
        extra,
    })
}

/// Build the selector for `batch cancel` from its CLI filters.
fn cancel_selector(
    queue: &Option<String>,
    status: Option<&str>,
    older_than_hours: Option<u32>,
) -> Result<JobSelector> {
    let statuses = match status {
        Some("pending") => vec![JobStatus::Pending],
        Some("running") => vec![JobStatus::Running],
        None => vec![JobStatus::Pending, JobStatus::Running],
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid status filter. Use 'pending' or 'running'"
            ));
        }
    };

    let mut extra = Vec::new();
    if let Some(hours) = older_than_hours {
        let cutoff = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
        extra.push(format!(
            "created_at < '{}'",
            cutoff.format("%Y-%m-%d %H:%M:%S")
        ));
    }

    Ok(JobSelector {
        statuses,
        queue: queue.clone(),
        extra,
    })
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

    let selector = retry_selector(
        &queue,
        status.as_deref(),
        failed_since_hours,
        max_attempts_reached,
    )?;
    let ids = select_job_ids(&pool, &selector).await?;
    let job_count = ids.len();

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

    // Each job goes through the library's guarded transition (and its side effects).
    let wrapper = pool.create_job_queue();
    let result = retry_many(&wrapper, &ids).await;

    println!("✅ Batch retry completed");
    println!("   Retried {} jobs", result.succeeded);
    if !result.skipped.is_empty() {
        println!(
            "   Skipped {} jobs that changed state or could not be retried",
            result.skipped.len()
        );
    }

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

    let selector = cancel_selector(&queue, status.as_deref(), older_than_hours)?;
    let ids = select_job_ids(&pool, &selector).await?;
    let job_count = ids.len();

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

    let wrapper = pool.create_job_queue();
    let result = cancel_many(&wrapper, &ids, &selector.statuses).await;

    println!("✅ Batch cancel completed");
    println!("   Cancelled {} jobs", result.succeeded);
    if !result.skipped.is_empty() {
        println!(
            "   Skipped {} jobs that changed state before they could be cancelled",
            result.skipped.len()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_selector_uses_capitalized_statuses() {
        let selector = retry_selector(&Some("emails".into()), Some("dead"), Some(2), true).unwrap();
        assert_eq!(selector.statuses, vec![JobStatus::Dead]);
        assert_eq!(selector.queue.as_deref(), Some("emails"));
        assert_eq!(selector.extra.len(), 2);

        let all = retry_selector(&None, None, None, false).unwrap();
        assert_eq!(all.statuses, vec![JobStatus::Failed, JobStatus::Dead]);
        assert!(retry_selector(&None, Some("bogus"), None, false).is_err());
    }

    #[test]
    fn cancel_selector_validates_status() {
        let sel = cancel_selector(&None, None, None).unwrap();
        assert_eq!(sel.statuses, vec![JobStatus::Pending, JobStatus::Running]);
        assert!(cancel_selector(&None, Some("failed"), None).is_err());
    }
}
