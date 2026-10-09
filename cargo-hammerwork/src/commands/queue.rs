use anyhow::Result;
use clap::Subcommand;
use hammerwork::JobQueue;
use hammerwork::queue::DatabaseQueue;
use sqlx::Row;
use tracing::info;

use crate::commands::job::priority_display;
use crate::commands::monitor::build_status_counts_query;
use crate::config::Config;
use crate::utils::database::{DatabasePool, JobQueueWrapper};
use crate::utils::display::StatsTable;
use crate::utils::sql::{
    Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg, execute_binds, fetch_i64,
};

#[derive(Subcommand)]
pub enum QueueCommand {
    #[command(about = "List all queues")]
    List {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
    },
    #[command(about = "Show queue statistics")]
    Stats {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Specific queue name")]
        queue: Option<String>,
        #[arg(long, help = "Show detailed breakdown by priority")]
        detailed: bool,
    },
    #[command(about = "Clear all jobs from a queue")]
    Clear {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name")]
        queue: String,
        #[arg(long, help = "Only clear pending jobs")]
        pending_only: bool,
        #[arg(long, help = "Confirm the operation")]
        confirm: bool,
    },
    #[command(about = "Pause a queue (prevent job processing)")]
    Pause {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name")]
        queue: String,
    },
    #[command(about = "Resume a paused queue")]
    Resume {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Queue name")]
        queue: String,
    },
    #[command(about = "List all paused queues")]
    Paused {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
    },
    #[command(about = "Get queue health status")]
    Health {
        #[arg(short = 'u', short_alias = 'd', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', short_alias = 'Q', long, help = "Specific queue name")]
        queue: Option<String>,
    },
}

impl QueueCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect_with_config(&db_url, config).await?;

        match self {
            QueueCommand::List { .. } => {
                list_queues(pool).await?;
            }
            QueueCommand::Stats {
                queue, detailed, ..
            } => {
                show_queue_stats(pool, queue.clone(), *detailed).await?;
            }
            QueueCommand::Clear {
                queue,
                pending_only,
                confirm,
                ..
            } => {
                clear_queue(pool, queue, *pending_only, *confirm).await?;
            }
            QueueCommand::Pause { queue, .. } => {
                pause_queue(pool, queue).await?;
            }
            QueueCommand::Resume { queue, .. } => {
                resume_queue(pool, queue).await?;
            }
            QueueCommand::Paused { .. } => {
                list_paused_queues(pool).await?;
            }
            QueueCommand::Health { queue, .. } => {
                show_queue_health(pool, queue.clone()).await?;
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url_option = match self {
            QueueCommand::List { database_url, .. } => database_url,
            QueueCommand::Stats { database_url, .. } => database_url,
            QueueCommand::Clear { database_url, .. } => database_url,
            QueueCommand::Pause { database_url, .. } => database_url,
            QueueCommand::Resume { database_url, .. } => database_url,
            QueueCommand::Paused { database_url, .. } => database_url,
            QueueCommand::Health { database_url, .. } => database_url,
        };

        url_option
            .as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
    }
}

/// One row of `queue list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSummary {
    pub name: String,
    pub total: i64,
    pub pending: i64,
    pub running: i64,
    pub completed: i64,
    pub failed: i64,
    pub dead: i64,
    pub paused: bool,
}

const QUEUE_SUMMARY_SQL: &str = r#"
    SELECT
        queue_name,
        COUNT(id) as total_jobs,
        COUNT(CASE WHEN status = 'Pending' THEN 1 END) as pending,
        COUNT(CASE WHEN status = 'Running' THEN 1 END) as running,
        COUNT(CASE WHEN status = 'Completed' THEN 1 END) as completed,
        COUNT(CASE WHEN status = 'Failed' THEN 1 END) as failed,
        COUNT(CASE WHEN status = 'Dead' THEN 1 END) as dead
    FROM hammerwork_jobs
    GROUP BY queue_name
    ORDER BY queue_name
"#;

/// Job counts per queue, plus paused queues that currently hold no jobs. Sorted by name.
pub async fn fetch_queue_summaries(pool: &DatabasePool) -> Result<Vec<QueueSummary>> {
    macro_rules! summaries {
        ($rows:expr) => {{
            let mut out = Vec::new();
            for row in $rows {
                out.push(QueueSummary {
                    name: row.try_get("queue_name")?,
                    total: row.try_get("total_jobs")?,
                    pending: row.try_get("pending")?,
                    running: row.try_get("running")?,
                    completed: row.try_get("completed")?,
                    failed: row.try_get("failed")?,
                    dead: row.try_get("dead")?,
                    paused: false,
                });
            }
            out
        }};
    }
    let (mut summaries, paused) = match pool {
        DatabasePool::Postgres(pg) => {
            let rows = sqlx::query(QUEUE_SUMMARY_SQL).fetch_all(pg).await?;
            let paused = JobQueue::new(pg.clone()).get_paused_queues().await?;
            (summaries!(rows), paused)
        }
        DatabasePool::MySQL(my) => {
            let rows = sqlx::query(QUEUE_SUMMARY_SQL).fetch_all(my).await?;
            let paused = JobQueue::new(my.clone()).get_paused_queues().await?;
            (summaries!(rows), paused)
        }
    };
    for info in paused {
        match summaries.iter_mut().find(|s| s.name == info.queue_name) {
            Some(summary) => summary.paused = true,
            None => summaries.push(QueueSummary {
                name: info.queue_name,
                total: 0,
                pending: 0,
                running: 0,
                completed: 0,
                failed: 0,
                dead: 0,
                paused: true,
            }),
        }
    }
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(summaries)
}

fn render_queue_summaries(summaries: &[QueueSummary]) -> String {
    let mut table = comfy_table::Table::new();
    table.set_header(vec![
        "Queue",
        "Status",
        "Total",
        "Pending",
        "Running",
        "Completed",
        "Failed",
        "Dead",
    ]);
    for s in summaries {
        table.add_row(vec![
            s.name.clone(),
            if s.paused {
                "⏸️ Paused"
            } else {
                "▶️ Active"
            }
            .to_string(),
            s.total.to_string(),
            s.pending.to_string(),
            s.running.to_string(),
            s.completed.to_string(),
            s.failed.to_string(),
            s.dead.to_string(),
        ]);
    }
    table.to_string()
}

async fn list_queues(pool: DatabasePool) -> Result<()> {
    let summaries = fetch_queue_summaries(&pool).await?;
    println!("📋 Queue Overview");
    println!("{}", render_queue_summaries(&summaries));
    Ok(())
}

/// `COUNT(*) as total`, optionally for one queue (bound, not interpolated).
pub fn build_queue_total_query(backend: Backend, queue: Option<&str>) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let filter = match queue {
        Some(q) => format!(" WHERE queue_name = {}", params.text(q)),
        None => String::new(),
    };
    (
        format!("SELECT COUNT(*) as total FROM hammerwork_jobs{filter}"),
        params.into_binds(),
    )
}

/// Counts by status and priority, optionally for one queue.
pub fn build_detailed_stats_query(backend: Backend, queue: Option<&str>) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let filter = match queue {
        Some(q) => format!(" WHERE queue_name = {}", params.text(q)),
        None => String::new(),
    };
    (
        format!(
            "SELECT status, priority, COUNT(*) as count FROM hammerwork_jobs{filter} GROUP BY status, priority ORDER BY status, priority"
        ),
        params.into_binds(),
    )
}

/// The counts shown by `queue health`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthMetric {
    Total,
    RecentFailures,
    LongRunning,
}

/// A `COUNT(*) as count` query for one health metric, optionally for one queue.
pub fn build_health_query(
    backend: Backend,
    metric: HealthMetric,
    queue: Option<&str>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let mut conditions = Vec::new();
    if let Some(q) = queue {
        conditions.push(format!("queue_name = {}", params.text(q)));
    }
    match metric {
        HealthMetric::Total => {}
        HealthMetric::RecentFailures => {
            conditions.push(format!("failed_at > {}", params.ago(1, IntervalUnit::Hour)));
        }
        HealthMetric::LongRunning => {
            conditions.push("status = 'Running'".to_string());
            conditions.push(format!(
                "started_at < {}",
                params.ago(1, IntervalUnit::Hour)
            ));
        }
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    (
        format!("SELECT COUNT(*) as count FROM hammerwork_jobs{where_clause}"),
        params.into_binds(),
    )
}

async fn show_queue_stats(pool: DatabasePool, queue: Option<String>, detailed: bool) -> Result<()> {
    if detailed {
        show_detailed_stats(pool, queue).await
    } else {
        show_basic_stats(pool, queue).await
    }
}

async fn show_basic_stats(pool: DatabasePool, queue: Option<String>) -> Result<()> {
    let (base_query, base_binds) = build_status_counts_query(pool.backend(), queue.as_deref());

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Status", "Count"]);

    match &pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = bind_pg(sqlx::query(&base_query), &base_binds)
                .fetch_all(pg_pool)
                .await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_icon = match status.to_lowercase().as_str() {
                    "pending" => "🟡",
                    "running" => "🔵",
                    "completed" => "🟢",
                    "failed" => "🔴",
                    "dead" => "💀",
                    "retrying" => "🟠",
                    _ => "❓",
                };

                table.add_row(vec![
                    format!("{} {}", status_icon, status),
                    count.to_string(),
                ]);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = bind_mysql(sqlx::query(&base_query), &base_binds)
                .fetch_all(mysql_pool)
                .await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_icon = match status.to_lowercase().as_str() {
                    "pending" => "🟡",
                    "running" => "🔵",
                    "completed" => "🟢",
                    "failed" => "🔴",
                    "dead" => "💀",
                    "retrying" => "🟠",
                    _ => "❓",
                };

                table.add_row(vec![
                    format!("{} {}", status_icon, status),
                    count.to_string(),
                ]);
            }
        }
    }

    println!("📊 Queue Statistics");
    if let Some(q) = &queue {
        println!("Queue: {}", q);
    }
    println!("{}", table);

    // Show total count
    let (total_query, total_binds) = build_queue_total_query(pool.backend(), queue.as_deref());
    let total = fetch_i64(&pool, &total_query, &total_binds, "total").await?;
    println!("\n📈 Total jobs: {}", total);

    Ok(())
}

async fn show_detailed_stats(pool: DatabasePool, queue: Option<String>) -> Result<()> {
    let (base_query, base_binds) = build_detailed_stats_query(pool.backend(), queue.as_deref());

    let mut stats_table = StatsTable::new();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = bind_pg(sqlx::query(&base_query), &base_binds)
                .fetch_all(&pg_pool)
                .await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let priority = priority_display(row.try_get("priority")?);
                let count: i64 = row.try_get("count")?;

                stats_table.add_stats_row(&status, &priority, count);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = bind_mysql(sqlx::query(&base_query), &base_binds)
                .fetch_all(&mysql_pool)
                .await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let priority = priority_display(row.try_get("priority")?);
                let count: i64 = row.try_get("count")?;

                stats_table.add_stats_row(&status, &priority, count);
            }
        }
    }

    println!("📊 Detailed Queue Statistics");
    if let Some(q) = &queue {
        println!("Queue: {}", q);
    }
    println!("{}", stats_table);

    Ok(())
}

async fn clear_queue(
    pool: DatabasePool,
    queue: &str,
    pending_only: bool,
    confirm: bool,
) -> Result<u64> {
    if !confirm {
        println!(
            "⚠️  This will permanently delete jobs from queue '{}'. Use --confirm to proceed.",
            queue
        );
        return Ok(0);
    }

    let mut params = SqlParams::new(pool.backend());
    let mut sql = format!(
        "DELETE FROM hammerwork_jobs WHERE queue_name = {}",
        params.text(queue)
    );
    if pending_only {
        sql.push_str(" AND status = 'Pending'");
    }
    let affected = execute_binds(&pool, &sql, params.binds()).await?;

    let job_type = if pending_only { "pending" } else { "all" };
    println!(
        "✅ Cleared {} {} jobs from queue '{}'",
        affected, job_type, queue
    );
    Ok(affected)
}

async fn pause_queue(pool: DatabasePool, queue: &str) -> Result<()> {
    match pool.create_job_queue() {
        JobQueueWrapper::Postgres(q) => q.pause_queue(queue, Some("cli")).await?,
        JobQueueWrapper::MySQL(q) => q.pause_queue(queue, Some("cli")).await?,
    }
    println!("⏸️  Queue '{}' has been paused", queue);
    info!("Queue '{}' has been paused via CLI", queue);
    Ok(())
}

async fn resume_queue(pool: DatabasePool, queue: &str) -> Result<()> {
    let was_paused = match pool.create_job_queue() {
        JobQueueWrapper::Postgres(q) => resume_if_paused(&q, queue).await?,
        JobQueueWrapper::MySQL(q) => resume_if_paused(&q, queue).await?,
    };
    if was_paused {
        println!("▶️  Queue '{}' has been resumed", queue);
        info!("Queue '{}' has been resumed via CLI", queue);
    } else {
        println!("ℹ️  Queue '{}' was not paused", queue);
    }
    Ok(())
}

async fn resume_if_paused<Q: DatabaseQueue>(queue: &Q, name: &str) -> Result<bool> {
    if !queue.is_queue_paused(name).await? {
        return Ok(false);
    }
    queue.resume_queue(name, Some("cli")).await?;
    Ok(true)
}

async fn show_queue_health(pool: DatabasePool, queue: Option<String>) -> Result<()> {
    // Calculate various health metrics
    let mut health_table = comfy_table::Table::new();
    health_table.set_header(vec!["Metric", "Value", "Status"]);

    let backend = pool.backend();
    let mut counts = [0i64; 3];
    for (slot, metric) in counts.iter_mut().zip([
        HealthMetric::Total,
        HealthMetric::RecentFailures,
        HealthMetric::LongRunning,
    ]) {
        let (sql, binds) = build_health_query(backend, metric, queue.as_deref());
        *slot = fetch_i64(&pool, &sql, &binds, "count").await?;
    }
    let [total_jobs, recent_failures, long_running] = counts;

    health_table.add_row(vec![
        "Total Jobs".to_string(),
        total_jobs.to_string(),
        if total_jobs < 10000 {
            "🟢 Good"
        } else {
            "🟡 High"
        }
        .to_string(),
    ]);

    health_table.add_row(vec![
        "Recent Failures (1h)".to_string(),
        recent_failures.to_string(),
        if recent_failures == 0 {
            "🟢 Good"
        } else if recent_failures < 10 {
            "🟡 Moderate"
        } else {
            "🔴 High"
        }
        .to_string(),
    ]);

    health_table.add_row(vec![
        "Long-running Jobs (>1h)".to_string(),
        long_running.to_string(),
        if long_running == 0 {
            "🟢 Good"
        } else if long_running < 5 {
            "🟡 Moderate"
        } else {
            "🔴 High"
        }
        .to_string(),
    ]);

    println!("🏥 Queue Health");
    if let Some(q) = &queue {
        println!("Queue: {}", q);
    }
    println!("{}", health_table);

    Ok(())
}

async fn list_paused_queues(pool: DatabasePool) -> Result<()> {
    let paused = match pool.create_job_queue() {
        JobQueueWrapper::Postgres(q) => q.get_paused_queues().await?,
        JobQueueWrapper::MySQL(q) => q.get_paused_queues().await?,
    };

    if paused.is_empty() {
        println!("✅ No paused queues found - all queues are active");
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Queue Name", "Paused At", "Paused By", "Reason"]);
    for info in paused {
        table.add_row(vec![
            info.queue_name,
            info.paused_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            info.paused_by.unwrap_or_else(|| "Unknown".to_string()),
            info.reason.unwrap_or_else(|| "-".to_string()),
        ]);
    }

    println!("⏸️  Paused Queues");
    println!("{}", table);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::*;

    #[test]
    fn test_queue_total_and_detailed_queries_bind_queue_name() {
        let (sql, binds) = build_queue_total_query(Backend::Postgres, Some(HOSTILE_QUEUE));
        assert_eq!(
            sql,
            "SELECT COUNT(*) as total FROM hammerwork_jobs WHERE queue_name = $1"
        );
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into())]);

        let (sql, binds) = build_queue_total_query(Backend::MySql, None);
        assert_eq!(sql, "SELECT COUNT(*) as total FROM hammerwork_jobs");
        assert!(binds.is_empty());

        let (sql, binds) = build_detailed_stats_query(Backend::MySql, Some(HOSTILE_QUEUE));
        assert!(sql.contains("WHERE queue_name = ? GROUP BY status, priority"));
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into())]);
        assert!(!sql.contains("DROP"));
    }

    #[test]
    fn test_health_queries_bind_queue_then_window() {
        let (sql, binds) = build_health_query(
            Backend::Postgres,
            HealthMetric::RecentFailures,
            Some(HOSTILE_QUEUE),
        );
        assert_eq!(
            sql,
            "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE queue_name = $1 AND failed_at > NOW() - make_interval(hours => $2::int)"
        );
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into()), Bind::Int(1)]);

        let (sql, binds) = build_health_query(
            Backend::MySql,
            HealthMetric::LongRunning,
            Some(HOSTILE_QUEUE),
        );
        assert_eq!(
            sql,
            "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE queue_name = ? AND status = 'Running' AND started_at < DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? HOUR)"
        );
        assert_eq!(binds.len(), 2);

        let (sql, binds) = build_health_query(Backend::Postgres, HealthMetric::Total, None);
        assert_eq!(sql, "SELECT COUNT(*) as count FROM hammerwork_jobs");
        assert!(binds.is_empty());

        let (sql, binds) = build_health_query(Backend::Postgres, HealthMetric::LongRunning, None);
        assert!(sql.contains(
            "WHERE status = 'Running' AND started_at < NOW() - make_interval(hours => $1::int)"
        ));
        assert_eq!(binds, vec![Bind::Int(1)]);
    }

    async fn queue_roundtrip(pool: DatabasePool) {
        let hostile = hostile_queue();
        let other = format!("other_{}", uuid::Uuid::new_v4().simple());
        seed(&pool, &SeedJob::new(&hostile, "Pending")).await;
        let mut failed = SeedJob::new(&hostile, "Failed");
        failed.failed_now = true;
        seed(&pool, &failed).await;
        let mut running = SeedJob::new(&hostile, "Running");
        running.started_long_ago = true;
        seed(&pool, &running).await;
        seed(&pool, &SeedJob::new(&other, "Pending")).await;

        let backend = pool.backend();
        let (sql, binds) = build_queue_total_query(backend, Some(&hostile));
        assert_eq!(fetch_i64(&pool, &sql, &binds, "total").await.unwrap(), 3);

        let (sql, binds) = build_health_query(backend, HealthMetric::Total, Some(&hostile));
        assert_eq!(fetch_i64(&pool, &sql, &binds, "count").await.unwrap(), 3);
        let (sql, binds) =
            build_health_query(backend, HealthMetric::RecentFailures, Some(&hostile));
        assert_eq!(fetch_i64(&pool, &sql, &binds, "count").await.unwrap(), 1);
        let (sql, binds) = build_health_query(backend, HealthMetric::LongRunning, Some(&hostile));
        assert_eq!(fetch_i64(&pool, &sql, &binds, "count").await.unwrap(), 1);

        let (sql, binds) = build_detailed_stats_query(backend, Some(&hostile));
        let statuses = column_strings(&pool, &sql, &binds, "status").await;
        assert_eq!(statuses, vec!["Failed", "Pending", "Running"]);

        show_queue_stats(pool.clone(), Some(hostile.clone()), false)
            .await
            .unwrap();
        show_queue_stats(pool.clone(), Some(hostile.clone()), true)
            .await
            .unwrap();
        show_queue_health(pool.clone(), Some(hostile.clone()))
            .await
            .unwrap();

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_queue_stats_and_health_are_injection_safe_postgres() {
        queue_roundtrip(pg_pool().await).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_queue_stats_and_health_are_injection_safe_mysql() {
        queue_roundtrip(mysql_pool().await).await;
    }

    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: QueueCommand,
    }

    fn parse(args: &[&str]) -> QueueCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        assert!(matches!(
            parse(&["list", "-d", "u"]),
            QueueCommand::List { database_url: Some(u) } if u == "u"
        ));
        match parse(&["stats", "-n", "q", "--detailed"]) {
            QueueCommand::Stats {
                queue, detailed, ..
            } => {
                assert_eq!(queue.as_deref(), Some("q"));
                assert!(detailed);
            }
            _ => panic!("expected Stats"),
        }
        match parse(&["clear", "-n", "q", "--pending-only", "--confirm"]) {
            QueueCommand::Clear {
                queue,
                pending_only,
                confirm,
                ..
            } => assert_eq!((queue.as_str(), pending_only, confirm), ("q", true, true)),
            _ => panic!("expected Clear"),
        }
        assert!(matches!(
            parse(&["pause", "-n", "q"]),
            QueueCommand::Pause { queue, .. } if queue == "q"
        ));
        assert!(matches!(
            parse(&["resume", "--queue", "q"]),
            QueueCommand::Resume { queue, .. } if queue == "q"
        ));
        assert!(matches!(parse(&["paused"]), QueueCommand::Paused { .. }));
        assert!(matches!(
            parse(&["health", "-n", "q"]),
            QueueCommand::Health { queue: Some(q), .. } if q == "q"
        ));
        // clear/pause/resume require a queue
        assert!(TestCli::try_parse_from(["test", "clear"]).is_err());
        assert!(TestCli::try_parse_from(["test", "pause"]).is_err());
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        assert_eq!(
            parse(&["paused"]).get_database_url(&config).unwrap(),
            "postgres://config/db"
        );
        assert_eq!(
            parse(&["health", "-d", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
        assert!(
            parse(&["list"])
                .get_database_url(&Config::default())
                .is_err()
        );
    }

    #[test]
    fn summaries_render_status_and_counts() {
        let rendered = render_queue_summaries(&[
            QueueSummary {
                name: "alpha".into(),
                total: 12,
                pending: 5,
                running: 4,
                completed: 3,
                failed: 2,
                dead: 1,
                paused: false,
            },
            QueueSummary {
                name: "beta".into(),
                total: 0,
                pending: 0,
                running: 0,
                completed: 0,
                failed: 0,
                dead: 0,
                paused: true,
            },
        ]);
        assert!(rendered.contains("alpha") && rendered.contains("▶️ Active"));
        assert!(rendered.contains("beta") && rendered.contains("⏸️ Paused"));
        assert!(rendered.contains("12"));
        for header in [
            "Queue",
            "Status",
            "Pending",
            "Running",
            "Completed",
            "Failed",
            "Dead",
        ] {
            assert!(rendered.contains(header), "{header}");
        }
    }

    async fn queue_commands(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let queue = hostile_queue();
        let empty = unique_queue("paused_empty");
        let run = |cmd: QueueCommand| {
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let q_arg = |queue: &str| queue.to_string();

        for status in ["Pending", "Pending", "Failed", "Completed"] {
            seed(&pool, &SeedJob::new(&queue, status)).await;
        }
        let mut running = SeedJob::new(&queue, "Running");
        running.started_long_ago = true;
        seed(&pool, &running).await;

        // list: counts per queue, and a paused queue without jobs still shows up
        let summaries = fetch_queue_summaries(&pool).await.unwrap();
        let mine = summaries.iter().find(|s| s.name == queue).unwrap();
        assert_eq!(
            (
                mine.total,
                mine.pending,
                mine.running,
                mine.completed,
                mine.failed,
                mine.dead
            ),
            (5, 2, 1, 1, 1, 0)
        );
        assert!(!mine.paused);
        run(QueueCommand::List { database_url: None })
            .await
            .unwrap();

        // pause records who paused it; resume of an active queue is a no-op message
        run(QueueCommand::Pause {
            database_url: None,
            queue: q_arg(&queue),
        })
        .await
        .unwrap();
        run(QueueCommand::Pause {
            database_url: None,
            queue: q_arg(&empty),
        })
        .await
        .unwrap();
        let library = pool.clone().create_job_queue();
        let info = match &library {
            JobQueueWrapper::Postgres(q) => q.get_queue_pause_info(&queue).await.unwrap(),
            JobQueueWrapper::MySQL(q) => q.get_queue_pause_info(&queue).await.unwrap(),
        }
        .expect("queue is paused");
        assert_eq!(info.paused_by.as_deref(), Some("cli"));
        let summaries = fetch_queue_summaries(&pool).await.unwrap();
        assert!(summaries.iter().find(|s| s.name == queue).unwrap().paused);
        let empty_row = summaries.iter().find(|s| s.name == empty).unwrap();
        assert!(empty_row.paused && empty_row.total == 0);
        // pausing twice just refreshes the row
        run(QueueCommand::Pause {
            database_url: None,
            queue: q_arg(&queue),
        })
        .await
        .unwrap();
        run(QueueCommand::Paused { database_url: None })
            .await
            .unwrap();

        run(QueueCommand::Resume {
            database_url: None,
            queue: q_arg(&queue),
        })
        .await
        .unwrap();
        run(QueueCommand::Resume {
            database_url: None,
            queue: q_arg(&queue),
        })
        .await
        .unwrap(); // already active
        let still_paused = match &library {
            JobQueueWrapper::Postgres(q) => q.is_queue_paused(&queue).await.unwrap(),
            JobQueueWrapper::MySQL(q) => q.is_queue_paused(&queue).await.unwrap(),
        };
        assert!(!still_paused);
        run(QueueCommand::Resume {
            database_url: None,
            queue: q_arg(&empty),
        })
        .await
        .unwrap();

        // stats and health, with and without a queue filter
        for (queue_filter, detailed) in [
            (Some(&queue), false),
            (Some(&queue), true),
            (None, false),
            (None, true),
        ] {
            run(QueueCommand::Stats {
                database_url: None,
                queue: queue_filter.cloned(),
                detailed,
            })
            .await
            .unwrap();
        }
        run(QueueCommand::Health {
            database_url: None,
            queue: Some(queue.clone()),
        })
        .await
        .unwrap();
        run(QueueCommand::Health {
            database_url: None,
            queue: None,
        })
        .await
        .unwrap();

        // clear refuses without --confirm, --pending-only keeps other statuses
        let clear = |pending_only: bool, confirm: bool| QueueCommand::Clear {
            database_url: None,
            queue: queue.clone(),
            pending_only,
            confirm,
        };
        run(clear(false, false)).await.unwrap();
        assert_eq!(count_jobs(&pool, &queue, None).await, 5);
        run(clear(true, true)).await.unwrap();
        assert_eq!(count_jobs(&pool, &queue, None).await, 3);
        assert_eq!(count_jobs(&pool, &queue, Some("Pending")).await, 0);
        run(clear(false, true)).await.unwrap();
        assert_eq!(count_jobs(&pool, &queue, None).await, 0);
        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&queue, &empty]).await;
    }

    db_tests!(
        queue_commands,
        test_queue_commands_postgres,
        test_queue_commands_mysql
    );

    #[tokio::test]
    async fn missing_database_url_is_reported_before_connecting() {
        let err = parse(&["list"])
            .execute(&Config::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Database URL is required"));
        let err = parse(&["list", "-d", "sqlite://x"])
            .execute(&Config::default())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Unsupported database URL"),
            "{err}"
        );
    }
}
