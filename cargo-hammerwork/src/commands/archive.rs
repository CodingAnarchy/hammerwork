use anyhow::Result;
use clap::Subcommand;
use tracing::info;

use crate::commands::backup::csv_field;
use crate::config::Config;
use crate::utils::database::DatabasePool;

#[derive(Subcommand)]
pub enum ArchiveCommand {
    #[command(about = "Archive jobs based on policy")]
    Run {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            short = 'Q',
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
            default_value_t = true,
            num_args = 0..=1,
            default_missing_value = "true",
            action = clap::ArgAction::Set,
            help = "Whether to compress archived payloads (--compress false to disable)"
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
        #[arg(short = 'Q', long, help = "Filter by queue name")]
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
        #[arg(short = 'Q', long, help = "Filter by queue name")]
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
        }
        .cloned()
        .or_else(|| config.get_database_url().map(|s| s.to_string()))
        .ok_or_else(|| {
            anyhow::anyhow!("Database URL not provided. Use --database-url or set in config file")
        })
    }
}

// Helper functions for archive operations
// Mirrors the flags of the corresponding clap subcommand one-to-one.
#[allow(clippy::too_many_arguments)]
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
    use hammerwork::archive::{ArchivalConfig, ArchivalPolicy};
    use hammerwork::queue::DatabaseQueue;

    info!("Running job archival...");

    if compression_level > 9 {
        anyhow::bail!("compression level must be between 0 and 9, got {compression_level}");
    }
    if batch_size == 0 {
        anyhow::bail!("batch size must be at least 1");
    }

    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::days(completed_after_days as i64))
        .archive_failed_after(Duration::days(failed_after_days as i64))
        .archive_dead_after(Duration::days(dead_after_days as i64))
        .archive_timed_out_after(Duration::days(timed_out_after_days as i64))
        .with_batch_size(batch_size)
        .compress_archived_payloads(compress)
        .enabled(!dry_run);

    let config = ArchivalConfig::new().with_compression_level(compression_level);

    let archival_reason = parse_reason(reason)?;

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

/// The `--reason` flag: `manual`, `automatic`, `compliance` or `maintenance`.
fn parse_reason(reason: Option<&str>) -> Result<hammerwork::archive::ArchivalReason> {
    use hammerwork::archive::ArchivalReason;
    Ok(match reason.map(str::to_lowercase).as_deref() {
        None | Some("automatic") => ArchivalReason::Automatic,
        Some("manual") => ArchivalReason::Manual,
        Some("compliance") => ArchivalReason::Compliance,
        Some("maintenance") => ArchivalReason::Maintenance,
        Some(other) => anyhow::bail!(
            "Unknown archival reason '{other}'. Valid options: manual, automatic, compliance, maintenance"
        ),
    })
}

/// Check `--format` against the formats a command supports.
fn check_format<'a>(format: &'a str, allowed: &[&str]) -> Result<&'a str> {
    if allowed.contains(&format) {
        Ok(format)
    } else {
        anyhow::bail!(
            "Unknown format '{format}'. Valid options: {}",
            allowed.join(", ")
        )
    }
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
    let format = check_format(format, &["table", "json", "csv"])?;

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

    println!("{}", render_archived_jobs(&archived_jobs, format)?);
    Ok(())
}

/// Archived jobs as `json`, `csv` or (anything else) a table.
fn render_archived_jobs(jobs: &[hammerwork::archive::ArchivedJob], format: &str) -> Result<String> {
    Ok(match format {
        "json" => serde_json::to_string_pretty(jobs)?,
        "csv" => {
            let mut lines = vec![
                "id,queue_name,status,created_at,archived_at,reason,payload_compressed,archived_by"
                    .to_string(),
            ];
            for job in jobs {
                lines.push(format!(
                    "{},{},{:?},{},{},{:?},{},{}",
                    job.id,
                    csv_field(&job.queue_name),
                    job.status,
                    job.created_at.to_rfc3339(),
                    job.archived_at.to_rfc3339(),
                    job.archival_reason,
                    job.payload_compressed,
                    csv_field(job.archived_by.as_deref().unwrap_or_default())
                ));
            }
            lines.join("\n")
        }
        _ => {
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

            for job in jobs {
                table.add_row(vec![
                    Cell::new(job.id.to_string()),
                    Cell::new(&job.queue_name),
                    Cell::new(format!("{:?}", job.status)),
                    Cell::new(job.created_at.format("%Y-%m-%d %H:%M:%S").to_string()),
                    Cell::new(job.archived_at.format("%Y-%m-%d %H:%M:%S").to_string()),
                    Cell::new(format!("{:?}", job.archival_reason)),
                    Cell::new(if job.payload_compressed { "Yes" } else { "No" }),
                    Cell::new(job.archived_by.as_deref().unwrap_or_default()),
                ]);
            }
            table.to_string()
        }
    })
}

async fn show_archival_stats(
    pool: DatabasePool,
    queue_name: Option<&str>,
    format: &str,
) -> Result<()> {
    use hammerwork::queue::DatabaseQueue;

    info!("Getting archival statistics...");
    let format = check_format(format, &["table", "json"])?;

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

    println!("{}", render_archival_stats(&stats, queue_name, format)?);
    Ok(())
}

fn render_archival_stats(
    stats: &hammerwork::archive::ArchivalStats,
    queue_name: Option<&str>,
    format: &str,
) -> Result<String> {
    Ok(match format {
        "json" => serde_json::to_string_pretty(stats)?,
        _ => format!(
            "Archival Statistics\n==================\nQueue: {}\nJobs archived: {}\nJobs purged: {}\n\
             Bytes archived: {}\nBytes purged: {}\nCompression ratio: {:.2}\nLast run at: {}",
            queue_name.unwrap_or("ALL"),
            stats.jobs_archived,
            stats.jobs_purged,
            stats.bytes_archived,
            stats.bytes_purged,
            stats.compression_ratio,
            stats.last_run_at
        ),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::database::JobQueueWrapper;
    use crate::utils::sql::{IntervalUnit, SqlParams, execute_binds, fetch_i64};
    use crate::utils::test_support::*;
    use clap::Parser;
    use hammerwork::JobStatus;
    use hammerwork::archive::{ArchivalReason, ArchivalStats, ArchivedJob};
    use hammerwork::queue::DatabaseQueue;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: ArchiveCommand,
    }

    fn parse(args: &[&str]) -> ArchiveCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn run_has_the_documented_defaults() {
        match parse(&["run"]) {
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
                assert_eq!(queue_name, None);
                assert_eq!(
                    (
                        completed_after_days,
                        failed_after_days,
                        dead_after_days,
                        timed_out_after_days
                    ),
                    (7, 30, 30, 30)
                );
                assert_eq!((batch_size, compression_level), (1000, 6));
                assert!(compress, "compression is on by default");
                assert!(!dry_run);
                assert!(reason.is_none() && archived_by.is_none());
            }
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn run_accepts_every_flag_and_compression_can_be_switched_off() {
        match parse(&[
            "run",
            "-Q",
            "q",
            "--completed-after-days",
            "1",
            "--failed-after-days",
            "2",
            "--dead-after-days",
            "3",
            "--timed-out-after-days",
            "4",
            "--batch-size",
            "5",
            "--compression-level",
            "9",
            "--dry-run",
            "--reason",
            "manual",
            "--archived-by",
            "me",
            "--compress",
            "false",
        ]) {
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
                assert_eq!(queue_name.as_deref(), Some("q"));
                assert_eq!(
                    (
                        completed_after_days,
                        failed_after_days,
                        dead_after_days,
                        timed_out_after_days
                    ),
                    (1, 2, 3, 4)
                );
                assert_eq!((batch_size, compression_level), (5, 9));
                assert!(!compress, "--compress false must disable compression");
                assert!(dry_run);
                assert_eq!(reason.as_deref(), Some("manual"));
                assert_eq!(archived_by.as_deref(), Some("me"));
            }
            _ => panic!("expected Run"),
        }
        // bare --compress and --compress=true keep it on
        for args in [&["run", "--compress"][..], &["run", "--compress", "true"]] {
            assert!(matches!(
                parse(args),
                ArchiveCommand::Run { compress: true, .. }
            ));
        }
    }

    #[test]
    fn other_subcommands_parse_their_flags() {
        assert!(matches!(
            parse(&["restore", "abc"]),
            ArchiveCommand::Restore { job_id, .. } if job_id == "abc"
        ));
        match parse(&["list", "-Q", "q", "-l", "5", "-o", "10", "--format", "csv"]) {
            ArchiveCommand::List {
                queue_name,
                limit,
                offset,
                format,
                ..
            } => {
                assert_eq!(queue_name.as_deref(), Some("q"));
                assert_eq!((limit, offset), (5, 10));
                assert_eq!(format.as_deref(), Some("csv"));
            }
            _ => panic!("expected List"),
        }
        assert!(matches!(
            parse(&["list"]),
            ArchiveCommand::List {
                limit: 100,
                offset: 0,
                ..
            }
        ));
        assert!(matches!(
            parse(&["stats", "--format", "json"]),
            ArchiveCommand::Stats { format: Some(f), .. } if f == "json"
        ));
        assert!(matches!(
            parse(&["purge", "--older-than-days", "9", "--confirm", "--dry-run"]),
            ArchiveCommand::Purge {
                older_than_days: 9,
                confirm: true,
                dry_run: true,
                ..
            }
        ));
        assert!(TestCli::try_parse_from(["test", "purge"]).is_err());
    }

    #[test]
    fn policy_subcommands_were_removed() {
        // They only printed that policy storage does not exist.
        for sub in ["set-policy", "get-policy", "remove-policy"] {
            assert!(
                TestCli::try_parse_from(["test", sub, "-Q", "q"]).is_err(),
                "{sub}"
            );
        }
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        assert_eq!(
            parse(&["stats"]).get_database_url(&config).unwrap(),
            "postgres://config/db"
        );
        assert_eq!(
            parse(&["list", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
        let err = parse(&["restore", "x"])
            .get_database_url(&Config::default())
            .unwrap_err();
        assert!(err.to_string().contains("Database URL not provided"));
    }

    #[test]
    fn reasons_and_formats_are_validated() {
        assert!(matches!(
            parse_reason(None).unwrap(),
            ArchivalReason::Automatic
        ));
        assert!(matches!(
            parse_reason(Some("MANUAL")).unwrap(),
            ArchivalReason::Manual
        ));
        assert!(matches!(
            parse_reason(Some("compliance")).unwrap(),
            ArchivalReason::Compliance
        ));
        assert!(matches!(
            parse_reason(Some("maintenance")).unwrap(),
            ArchivalReason::Maintenance
        ));
        assert!(matches!(
            parse_reason(Some("automatic")).unwrap(),
            ArchivalReason::Automatic
        ));
        let err = parse_reason(Some("because")).unwrap_err().to_string();
        assert!(err.contains("Unknown archival reason 'because'"), "{err}");

        assert_eq!(check_format("json", &["table", "json"]).unwrap(), "json");
        let err = check_format("xml", &["table", "json"])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Unknown format 'xml'") && err.contains("table, json"),
            "{err}"
        );
    }

    fn sample_job(queue: &str, by: Option<&str>) -> ArchivedJob {
        let at = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        ArchivedJob {
            id: uuid::Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            queue_name: queue.to_string(),
            status: JobStatus::Completed,
            created_at: at,
            archived_at: at,
            archival_reason: ArchivalReason::Manual,
            original_payload_size: Some(10),
            payload_compressed: true,
            archived_by: by.map(String::from),
        }
    }

    #[test]
    fn archived_jobs_render_as_table_json_and_csv() {
        let jobs = vec![sample_job("emails", Some("cli")), sample_job("a,b", None)];

        let table = render_archived_jobs(&jobs, "table").unwrap();
        for expected in [
            "Job ID",
            "emails",
            "Completed",
            "Manual",
            "Yes",
            "cli",
            "2030-01-02 03:04:05",
        ] {
            assert!(table.contains(expected), "{expected} in {table}");
        }

        let json: serde_json::Value =
            serde_json::from_str(&render_archived_jobs(&jobs, "json").unwrap()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["queue_name"], "emails");
        assert_eq!(json[0]["payload_compressed"], true);

        let csv = render_archived_jobs(&jobs, "csv").unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("id,queue_name,status"));
        assert!(lines[1].contains(",emails,Completed,2030-01-02T03:04:05+00:00,"));
        assert!(lines[1].ends_with(",true,cli"));
        assert!(
            lines[2].contains(",\"a,b\","),
            "comma in queue name is quoted: {}",
            lines[2]
        );
        assert!(lines[2].ends_with(",true,"));

        assert_eq!(render_archived_jobs(&[], "csv").unwrap().lines().count(), 1);
    }

    #[test]
    fn stats_render_as_table_or_json() {
        let stats = ArchivalStats {
            jobs_archived: 7,
            jobs_purged: 2,
            bytes_archived: 1234,
            bytes_purged: 56,
            compression_ratio: 2.5,
            ..ArchivalStats::default()
        };
        let table = render_archival_stats(&stats, Some("emails"), "table").unwrap();
        for expected in [
            "Queue: emails",
            "Jobs archived: 7",
            "Jobs purged: 2",
            "Bytes archived: 1234",
            "Bytes purged: 56",
            "Compression ratio: 2.50",
        ] {
            assert!(table.contains(expected), "{expected} in {table}");
        }
        assert!(
            render_archival_stats(&stats, None, "table")
                .unwrap()
                .contains("Queue: ALL")
        );
        let json: serde_json::Value =
            serde_json::from_str(&render_archival_stats(&stats, None, "json").unwrap()).unwrap();
        assert_eq!(json["jobs_archived"], 7);
        assert_eq!(json["compression_ratio"], 2.5);
    }

    async fn archived_count(pool: &DatabasePool, queue: &str) -> i64 {
        let mut params = SqlParams::new(pool.backend());
        let sql = format!(
            "SELECT COUNT(*) AS count FROM hammerwork_jobs_archive WHERE queue_name = {}",
            params.text(queue)
        );
        fetch_i64(pool, &sql, params.binds(), "count")
            .await
            .unwrap()
    }

    async fn archive_lifecycle(base_url: String) {
        let db = ScratchDb::create(&base_url).await;
        let pool = &db.pool;
        let config = db.config();
        let run = |args: &[&str]| {
            let cmd = parse(args);
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let q = "arch";

        let mut done = SeedJob::new(q, "Completed");
        done.completed_now = true;
        done.payload = r#"{"data": "compress me compress me compress me"}"#;
        let old_done = seed(pool, &done).await;
        backdate(pool, &old_done, "completed_at", 10).await;
        let fresh_done = seed(pool, &done).await;
        let mut failed = SeedJob::new(q, "Failed");
        failed.failed_now = true;
        let old_failed = seed(pool, &failed).await;
        backdate(pool, &old_failed, "failed_at", 40).await;
        let pending = seed(pool, &SeedJob::new(q, "Pending")).await;
        let other = seed(pool, &SeedJob::new("arch_other", "Completed")).await;
        backdate(pool, &other, "completed_at", 10).await;

        // dry run and bad flags change nothing
        run(&["run", "-Q", q, "--dry-run"]).await.unwrap();
        for bad in [
            &["run", "-Q", q, "--compression-level", "10"][..],
            &["run", "-Q", q, "--batch-size", "0"],
            &["run", "-Q", q, "--reason", "whim"],
        ] {
            assert!(run(bad).await.is_err(), "{bad:?}");
        }
        assert_eq!(archived_count(pool, q).await, 0);
        assert_eq!(count_jobs(pool, q, None).await, 4);

        // real run: old finished jobs move to the archive, the rest stay; other queues untouched
        run(&[
            "run",
            "-Q",
            q,
            "--archived-by",
            "tester",
            "--reason",
            "manual",
        ])
        .await
        .unwrap();
        assert_eq!(archived_count(pool, q).await, 2);
        assert_eq!(count_jobs(pool, q, None).await, 2);
        assert!(job_column(pool, &old_done, "status").await.is_none());
        assert!(job_column(pool, &fresh_done, "status").await.is_some());
        assert!(job_column(pool, &pending, "status").await.is_some());
        assert!(job_column(pool, &other, "status").await.is_some());

        let library = pool.clone().create_job_queue();
        let listed = match &library {
            JobQueueWrapper::Postgres(l) => l
                .list_archived_jobs(Some(q), Some(10), Some(0))
                .await
                .unwrap(),
            JobQueueWrapper::MySQL(l) => l
                .list_archived_jobs(Some(q), Some(10), Some(0))
                .await
                .unwrap(),
        };
        assert_eq!(listed.len(), 2);
        assert!(
            listed
                .iter()
                .all(|j| j.archived_by.as_deref() == Some("tester"))
        );
        assert!(
            listed
                .iter()
                .all(|j| matches!(j.archival_reason, ArchivalReason::Manual))
        );
        let entry = listed
            .iter()
            .find(|j| j.id.to_string() == old_done)
            .unwrap();
        assert!(
            entry.payload_compressed,
            "a payload with content is compressed by default"
        );

        // compression off is honoured
        let mut more = SeedJob::new(q, "Completed");
        more.completed_now = true;
        let uncompressed = seed(pool, &more).await;
        backdate(pool, &uncompressed, "completed_at", 9).await;
        run(&["run", "-Q", q, "--compress", "false"]).await.unwrap();
        let listed = match &library {
            JobQueueWrapper::Postgres(l) => l
                .list_archived_jobs(Some(q), Some(10), Some(0))
                .await
                .unwrap(),
            JobQueueWrapper::MySQL(l) => l
                .list_archived_jobs(Some(q), Some(10), Some(0))
                .await
                .unwrap(),
        };
        let entry = listed
            .iter()
            .find(|j| j.id.to_string() == uncompressed)
            .unwrap();
        assert!(
            !entry.payload_compressed,
            "--compress false stores the payload as is"
        );

        // list / stats in every format
        for format in ["table", "json", "csv"] {
            run(&["list", "-Q", q, "--format", format]).await.unwrap();
        }
        run(&["list", "-l", "1", "-o", "1"]).await.unwrap();
        for format in ["table", "json"] {
            run(&["stats", "-Q", q, "--format", format]).await.unwrap();
        }
        run(&["stats"]).await.unwrap();
        assert!(run(&["list", "--format", "xml"]).await.is_err());
        assert!(run(&["stats", "--format", "csv"]).await.is_err());

        // restore brings a job back; unknown and malformed ids are errors
        run(&["restore", &old_done]).await.unwrap();
        assert!(job_column(pool, &old_done, "status").await.is_some());
        assert_eq!(archived_count(pool, q).await, 2);
        assert!(
            run(&["restore", &uuid::Uuid::new_v4().to_string()])
                .await
                .is_err()
        );
        let err = run(&["restore", "not-a-uuid"])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.to_lowercase().contains("uuid") || err.contains("invalid"),
            "{err}"
        );

        // purge: needs --confirm or --dry-run, only deletes archives older than the cutoff
        let err = run(&["purge", "--older-than-days", "5"])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("requires --confirm"), "{err}");
        run(&["purge", "--older-than-days", "5", "--dry-run"])
            .await
            .unwrap();
        run(&["purge", "--older-than-days", "5", "--confirm"])
            .await
            .unwrap();
        assert_eq!(
            archived_count(pool, q).await,
            2,
            "nothing is old enough yet"
        );
        let mut params = SqlParams::new(pool.backend());
        let ago = params.ago(30, IntervalUnit::Day);
        let sql = format!(
            "UPDATE hammerwork_jobs_archive SET archived_at = {ago} WHERE queue_name = {}",
            params.text(q)
        );
        execute_binds(pool, &sql, params.binds()).await.unwrap();
        run(&["purge", "--older-than-days", "5", "--confirm"])
            .await
            .unwrap();
        assert_eq!(archived_count(pool, q).await, 0);

        assert!(table_exists(pool).await);
        db.drop_db().await;
    }

    db_tests!(
        archive_lifecycle,
        test_archive_lifecycle_postgres,
        test_archive_lifecycle_mysql
    );
}
