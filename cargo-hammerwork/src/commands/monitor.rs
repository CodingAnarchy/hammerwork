use anyhow::Result;
use clap::Subcommand;
use sqlx::Row;
use std::time::Duration;
use tokio::time::interval;

use crate::commands::job::priority_display;
use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::sql::{Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg, fetch_i64};

#[derive(Subcommand)]
pub enum MonitorCommand {
    #[command(about = "Real-time monitoring dashboard")]
    Dashboard {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            short = 'i',
            long,
            default_value = "5",
            help = "Refresh interval in seconds"
        )]
        refresh: u64,
        #[arg(short = 'n', long, help = "Specific queue to monitor")]
        queue: Option<String>,
    },
    #[command(about = "Check system health")]
    Health {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Output format (table, json)")]
        format: Option<String>,
    },
    #[command(about = "Show performance metrics")]
    Metrics {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 't', long, help = "Time period (1h, 24h, 7d)")]
        period: Option<String>,
        #[arg(short = 'n', long, help = "Specific queue to analyze")]
        queue: Option<String>,
    },
}

impl MonitorCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

        match self {
            MonitorCommand::Dashboard { refresh, queue, .. } => {
                run_dashboard(pool, *refresh, queue.clone()).await?;
            }
            MonitorCommand::Health { format, .. } => {
                check_health(pool, format.clone()).await?;
            }
            MonitorCommand::Metrics { period, queue, .. } => {
                show_metrics(pool, period.clone(), queue.clone()).await?;
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url_option = match self {
            MonitorCommand::Dashboard { database_url, .. } => database_url,
            MonitorCommand::Health { database_url, .. } => database_url,
            MonitorCommand::Metrics { database_url, .. } => database_url,
        };

        url_option
            .as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
    }
}

async fn run_dashboard(pool: DatabasePool, refresh_secs: u64, queue: Option<String>) -> Result<()> {
    run_dashboard_until(pool, refresh_secs, queue, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// The dashboard loop; stops when `shutdown` completes (Ctrl+C in the CLI).
async fn run_dashboard_until(
    pool: DatabasePool,
    refresh_secs: u64,
    queue: Option<String>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<()> {
    if refresh_secs == 0 {
        anyhow::bail!("refresh interval must be at least 1 second");
    }
    println!("📊 Hammerwork Dashboard");
    println!("Press Ctrl+C to exit\n");

    let mut interval = interval(Duration::from_secs(refresh_secs));
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Clear screen
                print!("\x1B[2J\x1B[1;1H");

                println!("📊 Hammerwork Dashboard - {}", chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"));
                if let Some(q) = &queue {
                    println!("🎯 Queue: {}", q);
                }
                println!("🔄 Refresh: {}s\n", refresh_secs);

                if let Err(e) = display_dashboard_data(&pool, &queue).await {
                    println!("❌ Error updating dashboard: {}", e);
                }

                println!("\nPress Ctrl+C to exit");
            }
            _ = &mut shutdown => {
                println!("\n👋 Dashboard stopped");
                break;
            }
        }
    }

    Ok(())
}

/// `status, COUNT(*)` grouped by status, optionally for one queue (bound, not interpolated).
pub fn build_status_counts_query(backend: Backend, queue: Option<&str>) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let filter = match queue {
        Some(q) => format!(" WHERE queue_name = {}", params.text(q)),
        None => String::new(),
    };
    (
        format!(
            "SELECT status, COUNT(*) as count FROM hammerwork_jobs{filter} GROUP BY status ORDER BY status"
        ),
        params.into_binds(),
    )
}

/// The ten most recent jobs, optionally for one queue.
pub fn build_recent_jobs_query(backend: Backend, queue: Option<&str>) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let filter = match queue {
        Some(q) => format!(" WHERE queue_name = {}", params.text(q)),
        None => String::new(),
    };
    (
        format!(
            "SELECT id, queue_name, status, priority, created_at FROM hammerwork_jobs{filter} ORDER BY created_at DESC LIMIT 10"
        ),
        params.into_binds(),
    )
}

/// The metrics window in hours for a `--period` value. Unknown values fall back to 24h.
pub fn metrics_period_hours(period: &str) -> u32 {
    match period {
        "1h" => 1,
        "24h" => 24,
        "7d" => 7 * 24,
        _ => 24,
    }
}

/// Job counts by outcome within the last `hours`. The window is evaluated first and the
/// optional queue filter last, matching the bind order.
pub fn build_throughput_query(
    backend: Backend,
    hours: u32,
    queue: Option<&str>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let since = params.ago(hours, IntervalUnit::Hour);
    let queue_filter = match queue {
        Some(q) => format!(" AND queue_name = {}", params.text(q)),
        None => String::new(),
    };
    (
        format!(
            "SELECT COUNT(*) as total_jobs, \
             COUNT(CASE WHEN status = 'Completed' THEN 1 END) as completed_jobs, \
             COUNT(CASE WHEN status = 'Failed' THEN 1 END) as failed_jobs \
             FROM hammerwork_jobs WHERE created_at > {since}{queue_filter}"
        ),
        params.into_binds(),
    )
}

/// Average processing time (seconds) of jobs completed within the last `hours`.
pub fn build_avg_time_query(
    backend: Backend,
    hours: u32,
    queue: Option<&str>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let since = params.ago(hours, IntervalUnit::Hour);
    let queue_filter = match queue {
        Some(q) => format!(" AND queue_name = {}", params.text(q)),
        None => String::new(),
    };
    let duration = match backend {
        Backend::Postgres => {
            "CAST(AVG(EXTRACT(EPOCH FROM (completed_at - started_at))) AS DOUBLE PRECISION)"
        }
        Backend::MySql => "CAST(AVG(TIMESTAMPDIFF(SECOND, started_at, completed_at)) AS DOUBLE)",
    };
    (
        format!(
            "SELECT {duration} as avg_duration FROM hammerwork_jobs \
             WHERE status = 'Completed' AND completed_at > {since}{queue_filter}"
        ),
        params.into_binds(),
    )
}

/// What the dashboard shows.
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardData {
    /// Job count per status.
    pub status_counts: Vec<(String, i64)>,
    pub recent: Vec<RecentJob>,
}

/// One row of the dashboard's "Recent Activity".
#[derive(Debug, Clone, PartialEq)]
pub struct RecentJob {
    pub id: String,
    pub queue: String,
    pub status: String,
    pub priority: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn fetch_dashboard_data(
    pool: &DatabasePool,
    queue: Option<&str>,
) -> Result<DashboardData> {
    let (status_query, status_binds) = build_status_counts_query(pool.backend(), queue);
    let (recent_query, recent_binds) = build_recent_jobs_query(pool.backend(), queue);
    let mut status_counts = Vec::new();
    let mut recent = Vec::new();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            for row in bind_pg(sqlx::query(&status_query), &status_binds)
                .fetch_all(pg_pool)
                .await?
            {
                status_counts.push((row.try_get("status")?, row.try_get("count")?));
            }
            for row in bind_pg(sqlx::query(&recent_query), &recent_binds)
                .fetch_all(pg_pool)
                .await?
            {
                recent.push(RecentJob {
                    id: row.try_get::<uuid::Uuid, _>("id")?.to_string(),
                    queue: row.try_get("queue_name")?,
                    status: row.try_get("status")?,
                    priority: priority_display(row.try_get("priority")?),
                    created_at: row.try_get("created_at")?,
                });
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            for row in bind_mysql(sqlx::query(&status_query), &status_binds)
                .fetch_all(mysql_pool)
                .await?
            {
                status_counts.push((row.try_get("status")?, row.try_get("count")?));
            }
            for row in bind_mysql(sqlx::query(&recent_query), &recent_binds)
                .fetch_all(mysql_pool)
                .await?
            {
                recent.push(RecentJob {
                    id: row.try_get("id")?,
                    queue: row.try_get("queue_name")?,
                    status: row.try_get("status")?,
                    priority: priority_display(row.try_get("priority")?),
                    created_at: row.try_get("created_at")?,
                });
            }
        }
    }
    Ok(DashboardData {
        status_counts,
        recent,
    })
}

fn status_icon(status: &str) -> &'static str {
    match status.to_lowercase().as_str() {
        "pending" => "🟡",
        "running" => "🔵",
        "completed" => "🟢",
        "failed" => "🔴",
        "dead" => "💀",
        _ => "",
    }
}

pub fn render_dashboard(data: &DashboardData) -> String {
    let mut stats_table = comfy_table::Table::new();
    stats_table.set_header(vec!["Status", "Count"]);
    for (status, count) in &data.status_counts {
        let label = match status_icon(status) {
            "" => status.clone(),
            icon => format!("{icon} {status}"),
        };
        stats_table.add_row(vec![label, count.to_string()]);
    }

    let mut recent_table = comfy_table::Table::new();
    recent_table.set_header(vec!["ID", "Queue", "Status", "Priority", "Created"]);
    for job in &data.recent {
        recent_table.add_row(vec![
            job.id[..8.min(job.id.len())].to_string(),
            job.queue.clone(),
            job.status.clone(),
            job.priority.clone(),
            job.created_at.format("%H:%M:%S").to_string(),
        ]);
    }

    format!("📈 Job Status Overview\n{stats_table}\n\n🕐 Recent Activity\n{recent_table}")
}

async fn display_dashboard_data(pool: &DatabasePool, queue: &Option<String>) -> Result<()> {
    let data = fetch_dashboard_data(pool, queue.as_deref()).await?;
    println!("{}", render_dashboard(&data));
    Ok(())
}

/// The counts behind `monitor health`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCount {
    /// Jobs `Running` for more than an hour.
    Stuck,
    /// Jobs created in the last hour.
    RecentJobs,
    /// Jobs that failed in the last hour.
    RecentFailures,
}

/// `COUNT(*) as count` for one health figure; the one-hour window is a bound parameter on
/// the database clock.
pub fn build_health_count_query(backend: Backend, which: HealthCount) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let hour_ago = params.ago(1, IntervalUnit::Hour);
    let condition = match which {
        HealthCount::Stuck => format!("status = 'Running' AND started_at < {hour_ago}"),
        HealthCount::RecentJobs => format!("created_at > {hour_ago}"),
        HealthCount::RecentFailures => format!("status = 'Failed' AND failed_at > {hour_ago}"),
    };
    (
        format!("SELECT COUNT(*) as count FROM hammerwork_jobs WHERE {condition}"),
        params.into_binds(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthLevel {
    Healthy,
    Warning,
    Critical,
}

impl HealthLevel {
    fn icon(self) -> &'static str {
        match self {
            HealthLevel::Healthy => "🟢",
            HealthLevel::Warning => "🟡",
            HealthLevel::Critical => "🔴",
        }
    }

    fn name(self) -> &'static str {
        match self {
            HealthLevel::Healthy => "healthy",
            HealthLevel::Warning => "warning",
            HealthLevel::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheck {
    pub level: HealthLevel,
    pub name: &'static str,
    pub value: String,
}

/// Grade the measurements: any stuck job is a warning (critical from five); the failure rate
/// of the last hour is a warning from 5% and critical from 15%.
pub fn evaluate_health(
    database: std::result::Result<(), String>,
    stuck: i64,
    recent_jobs: i64,
    recent_failures: i64,
) -> Vec<HealthCheck> {
    let database = match database {
        Ok(()) => HealthCheck {
            level: HealthLevel::Healthy,
            name: "Database",
            value: "Connected".to_string(),
        },
        Err(e) => HealthCheck {
            level: HealthLevel::Critical,
            name: "Database",
            value: format!("Connection Failed: {e}"),
        },
    };
    let stuck_check = if stuck == 0 {
        HealthCheck {
            level: HealthLevel::Healthy,
            name: "Stuck Jobs",
            value: "None".to_string(),
        }
    } else {
        HealthCheck {
            level: if stuck < 5 {
                HealthLevel::Warning
            } else {
                HealthLevel::Critical
            },
            name: "Stuck Jobs",
            value: format!("{stuck} jobs"),
        }
    };
    let rate = if recent_jobs > 0 {
        recent_failures as f64 / recent_jobs as f64 * 100.0
    } else {
        0.0
    };
    let failure_check = HealthCheck {
        level: if rate < 5.0 {
            HealthLevel::Healthy
        } else if rate < 15.0 {
            HealthLevel::Warning
        } else {
            HealthLevel::Critical
        },
        name: "Failure Rate",
        value: format!("{rate:.1}%"),
    };
    vec![database, stuck_check, failure_check]
}

/// The report for `format` (`json` or a table).
pub fn render_health(
    checks: &[HealthCheck],
    format: Option<&str>,
    timestamp: chrono::DateTime<chrono::Utc>,
) -> Result<String> {
    match format {
        Some("json") => {
            let json = serde_json::json!({
                "timestamp": timestamp.to_rfc3339(),
                "checks": checks.iter().map(|c| serde_json::json!({
                    "check": c.name,
                    "status": c.level.name(),
                    "value": c.value,
                })).collect::<Vec<_>>()
            });
            Ok(serde_json::to_string_pretty(&json)?)
        }
        _ => {
            let mut table = comfy_table::Table::new();
            table.set_header(vec!["Status", "Check", "Value"]);
            for c in checks {
                table.add_row(vec![c.level.icon(), c.name, &c.value]);
            }
            Ok(format!("🏥 System Health Check\n{table}"))
        }
    }
}

async fn check_health(pool: DatabasePool, format: Option<String>) -> Result<()> {
    let database = match &pool {
        DatabasePool::Postgres(p) => sqlx::query("SELECT 1").fetch_one(p).await.map(|_| ()),
        DatabasePool::MySQL(p) => sqlx::query("SELECT 1").fetch_one(p).await.map(|_| ()),
    }
    .map_err(|e| e.to_string());

    let backend = pool.backend();
    let mut counts = [0i64; 3];
    for (slot, which) in counts.iter_mut().zip([
        HealthCount::Stuck,
        HealthCount::RecentJobs,
        HealthCount::RecentFailures,
    ]) {
        let (sql, binds) = build_health_count_query(backend, which);
        *slot = fetch_i64(&pool, &sql, &binds, "count").await?;
    }
    let [stuck, recent_jobs, recent_failures] = counts;

    let checks = evaluate_health(database, stuck, recent_jobs, recent_failures);
    println!(
        "{}",
        render_health(&checks, format.as_deref(), chrono::Utc::now())?
    );
    Ok(())
}

/// Throughput of a metrics window.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub total: i64,
    pub completed: i64,
    pub failed: i64,
    /// Average seconds from start to completion of the jobs completed in the window.
    pub avg_duration: Option<f64>,
}

pub async fn fetch_metrics(
    pool: &DatabasePool,
    hours: u32,
    queue: Option<&str>,
) -> Result<Metrics> {
    let backend = pool.backend();
    let (throughput_query, throughput_binds) = build_throughput_query(backend, hours, queue);
    let (avg_query, avg_binds) = build_avg_time_query(backend, hours, queue);

    Ok(match pool {
        DatabasePool::Postgres(pg_pool) => {
            let t = bind_pg(sqlx::query(&throughput_query), &throughput_binds)
                .fetch_one(pg_pool)
                .await?;
            let a = bind_pg(sqlx::query(&avg_query), &avg_binds)
                .fetch_one(pg_pool)
                .await?;
            Metrics {
                total: t.try_get("total_jobs")?,
                completed: t.try_get("completed_jobs")?,
                failed: t.try_get("failed_jobs")?,
                avg_duration: a.try_get("avg_duration")?,
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let t = bind_mysql(sqlx::query(&throughput_query), &throughput_binds)
                .fetch_one(mysql_pool)
                .await?;
            let a = bind_mysql(sqlx::query(&avg_query), &avg_binds)
                .fetch_one(mysql_pool)
                .await?;
            Metrics {
                total: t.try_get("total_jobs")?,
                completed: t.try_get("completed_jobs")?,
                failed: t.try_get("failed_jobs")?,
                avg_duration: a.try_get("avg_duration")?,
            }
        }
    })
}

pub fn render_metrics(m: &Metrics) -> String {
    let pct = |n: i64| {
        if m.total > 0 {
            n as f64 / m.total as f64 * 100.0
        } else {
            0.0
        }
    };
    let mut out = format!(
        "📈 Throughput:\n   Total Jobs: {}\n   Completed: {} ({:.1}%)\n   Failed: {} ({:.1}%)",
        m.total,
        m.completed,
        pct(m.completed),
        m.failed,
        pct(m.failed)
    );
    if let Some(avg) = m.avg_duration {
        out.push_str(&format!("\n   Avg Processing Time: {avg:.1}s"));
    }
    out
}

async fn show_metrics(
    pool: DatabasePool,
    period: Option<String>,
    queue: Option<String>,
) -> Result<()> {
    let period_str = period.as_deref().unwrap_or("24h");
    let hours = metrics_period_hours(period_str);

    println!("📊 Performance Metrics ({})", period_str);
    if let Some(q) = &queue {
        println!("🎯 Queue: {}", q);
    }
    println!("═══════════════════════════════");

    let metrics = fetch_metrics(&pool, hours, queue.as_deref()).await?;
    println!("{}", render_metrics(&metrics));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: MonitorCommand,
    }

    #[test]
    fn test_simulated_logs_subcommand_was_removed() {
        assert!(TestCli::try_parse_from(["t", "logs"]).is_err());
        assert!(TestCli::try_parse_from(["t", "health"]).is_ok());
    }

    #[tokio::test]
    async fn test_dashboard_rejects_zero_refresh_interval() {
        // The check runs before the pool is touched, so a lazily connected pool is enough.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://u:p@127.0.0.1:1/none")
            .unwrap();
        let err = run_dashboard(DatabasePool::Postgres(pool), 0, None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least 1 second"));
    }

    use crate::utils::sql::fetch_i64;
    use crate::utils::test_support::*;

    #[test]
    fn test_status_and_recent_queries_bind_queue_name() {
        let (sql, binds) = build_status_counts_query(Backend::Postgres, Some(HOSTILE_QUEUE));
        assert!(sql.contains("WHERE queue_name = $1 GROUP BY status"));
        assert_eq!(binds, vec![Bind::Text(HOSTILE_QUEUE.into())]);
        assert!(!sql.contains("DROP"));

        let (sql, binds) = build_recent_jobs_query(Backend::MySql, Some(HOSTILE_QUEUE));
        assert!(sql.contains("WHERE queue_name = ? ORDER BY"));
        assert_eq!(binds.len(), 1);

        let (sql, binds) = build_recent_jobs_query(Backend::MySql, None);
        assert!(!sql.contains("WHERE") && binds.is_empty());
    }

    #[test]
    fn test_metrics_queries_bind_window_then_queue() {
        let (sql, binds) = build_throughput_query(Backend::Postgres, 24, Some(HOSTILE_QUEUE));
        assert!(
            sql.contains(
                "created_at > NOW() - make_interval(hours => $1::int) AND queue_name = $2"
            )
        );
        assert_eq!(binds, vec![Bind::Int(24), Bind::Text(HOSTILE_QUEUE.into())]);

        let (sql, binds) = build_avg_time_query(Backend::MySql, 1, Some(HOSTILE_QUEUE));
        assert!(sql.contains(
            "completed_at > DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? HOUR) AND queue_name = ?"
        ));
        assert_eq!(binds, vec![Bind::Int(1), Bind::Text(HOSTILE_QUEUE.into())]);

        assert_eq!(metrics_period_hours("1h"), 1);
        assert_eq!(metrics_period_hours("7d"), 168);
        assert_eq!(metrics_period_hours("'; DROP TABLE x --"), 24);
    }

    async fn monitor_roundtrip(pool: DatabasePool) {
        let hostile = hostile_queue();
        let other = format!("other_{}", uuid::Uuid::new_v4().simple());
        seed(&pool, &SeedJob::new(&hostile, "Pending")).await;
        let mut done = SeedJob::new(&hostile, "Completed");
        done.completed_now = true;
        done.started_long_ago = true;
        seed(&pool, &done).await;
        seed(&pool, &SeedJob::new(&other, "Pending")).await;

        let (sql, binds) = build_throughput_query(pool.backend(), 1, Some(&hostile));
        assert_eq!(
            fetch_i64(&pool, &sql, &binds, "total_jobs").await.unwrap(),
            2
        );
        assert_eq!(
            fetch_i64(&pool, &sql, &binds, "completed_jobs")
                .await
                .unwrap(),
            1
        );
        let (sql, binds) = build_status_counts_query(pool.backend(), Some(&hostile));
        let statuses = column_strings(&pool, &sql, &binds, "status").await;
        assert_eq!(statuses, vec!["Completed", "Pending"]);
        let (sql, binds) = build_recent_jobs_query(pool.backend(), Some(&hostile));
        let queues = column_strings(&pool, &sql, &binds, "queue_name").await;
        assert_eq!(queues, vec![hostile.clone(), hostile.clone()]);

        display_dashboard_data(&pool, &Some(hostile.clone()))
            .await
            .unwrap();
        show_metrics(pool.clone(), Some("1h".into()), Some(hostile.clone()))
            .await
            .unwrap();

        assert!(table_exists(&pool).await);
        cleanup(&pool, &[&hostile, &other]).await;
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_monitor_queue_filter_is_injection_safe_postgres() {
        monitor_roundtrip(pg_pool().await).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_monitor_queue_filter_is_injection_safe_mysql() {
        monitor_roundtrip(mysql_pool().await).await;
    }

    fn parse(args: &[&str]) -> MonitorCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&["dashboard"]) {
            MonitorCommand::Dashboard { refresh, queue, .. } => {
                assert_eq!(refresh, 5, "default refresh");
                assert_eq!(queue, None);
            }
            _ => panic!("expected Dashboard"),
        }
        match parse(&["dashboard", "-i", "2", "-n", "q", "-u", "u"]) {
            MonitorCommand::Dashboard {
                refresh,
                queue,
                database_url,
            } => {
                assert_eq!(refresh, 2);
                assert_eq!(queue.as_deref(), Some("q"));
                assert_eq!(database_url.as_deref(), Some("u"));
            }
            _ => panic!("expected Dashboard"),
        }
        match parse(&["health", "--format", "json"]) {
            MonitorCommand::Health { format, .. } => assert_eq!(format.as_deref(), Some("json")),
            _ => panic!("expected Health"),
        }
        match parse(&["metrics", "-t", "7d", "-n", "q"]) {
            MonitorCommand::Metrics { period, queue, .. } => {
                assert_eq!(period.as_deref(), Some("7d"));
                assert_eq!(queue.as_deref(), Some("q"));
            }
            _ => panic!("expected Metrics"),
        }
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        assert_eq!(
            parse(&["health"]).get_database_url(&config).unwrap(),
            "postgres://config/db"
        );
        assert_eq!(
            parse(&["metrics", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
        assert!(
            parse(&["dashboard"])
                .get_database_url(&Config::default())
                .is_err()
        );
    }

    #[test]
    fn health_query_windows_use_the_database_clock() {
        let (sql, binds) = build_health_count_query(Backend::Postgres, HealthCount::Stuck);
        assert_eq!(
            sql,
            "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Running' AND started_at < NOW() - make_interval(hours => $1::int)"
        );
        assert_eq!(binds, vec![Bind::Int(1)]);
        let (sql, _) = build_health_count_query(Backend::MySql, HealthCount::RecentJobs);
        assert!(
            sql.ends_with("created_at > DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? HOUR)"),
            "{sql}"
        );
        let (sql, _) = build_health_count_query(Backend::MySql, HealthCount::RecentFailures);
        assert!(sql.contains("status = 'Failed' AND failed_at > DATE_SUB(UTC_TIMESTAMP(6)"));
    }

    #[test]
    fn health_levels_follow_the_thresholds() {
        let level = |checks: &[HealthCheck], name: &str| {
            checks.iter().find(|c| c.name == name).unwrap().level
        };
        let ok = evaluate_health(Ok(()), 0, 0, 0);
        assert!(ok.iter().all(|c| c.level == HealthLevel::Healthy));
        assert_eq!(ok[1].value, "None");
        assert_eq!(ok[2].value, "0.0%");

        let stuck = evaluate_health(Ok(()), 3, 100, 4);
        assert_eq!(level(&stuck, "Stuck Jobs"), HealthLevel::Warning);
        assert_eq!(level(&stuck, "Failure Rate"), HealthLevel::Healthy);
        assert_eq!(stuck[1].value, "3 jobs");

        let bad = evaluate_health(Ok(()), 5, 100, 5);
        assert_eq!(level(&bad, "Stuck Jobs"), HealthLevel::Critical);
        assert_eq!(level(&bad, "Failure Rate"), HealthLevel::Warning);
        assert_eq!(bad[2].value, "5.0%");

        let worst = evaluate_health(Err("boom".into()), 0, 10, 2);
        assert_eq!(level(&worst, "Database"), HealthLevel::Critical);
        assert_eq!(worst[0].value, "Connection Failed: boom");
        assert_eq!(level(&worst, "Failure Rate"), HealthLevel::Critical);
    }

    #[test]
    fn health_renders_as_a_table_or_json() {
        let checks = evaluate_health(Ok(()), 1, 10, 1);
        let ts = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let table = render_health(&checks, None, ts).unwrap();
        assert!(table.contains("System Health Check"));
        assert!(table.contains("🟢") && table.contains("🟡"));
        assert!(table.contains("Stuck Jobs") && table.contains("10.0%"));

        let json = render_health(&checks, Some("json"), ts).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["timestamp"], "2030-01-02T03:04:05+00:00");
        let list = parsed["checks"].as_array().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["check"], "Database");
        assert_eq!(list[0]["status"], "healthy");
        assert_eq!(list[1]["status"], "warning");
        assert_eq!(list[2]["status"], "warning");
        assert_eq!(list[2]["value"], "10.0%");
    }

    #[test]
    fn dashboard_and_metrics_render_what_they_are_given() {
        let created = chrono::DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let out = render_dashboard(&DashboardData {
            status_counts: vec![("Pending".into(), 3), ("Odd".into(), 1)],
            recent: vec![RecentJob {
                id: "550e8400-e29b-41d4-a716-446655440000".into(),
                queue: "emails".into(),
                status: "Pending".into(),
                priority: "high".into(),
                created_at: created,
            }],
        });
        assert!(out.contains("🟡 Pending") && out.contains("Odd"));
        assert!(!out.contains("❓"), "unknown statuses get no icon");
        assert!(out.contains("550e8400") && !out.contains("550e8400-e29b"));
        assert!(out.contains("emails") && out.contains("03:04:05"));

        let metrics = render_metrics(&Metrics {
            total: 4,
            completed: 3,
            failed: 1,
            avg_duration: Some(2.5),
        });
        assert!(metrics.contains("Total Jobs: 4"));
        assert!(metrics.contains("Completed: 3 (75.0%)"));
        assert!(metrics.contains("Failed: 1 (25.0%)"));
        assert!(metrics.contains("Avg Processing Time: 2.5s"));
        let empty = render_metrics(&Metrics {
            total: 0,
            completed: 0,
            failed: 0,
            avg_duration: None,
        });
        assert!(empty.contains("Completed: 0 (0.0%)") && !empty.contains("Avg"));
    }

    async fn monitor_commands(url: String) {
        let config = config_for(&url);
        let pool = DatabasePool::connect(&url, 2).await.unwrap();
        let queue = unique_queue("monitor");
        let other = unique_queue("monitor_other");

        seed(&pool, &SeedJob::new(&queue, "Pending")).await;
        let mut done = SeedJob::new(&queue, "Completed");
        done.started_long_ago = false;
        done.completed_now = true;
        let done_id = seed(&pool, &done).await;
        let mut failed = SeedJob::new(&queue, "Failed");
        failed.failed_now = true;
        seed(&pool, &failed).await;
        let mut stuck = SeedJob::new(&queue, "Running");
        stuck.started_long_ago = true;
        seed(&pool, &stuck).await;
        seed(&pool, &SeedJob::new(&other, "Pending")).await;
        // The completed job ran for exactly 30 seconds.
        let mut params = SqlParams::new(pool.backend());
        let id = params.uuid(&done_id);
        let sql = match pool.backend() {
            Backend::Postgres => format!(
                "UPDATE hammerwork_jobs SET started_at = completed_at - INTERVAL '30 seconds' WHERE id = {id}"
            ),
            Backend::MySql => format!(
                "UPDATE hammerwork_jobs SET started_at = DATE_SUB(completed_at, INTERVAL 30 SECOND) WHERE id = {id}"
            ),
        };
        crate::utils::sql::execute_binds(&pool, &sql, params.binds())
            .await
            .unwrap();

        // dashboard data is scoped to the queue
        let data = fetch_dashboard_data(&pool, Some(&queue)).await.unwrap();
        let mut counts = data.status_counts.clone();
        counts.sort();
        assert_eq!(
            counts,
            vec![
                ("Completed".to_string(), 1),
                ("Failed".to_string(), 1),
                ("Pending".to_string(), 1),
                ("Running".to_string(), 1),
            ]
        );
        assert_eq!(data.recent.len(), 4);
        assert!(data.recent.iter().all(|j| j.queue == queue));
        assert!(data.recent.iter().all(|j| j.priority == "normal"));
        let all = fetch_dashboard_data(&pool, None).await.unwrap();
        assert!(all.recent.len() <= 10);

        // metrics: counts and the average processing time of the completed job
        let metrics = fetch_metrics(&pool, 1, Some(&queue)).await.unwrap();
        assert_eq!(
            (metrics.total, metrics.completed, metrics.failed),
            (4, 1, 1)
        );
        let avg = metrics.avg_duration.expect("one completed job");
        assert!((avg - 30.0).abs() < 1.0, "avg {avg}");
        assert_eq!(
            fetch_metrics(&pool, 1, Some(&other))
                .await
                .unwrap()
                .avg_duration,
            None
        );

        // health counts: our long-running job is stuck, our failure is recent
        for (which, minimum) in [
            (HealthCount::Stuck, 1),
            (HealthCount::RecentJobs, 5),
            (HealthCount::RecentFailures, 1),
        ] {
            let (sql, binds) = build_health_count_query(pool.backend(), which);
            let n = fetch_i64(&pool, &sql, &binds, "count").await.unwrap();
            assert!(n >= minimum, "{which:?}: {n}");
        }

        // the commands themselves
        for cmd in [
            parse(&["health"]),
            parse(&["health", "--format", "json"]),
            parse(&["metrics", "-n", &queue]),
            parse(&["metrics", "-t", "1h"]),
            parse(&["metrics", "-t", "7d", "-n", &queue]),
        ] {
            cmd.execute(&config).await.unwrap();
        }
        let err = parse(&["dashboard", "-i", "0"])
            .execute(&config)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("at least 1 second"));

        // the dashboard refreshes until told to stop
        run_dashboard_until(
            pool.clone(),
            1,
            Some(queue.clone()),
            tokio::time::sleep(std::time::Duration::from_millis(300)),
        )
        .await
        .unwrap();

        cleanup(&pool, &[&queue, &other]).await;
    }

    db_tests!(
        monitor_commands,
        test_monitor_commands_postgres,
        test_monitor_commands_mysql
    );

    #[tokio::test]
    async fn dashboard_reports_a_database_error_and_keeps_running() {
        // A pool that cannot connect: each refresh prints the error instead of aborting.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(100))
            .connect_lazy("postgres://u:p@127.0.0.1:1/none")
            .unwrap();
        run_dashboard_until(
            DatabasePool::Postgres(pool),
            1,
            None,
            tokio::time::sleep(std::time::Duration::from_millis(250)),
        )
        .await
        .unwrap();
    }
}
