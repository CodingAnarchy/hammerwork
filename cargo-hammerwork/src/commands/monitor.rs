use anyhow::Result;
use clap::Subcommand;
use sqlx::Row;
use std::time::Duration;
use tokio::time::interval;

use crate::config::Config;
use crate::utils::database::DatabasePool;

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

async fn display_dashboard_data(pool: &DatabasePool, queue: &Option<String>) -> Result<()> {
    // Quick stats
    let mut stats_table = comfy_table::Table::new();
    stats_table.set_header(vec!["Status", "Count"]);

    let queue_filter = if let Some(q) = queue {
        format!(" WHERE queue_name = '{}'", q)
    } else {
        String::new()
    };

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let query = format!(
                "SELECT status, COUNT(*) as count FROM hammerwork_jobs{} GROUP BY status ORDER BY status",
                queue_filter
            );
            let rows = sqlx::query(&query).fetch_all(pg_pool).await?;

            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_with_icon = match status.as_str() {
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
            let query = format!(
                "SELECT status, COUNT(*) as count FROM hammerwork_jobs{} GROUP BY status ORDER BY status",
                queue_filter
            );
            let rows = sqlx::query(&query).fetch_all(mysql_pool).await?;

            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_with_icon = match status.as_str() {
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
    let recent_query = format!(
        "SELECT id, queue_name, status, priority, created_at FROM hammerwork_jobs{} ORDER BY created_at DESC LIMIT 10",
        queue_filter
    );

    let mut recent_table = comfy_table::Table::new();
    recent_table.set_header(vec!["ID", "Queue", "Status", "Priority", "Created"]);

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = sqlx::query(&recent_query).fetch_all(pg_pool).await?;
            for row in rows {
                let id: uuid::Uuid = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority: String = row.try_get("priority")?;
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
            let rows = sqlx::query(&recent_query).fetch_all(mysql_pool).await?;
            for row in rows {
                let id: String = row.try_get("id")?;
                let queue_name: String = row.try_get("queue_name")?;
                let status: String = row.try_get("status")?;
                let priority: String = row.try_get("priority")?;
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
    let (pg_interval, mysql_interval) = match period_str {
        "1h" => ("1 hour", "1 HOUR"),
        "24h" => ("24 hours", "24 HOUR"),
        "7d" => ("7 days", "7 DAY"),
        _ => ("24 hours", "24 HOUR"),
    };

    println!("📊 Performance Metrics ({})", period_str);
    if let Some(q) = &queue {
        println!("🎯 Queue: {}", q);
    }
    println!("═══════════════════════════════");

    let queue_filter = if let Some(q) = queue {
        format!(" AND queue_name = '{}'", q)
    } else {
        String::new()
    };

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            // Throughput metrics
            let throughput_result = sqlx::query(&format!(
                "SELECT 
                    COUNT(*) as total_jobs,
                    COUNT(CASE WHEN status = 'Completed' THEN 1 END) as completed_jobs,
                    COUNT(CASE WHEN status = 'Failed' THEN 1 END) as failed_jobs
                 FROM hammerwork_jobs 
                 WHERE created_at > NOW() - INTERVAL '{}'{}",
                pg_interval, queue_filter
            ))
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
            let avg_time_result = sqlx::query(&format!(
                "SELECT CAST(AVG(EXTRACT(EPOCH FROM (completed_at - started_at))) AS DOUBLE PRECISION) as avg_duration
                 FROM hammerwork_jobs 
                 WHERE status = 'Completed' AND completed_at > NOW() - INTERVAL '{}'{}",
                pg_interval, queue_filter
            ))
            .fetch_one(&pg_pool)
            .await?;

            if let Some(avg_duration) = avg_time_result.try_get::<Option<f64>, _>("avg_duration")? {
                println!("   Avg Processing Time: {:.1}s", avg_duration);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let throughput_result = sqlx::query(&format!(
                "SELECT 
                    COUNT(*) as total_jobs,
                    COUNT(CASE WHEN status = 'Completed' THEN 1 END) as completed_jobs,
                    COUNT(CASE WHEN status = 'Failed' THEN 1 END) as failed_jobs
                 FROM hammerwork_jobs 
                 WHERE created_at > DATE_SUB(NOW(), INTERVAL {}){}",
                mysql_interval, queue_filter
            ))
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
            let avg_time_result = sqlx::query(&format!(
                "SELECT CAST(AVG(TIMESTAMPDIFF(SECOND, started_at, completed_at)) AS DOUBLE) as avg_duration
                 FROM hammerwork_jobs 
                 WHERE status = 'Completed' AND completed_at > DATE_SUB(NOW(), INTERVAL {}){}",
                mysql_interval, queue_filter
            ))
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
}
