//! Worker visibility commands.
//!
//! Hammerwork has no worker registry: workers are ordinary library objects living
//! inside the applications that embed them, so there is nothing the CLI could list,
//! start or stop remotely. What the database does record is the lease each worker
//! holds on the job it is running (`last_heartbeat_at` / `lease_expires_at`, renewed
//! by the worker's heartbeat). `worker status` reports exactly that.

use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Subcommand;
use sqlx::Row;

use crate::config::Config;
use crate::utils::database::DatabasePool;

#[derive(Subcommand)]
pub enum WorkerCommand {
    #[command(
        about = "Show running jobs and their worker leases, grouped by queue",
        long_about = "Show the jobs currently in Running state and the lease their worker holds \
            on each (renewed by worker heartbeats), grouped by queue.\n\n\
            There is no worker registry, so this reports leases, not worker processes: a \
            Running job whose lease has expired belongs to a worker that stopped heartbeating \
            (crashed or hung) and can be reclaimed with `job requeue-stale`. Requires \
            migration 015_add_job_leases.\n\n\
            Example: cargo hammerwork worker status --queue emails"
    )]
    Status {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Only show this queue")]
        queue: Option<String>,
        #[arg(long, help = "Also list every running job")]
        jobs: bool,
    },
}

impl WorkerCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        match self {
            WorkerCommand::Status {
                database_url,
                queue,
                jobs,
            } => {
                let db_url = database_url
                    .as_deref()
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;
                let pool = DatabasePool::connect(db_url, config.get_connection_pool_size()).await?;
                let (now, running) = fetch_running_jobs(&pool, queue.as_deref()).await?;
                print!("{}", render_status(&running, now, *jobs));
                Ok(())
            }
        }
    }
}

/// A job in `Running` state together with its lease columns.
#[derive(Debug, Clone, PartialEq)]
pub struct RunningJob {
    pub id: String,
    pub queue_name: String,
    pub attempts: i32,
    pub started_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub lease_expires_at: Option<DateTime<Utc>>,
}

/// Health of the lease behind a running job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// The lease is in the future: the worker is heartbeating.
    Active,
    /// The lease has run out: the worker stopped heartbeating.
    Expired,
    /// The job never recorded a lease (started before heartbeats existed or the
    /// worker has not heartbeated yet).
    NoLease,
}

impl RunningJob {
    pub fn lease_state(&self, now: DateTime<Utc>) -> LeaseState {
        match self.lease_expires_at {
            Some(expires) if expires > now => LeaseState::Active,
            Some(_) => LeaseState::Expired,
            None => LeaseState::NoLease,
        }
    }
}

/// Per-queue rollup of running jobs.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueLeaseSummary {
    pub queue_name: String,
    pub running: usize,
    pub active: usize,
    pub expired: usize,
    pub no_lease: usize,
    /// Age of the stalest heartbeat among jobs that have one.
    pub oldest_heartbeat_age_secs: Option<i64>,
}

/// Groups running jobs by queue (sorted by queue name).
pub fn summarize(jobs: &[RunningJob], now: DateTime<Utc>) -> Vec<QueueLeaseSummary> {
    let mut by_queue: std::collections::BTreeMap<&str, QueueLeaseSummary> = Default::default();
    for job in jobs {
        let entry = by_queue
            .entry(job.queue_name.as_str())
            .or_insert_with(|| QueueLeaseSummary {
                queue_name: job.queue_name.clone(),
                running: 0,
                active: 0,
                expired: 0,
                no_lease: 0,
                oldest_heartbeat_age_secs: None,
            });
        entry.running += 1;
        match job.lease_state(now) {
            LeaseState::Active => entry.active += 1,
            LeaseState::Expired => entry.expired += 1,
            LeaseState::NoLease => entry.no_lease += 1,
        }
        if let Some(beat) = job.last_heartbeat_at {
            let age = (now - beat).num_seconds().max(0);
            entry.oldest_heartbeat_age_secs = Some(
                entry
                    .oldest_heartbeat_age_secs
                    .map_or(age, |current| current.max(age)),
            );
        }
    }
    by_queue.into_values().collect()
}

fn format_age(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m {}s ago", s / 60, s % 60),
        s => format!("{}h {}m ago", s / 3600, (s % 3600) / 60),
    }
}

fn format_lease(job: &RunningJob, now: DateTime<Utc>) -> String {
    match (job.lease_state(now), job.lease_expires_at) {
        (LeaseState::Active, Some(expires)) => {
            format!("active ({}s left)", (expires - now).num_seconds())
        }
        (LeaseState::Expired, Some(expires)) => {
            format!("EXPIRED ({}s ago)", (now - expires).num_seconds())
        }
        _ => "none".to_string(),
    }
}

/// Renders the status report. Pure so it can be tested without a database.
pub fn render_status(jobs: &[RunningJob], now: DateTime<Utc>, list_jobs: bool) -> String {
    if jobs.is_empty() {
        return "No jobs are currently running.\n".to_string();
    }

    let mut out = String::new();
    let mut table = comfy_table::Table::new();
    table.set_header(vec![
        "Queue",
        "Running",
        "Lease active",
        "Lease expired",
        "No lease",
        "Stalest heartbeat",
    ]);
    let summaries = summarize(jobs, now);
    for s in &summaries {
        table.add_row(vec![
            s.queue_name.clone(),
            s.running.to_string(),
            s.active.to_string(),
            s.expired.to_string(),
            s.no_lease.to_string(),
            s.oldest_heartbeat_age_secs
                .map(format_age)
                .unwrap_or_else(|| "-".to_string()),
        ]);
    }
    out.push_str(&format!("{table}\n"));

    if list_jobs {
        let mut jobs_table = comfy_table::Table::new();
        jobs_table.set_header(vec![
            "Job ID",
            "Queue",
            "Attempt",
            "Started",
            "Last heartbeat",
            "Lease",
        ]);
        let mut sorted: Vec<&RunningJob> = jobs.iter().collect();
        sorted.sort_by(|a, b| (&a.queue_name, a.started_at).cmp(&(&b.queue_name, b.started_at)));
        for job in sorted {
            jobs_table.add_row(vec![
                job.id.clone(),
                job.queue_name.clone(),
                job.attempts.to_string(),
                job.started_at
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                    .unwrap_or_else(|| "-".to_string()),
                job.last_heartbeat_at
                    .map(|t| format_age((now - t).num_seconds().max(0)))
                    .unwrap_or_else(|| "-".to_string()),
                format_lease(job, now),
            ]);
        }
        out.push_str(&format!("\n{jobs_table}\n"));
    }

    let total: usize = summaries.iter().map(|s| s.running).sum();
    let expired: usize = summaries.iter().map(|s| s.expired).sum();
    out.push_str(&format!(
        "\n{total} running job(s) across {} queue(s); {expired} with an expired lease.\n",
        summaries.len()
    ));
    if expired > 0 {
        out.push_str(
            "Expired leases mean a worker stopped heartbeating; run `job requeue-stale` to reclaim them.\n",
        );
    }
    out
}

/// Fetches the database clock and every Running job (optionally for one queue).
pub async fn fetch_running_jobs(
    pool: &DatabasePool,
    queue: Option<&str>,
) -> Result<(DateTime<Utc>, Vec<RunningJob>)> {
    match pool {
        DatabasePool::Postgres(pg) => {
            let now: DateTime<Utc> = sqlx::query_scalar("SELECT NOW()").fetch_one(pg).await?;
            let mut sql = String::from(
                "SELECT id, queue_name, attempts, started_at, last_heartbeat_at, lease_expires_at \
                 FROM hammerwork_jobs WHERE status = 'Running'",
            );
            if queue.is_some() {
                sql.push_str(" AND queue_name = $1");
            }
            sql.push_str(" ORDER BY queue_name, started_at");
            let mut q = sqlx::query(&sql);
            if let Some(name) = queue {
                q = q.bind(name.to_string());
            }
            let rows = q.fetch_all(pg).await?;
            let mut jobs = Vec::with_capacity(rows.len());
            for row in rows {
                let id: uuid::Uuid = row.try_get("id")?;
                jobs.push(RunningJob {
                    id: id.to_string(),
                    queue_name: row.try_get("queue_name")?,
                    attempts: row.try_get("attempts")?,
                    started_at: row.try_get("started_at")?,
                    last_heartbeat_at: row.try_get("last_heartbeat_at")?,
                    lease_expires_at: row.try_get("lease_expires_at")?,
                });
            }
            Ok((now, jobs))
        }
        DatabasePool::MySQL(my) => {
            let now: chrono::NaiveDateTime = sqlx::query_scalar("SELECT UTC_TIMESTAMP(6)")
                .fetch_one(my)
                .await?;
            let mut sql = String::from(
                "SELECT id, queue_name, attempts, started_at, last_heartbeat_at, lease_expires_at \
                 FROM hammerwork_jobs WHERE status = 'Running'",
            );
            if queue.is_some() {
                sql.push_str(" AND queue_name = ?");
            }
            sql.push_str(" ORDER BY queue_name, started_at");
            let mut q = sqlx::query(&sql);
            if let Some(name) = queue {
                q = q.bind(name.to_string());
            }
            let rows = q.fetch_all(my).await?;
            let mut jobs = Vec::with_capacity(rows.len());
            for row in rows {
                jobs.push(RunningJob {
                    id: row.try_get("id")?,
                    queue_name: row.try_get("queue_name")?,
                    attempts: row.try_get("attempts")?,
                    started_at: row.try_get("started_at")?,
                    last_heartbeat_at: row.try_get("last_heartbeat_at")?,
                    lease_expires_at: row.try_get("lease_expires_at")?,
                });
            }
            Ok((now.and_utc(), jobs))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: WorkerCommand,
    }

    fn job(queue: &str, beat_secs_ago: Option<i64>, lease_secs_left: Option<i64>) -> RunningJob {
        let now = Utc::now();
        RunningJob {
            id: uuid::Uuid::new_v4().to_string(),
            queue_name: queue.to_string(),
            attempts: 1,
            started_at: Some(now - Duration::seconds(100)),
            last_heartbeat_at: beat_secs_ago.map(|s| now - Duration::seconds(s)),
            lease_expires_at: lease_secs_left.map(|s| now + Duration::seconds(s)),
        }
    }

    #[test]
    fn test_removed_placeholder_subcommands_are_rejected() {
        for sub in ["start", "list", "stop"] {
            assert!(
                TestCli::try_parse_from(["t", sub]).is_err(),
                "`worker {sub}` must not exist"
            );
        }
        assert!(TestCli::try_parse_from(["t", "status", "-n", "emails", "--jobs"]).is_ok());
    }

    #[test]
    fn test_lease_states() {
        let now = Utc::now();
        assert_eq!(
            job("q", Some(1), Some(30)).lease_state(now),
            LeaseState::Active
        );
        assert_eq!(
            job("q", Some(90), Some(-5)).lease_state(now),
            LeaseState::Expired
        );
        assert_eq!(job("q", None, None).lease_state(now), LeaseState::NoLease);
    }

    #[test]
    fn test_summarize_groups_by_queue() {
        let now = Utc::now();
        let jobs = vec![
            job("b", Some(2), Some(28)),
            job("a", Some(120), Some(-90)),
            job("a", Some(3), Some(27)),
            job("a", None, None),
        ];
        let s = summarize(&jobs, now);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].queue_name, "a");
        assert_eq!(
            (s[0].running, s[0].active, s[0].expired, s[0].no_lease),
            (3, 1, 1, 1)
        );
        assert!(s[0].oldest_heartbeat_age_secs.unwrap() >= 119);
        assert_eq!(s[1].queue_name, "b");
        assert_eq!(s[1].running, 1);
    }

    #[test]
    fn test_render_status_empty_and_expired() {
        let now = Utc::now();
        assert_eq!(
            render_status(&[], now, false),
            "No jobs are currently running.\n"
        );
        let out = render_status(&[job("a", Some(120), Some(-90))], now, true);
        assert!(out.contains("EXPIRED"));
        assert!(out.contains("requeue-stale"));
        assert!(out.contains("1 running job(s) across 1 queue(s); 1 with an expired lease."));
    }

    async fn status_roundtrip(url: &str) {
        let pool = DatabasePool::connect(url, 2).await.unwrap();
        let queue = format!("wstatus_{}", uuid::Uuid::new_v4().simple());
        let is_pg = matches!(pool, DatabasePool::Postgres(_));
        let now_expr = if is_pg { "NOW()" } else { "UTC_TIMESTAMP(6)" };
        let in_30 = if is_pg {
            "NOW() + INTERVAL '30 seconds'"
        } else {
            "DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 SECOND)"
        };
        let past = if is_pg {
            "NOW() - INTERVAL '30 seconds'"
        } else {
            "DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 30 SECOND)"
        };
        let ids: Vec<String> = (0..3).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let leases = [in_30, past, "NULL"];
        for (id, lease) in ids.iter().zip(leases) {
            let sql = if is_pg {
                format!(
                    "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, priority, attempts, max_attempts, created_at, scheduled_at, started_at, last_heartbeat_at, lease_expires_at) \
                     VALUES ($1::uuid, $2, '{{}}'::jsonb, 'Running', 2, 1, 3, {now_expr}, {now_expr}, {now_expr}, {now_expr}, {lease})"
                )
            } else {
                format!(
                    "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, priority, attempts, max_attempts, created_at, scheduled_at, started_at, last_heartbeat_at, lease_expires_at) \
                     VALUES (?, ?, '{{}}', 'Running', 2, 1, 3, {now_expr}, {now_expr}, {now_expr}, {now_expr}, {lease})"
                )
            };
            match &pool {
                DatabasePool::Postgres(p) => {
                    sqlx::query(&sql)
                        .bind(id)
                        .bind(&queue)
                        .execute(p)
                        .await
                        .unwrap();
                }
                DatabasePool::MySQL(p) => {
                    sqlx::query(&sql)
                        .bind(id)
                        .bind(&queue)
                        .execute(p)
                        .await
                        .unwrap();
                }
            }
        }

        let (now, jobs) = fetch_running_jobs(&pool, Some(&queue)).await.unwrap();
        let summary = summarize(&jobs, now);
        assert_eq!(summary.len(), 1);
        let s = &summary[0];
        assert_eq!((s.running, s.active, s.expired, s.no_lease), (3, 1, 1, 1));

        let (_, other) = fetch_running_jobs(&pool, Some("no_such_queue_xyz"))
            .await
            .unwrap();
        assert!(other.is_empty());

        match &pool {
            DatabasePool::Postgres(p) => {
                sqlx::query("DELETE FROM hammerwork_jobs WHERE queue_name = $1")
                    .bind(&queue)
                    .execute(p)
                    .await
                    .unwrap();
            }
            DatabasePool::MySQL(p) => {
                sqlx::query("DELETE FROM hammerwork_jobs WHERE queue_name = ?")
                    .bind(&queue)
                    .execute(p)
                    .await
                    .unwrap();
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_worker_status_lease_summary_postgres() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        status_roundtrip(&url).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_worker_status_lease_summary_mysql() {
        let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
        status_roundtrip(&url).await;
    }
}
