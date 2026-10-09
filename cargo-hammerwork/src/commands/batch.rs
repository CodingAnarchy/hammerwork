use anyhow::{Result, anyhow};
use clap::Subcommand;
use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobStatus};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use tracing::info;

use crate::commands::backup::{CSV_HEADER, JOB_DATA_COLUMNS, JobData, csv_row, fetch_job_data};
use crate::commands::job::status_db_value;
use crate::config::Config;
use crate::utils::database::{DatabasePool, JobQueueWrapper};
use crate::utils::job_ops::{JobSelector, cancel_many, retry_many, select_job_ids};
use crate::utils::sql::{Backend, Bind, SqlParams};
use crate::utils::validation::{validate_priority, validate_status};

#[derive(Subcommand)]
pub enum BatchCommand {
    #[command(about = "Enqueue multiple jobs from a file or stdin")]
    Enqueue {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'f', long, help = "Input file path (JSON lines format)")]
        file: Option<String>,
        #[arg(
            short = 'n',
            short_alias = 'Q',
            long,
            help = "Queue for jobs whose line has no \"queue\" of its own (required)"
        )]
        queue: String,
        #[arg(short = 'r', long, help = "Default priority")]
        priority: Option<String>,
        #[arg(
            long,
            alias = "progress-every",
            help = "Print a progress line every N jobs (default 100); does not change how jobs are inserted"
        )]
        batch_size: Option<u32>,
        #[arg(long, help = "Continue on errors")]
        continue_on_error: bool,
    },
    #[command(about = "Retry multiple jobs by criteria")]
    Retry {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(short = 't', long, help = "Status filter (failed, dead)")]
        status: Option<String>,
        #[arg(long, help = "Hours since last failure")]
        failed_since_hours: Option<u32>,
        #[arg(long, help = "Only retry jobs that have used up all their attempts")]
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
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name filter")]
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
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name filter")]
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
        let pool = DatabasePool::connect_with_config(&db_url, config).await?;

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
                let options = ExportOptions {
                    queue: queue.as_deref(),
                    status: status.as_deref(),
                    format: format.as_deref().unwrap_or("json"),
                    include_payload: *include_payload,
                    limit: *limit,
                };
                let count = batch_export(&pool, output, &options).await?;
                println!("📤 Exported {} jobs to {}", count, output);
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

/// Totals of a `batch enqueue` run.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EnqueueSummary {
    pub processed: u32,
    pub errors: u32,
}

/// Build the job for one JSON-lines record: `{"queue": "...", "payload": ..., "priority": "..."}`.
/// `queue` and `priority` fall back to the command's defaults; `payload` is required.
pub fn parse_job_line(
    line: &str,
    default_queue: &str,
    default_priority: Option<&str>,
) -> Result<Job> {
    let record: Value = serde_json::from_str(line).map_err(|e| anyhow!("Invalid JSON - {}", e))?;
    let record = record
        .as_object()
        .ok_or_else(|| anyhow!("each line must be a JSON object"))?;

    let queue = match record.get("queue") {
        None | Some(Value::Null) => default_queue,
        Some(Value::String(q)) if !q.is_empty() => q,
        Some(other) => return Err(anyhow!("'queue' must be a non-empty string, got {}", other)),
    };
    let payload = record
        .get("payload")
        .ok_or_else(|| anyhow!("missing 'payload'"))?
        .clone();
    let priority = match record.get("priority") {
        None | Some(Value::Null) => default_priority,
        Some(Value::String(p)) => Some(p.as_str()),
        Some(other) => return Err(anyhow!("'priority' must be a string, got {}", other)),
    };

    let mut job = Job::new(queue.to_string(), payload);
    if let Some(priority) = priority {
        job = job.with_priority(validate_priority(priority)?);
    }
    Ok(job)
}

/// Enqueue every record of `reader` (see [`parse_job_line`]). A bad record stops the run unless
/// `continue_on_error`, and is reported on stderr either way.
pub async fn enqueue_lines(
    pool: DatabasePool,
    reader: impl BufRead,
    default_queue: &str,
    default_priority: Option<&str>,
    progress_every: u32,
    continue_on_error: bool,
) -> Result<EnqueueSummary> {
    let queue = pool.create_job_queue();
    let mut summary = EnqueueSummary::default();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let number = line_num + 1;

        let outcome = match parse_job_line(&line, default_queue, default_priority) {
            Ok(job) => match &queue {
                JobQueueWrapper::Postgres(q) => q.enqueue(job).await.map(|_| ()),
                JobQueueWrapper::MySQL(q) => q.enqueue(job).await.map(|_| ()),
            }
            .map_err(|e| anyhow!("enqueue failed - {}", e)),
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => {
                summary.processed += 1;
                if progress_every > 0 && summary.processed % progress_every == 0 {
                    println!("✅ Processed {} jobs", summary.processed);
                }
            }
            Err(e) => {
                summary.errors += 1;
                eprintln!("❌ Line {}: {}", number, e);
                if !continue_on_error {
                    return Err(anyhow!("Batch enqueue failed at line {}: {}", number, e));
                }
            }
        }
    }
    Ok(summary)
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
    if batch_size == 0 {
        return Err(anyhow!("--batch-size must be at least 1"));
    }

    let reader: Box<dyn BufRead> = if let Some(file_path) = file {
        Box::new(BufReader::new(File::open(&file_path).map_err(|e| {
            anyhow!("Cannot open input file {}: {}", file_path, e)
        })?))
    } else {
        println!("📥 Reading from stdin (provide JSON lines format)...");
        Box::new(BufReader::new(std::io::stdin()))
    };

    let summary = enqueue_lines(
        pool,
        reader,
        default_queue,
        default_priority.as_deref(),
        batch_size,
        continue_on_error,
    )
    .await?;

    println!("📊 Batch Enqueue Summary");
    println!("════════════════════════");
    println!("Total jobs processed: {}", summary.processed);
    if summary.errors > 0 {
        println!("Total errors: {}", summary.errors);
    }

    info!(
        "Batch enqueue completed: {} processed, {} errors",
        summary.processed, summary.errors
    );
    Ok(())
}

/// What `batch export` writes.
#[derive(Debug, Clone, Copy)]
pub struct ExportOptions<'a> {
    pub queue: Option<&'a str>,
    /// A status name accepted by `job list --status` (`pending`, `timed_out`, ...).
    pub status: Option<&'a str>,
    /// `json` (one array), `jsonl` (one object per line) or `csv`.
    pub format: &'a str,
    pub include_payload: bool,
    pub limit: Option<u32>,
}

/// The `SELECT` behind `batch export`. Queue, status and limit are bound.
pub fn build_export_query(
    backend: Backend,
    queue: Option<&str>,
    status: Option<&str>,
    limit: Option<u32>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let mut conditions = Vec::new();
    if let Some(queue) = queue {
        conditions.push(format!("queue_name = {}", params.text(queue)));
    }
    if let Some(status) = status {
        conditions.push(format!("status = {}", params.text(status_db_value(status))));
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let limit_clause = limit
        .map(|l| format!(" {}", params.limit(l)))
        .unwrap_or_default();
    (
        format!(
            "SELECT {JOB_DATA_COLUMNS} FROM hammerwork_jobs{where_clause} ORDER BY created_at ASC, id ASC{limit_clause}"
        ),
        params.into_binds(),
    )
}

/// The export as text in `options.format`, without the payload unless asked for.
pub(crate) fn render_export(jobs: &[JobData], options: &ExportOptions<'_>) -> Result<String> {
    let json_of = |job: &JobData| -> Result<Value> {
        let mut value = serde_json::to_value(job)?;
        if !options.include_payload {
            value.as_object_mut().map(|o| o.remove("payload"));
        }
        Ok(value)
    };
    Ok(match options.format {
        "json" => {
            let all: Result<Vec<Value>> = jobs.iter().map(json_of).collect();
            serde_json::to_string_pretty(&all?)?
        }
        "jsonl" => {
            let lines: Result<Vec<String>> = jobs
                .iter()
                .map(|j| Ok(serde_json::to_string(&json_of(j)?)?))
                .collect();
            lines?.join("\n")
        }
        "csv" => {
            let mut lines = Vec::new();
            if options.include_payload {
                lines.push(CSV_HEADER.to_string());
                lines.extend(jobs.iter().map(csv_row));
            } else {
                // Same columns without `payload` (the third one).
                let drop_payload = |fields: Vec<String>| {
                    fields
                        .into_iter()
                        .enumerate()
                        .filter(|(i, _)| *i != 2)
                        .map(|(_, f)| f)
                        .collect::<Vec<_>>()
                        .join(",")
                };
                lines.push(drop_payload(
                    CSV_HEADER.split(',').map(String::from).collect(),
                ));
                lines.extend(jobs.iter().map(|job| {
                    let mut no_payload = JobData {
                        id: job.id.clone(),
                        queue_name: job.queue_name.clone(),
                        payload: Value::Null,
                        status: job.status.clone(),
                        priority: job.priority.clone(),
                        attempts: job.attempts,
                        max_attempts: job.max_attempts,
                        created_at: job.created_at,
                        scheduled_at: job.scheduled_at,
                        started_at: job.started_at,
                        completed_at: job.completed_at,
                        failed_at: job.failed_at,
                        error_message: job.error_message.clone(),
                    };
                    no_payload.payload = Value::Null;
                    // Re-split the row on its unquoted commas to drop the payload cell.
                    let row = csv_row(&no_payload);
                    drop_payload(split_csv_row(&row))
                }));
            }
            lines.join("\n")
        }
        other => {
            return Err(anyhow!(
                "Unknown export format '{}'. Valid options: csv, json, jsonl",
                other
            ));
        }
    })
}

/// Split one CSV record into fields, keeping quoted fields (with their quotes) intact.
fn split_csv_row(row: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in row.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    fields.push(current);
    fields
}

/// Write the matching jobs to `output`; returns how many were exported.
pub async fn batch_export(
    pool: &DatabasePool,
    output: &str,
    options: &ExportOptions<'_>,
) -> Result<usize> {
    if !["csv", "json", "jsonl"].contains(&options.format) {
        return Err(anyhow!(
            "Unknown export format '{}'. Valid options: csv, json, jsonl",
            options.format
        ));
    }
    if let Some(status) = options.status {
        validate_status(status)?;
    }

    let (query, binds) =
        build_export_query(pool.backend(), options.queue, options.status, options.limit);
    let jobs = fetch_job_data(pool, &query, &binds).await?;
    let text = render_export(&jobs, options)?;

    let mut writer = BufWriter::new(
        File::create(output).map_err(|e| anyhow!("Cannot create {}: {}", output, e))?,
    );
    writeln!(writer, "{}", text)?;
    writer.flush()?;
    Ok(jobs.len())
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

    let failed_after = failed_since_hours
        .map(|hours| chrono::Utc::now() - chrono::Duration::hours(i64::from(hours)));

    Ok(JobSelector {
        statuses,
        queue: queue.clone(),
        failed_after,
        attempts_exhausted: max_attempts_reached,
        ..Default::default()
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

    let created_before = older_than_hours
        .map(|hours| chrono::Utc::now() - chrono::Duration::hours(i64::from(hours)));

    Ok(JobSelector {
        statuses,
        queue: queue.clone(),
        created_before,
        ..Default::default()
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
    use crate::utils::test_support::*;
    use clap::Parser;
    use hammerwork::JobPriority;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: BatchCommand,
    }

    fn parse(args: &[&str]) -> BatchCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&[
            "enqueue",
            "-n",
            "q",
            "-f",
            "jobs.jsonl",
            "-r",
            "high",
            "--batch-size",
            "5",
            "--continue-on-error",
        ]) {
            BatchCommand::Enqueue {
                file,
                queue,
                priority,
                batch_size,
                continue_on_error,
                ..
            } => {
                assert_eq!(file.as_deref(), Some("jobs.jsonl"));
                assert_eq!(queue, "q");
                assert_eq!(priority.as_deref(), Some("high"));
                assert_eq!(batch_size, Some(5));
                assert!(continue_on_error);
            }
            _ => panic!("expected Enqueue"),
        }
        match parse(&[
            "retry",
            "-n",
            "q",
            "-t",
            "dead",
            "--failed-since-hours",
            "3",
            "--max-attempts-reached",
            "--confirm",
            "--dry-run",
        ]) {
            BatchCommand::Retry {
                queue,
                status,
                failed_since_hours,
                max_attempts_reached,
                confirm,
                dry_run,
                ..
            } => {
                assert_eq!(queue.as_deref(), Some("q"));
                assert_eq!(status.as_deref(), Some("dead"));
                assert_eq!(failed_since_hours, Some(3));
                assert!(max_attempts_reached && confirm && dry_run);
            }
            _ => panic!("expected Retry"),
        }
        match parse(&[
            "cancel",
            "-t",
            "pending",
            "--older-than-hours",
            "2",
            "--confirm",
        ]) {
            BatchCommand::Cancel {
                status,
                older_than_hours,
                confirm,
                dry_run,
                ..
            } => {
                assert_eq!(status.as_deref(), Some("pending"));
                assert_eq!(older_than_hours, Some(2));
                assert!(confirm && !dry_run);
            }
            _ => panic!("expected Cancel"),
        }
        match parse(&[
            "export",
            "-o",
            "out",
            "-n",
            "q",
            "-t",
            "failed",
            "--format",
            "csv",
            "--include-payload",
            "--limit",
            "9",
        ]) {
            BatchCommand::Export {
                output,
                queue,
                status,
                format,
                include_payload,
                limit,
                ..
            } => {
                assert_eq!(output, "out");
                assert_eq!(queue.as_deref(), Some("q"));
                assert_eq!(status.as_deref(), Some("failed"));
                assert_eq!(format.as_deref(), Some("csv"));
                assert!(include_payload);
                assert_eq!(limit, Some(9));
            }
            _ => panic!("expected Export"),
        }
        assert!(
            TestCli::try_parse_from(["test", "enqueue"]).is_err(),
            "--queue is required"
        );
        assert!(
            TestCli::try_parse_from(["test", "export"]).is_err(),
            "--output is required"
        );
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        assert_eq!(
            parse(&["retry"]).get_database_url(&config).unwrap(),
            "postgres://config/db"
        );
        assert_eq!(
            parse(&["cancel", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
        assert!(
            parse(&["retry"])
                .get_database_url(&Config::default())
                .is_err()
        );
    }

    #[test]
    fn retry_selector_uses_capitalized_statuses() {
        let selector = retry_selector(&Some("emails".into()), Some("dead"), Some(2), true).unwrap();
        assert_eq!(selector.statuses, vec![JobStatus::Dead]);
        assert_eq!(selector.queue.as_deref(), Some("emails"));
        assert!(selector.failed_after.is_some());
        assert!(selector.attempts_exhausted);

        let failed = retry_selector(&None, Some("failed"), None, false).unwrap();
        assert_eq!(failed.statuses, vec![JobStatus::Failed]);
        let all = retry_selector(&None, None, None, false).unwrap();
        assert_eq!(all.statuses, vec![JobStatus::Failed, JobStatus::Dead]);
        assert!(retry_selector(&None, Some("bogus"), None, false).is_err());
    }

    #[test]
    fn cancel_selector_validates_status() {
        let sel = cancel_selector(&None, None, None).unwrap();
        assert_eq!(sel.statuses, vec![JobStatus::Pending, JobStatus::Running]);
        let sel = cancel_selector(&Some("q".into()), Some("running"), Some(1)).unwrap();
        assert_eq!(sel.statuses, vec![JobStatus::Running]);
        assert!(sel.created_before.is_some() && sel.queue.as_deref() == Some("q"));
        assert_eq!(
            cancel_selector(&None, Some("pending"), None)
                .unwrap()
                .statuses,
            vec![JobStatus::Pending]
        );
        assert!(cancel_selector(&None, Some("failed"), None).is_err());
    }

    #[test]
    fn job_lines_use_defaults_and_validate_every_field() {
        let job = parse_job_line(r#"{"payload": {"n": 1}}"#, "default_q", None).unwrap();
        assert_eq!(job.queue_name, "default_q");
        assert_eq!(job.payload["n"], 1);
        assert_eq!(job.priority, JobPriority::Normal);

        let job = parse_job_line(r#"{"payload": 5}"#, "q", Some("low")).unwrap();
        assert_eq!(job.priority, JobPriority::Low);

        let job = parse_job_line(
            r#"{"queue": "own", "payload": [], "priority": "critical"}"#,
            "q",
            Some("low"),
        )
        .unwrap();
        assert_eq!(
            (job.queue_name.as_str(), job.priority),
            ("own", JobPriority::Critical)
        );

        for (line, expected) in [
            ("not json", "Invalid JSON"),
            ("[1, 2]", "must be a JSON object"),
            (r#"{"queue": "q"}"#, "missing 'payload'"),
            (r#"{"payload": {}, "queue": ""}"#, "non-empty string"),
            (r#"{"payload": {}, "queue": 5}"#, "non-empty string"),
            (r#"{"payload": {}, "priority": 2}"#, "must be a string"),
            (
                r#"{"payload": {}, "priority": "urgent"}"#,
                "Invalid priority",
            ),
        ] {
            let err = parse_job_line(line, "q", None).unwrap_err().to_string();
            assert!(err.contains(expected), "{line}: {err}");
        }
        assert!(parse_job_line(r#"{"payload": {}}"#, "q", Some("urgent")).is_err());
    }

    #[test]
    fn export_query_binds_filters_and_limit() {
        let (sql, binds) = build_export_query(
            Backend::Postgres,
            Some(HOSTILE_QUEUE),
            Some("timed_out"),
            Some(5),
        );
        assert!(
            sql.ends_with(
                "WHERE queue_name = $1 AND status = $2 ORDER BY created_at ASC, id ASC LIMIT $3"
            ),
            "{sql}"
        );
        assert_eq!(
            binds,
            vec![
                Bind::Text(HOSTILE_QUEUE.into()),
                Bind::Text("TimedOut".into()),
                Bind::Int(5)
            ]
        );
        let (sql, binds) = build_export_query(Backend::MySql, None, None, None);
        assert!(!sql.contains("WHERE") && !sql.contains("LIMIT") && binds.is_empty());
    }

    fn job_data(id: &str, payload: Value) -> JobData {
        let at = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        JobData {
            id: id.into(),
            queue_name: "emails".into(),
            payload,
            status: "Failed".into(),
            priority: "high".into(),
            attempts: 2,
            max_attempts: 3,
            created_at: at,
            scheduled_at: at,
            started_at: None,
            completed_at: None,
            failed_at: Some(at),
            error_message: Some("bad, \"worse\"".into()),
        }
    }

    #[test]
    fn export_renders_json_jsonl_and_csv_with_or_without_payload() {
        let jobs = vec![
            job_data("id-1", serde_json::json!({"to": "a@example.com"})),
            job_data("id-2", serde_json::json!({"to": "b,c"})),
        ];
        let opts = |format, include_payload| ExportOptions {
            queue: None,
            status: None,
            format,
            include_payload,
            limit: None,
        };

        let json: Value =
            serde_json::from_str(&render_export(&jobs, &opts("json", true)).unwrap()).unwrap();
        assert_eq!(json.as_array().unwrap().len(), 2);
        assert_eq!(json[0]["payload"]["to"], "a@example.com");
        assert_eq!(json[1]["queue_name"], "emails");
        let json: Value =
            serde_json::from_str(&render_export(&jobs, &opts("json", false)).unwrap()).unwrap();
        assert!(
            json[0].get("payload").is_none(),
            "payload is left out by default"
        );
        assert_eq!(json[0]["error_message"], "bad, \"worse\"");

        let jsonl = render_export(&jobs, &opts("jsonl", true)).unwrap();
        let lines: Vec<Value> = jsonl
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["id"], "id-2");

        let csv = render_export(&jobs, &opts("csv", true)).unwrap();
        assert!(csv.starts_with(CSV_HEADER));
        assert_eq!(csv.lines().count(), 3);
        assert!(csv.contains("\"{\"\"to\"\":\"\"b,c\"\"}\""), "{csv}");

        let csv = render_export(&jobs, &opts("csv", false)).unwrap();
        let mut lines = csv.lines();
        let header = lines.next().unwrap();
        assert!(!header.contains("payload") && header.starts_with("id,queue_name,status"));
        let row = lines.next().unwrap();
        assert!(row.starts_with("id-1,emails,Failed,high,2,3,"), "{row}");
        assert!(row.ends_with(",\"bad, \"\"worse\"\"\""), "{row}");
        assert!(!row.contains("a@example.com"));
        assert_eq!(split_csv_row(row).len(), 12);

        let err = render_export(&jobs, &opts("xml", true))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Unknown export format 'xml'"), "{err}");
    }

    #[test]
    fn csv_rows_split_on_unquoted_commas_only() {
        assert_eq!(split_csv_row("a,\"b,c\",,d"), vec!["a", "\"b,c\"", "", "d"]);
        assert_eq!(split_csv_row(""), vec![""]);
    }

    async fn batch_commands(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let queue = hostile_queue();
        let other = unique_queue("batch_other");
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name).to_str().unwrap().to_string();
        let run = |args: Vec<String>| {
            let cmd = parse(&args.iter().map(String::as_str).collect::<Vec<_>>());
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        fn s(v: &str) -> String {
            v.to_string()
        }

        // --- enqueue: from a JSON-lines file, with per-line overrides and blank lines
        let input = path("jobs.jsonl");
        std::fs::write(
            &input,
            format!(
                "{}\n\n{}\n{}\n",
                r#"{"payload": {"n": 1}}"#,
                r#"{"payload": {"n": 2}, "priority": "critical"}"#,
                serde_json::json!({"queue": other, "payload": {"n": 3}})
            ),
        )
        .unwrap();
        run(vec![
            s("enqueue"),
            s("-n"),
            queue.clone(),
            s("-f"),
            input.clone(),
            s("-r"),
            s("low"),
            s("--batch-size"),
            s("2"),
        ])
        .await
        .unwrap();
        assert_eq!(count_jobs(&pool, &queue, Some("Pending")).await, 2);
        assert_eq!(count_jobs(&pool, &other, Some("Pending")).await, 1);
        let priorities = column_strings(
            &pool,
            &format!(
                "SELECT CAST(priority AS CHAR) AS p FROM hammerwork_jobs WHERE queue_name = {} ORDER BY priority",
                if pool.backend() == Backend::Postgres { "$1" } else { "?" }
            ),
            &[Bind::Text(queue.clone())],
            "p",
        )
        .await;
        assert_eq!(
            priorities,
            vec!["1", "4"],
            "default low=1 and per-line critical=4"
        );

        // a bad line aborts the run unless --continue-on-error
        let bad = path("bad.jsonl");
        std::fs::write(
            &bad,
            "{\"payload\": 1}\nnot json\n{\"payload\": 2}\n{\"queue\": \"x\"}\n",
        )
        .unwrap();
        let err = run(vec![
            s("enqueue"),
            s("-n"),
            queue.clone(),
            s("-f"),
            bad.clone(),
        ])
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("line 2"), "{err}");
        assert_eq!(
            count_jobs(&pool, &queue, None).await,
            3,
            "the first line was enqueued before the failure"
        );
        run(vec![
            s("enqueue"),
            s("-n"),
            queue.clone(),
            s("-f"),
            bad.clone(),
            s("--continue-on-error"),
        ])
        .await
        .unwrap();
        assert_eq!(count_jobs(&pool, &queue, None).await, 5);

        // input problems
        assert!(
            run(vec![
                s("enqueue"),
                s("-n"),
                queue.clone(),
                s("-f"),
                path("missing.jsonl")
            ])
            .await
            .is_err()
        );
        let err = run(vec![
            s("enqueue"),
            s("-n"),
            queue.clone(),
            s("-f"),
            input.clone(),
            s("--batch-size"),
            s("0"),
        ])
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("--batch-size must be at least 1"), "{err}");

        // --- retry: failed/dead jobs of this queue only
        let mut failed = SeedJob::new(&queue, "Failed");
        failed.failed_now = true;
        let failed_id = seed(&pool, &failed).await;
        let dead_id = seed(&pool, &SeedJob::new(&queue, "Dead")).await;
        let foreign_failed = seed(&pool, &SeedJob::new(&other, "Failed")).await;
        let retry = |extra: &[&str]| {
            let mut args = vec![s("retry"), s("-n"), queue.clone()];
            args.extend(extra.iter().map(|e| s(e)));
            args
        };
        run(retry(&[])).await.unwrap(); // needs --confirm
        run(retry(&["--dry-run"])).await.unwrap();
        assert_eq!(job_status(&pool, &failed_id).await, "Failed");
        assert!(run(retry(&["--dry-run", "-t", "weird"])).await.is_err());
        run(retry(&["--confirm", "-t", "failed"])).await.unwrap();
        assert_eq!(job_status(&pool, &failed_id).await, "Pending");
        assert_eq!(job_status(&pool, &dead_id).await, "Dead");
        run(retry(&["--confirm"])).await.unwrap();
        assert_eq!(job_status(&pool, &dead_id).await, "Pending");
        assert_eq!(
            job_status(&pool, &foreign_failed).await,
            "Failed",
            "other queues are untouched"
        );
        run(retry(&["--confirm"])).await.unwrap(); // nothing left to retry

        // --- cancel: removes matching jobs
        let running = {
            let mut job = SeedJob::new(&queue, "Running");
            job.started_long_ago = true;
            seed(&pool, &job).await
        };
        let before = count_jobs(&pool, &queue, None).await;
        let cancel = |extra: &[&str]| {
            let mut args = vec![s("cancel"), s("-n"), queue.clone()];
            args.extend(extra.iter().map(|e| s(e)));
            args
        };
        run(cancel(&[])).await.unwrap(); // needs --confirm
        run(cancel(&["--dry-run"])).await.unwrap();
        assert_eq!(count_jobs(&pool, &queue, None).await, before);
        assert!(run(cancel(&["--dry-run", "-t", "failed"])).await.is_err());
        run(cancel(&["--confirm", "-t", "running"])).await.unwrap();
        assert!(job_column(&pool, &running, "status").await.is_none());
        run(cancel(&[
            "--confirm",
            "-t",
            "pending",
            "--older-than-hours",
            "1",
        ]))
        .await
        .unwrap();
        assert_eq!(
            count_jobs(&pool, &queue, Some("Pending")).await,
            before as i64 - 1,
            "nothing is older than an hour"
        );
        run(cancel(&["--confirm"])).await.unwrap();
        assert_eq!(count_jobs(&pool, &queue, None).await, 0);
        run(cancel(&["--confirm"])).await.unwrap(); // nothing left

        // --- export
        let mut done = SeedJob::new(&other, "Completed");
        done.payload = r#"{"report": "weekly"}"#;
        let done_id = seed(&pool, &done).await;
        let export = |extra: &[&str]| {
            let mut args = vec![s("export"), s("-n"), other.clone()];
            args.extend(extra.iter().map(|e| s(e)));
            args
        };
        let out = path("export.json");
        run(export(&[
            "-o",
            &out,
            "-t",
            "completed",
            "--include-payload",
        ]))
        .await
        .unwrap();
        let exported: Value =
            serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
        assert_eq!(exported.as_array().unwrap().len(), 1);
        assert_eq!(exported[0]["id"], done_id.as_str());
        assert_eq!(exported[0]["payload"]["report"], "weekly");
        let out = path("export.jsonl");
        run(export(&["-o", &out, "--format", "jsonl", "--limit", "2"]))
            .await
            .unwrap();
        let lines = std::fs::read_to_string(&out).unwrap();
        assert_eq!(lines.lines().count(), 2, "limit applies");
        assert!(!lines.contains("weekly"), "payload excluded by default");
        let out = path("export.csv");
        run(export(&["-o", &out, "--format", "csv", "-t", "pending"]))
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), 2);
        assert!(run(export(&["-o", &out, "--format", "xml"])).await.is_err());
        assert!(run(export(&["-o", &out, "-t", "bogus"])).await.is_err());
        assert!(
            run(export(&[
                "-o",
                dir.path().join("no/such/dir/x").to_str().unwrap()
            ]))
            .await
            .is_err()
        );

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&queue, &other]).await;
    }

    db_tests!(
        batch_commands,
        test_batch_commands_postgres,
        test_batch_commands_mysql
    );
}
