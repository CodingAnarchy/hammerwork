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
        #[arg(short = 'i', long, default_value = "5", help = "Refresh interval in seconds")]
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
    #[command(about = "Tail job logs (placeholder)")]
    Logs {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'l', long, help = "Number of recent log entries")]
        lines: Option<u32>,
        #[arg(short = 'n', long, help = "Filter by queue")]
        queue: Option<String>,
        #[arg(long, help = "Follow logs in real-time")]
        follow: bool,
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
            MonitorCommand::Logs {
                lines,
                queue,
                follow,
                ..
            } => {
                show_logs(pool, *lines, queue.clone(), *follow).await?;
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url_option = match self {
            MonitorCommand::Dashboard { database_url, .. } => database_url,
            MonitorCommand::Health { database_url, .. } => database_url,
            MonitorCommand::Metrics { database_url, .. } => database_url,
            MonitorCommand::Logs { database_url, .. } => database_url,
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
                Err(_) => ("🔴", "Database", "Connection Failed".to_string()),
            }
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            match sqlx::query("SELECT 1").fetch_one(mysql_pool).await {
                Ok(_) => ("🟢", "Database", "Connected".to_string()),
                Err(_) => ("🔴", "Database", "Connection Failed".to_string()),
            }
        }
    };
    health_data.push(db_status);

    // Check for stuck jobs (running > 1 hour)
    let stuck_count = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            let result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'running' AND started_at < NOW() - INTERVAL '1 hour'"
            ).fetch_one(pg_pool).await?;
            result.try_get::<i64, _>("count").unwrap_or(0)
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'running' AND started_at < DATE_SUB(NOW(), INTERVAL 1 HOUR)"
            ).fetch_one(mysql_pool).await?;
            result.try_get::<i64, _>("count").unwrap_or(0)
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
                 WHERE created_at > NOW() - INTERVAL '1 hour'"
            ).fetch_one(pg_pool).await?;
            
            let failed_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'failed' AND failed_at > NOW() - INTERVAL '1 hour'"
            ).fetch_one(pg_pool).await?;
            
            (total_result.try_get::<i64, _>("count").unwrap_or(0), failed_result.try_get::<i64, _>("count").unwrap_or(0))
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let total_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE created_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)"
            ).fetch_one(mysql_pool).await?;
            
            let failed_result = sqlx::query(
                "SELECT COUNT(*) as count FROM hammerwork_jobs 
                 WHERE status = 'failed' AND failed_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)"
            ).fetch_one(mysql_pool).await?;
            
            (total_result.try_get::<i64, _>("count").unwrap_or(0), failed_result.try_get::<i64, _>("count").unwrap_or(0))
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

async fn show_metrics(pool: DatabasePool, period: Option<String>, queue: Option<String>) -> Result<()> {
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
                    COUNT(CASE WHEN status = 'completed' THEN 1 END) as completed_jobs,
                    COUNT(CASE WHEN status = 'failed' THEN 1 END) as failed_jobs
                 FROM hammerwork_jobs 
                 WHERE created_at > NOW() - INTERVAL '{}'{}",
                pg_interval, queue_filter
            )).fetch_one(&pg_pool).await?;

            let total: i64 = throughput_result.try_get("total_jobs")?;
            let completed: i64 = throughput_result.try_get("completed_jobs")?;
            let failed: i64 = throughput_result.try_get("failed_jobs")?;

            println!("📈 Throughput:");
            println!("   Total Jobs: {}", total);
            println!("   Completed: {} ({:.1}%)", completed, if total > 0 { (completed as f64 / total as f64) * 100.0 } else { 0.0 });
            println!("   Failed: {} ({:.1}%)", failed, if total > 0 { (failed as f64 / total as f64) * 100.0 } else { 0.0 });

            // Average processing time for completed jobs
            let avg_time_result = sqlx::query(&format!(
                "SELECT AVG(EXTRACT(EPOCH FROM (completed_at - started_at))) as avg_duration
                 FROM hammerwork_jobs 
                 WHERE status = 'completed' AND completed_at > NOW() - INTERVAL '{}'{}",
                pg_interval, queue_filter
            )).fetch_one(&pg_pool).await?;

            if let Ok(Some(avg_duration)) = avg_time_result.try_get::<Option<f64>, _>("avg_duration") {
                println!("   Avg Processing Time: {:.1}s", avg_duration);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let throughput_result = sqlx::query(&format!(
                "SELECT 
                    COUNT(*) as total_jobs,
                    COUNT(CASE WHEN status = 'completed' THEN 1 END) as completed_jobs,
                    COUNT(CASE WHEN status = 'failed' THEN 1 END) as failed_jobs
                 FROM hammerwork_jobs 
                 WHERE created_at > DATE_SUB(NOW(), INTERVAL {}){}",
                mysql_interval, queue_filter
            )).fetch_one(&mysql_pool).await?;

            let total: i64 = throughput_result.try_get("total_jobs")?;
            let completed: i64 = throughput_result.try_get("completed_jobs")?;
            let failed: i64 = throughput_result.try_get("failed_jobs")?;

            println!("📈 Throughput:");
            println!("   Total Jobs: {}", total);
            println!("   Completed: {} ({:.1}%)", completed, if total > 0 { (completed as f64 / total as f64) * 100.0 } else { 0.0 });
            println!("   Failed: {} ({:.1}%)", failed, if total > 0 { (failed as f64 / total as f64) * 100.0 } else { 0.0 });

            // Average processing time for completed jobs
            let avg_time_result = sqlx::query(&format!(
                "SELECT AVG(TIMESTAMPDIFF(SECOND, started_at, completed_at)) as avg_duration
                 FROM hammerwork_jobs 
                 WHERE status = 'completed' AND completed_at > DATE_SUB(NOW(), INTERVAL {}){}",
                mysql_interval, queue_filter
            )).fetch_one(&mysql_pool).await?;

            if let Ok(Some(avg_duration)) = avg_time_result.try_get::<Option<f64>, _>("avg_duration") {
                println!("   Avg Processing Time: {:.1}s", avg_duration);
            }
        }
    }

    Ok(())
}

async fn show_logs(_pool: DatabasePool, lines: Option<u32>, queue: Option<String>, follow: bool) -> Result<()> {
    let lines_count = lines.unwrap_or(50);
    
    println!("📜 Job Logs (Placeholder)");
    if let Some(q) = &queue {
        println!("🎯 Queue: {}", q);
    }
    println!("📄 Lines: {}", lines_count);
    if follow {
        println!("👁️  Following logs...");
    }
    println!("═══════════════════════════");

    // In a real implementation, this would:
    // 1. Query job events/logs from a logs table
    // 2. Show recent job state changes
    // 3. Display error messages and stack traces
    // 4. Support real-time tailing with --follow

    // Mock log entries
    let mock_logs = vec![
        ("2024-06-28 10:30:15", "INFO", "emails", "Job 12345678 started processing"),
        ("2024-06-28 10:30:14", "INFO", "emails", "Job 87654321 completed successfully"),
        ("2024-06-28 10:30:12", "ERROR", "reports", "Job 11111111 failed: Connection timeout"),
        ("2024-06-28 10:30:10", "INFO", "notifications", "Job 22222222 enqueued"),
        ("2024-06-28 10:30:08", "WARN", "emails", "Job 33333333 retry attempt 2/3"),
    ];

    for (timestamp, level, log_queue, message) in mock_logs.iter().take(lines_count as usize) {
        if let Some(filter_queue) = &queue {
            if log_queue != filter_queue {
                continue;
            }
        }

        let level_icon = match *level {
            "INFO" => "ℹ️",
            "WARN" => "⚠️",
            "ERROR" => "❌",
            _ => "📝",
        };

        println!("{} {} [{}] {}: {}", timestamp, level_icon, level, log_queue, message);
    }

    if follow {
        println!("\n👁️  Following logs (Ctrl+C to stop)...");
        println!("💡 This is a placeholder. Real implementation would stream live logs.");
        
        // Simulate following logs
        tokio::select! {
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(5)) => {
                println!("⏰ Demo timeout - stopping log follow");
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n👋 Log following stopped");
            }
        }
    }

    Ok(())
}