use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, Utc};
use clap::Subcommand;
use hammerwork::cron::CronSchedule;
use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobId};
use serde_json::Value;
use sqlx::Row;
use tracing::info;

use crate::config::Config;
use crate::utils::database::{DatabasePool, JobQueueWrapper};
use crate::utils::sql::{Backend, Bind, SqlParams, bind_mysql, bind_pg, execute_binds};
use crate::utils::validation::{validate_json_payload, validate_priority};

#[derive(Subcommand)]
pub enum CronCommand {
    #[command(about = "List scheduled/recurring jobs")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(long, help = "Show only active cron jobs")]
        active_only: bool,
        #[arg(long, help = "Show priority, status and payload as well")]
        detailed: bool,
    },
    #[command(about = "Create a new cron-scheduled job")]
    Create {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name")]
        queue: String,
        #[arg(short = 'j', long, help = "Job payload as JSON")]
        payload: String,
        #[arg(
            short = 's',
            long,
            help = "Cron expression with a leading seconds field (e.g. \"0 0 9 * * MON-FRI\")"
        )]
        schedule: String,
        #[arg(short = 'z', long, help = "Timezone (e.g., UTC, America/New_York)")]
        timezone: Option<String>,
        #[arg(short = 'r', long, help = "Job priority")]
        priority: Option<String>,
    },
    #[command(about = "Enable cron job execution")]
    Enable {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
    },
    #[command(about = "Disable cron job execution")]
    Disable {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
    },
    #[command(about = "Show next execution times for cron jobs")]
    Next {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(
            short = 'c',
            long,
            default_value = "10",
            help = "Number of upcoming executions to show"
        )]
        count: u32,
        #[arg(long, help = "Show next N hours of executions")]
        hours: Option<u32>,
    },
    #[command(about = "Update cron job schedule")]
    Update {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
        #[arg(short = 's', long, help = "New cron schedule expression")]
        schedule: Option<String>,
        #[arg(short = 'z', long, help = "New timezone")]
        timezone: Option<String>,
        #[arg(short = 'r', long, help = "New priority")]
        priority: Option<String>,
    },
    #[command(about = "Delete a cron job")]
    Delete {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(help = "Job ID")]
        job_id: String,
        #[arg(long, help = "Confirm the deletion")]
        confirm: bool,
    },
}

impl CronCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect_with_config(&db_url, config).await?;

        match self {
            CronCommand::List {
                queue,
                active_only,
                detailed,
                ..
            } => {
                list_cron_jobs(&pool, queue.as_deref(), *active_only, *detailed).await?;
            }
            CronCommand::Create {
                queue,
                payload,
                schedule,
                timezone,
                priority,
                ..
            } => {
                create_cron_job(
                    pool,
                    queue,
                    payload,
                    schedule,
                    timezone.as_deref(),
                    priority.as_deref(),
                )
                .await?;
            }
            CronCommand::Enable { job_id, .. } => {
                toggle_cron_job(&pool, job_id, true).await?;
            }
            CronCommand::Disable { job_id, .. } => {
                toggle_cron_job(&pool, job_id, false).await?;
            }
            CronCommand::Next {
                queue,
                count,
                hours,
                ..
            } => {
                show_next_executions(&pool, queue.as_deref(), *count, *hours).await?;
            }
            CronCommand::Update {
                job_id,
                schedule,
                timezone,
                priority,
                ..
            } => {
                update_cron_job(
                    &pool,
                    job_id,
                    schedule.as_deref(),
                    timezone.as_deref(),
                    priority.as_deref(),
                )
                .await?;
            }
            CronCommand::Delete {
                job_id, confirm, ..
            } => {
                delete_cron_job(&pool, job_id, *confirm).await?;
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url = match self {
            CronCommand::List { database_url, .. } => database_url,
            CronCommand::Create { database_url, .. } => database_url,
            CronCommand::Enable { database_url, .. } => database_url,
            CronCommand::Disable { database_url, .. } => database_url,
            CronCommand::Next { database_url, .. } => database_url,
            CronCommand::Update { database_url, .. } => database_url,
            CronCommand::Delete { database_url, .. } => database_url,
        };

        url.as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .ok_or_else(|| anyhow!("Database URL is required"))
            .map(|s| s.to_string())
    }
}

/// A cron job as stored in `hammerwork_jobs`.
#[derive(Debug, Clone, PartialEq)]
pub struct CronJobRow {
    pub id: String,
    pub queue: String,
    pub schedule: String,
    pub timezone: String,
    pub status: String,
    pub priority: i32,
    /// `recurring`: the job is rescheduled after each run. `cron disable` clears it.
    pub active: bool,
    pub next_run_at: Option<DateTime<Utc>>,
    pub payload: Value,
}

/// Select cron jobs, optionally restricted to one queue, to active ones and to one id.
/// Every filter value is bound; the id is cast to `uuid` on PostgreSQL.
pub fn build_cron_select_query(
    backend: Backend,
    queue: Option<&str>,
    active_only: bool,
    job_id: Option<&str>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let id_expr = match backend {
        Backend::Postgres => "CAST(id AS TEXT)",
        Backend::MySql => "id",
    };
    let mut query = format!(
        "SELECT {id_expr} AS id, queue_name, cron_schedule, timezone, status, priority, recurring, \
         next_run_at, payload FROM hammerwork_jobs WHERE cron_schedule IS NOT NULL"
    );
    if let Some(queue_name) = queue {
        query.push_str(&format!(" AND queue_name = {}", params.text(queue_name)));
    }
    if active_only {
        query.push_str(" AND recurring = true");
    }
    if let Some(id) = job_id {
        query.push_str(&format!(" AND id = {}", params.uuid(id)));
    }
    query.push_str(" ORDER BY queue_name, next_run_at, id");
    (query, params.into_binds())
}

macro_rules! cron_row {
    ($row:expr) => {{
        let row = $row;
        CronJobRow {
            id: row.try_get("id")?,
            queue: row.try_get("queue_name")?,
            schedule: row.try_get("cron_schedule")?,
            timezone: row
                .try_get::<Option<String>, _>("timezone")?
                .unwrap_or_else(|| "UTC".to_string()),
            status: row.try_get("status")?,
            priority: row.try_get("priority")?,
            active: row.try_get("recurring")?,
            next_run_at: row.try_get("next_run_at")?,
            payload: row.try_get("payload")?,
        }
    }};
}

/// Fetch cron jobs (see [`build_cron_select_query`]).
pub async fn fetch_cron_jobs(
    pool: &DatabasePool,
    queue: Option<&str>,
    active_only: bool,
    job_id: Option<&str>,
) -> Result<Vec<CronJobRow>> {
    let (sql, binds) = build_cron_select_query(pool.backend(), queue, active_only, job_id);
    let mut out = Vec::new();
    match pool {
        DatabasePool::Postgres(pg) => {
            for row in bind_pg(sqlx::query(&sql), &binds).fetch_all(pg).await? {
                out.push(cron_row!(&row));
            }
        }
        DatabasePool::MySQL(my) => {
            for row in bind_mysql(sqlx::query(&sql), &binds).fetch_all(my).await? {
                out.push(cron_row!(&row));
            }
        }
    }
    Ok(out)
}

fn parse_job_id(job_id: &str) -> Result<JobId> {
    JobId::parse_str(job_id).map_err(|e| anyhow!("Invalid job ID '{}': {}", job_id, e))
}

fn fmt_time(time: Option<DateTime<Utc>>) -> String {
    time.map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn render_cron_jobs(jobs: &[CronJobRow], detailed: bool) -> String {
    let mut table = comfy_table::Table::new();
    let mut header = vec!["ID", "Queue", "Schedule", "Timezone", "Active", "Next Run"];
    if detailed {
        header.extend(["Status", "Priority", "Payload"]);
    }
    table.set_header(header);
    for job in jobs {
        let mut row = vec![
            job.id.clone(),
            job.queue.clone(),
            job.schedule.clone(),
            job.timezone.clone(),
            if job.active { "yes" } else { "no" }.to_string(),
            fmt_time(job.next_run_at),
        ];
        if detailed {
            row.push(job.status.clone());
            row.push(crate::commands::job::priority_display(job.priority));
            row.push(job.payload.to_string());
        }
        table.add_row(row);
    }
    table.to_string()
}

async fn list_cron_jobs(
    pool: &DatabasePool,
    queue: Option<&str>,
    active_only: bool,
    detailed: bool,
) -> Result<()> {
    println!("📅 Cron Jobs");
    println!("═══════════");

    let jobs = fetch_cron_jobs(pool, queue, active_only, None).await?;
    if jobs.is_empty() {
        println!("📅 No cron jobs found");
        if let Some(q) = queue {
            println!("   Queue filter: {}", q);
        }
        if active_only {
            println!("   Filter: Active only");
        }
    } else {
        println!("Found {} cron jobs", jobs.len());
        println!("{}", render_cron_jobs(&jobs, detailed));
    }
    Ok(())
}

/// Validate the arguments of `cron create` and build the job to enqueue.
pub fn build_cron_job(
    queue: &str,
    payload: &str,
    schedule: &str,
    timezone: Option<&str>,
    priority: Option<&str>,
) -> Result<Job> {
    let payload = validate_json_payload(payload)?;
    let cron = CronSchedule::with_timezone(schedule, timezone.unwrap_or("UTC"))?;
    let mut job = Job::new(queue.to_string(), payload).with_cron(cron)?;
    if let Some(priority) = priority {
        job = job.with_priority(validate_priority(priority)?);
    }
    Ok(job)
}

async fn create_cron_job(
    pool: DatabasePool,
    queue: &str,
    payload: &str,
    schedule: &str,
    timezone: Option<&str>,
    priority: Option<&str>,
) -> Result<()> {
    let job = build_cron_job(queue, payload, schedule, timezone, priority)?;
    let next_run = job.next_run_at;
    let priority = job.priority;
    let timezone = job.timezone.clone().unwrap_or_else(|| "UTC".to_string());

    info!("Creating cron job with schedule: {}", schedule);
    let job_id = match pool.create_job_queue() {
        JobQueueWrapper::Postgres(q) => q.enqueue_cron_job(job).await?,
        JobQueueWrapper::MySQL(q) => q.enqueue_cron_job(job).await?,
    };

    println!("✅ Cron job created successfully");
    println!("   Job ID: {}", job_id);
    println!("   Queue: {}", queue);
    println!("   Schedule: {}", schedule);
    println!("   Priority: {}", priority);
    println!("   Timezone: {}", timezone);
    println!("   Next run: {}", fmt_time(next_run));

    info!("Created cron job: {}", job_id);
    Ok(())
}

async fn toggle_cron_job(pool: &DatabasePool, job_id: &str, enable: bool) -> Result<()> {
    let id = parse_job_id(job_id)?.to_string();
    let status = if enable { "enabled" } else { "disabled" };

    let mut params = SqlParams::new(pool.backend());
    let id_expr = params.uuid(&id);
    let flag = if enable { "true" } else { "false" };
    let sql = format!(
        "UPDATE hammerwork_jobs SET recurring = {flag} WHERE id = {id_expr} AND cron_schedule IS NOT NULL"
    );
    let updated = execute_binds(pool, &sql, params.binds()).await?;
    if updated == 0 {
        return Err(anyhow!("Cron job not found: {}", job_id));
    }

    println!("✅ Cron job {} successfully", status);
    println!("   Job ID: {}", job_id);

    info!("Cron job {} {}", job_id, status);
    Ok(())
}

/// One upcoming execution of a cron job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upcoming {
    pub at: DateTime<Utc>,
    pub queue: String,
    pub job_id: String,
    pub schedule: String,
}

/// The next `count` executions across active `jobs` after `from`, soonest first, optionally
/// only those before `until`. Jobs with a schedule that no longer parses are skipped.
pub fn upcoming_executions(
    jobs: &[CronJobRow],
    from: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
    count: usize,
) -> Vec<Upcoming> {
    let mut out = Vec::new();
    for job in jobs.iter().filter(|j| j.active) {
        let Ok(schedule) = CronSchedule::with_timezone(&job.schedule, &job.timezone) else {
            continue;
        };
        let mut cursor = from;
        for _ in 0..count {
            let Some(next) = schedule.next_execution(cursor) else {
                break;
            };
            if until.is_some_and(|limit| next > limit) {
                break;
            }
            out.push(Upcoming {
                at: next,
                queue: job.queue.clone(),
                job_id: job.id.clone(),
                schedule: job.schedule.clone(),
            });
            cursor = next;
        }
    }
    out.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.job_id.cmp(&b.job_id)));
    out.truncate(count);
    out
}

async fn show_next_executions(
    pool: &DatabasePool,
    queue: Option<&str>,
    count: u32,
    hours: Option<u32>,
) -> Result<()> {
    println!("⏰ Upcoming Cron Job Executions");
    println!("═══════════════════════════════");

    let jobs = fetch_cron_jobs(pool, queue, true, None).await?;
    let now = Utc::now();
    let until = hours.map(|h| now + Duration::hours(i64::from(h)));
    let upcoming = upcoming_executions(&jobs, now, until, count as usize);

    if upcoming.is_empty() {
        println!("📅 No upcoming cron job executions found");
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Time (UTC)", "Queue", "Schedule", "Job ID"]);
    for item in &upcoming {
        table.add_row(vec![
            fmt_time(Some(item.at)),
            item.queue.clone(),
            item.schedule.clone(),
            item.job_id.clone(),
        ]);
    }
    println!("{}", table);
    Ok(())
}

async fn update_cron_job(
    pool: &DatabasePool,
    job_id: &str,
    schedule: Option<&str>,
    timezone: Option<&str>,
    priority: Option<&str>,
) -> Result<()> {
    if schedule.is_none() && timezone.is_none() && priority.is_none() {
        return Err(anyhow!("No updates specified"));
    }
    let id = parse_job_id(job_id)?.to_string();
    let priority = priority.map(validate_priority).transpose()?;

    let current = fetch_cron_jobs(pool, None, false, Some(&id))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Cron job not found: {}", job_id))?;

    // Validate the schedule/timezone pair as it will be stored and find the next run.
    let new_schedule = schedule.unwrap_or(&current.schedule);
    let new_timezone = timezone.unwrap_or(&current.timezone);
    let cron = CronSchedule::with_timezone(new_schedule, new_timezone)?;
    let next_run = cron.next_execution_from_now();

    let mut params = SqlParams::new(pool.backend());
    let mut sets = Vec::new();
    if schedule.is_some() || timezone.is_some() {
        sets.push(format!("cron_schedule = {}", params.text(new_schedule)));
        sets.push(format!("timezone = {}", params.text(new_timezone)));
        if let Some(next) = next_run {
            sets.push(format!("next_run_at = {}", params.time(next)));
            // A job waiting for its next run waits for the new one.
            if current.status == "Pending" {
                sets.push(format!("scheduled_at = {}", params.time(next)));
            }
        }
    }
    if let Some(priority) = priority {
        sets.push(format!(
            "priority = {}",
            params.int(i64::from(priority.as_i32()))
        ));
    }
    let id_expr = params.uuid(&id);
    let sql = format!(
        "UPDATE hammerwork_jobs SET {} WHERE id = {id_expr} AND cron_schedule IS NOT NULL",
        sets.join(", ")
    );
    execute_binds(pool, &sql, params.binds()).await?;

    println!("✅ Cron job updated successfully");
    println!("   Job ID: {}", job_id);
    if schedule.is_some() {
        println!("   New Schedule: {}", new_schedule);
    }
    if timezone.is_some() {
        println!("   New Timezone: {}", new_timezone);
    }
    if let Some(p) = priority {
        println!("   New Priority: {}", p);
    }
    if let Some(next) = next_run.filter(|_| schedule.is_some() || timezone.is_some()) {
        println!("   Next run: {}", fmt_time(Some(next)));
    }

    info!("Updated cron job: {}", job_id);
    Ok(())
}

async fn delete_cron_job(pool: &DatabasePool, job_id: &str, confirm: bool) -> Result<()> {
    let id = parse_job_id(job_id)?.to_string();
    if !confirm {
        println!("⚠️  This will permanently delete the cron job. Use --confirm to proceed.");
        return Ok(());
    }

    let mut params = SqlParams::new(pool.backend());
    let id_expr = params.uuid(&id);
    let sql =
        format!("DELETE FROM hammerwork_jobs WHERE id = {id_expr} AND cron_schedule IS NOT NULL");
    let deleted = execute_binds(pool, &sql, params.binds()).await?;
    if deleted == 0 {
        return Err(anyhow!("Cron job not found: {}", job_id));
    }

    println!("✅ Cron job deleted successfully");
    println!("   Job ID: {}", job_id);

    info!("Deleted cron job: {}", job_id);
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
        command: CronCommand,
    }

    fn parse(args: &[&str]) -> CronCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&["list", "-n", "q", "--active-only", "--detailed", "-u", "u"]) {
            CronCommand::List {
                database_url,
                queue,
                active_only,
                detailed,
            } => {
                assert_eq!(database_url.as_deref(), Some("u"));
                assert_eq!(queue.as_deref(), Some("q"));
                assert!(active_only && detailed);
            }
            _ => panic!("expected List"),
        }
        match parse(&[
            "create",
            "-n",
            "q",
            "-j",
            "{}",
            "-s",
            "0 * * * * *",
            "-z",
            "UTC",
            "-r",
            "high",
        ]) {
            CronCommand::Create {
                queue,
                payload,
                schedule,
                timezone,
                priority,
                ..
            } => {
                assert_eq!((queue.as_str(), payload.as_str()), ("q", "{}"));
                assert_eq!(schedule, "0 * * * * *");
                assert_eq!(timezone.as_deref(), Some("UTC"));
                assert_eq!(priority.as_deref(), Some("high"));
            }
            _ => panic!("expected Create"),
        }
        match parse(&["next"]) {
            CronCommand::Next { count, hours, .. } => {
                assert_eq!(count, 10, "default count");
                assert_eq!(hours, None);
            }
            _ => panic!("expected Next"),
        }
        match parse(&["next", "-c", "3", "--hours", "6", "-n", "q"]) {
            CronCommand::Next {
                count,
                hours,
                queue,
                ..
            } => assert_eq!((count, hours, queue.as_deref()), (3, Some(6), Some("q"))),
            _ => panic!("expected Next"),
        }
        assert!(matches!(
            parse(&["enable", "abc"]),
            CronCommand::Enable { job_id, .. } if job_id == "abc"
        ));
        assert!(matches!(
            parse(&["disable", "abc"]),
            CronCommand::Disable { job_id, .. } if job_id == "abc"
        ));
        match parse(&["update", "abc", "-s", "s", "-z", "z", "-r", "low"]) {
            CronCommand::Update {
                job_id,
                schedule,
                timezone,
                priority,
                ..
            } => {
                assert_eq!(job_id, "abc");
                assert_eq!(schedule.as_deref(), Some("s"));
                assert_eq!(timezone.as_deref(), Some("z"));
                assert_eq!(priority.as_deref(), Some("low"));
            }
            _ => panic!("expected Update"),
        }
        assert!(matches!(
            parse(&["delete", "abc", "--confirm"]),
            CronCommand::Delete { confirm: true, .. }
        ));
        assert!(matches!(
            parse(&["delete", "abc"]),
            CronCommand::Delete { confirm: false, .. }
        ));
        assert!(TestCli::try_parse_from(["test", "create", "-n", "q"]).is_err());
        assert!(
            TestCli::try_parse_from([
                "test",
                "create",
                "-n",
                "q",
                "-j",
                "{}",
                "--description",
                "x"
            ])
            .is_err(),
            "--description was never stored anywhere and was removed"
        );
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        assert_eq!(
            parse(&["list"]).get_database_url(&config).unwrap(),
            "postgres://config/db"
        );
        assert_eq!(
            parse(&["list", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
        let err = parse(&["list"])
            .get_database_url(&Config::default())
            .unwrap_err();
        assert!(err.to_string().contains("Database URL is required"));
    }

    #[test]
    fn select_query_binds_queue_and_casts_the_id_on_postgres() {
        let (sql, binds) = build_cron_select_query(
            Backend::Postgres,
            Some(HOSTILE_QUEUE),
            true,
            Some("11111111-1111-1111-1111-111111111111"),
        );
        assert!(
            sql.contains("AND queue_name = $1 AND recurring = true AND id = $2::uuid ORDER BY"),
            "{sql}"
        );
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0], Bind::Text(HOSTILE_QUEUE.into()));
        assert!(!sql.contains("DROP"));

        let (sql, binds) = build_cron_select_query(Backend::MySql, None, false, None);
        assert!(!sql.contains("recurring = true") && !sql.contains('?'));
        assert!(binds.is_empty());
        let (sql, _) = build_cron_select_query(Backend::MySql, Some("q"), false, Some("id"));
        assert!(sql.contains("queue_name = ? AND id = ?"), "{sql}");
    }

    #[test]
    fn create_validates_payload_schedule_timezone_and_priority() {
        let job = build_cron_job(
            "q",
            r#"{"report": "daily"}"#,
            "0 0 9 * * MON-FRI",
            Some("America/New_York"),
            Some("high"),
        )
        .unwrap();
        assert_eq!(job.queue_name, "q");
        assert_eq!(job.payload["report"], "daily");
        assert_eq!(job.cron_schedule.as_deref(), Some("0 0 9 * * MON-FRI"));
        assert_eq!(job.timezone.as_deref(), Some("America/New_York"));
        assert_eq!(job.priority, JobPriority::High);
        assert!(job.recurring);
        assert!(job.next_run_at.unwrap() > Utc::now());

        let defaults = build_cron_job("q", "{}", "0 * * * * *", None, None).unwrap();
        assert_eq!(defaults.timezone.as_deref(), Some("UTC"));
        assert_eq!(defaults.priority, JobPriority::Normal);

        for (payload, schedule, tz, priority, expected) in [
            ("{nope", "0 * * * * *", None, None, "Invalid JSON payload"),
            ("{}", "* * * * *", None, None, "Invalid cron expression"),
            ("{}", "garbage", None, None, "Invalid cron expression"),
            (
                "{}",
                "0 * * * * *",
                Some("Mars/Base"),
                None,
                "Invalid timezone",
            ),
            (
                "{}",
                "0 * * * * *",
                None,
                Some("urgent"),
                "Invalid priority",
            ),
        ] {
            let err = build_cron_job("q", payload, schedule, tz, priority)
                .unwrap_err()
                .to_string();
            assert!(err.contains(expected), "{schedule} {tz:?}: {err}");
        }
    }

    fn row(id: &str, schedule: &str, tz: &str, active: bool) -> CronJobRow {
        CronJobRow {
            id: id.into(),
            queue: "q".into(),
            schedule: schedule.into(),
            timezone: tz.into(),
            status: "Pending".into(),
            priority: 2,
            active,
            next_run_at: None,
            payload: serde_json::json!({"k": 1}),
        }
    }

    #[test]
    fn upcoming_executions_merge_jobs_in_time_order_within_the_window() {
        let from = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let jobs = vec![
            row("hourly", "0 0 * * * *", "UTC", true),
            row("half-hour", "0 30 * * * *", "UTC", true),
            row("disabled", "0 * * * * *", "UTC", false),
            row("broken", "nonsense", "UTC", true),
        ];
        let next = upcoming_executions(&jobs, from, None, 4);
        let seen: Vec<(String, String)> = next
            .iter()
            .map(|u| (u.at.format("%H:%M").to_string(), u.job_id.clone()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("00:30".to_string(), "half-hour".to_string()),
                ("01:00".to_string(), "hourly".to_string()),
                ("01:30".to_string(), "half-hour".to_string()),
                ("02:00".to_string(), "hourly".to_string()),
            ]
        );

        let until = from + Duration::minutes(100);
        let windowed = upcoming_executions(&jobs, from, Some(until), 10);
        assert_eq!(windowed.len(), 3, "00:30, 01:00, 01:30");
        assert!(windowed.iter().all(|u| u.at <= until));
        assert!(upcoming_executions(&jobs, from, Some(from), 10).is_empty());
        assert!(upcoming_executions(&[], from, None, 5).is_empty());
    }

    #[test]
    fn upcoming_executions_respect_the_job_timezone() {
        let from = DateTime::parse_from_rfc3339("2030-06-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // 09:00 in New York is 13:00 UTC during daylight saving time.
        let jobs = vec![row("ny", "0 0 9 * * *", "America/New_York", true)];
        let next = upcoming_executions(&jobs, from, None, 1);
        assert_eq!(next[0].at.format("%H:%M").to_string(), "13:00");
    }

    #[test]
    fn rendering_shows_state_and_optional_details() {
        let mut job = row(
            "11111111-1111-1111-1111-111111111111",
            "0 * * * * *",
            "UTC",
            false,
        );
        job.priority = 4;
        let basic = render_cron_jobs(&[job.clone()], false);
        assert!(basic.contains("11111111-1111-1111-1111-111111111111"));
        assert!(basic.contains("no") && basic.contains("0 * * * * *"));
        assert!(!basic.contains("Payload"));
        let detailed = render_cron_jobs(&[job], true);
        assert!(detailed.contains("Payload") && detailed.contains("{\"k\":1}"));
        assert!(detailed.contains("critical"), "{detailed}");
    }

    async fn cron_lifecycle(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let queue = hostile_queue();
        let other = unique_queue("cron_other");
        let run = |cmd: CronCommand| {
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let create = |queue: &str, schedule: &str, tz: Option<&str>, priority: Option<&str>| {
            CronCommand::Create {
                database_url: None,
                queue: queue.to_string(),
                payload: r#"{"task": "report"}"#.to_string(),
                schedule: schedule.to_string(),
                timezone: tz.map(String::from),
                priority: priority.map(String::from),
            }
        };

        // create stores a real recurring job with a computed next run
        run(create(
            &queue,
            "0 0 9 * * *",
            Some("Europe/Paris"),
            Some("high"),
        ))
        .await
        .unwrap();
        run(create(&other, "0 30 * * * *", None, None))
            .await
            .unwrap();
        let jobs = fetch_cron_jobs(&pool, Some(&queue), false, None)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        let job = &jobs[0];
        assert_eq!(job.schedule, "0 0 9 * * *");
        assert_eq!(job.timezone, "Europe/Paris");
        assert_eq!(job.priority, JobPriority::High.as_i32());
        assert_eq!(job.status, "Pending");
        assert!(job.active);
        assert_eq!(job.payload["task"], "report");
        assert!(job.next_run_at.unwrap() > Utc::now());
        let id = job.id.clone();

        // invalid input is rejected without writing anything
        assert!(
            run(create(&queue, "* * * * *", None, None)).await.is_err(),
            "five-field expressions are not accepted by the scheduler"
        );
        assert_eq!(count_jobs(&pool, &queue, None).await, 1);

        // list / next run against real rows
        run(CronCommand::List {
            database_url: None,
            queue: Some(queue.clone()),
            active_only: true,
            detailed: true,
        })
        .await
        .unwrap();
        run(CronCommand::Next {
            database_url: None,
            queue: Some(queue.clone()),
            count: 3,
            hours: Some(72),
        })
        .await
        .unwrap();

        // disable / enable flip `recurring` and are scoped to cron jobs
        let toggle = |enable: bool, id: &str| {
            let id = id.to_string();
            if enable {
                CronCommand::Enable {
                    database_url: None,
                    job_id: id,
                }
            } else {
                CronCommand::Disable {
                    database_url: None,
                    job_id: id,
                }
            }
        };
        run(toggle(false, &id)).await.unwrap();
        assert!(
            !fetch_cron_jobs(&pool, Some(&queue), false, Some(&id))
                .await
                .unwrap()[0]
                .active
        );
        assert!(
            fetch_cron_jobs(&pool, Some(&queue), true, None)
                .await
                .unwrap()
                .is_empty()
        );
        run(toggle(true, &id)).await.unwrap();
        assert!(
            fetch_cron_jobs(&pool, Some(&queue), true, None)
                .await
                .unwrap()[0]
                .active
        );

        // unknown / malformed ids
        let missing = uuid::Uuid::new_v4().to_string();
        let err = run(toggle(true, &missing)).await.unwrap_err().to_string();
        assert!(err.contains("Cron job not found"), "{err}");
        let err = run(toggle(false, "not-a-uuid"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Invalid job ID"), "{err}");

        // a non-cron job is never touched by the cron commands
        let plain = seed(&pool, &SeedJob::new(&queue, "Pending")).await;
        assert!(run(toggle(false, &plain)).await.is_err());

        // update: schedule + timezone recompute the next run and move the pending job
        let before = job_column(&pool, &id, "next_run_at").await;
        run(CronCommand::Update {
            database_url: None,
            job_id: id.clone(),
            schedule: Some("0 15 * * * *".into()),
            timezone: Some("UTC".into()),
            priority: Some("critical".into()),
        })
        .await
        .unwrap();
        let updated = &fetch_cron_jobs(&pool, Some(&queue), false, Some(&id))
            .await
            .unwrap()[0];
        assert_eq!(updated.schedule, "0 15 * * * *");
        assert_eq!(updated.timezone, "UTC");
        assert_eq!(updated.priority, JobPriority::Critical.as_i32());
        assert_eq!(
            updated.next_run_at.unwrap().format("%M:%S").to_string(),
            "15:00"
        );
        assert_ne!(job_column(&pool, &id, "next_run_at").await, before);
        let scheduled = job_column(&pool, &id, "scheduled_at").await;
        assert!(scheduled.is_some());

        // priority-only update leaves the schedule alone
        run(CronCommand::Update {
            database_url: None,
            job_id: id.clone(),
            schedule: None,
            timezone: None,
            priority: Some("low".into()),
        })
        .await
        .unwrap();
        let updated = &fetch_cron_jobs(&pool, Some(&queue), false, Some(&id))
            .await
            .unwrap()[0];
        assert_eq!(updated.schedule, "0 15 * * * *");
        assert_eq!(updated.priority, JobPriority::Low.as_i32());

        // update errors
        for (schedule, tz, priority, expected) in [
            (None, None, None, "No updates specified"),
            (Some("bogus"), None, None, "Invalid cron expression"),
            (None, Some("Mars/Base"), None, "Invalid timezone"),
            (None, None, Some("urgent"), "Invalid priority"),
        ] {
            let err = run(CronCommand::Update {
                database_url: None,
                job_id: id.clone(),
                schedule: schedule.map(String::from),
                timezone: tz.map(String::from),
                priority: priority.map(String::from),
            })
            .await
            .unwrap_err()
            .to_string();
            assert!(err.contains(expected), "{err}");
        }
        let err = run(CronCommand::Update {
            database_url: None,
            job_id: missing.clone(),
            schedule: None,
            timezone: None,
            priority: Some("low".into()),
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("Cron job not found"), "{err}");

        // delete needs --confirm, then removes only the cron job
        let delete = |confirm: bool, id: &str| CronCommand::Delete {
            database_url: None,
            job_id: id.to_string(),
            confirm,
        };
        run(delete(false, &id)).await.unwrap();
        assert_eq!(
            fetch_cron_jobs(&pool, Some(&queue), false, None)
                .await
                .unwrap()
                .len(),
            1
        );
        run(delete(true, &id)).await.unwrap();
        assert!(
            fetch_cron_jobs(&pool, Some(&queue), false, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            count_jobs(&pool, &queue, None).await,
            1,
            "plain job survives"
        );
        assert!(run(delete(true, &id)).await.is_err());
        assert!(run(delete(true, &plain)).await.is_err(), "not a cron job");

        // the other queue was never affected
        assert_eq!(
            fetch_cron_jobs(&pool, Some(&other), true, None)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&queue, &other]).await;
    }

    db_tests!(
        cron_lifecycle,
        test_cron_lifecycle_postgres,
        test_cron_lifecycle_mysql
    );
}
