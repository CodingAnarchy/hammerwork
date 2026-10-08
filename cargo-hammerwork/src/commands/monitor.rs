use anyhow::Result;
use clap::Subcommand;
use sqlx::Row;
use std::time::Duration;
use tokio::time::interval;

use crate::commands::job::priority_display;
use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::sql::{Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg};

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
    if refresh_secs == 0 {
        anyhow::bail!("refresh interval must be at least 1 second");
    }
    println!("📊 Hammerwork Dashboard");
    println!("Press Ctrl+C to exit\n");

    let mut interval = interval(Duration::from_secs(refresh_secs));

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
            _ = tokio::signal::ctrl_c() => {
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

async fn display_dashboard_data(pool: &DatabasePool, queue: &Option<String>) -> Result<()> {
    // Quick stats
    let mut stats_table = comfy_table::Table::new();
    stats_table.set_header(vec!["Status", "Count"]);

    let (status_query, status_binds) = build_status_counts_query(pool.backend(), queue.as_deref());

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = bind_pg(sqlx::query(&status_query), &status_binds)
                .fetch_all(pg_pool)
                .await?;

            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_with_icon = match status.to_lowercase().as_str() {
                    "pending" => format!("🟡 {}", status),
                    "running" => format!("🔵 {}", status),
                    "completed" => format!("🟢 {}", status),
                    "failed" => format!("🔴 {}", status),
                    "dead" => format!("💀 {}", status),
                    _ => status,
                };

                stats_table.add_row(vec![status_with_icon, count.to_string()]);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = bind_mysql(sqlx::query(&status_query), &status_binds)
                .fetch_all(mysql_pool)
                .await?;

            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_with_icon = match status.to_lowercase().as_str() {
                    "pending" => format!("🟡 {}", status),
                    "running" => format!("🔵 {}", status),
                    "completed" => format!("🟢 {}", status),
                    "failed" => format!("🔴 {}", status),
                    "dead" => format!("💀 {}", status),
                    _ => status,
                };

                stats_table.add_row(vec![status_with_icon, count.to_string()]);
            }
        }
    }

    println!("📈 Job Status Overview");
    println!("{}", stats_table);

    // Recent activity (last 10 jobs)
    let (recent_query, recent_binds) = build_recent_jobs_query(pool.backend(), queue.as_deref());

    let mut recent_table = comfy_table::Table::new();
    recent_table.set_header(vec!["ID", "Queue", "Status", "Priority", "Created"]);

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = bind_pg(sqlx::query(&recent_query), &recent_binds)
                .fetch_all(pg_pool)
                .await?;
            for row in rows {
                let id: uuid::Uuid = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority = priority_display(row.try_get("priority")?);
                let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;

                recent_table.add_row(vec![
                    &id.to_string()[..8],
                    &queue_name,
                    &status,
                    &priority,
                    &created_at.format("%H:%M:%S").to_string(),
                ]);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = bind_mysql(sqlx::query(&recent_query), &recent_binds)
                .fetch_all(mysql_pool)
                .await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority = priority_display(row.try_get("priority")?);
                let created_at: chrono::DateTime<chrono::Utc> = row.try_get("created_at")?;

                recent_table.add_row(vec![
                    &id[..8],
                    &queue_name,
                    &status,
                    &priority,
                    &created_at.format("%H:%M:%S").to_string(),
                ]);
            }
        }
    }

    println!("\n🕐 Recent Activity");
    println!("{}", recent_table);

    Ok(())
}

async fn check_health(pool: DatabasePool, format: Option<String>) -> Result<()> {
    let mut health_data: Vec<(&str, &str, String)> = Vec::new();

    // Check database connectivity
    let db_status: (&str, &str, String) = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            match sqlx::query("SELECT 1").fetch_one(pg_pool).await {
                Ok(_) => ("🟢", "Database", "Connected".to_string()),
                Err(e) => ("🔴", "Database", format!("Connection Failed: {}", e)),
            }
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            match sqlx::query("SELECT 1").fetch_one(mysql_pool).await {
                Ok(_) => ("🟢", "Database", "Connected".to_string()),
                Err(e) => ("🔴", "Database", format!("Connection Failed: {}", e)),
            }
        }
    };
    health_data.push(db_status);

    // Check for stuck jobs (running > 1 hour)
    let stuck_count = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            let result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'Running' AND started_at < NOW() - INTERVAL '1 hour'",
            )
            .fetch_one(pg_pool)
            .await?;
            result.try_get::<i64, _>("count")?
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'Running' AND started_at < DATE_SUB(NOW(), INTERVAL 1 HOUR)",
            )
            .fetch_one(mysql_pool)
            .await?;
            result.try_get::<i64, _>("count")?
        }
    };

    let stuck_status = if stuck_count == 0 {
        ("🟢", "Stuck Jobs", "None".to_string())
    } else if stuck_count < 5 {
        ("🟡", "Stuck Jobs", format!("{} jobs", stuck_count))
    } else {
        ("🔴", "Stuck Jobs", format!("{} jobs", stuck_count))
    };
    health_data.push(stuck_status);

    // Check failure rate in last hour
    let (total_recent, failed_recent) = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            let total_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE created_at > NOW() - INTERVAL '1 hour'",
            )
            .fetch_one(pg_pool)
            .await?;

            let failed_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'Failed' AND failed_at > NOW() - INTERVAL '1 hour'",
            )
            .fetch_one(pg_pool)
            .await?;

            (
                total_result.try_get::<i64, _>("count")?,
                failed_result.try_get::<i64, _>("count")?,
            )
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let total_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE created_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)",
            )
            .fetch_one(mysql_pool)
            .await?;

            let failed_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'Failed' AND failed_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)",
            )
            .fetch_one(mysql_pool)
            .await?;

            (
                total_result.try_get::<i64, _>("count")?,
                failed_result.try_get::<i64, _>("count")?,
            )
        }
    };

    let failure_rate = if total_recent > 0 {
        (failed_recent as f64 / total_recent as f64) * 100.0
    } else {
        0.0
    };

    let failure_status = if failure_rate < 5.0 {
        ("🟢", "Failure Rate", format!("{:.1}%", failure_rate))
    } else if failure_rate < 15.0 {
        ("🟡", "Failure Rate", format!("{:.1}%", failure_rate))
    } else {
        ("🔴", "Failure Rate", format!("{:.1}%", failure_rate))
    };
    health_data.push(failure_status);

    // Output results
    match format.as_deref() {
        Some("json") => {
            let json_health = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "checks": health_data.iter().map(|(status, check, value)| {
                    serde_json::json!({
                        "check": check,
                        "status": if status.contains("🟢") { "healthy" } else if status.contains("🟡") { "warning" } else { "critical" },
                        "value": value
                    })
                }).collect::<Vec<_>>()
            });
            println!("{}", serde_json::to_string_pretty(&json_health)?);
        }
        _ => {
            let mut table = comfy_table::Table::new();
            table.set_header(vec!["Status", "Check", "Value"]);

            for (status, check, value) in health_data {
                table.add_row(vec![status, check, &value]);
            }

            println!("🏥 System Health Check");
            println!("{}", table);
        }
    }

    Ok(())
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

    let backend = pool.backend();
    let (throughput_query, throughput_binds) =
        build_throughput_query(backend, hours, queue.as_deref());
    let (avg_query, avg_binds) = build_avg_time_query(backend, hours, queue.as_deref());

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            // Throughput metrics
            let throughput_result = bind_pg(sqlx::query(&throughput_query), &throughput_binds)
                .fetch_one(&pg_pool)
                .await?;

            let total: i64 = throughput_result.try_get("total_jobs")?;
            let completed: i64 = throughput_result.try_get("completed_jobs")?;
            let failed: i64 = throughput_result.try_get("failed_jobs")?;

            println!("📈 Throughput:");
            println!("   Total Jobs: {}", total);
            println!(
                "   Completed: {} ({:.1}%)",
                completed,
                if total > 0 {
                    (completed as f64 / total as f64) * 100.0
                } else {
                    0.0
                }
            );
            println!(
                "   Failed: {} ({:.1}%)",
                failed,
                if total > 0 {
                    (failed as f64 / total as f64) * 100.0
                } else {
                    0.0
                }
            );

            // Average processing time for completed jobs
            let avg_time_result = bind_pg(sqlx::query(&avg_query), &avg_binds)
                .fetch_one(&pg_pool)
                .await?;

            if let Some(avg_duration) = avg_time_result.try_get::<Option<f64>, _>("avg_duration")? {
                println!("   Avg Processing Time: {:.1}s", avg_duration);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let throughput_result = bind_mysql(sqlx::query(&throughput_query), &throughput_binds)
                .fetch_one(&mysql_pool)
                .await?;

            let total: i64 = throughput_result.try_get("total_jobs")?;
            let completed: i64 = throughput_result.try_get("completed_jobs")?;
            let failed: i64 = throughput_result.try_get("failed_jobs")?;

            println!("📈 Throughput:");
            println!("   Total Jobs: {}", total);
            println!(
                "   Completed: {} ({:.1}%)",
                completed,
                if total > 0 {
                    (completed as f64 / total as f64) * 100.0
                } else {
                    0.0
                }
            );
            println!(
                "   Failed: {} ({:.1}%)",
                failed,
                if total > 0 {
                    (failed as f64 / total as f64) * 100.0
                } else {
                    0.0
                }
            );

            // Average processing time for completed jobs
            let avg_time_result = bind_mysql(sqlx::query(&avg_query), &avg_binds)
                .fetch_one(&mysql_pool)
                .await?;

            if let Some(avg_duration) = avg_time_result.try_get::<Option<f64>, _>("avg_duration")? {
                println!("   Avg Processing Time: {:.1}s", avg_duration);
            }
        }
    }

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
        assert!(sql.contains("completed_at > DATE_SUB(NOW(), INTERVAL ? HOUR) AND queue_name = ?"));
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
}
