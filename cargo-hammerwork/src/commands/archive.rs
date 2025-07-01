use anyhow::Result;
use clap::Subcommand;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;

#[derive(Subcommand)]
pub enum ArchiveCommand {
    #[command(about = "Archive jobs based on policy")]
    Run {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            short = 'q',
            long,
            help = "Queue name to archive (all queues if not specified)"
        )]
        queue_name: Option<String>,
        #[arg(
            long,
            default_value = "7",
            help = "Days to keep completed jobs before archiving"
        )]
        completed_after_days: u32,
        #[arg(
            long,
            default_value = "30",
            help = "Days to keep failed jobs before archiving"
        )]
        failed_after_days: u32,
        #[arg(
            long,
            default_value = "30",
            help = "Days to keep dead jobs before archiving"
        )]
        dead_after_days: u32,
        #[arg(
            long,
            default_value = "30",
            help = "Days to keep timed out jobs before archiving"
        )]
        timed_out_after_days: u32,
        #[arg(
            long,
            default_value = "1000",
            help = "Maximum number of jobs to archive in one batch"
        )]
        batch_size: usize,
        #[arg(
            long,
            default_value = "true",
            help = "Whether to compress archived payloads"
        )]
        compress: bool,
        #[arg(long, default_value = "6", help = "Compression level (0-9)")]
        compression_level: u32,
        #[arg(long, help = "Dry run - show what would be archived without archiving")]
        dry_run: bool,
        #[arg(long, help = "Reason for archival")]
        reason: Option<String>,
        #[arg(long, help = "Who initiated the archival")]
        archived_by: Option<String>,
    },
    #[command(about = "Restore an archived job back to the active queue")]
    Restore {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID to restore")]
        job_id: String,
    },
    #[command(about = "List archived jobs")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'q', long, help = "Filter by queue name")]
        queue_name: Option<String>,
        #[arg(
            short = 'l',
            long,
            default_value = "100",
            help = "Maximum number of jobs to list"
        )]
        limit: u32,
        #[arg(
            short = 'o',
            long,
            default_value = "0",
            help = "Number of jobs to skip"
        )]
        offset: u32,
        #[arg(long, help = "Output format (table, json, csv)")]
        format: Option<String>,
    },
    #[command(about = "Get archival statistics")]
    Stats {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'q', long, help = "Filter by queue name")]
        queue_name: Option<String>,
        #[arg(long, help = "Output format (table, json)")]
        format: Option<String>,
    },
    #[command(about = "Permanently delete archived jobs")]
    Purge {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Delete archived jobs older than this many days")]
        older_than_days: u32,
        #[arg(long, help = "Confirm the purge operation")]
        confirm: bool,
        #[arg(long, help = "Dry run - show what would be deleted")]
        dry_run: bool,
    },
    #[command(about = "Set archival policy for a queue")]
    SetPolicy {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'q', long, help = "Queue name")]
        queue_name: String,
        #[arg(long, help = "Days to keep completed jobs before archiving")]
        completed_after_days: Option<u32>,
        #[arg(long, help = "Days to keep failed jobs before archiving")]
        failed_after_days: Option<u32>,
        #[arg(long, help = "Days to keep dead jobs before archiving")]
        dead_after_days: Option<u32>,
        #[arg(long, help = "Days to keep timed out jobs before archiving")]
        timed_out_after_days: Option<u32>,
        #[arg(long, help = "Maximum number of jobs to archive in one batch")]
        batch_size: Option<usize>,
        #[arg(long, help = "Whether to compress archived payloads")]
        compress: Option<bool>,
        #[arg(long, help = "Enable or disable the policy")]
        enabled: Option<bool>,
    },
    #[command(about = "Get archival policy for a queue")]
    GetPolicy {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'q', long, help = "Queue name")]
        queue_name: String,
        #[arg(long, help = "Output format (table, json)")]
        format: Option<String>,
    },
    #[command(about = "Remove archival policy for a queue")]
    RemovePolicy {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'q', long, help = "Queue name")]
        queue_name: String,
        #[arg(long, help = "Confirm the removal")]
        confirm: bool,
    },
}

impl ArchiveCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

        match self {
            ArchiveCommand::Run {
                queue_name,
                completed_after_days,
                failed_after_days,
                dead_after_days,
                timed_out_after_days,
                batch_size,
                compress,
                compression_level,
                dry_run,
                reason,
                archived_by,
                ..
            } => {
                archive_jobs(
                    pool,
                    queue_name.as_deref(),
                    *completed_after_days,
                    *failed_after_days,
                    *dead_after_days,
                    *timed_out_after_days,
                    *batch_size,
                    *compress,
                    *compression_level,
                    *dry_run,
                    reason.as_deref(),
                    archived_by.as_deref(),
                )
                .await?;
            }
            ArchiveCommand::Restore { job_id, .. } => {
                restore_archived_job(pool, job_id).await?;
            }
            ArchiveCommand::List {
                queue_name,
                limit,
                offset,
                format,
                ..
            } => {
                list_archived_jobs(
                    pool,
                    queue_name.as_deref(),
                    *limit,
                    *offset,
                    format.as_deref().unwrap_or("table"),
                )
                .await?;
            }
            ArchiveCommand::Stats {
                queue_name, format, ..
            } => {
                show_archival_stats(
                    pool,
                    queue_name.as_deref(),
                    format.as_deref().unwrap_or("table"),
                )
                .await?;
            }
            ArchiveCommand::Purge {
                older_than_days,
                confirm,
                dry_run,
                ..
            } => {
                purge_archived_jobs(pool, *older_than_days, *confirm, *dry_run).await?;
            }
            ArchiveCommand::SetPolicy {
                queue_name,
                completed_after_days,
                failed_after_days,
                dead_after_days,
                timed_out_after_days,
                batch_size,
                compress,
                enabled,
                ..
            } => {
                set_archival_policy(
                    pool,
                    queue_name,
                    *completed_after_days,
                    *failed_after_days,
                    *dead_after_days,
                    *timed_out_after_days,
                    *batch_size,
                    *compress,
                    *enabled,
                )
                .await?;
            }
            ArchiveCommand::GetPolicy {
                queue_name, format, ..
            } => {
                get_archival_policy(pool, queue_name, format.as_deref().unwrap_or("table")).await?;
            }
            ArchiveCommand::RemovePolicy {
                queue_name,
                confirm,
                ..
            } => {
                remove_archival_policy(pool, queue_name, *confirm).await?;
            }
        }

        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        match self {
            ArchiveCommand::Run { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::Restore { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::List { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::Stats { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::Purge { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::SetPolicy { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::GetPolicy { database_url, .. } => database_url.as_ref(),
            ArchiveCommand::RemovePolicy { database_url, .. } => database_url.as_ref(),
        }
        .cloned()
        .or_else(|| config.get_database_url().map(|s| s.to_string()))
        .ok_or_else(|| {
            anyhow::anyhow!("Database URL not provided. Use --database-url or set in config file")
        })
    }
}

// Helper functions for archive operations
async fn archive_jobs(
    pool: DatabasePool,
    queue_name: Option<&str>,
    completed_after_days: u32,
    failed_after_days: u32,
    dead_after_days: u32,
    timed_out_after_days: u32,
    batch_size: usize,
    compress: bool,
    compression_level: u32,
    dry_run: bool,
    reason: Option<&str>,
    archived_by: Option<&str>,
) -> Result<()> {
    use chrono::Duration;
    use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};
    use hammerwork::queue::DatabaseQueue;

    info!("Running job archival...");

    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::days(completed_after_days as i64))
        .archive_failed_after(Duration::days(failed_after_days as i64))
        .archive_dead_after(Duration::days(dead_after_days as i64))
        .archive_timed_out_after(Duration::days(timed_out_after_days as i64))
        .with_batch_size(batch_size)
        .compress_archived_payloads(compress)
        .enabled(!dry_run);

    let config = ArchivalConfig::new().with_compression_level(compression_level);

    let archival_reason = reason
        .map(|r| match r.to_lowercase().as_str() {
            "manual" => ArchivalReason::Manual,
            "compliance" => ArchivalReason::Compliance,
            "maintenance" => ArchivalReason::Maintenance,
            _ => ArchivalReason::Automatic,
        })
        .unwrap_or(ArchivalReason::Automatic);

    if dry_run {
        println!("DRY RUN: Would archive jobs with the following policy:");
        println!("  Queue: {}", queue_name.unwrap_or("ALL"));
        println!("  Completed after: {} days", completed_after_days);
        println!("  Failed after: {} days", failed_after_days);
        println!("  Dead after: {} days", dead_after_days);
        println!("  Timed out after: {} days", timed_out_after_days);
        println!("  Batch size: {}", batch_size);
        println!("  Compress: {}", compress);
        println!("  Compression level: {}", compression_level);
        println!("  Reason: {:?}", archival_reason);
        println!("  Archived by: {}", archived_by.unwrap_or("CLI"));
        return Ok(());
    }

    let stats = match pool {
        DatabasePool::Postgres(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue
                .archive_jobs(queue_name, &policy, &config, archival_reason, archived_by)
                .await?
        }
        DatabasePool::MySQL(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue
                .archive_jobs(queue_name, &policy, &config, archival_reason, archived_by)
                .await?
        }
    };

    println!("Archival completed successfully!");
    println!("  Jobs archived: {}", stats.jobs_archived);
    println!("  Bytes archived: {}", stats.bytes_archived);
    println!("  Compression ratio: {:.2}", stats.compression_ratio);
    println!("  Duration: {:?}", stats.operation_duration);

    Ok(())
}

async fn restore_archived_job(pool: DatabasePool, job_id: &str) -> Result<()> {
    use hammerwork::queue::DatabaseQueue;
    use uuid::Uuid;

    info!("Restoring archived job: {}", job_id);

    let job_uuid = Uuid::parse_str(job_id)?;

    let job = match pool {
        DatabasePool::Postgres(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue.restore_archived_job(job_uuid).await?
        }
        DatabasePool::MySQL(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue.restore_archived_job(job_uuid).await?
        }
    };

    println!("Job restored successfully!");
    println!("  Job ID: {}", job.id);
    println!("  Queue: {}", job.queue_name);
    println!("  Status: {:?}", job.status);
    println!("  Scheduled at: {}", job.scheduled_at);

    Ok(())
}

async fn list_archived_jobs(
    pool: DatabasePool,
    queue_name: Option<&str>,
    limit: u32,
    offset: u32,
    format: &str,
) -> Result<()> {
    use hammerwork::queue::DatabaseQueue;

    info!("Listing archived jobs...");

    let archived_jobs = match pool {
        DatabasePool::Postgres(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue
                .list_archived_jobs(queue_name, Some(limit), Some(offset))
                .await?
        }
        DatabasePool::MySQL(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue
                .list_archived_jobs(queue_name, Some(limit), Some(offset))
                .await?
        }
    };

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&archived_jobs)?);
        }
        "csv" => {
            println!(
                "id,queue_name,status,created_at,archived_at,reason,payload_compressed,archived_by"
            );
            for job in archived_jobs {
                println!(
                    "{},{},{:?},{},{},{:?},{},{}",
                    job.id,
                    job.queue_name,
                    job.status,
                    job.created_at,
                    job.archived_at,
                    job.archival_reason,
                    job.payload_compressed,
                    job.archived_by.unwrap_or_default()
                );
            }
        }
        _ => {
            // Table format
            use comfy_table::{Attribute, Cell, ContentArrangement, Table, presets::UTF8_FULL};

            let mut table = Table::new();
            table
                .load_preset(UTF8_FULL)
                .set_content_arrangement(ContentArrangement::Dynamic)
                .set_header(vec![
                    Cell::new("Job ID").add_attribute(Attribute::Bold),
                    Cell::new("Queue").add_attribute(Attribute::Bold),
                    Cell::new("Status").add_attribute(Attribute::Bold),
                    Cell::new("Created At").add_attribute(Attribute::Bold),
                    Cell::new("Archived At").add_attribute(Attribute::Bold),
                    Cell::new("Reason").add_attribute(Attribute::Bold),
                    Cell::new("Compressed").add_attribute(Attribute::Bold),
                    Cell::new("Archived By").add_attribute(Attribute::Bold),
                ]);

            for job in archived_jobs {
                table.add_row(vec![
                    Cell::new(job.id.to_string()),
                    Cell::new(&job.queue_name),
                    Cell::new(format!("{:?}", job.status)),
                    Cell::new(job.created_at.format("%Y-%m-%d %H:%M:%S").to_string()),
                    Cell::new(job.archived_at.format("%Y-%m-%d %H:%M:%S").to_string()),
                    Cell::new(format!("{:?}", job.archival_reason)),
                    Cell::new(if job.payload_compressed { "Yes" } else { "No" }),
                    Cell::new(job.archived_by.unwrap_or_default()),
                ]);
            }

            println!("{table}");
        }
    }

    Ok(())
}

async fn show_archival_stats(
    pool: DatabasePool,
    queue_name: Option<&str>,
    format: &str,
) -> Result<()> {
    use hammerwork::queue::DatabaseQueue;

    info!("Getting archival statistics...");

    let stats = match pool {
        DatabasePool::Postgres(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue.get_archival_stats(queue_name).await?
        }
        DatabasePool::MySQL(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue.get_archival_stats(queue_name).await?
        }
    };

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&stats)?);
        }
        _ => {
            println!("Archival Statistics");
            println!("==================");
            if let Some(queue) = queue_name {
                println!("Queue: {}", queue);
            } else {
                println!("Queue: ALL");
            }
            println!("Jobs archived: {}", stats.jobs_archived);
            println!("Jobs purged: {}", stats.jobs_purged);
            println!("Bytes archived: {}", stats.bytes_archived);
            println!("Bytes purged: {}", stats.bytes_purged);
            println!("Compression ratio: {:.2}", stats.compression_ratio);
            println!("Last run at: {}", stats.last_run_at);
        }
    }

    Ok(())
}

async fn purge_archived_jobs(
    pool: DatabasePool,
    older_than_days: u32,
    confirm: bool,
    dry_run: bool,
) -> Result<()> {
    use chrono::{Duration, Utc};
    use hammerwork::queue::DatabaseQueue;

    info!(
        "Purging archived jobs older than {} days...",
        older_than_days
    );

    let cutoff_date = Utc::now() - Duration::days(older_than_days as i64);

    if !confirm && !dry_run {
        return Err(anyhow::anyhow!(
            "Purge operation requires --confirm flag or --dry-run"
        ));
    }

    if dry_run {
        println!(
            "DRY RUN: Would permanently delete archived jobs older than {}",
            cutoff_date
        );
        return Ok(());
    }

    let deleted_count = match pool {
        DatabasePool::Postgres(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue.purge_archived_jobs(cutoff_date).await?
        }
        DatabasePool::MySQL(pool) => {
            let queue = hammerwork::JobQueue::new(pool);
            queue.purge_archived_jobs(cutoff_date).await?
        }
    };

    println!("Purged {} archived jobs", deleted_count);

    Ok(())
}

// Policy management functions (these would need to be implemented with a separate config storage)
async fn set_archival_policy(
    _pool: DatabasePool,
    queue_name: &str,
    _completed_after_days: Option<u32>,
    _failed_after_days: Option<u32>,
    _dead_after_days: Option<u32>,
    _timed_out_after_days: Option<u32>,
    _batch_size: Option<usize>,
    _compress: Option<bool>,
    _enabled: Option<bool>,
) -> Result<()> {
    // This would require a separate policy storage system
    // For now, just show what would be set
    println!("Setting archival policy for queue: {}", queue_name);
    println!("Note: Policy management requires additional configuration storage");
    println!("Use the 'run' command with specific parameters for one-time archival operations");
    Ok(())
}

async fn get_archival_policy(_pool: DatabasePool, queue_name: &str, _format: &str) -> Result<()> {
    println!("Getting archival policy for queue: {}", queue_name);
    println!("Note: Policy management requires additional configuration storage");
    println!("No persistent policies are currently configured");
    Ok(())
}

async fn remove_archival_policy(
    _pool: DatabasePool,
    queue_name: &str,
    _confirm: bool,
) -> Result<()> {
    println!("Removing archival policy for queue: {}", queue_name);
    println!("Note: Policy management requires additional configuration storage");
    println!("No persistent policies are currently configured");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_command_parsing() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestApp {
            #[command(subcommand)]
            command: ArchiveCommand,
        }

        let app = TestApp::try_parse_from(&[
            "test",
            "run",
            "--completed-after-days",
            "7",
            "--failed-after-days",
            "30",
            "--batch-size",
            "500",
        ]);

        assert!(app.is_ok());
    }

    #[test]
    fn test_restore_command_parsing() {
        use clap::Parser;

        #[derive(Parser)]
        struct TestApp {
            #[command(subcommand)]
            command: ArchiveCommand,
        }

        let app =
            TestApp::try_parse_from(&["test", "restore", "550e8400-e29b-41d4-a716-446655440000"]);

        assert!(app.is_ok());
    }
}
