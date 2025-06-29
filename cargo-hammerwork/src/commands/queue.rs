use anyhow::Result;
use clap::Subcommand;
use sqlx::Row;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::display::StatsTable;

#[derive(Subcommand)]
pub enum QueueCommand {
    #[command(about = "List all queues")]
    List {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
    },
    #[command(about = "Show queue statistics")]
    Stats {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Specific queue name")]
        queue: Option<String>,
        #[arg(long, help = "Show detailed breakdown by priority")]
        detailed: bool,
    },
    #[command(about = "Clear all jobs from a queue")]
    Clear {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name")]
        queue: String,
        #[arg(long, help = "Only clear pending jobs")]
        pending_only: bool,
        #[arg(long, help = "Confirm the operation")]
        confirm: bool,
    },
    #[command(about = "Pause a queue (prevent job processing)")]
    Pause {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name")]
        queue: String,
    },
    #[command(about = "Resume a paused queue")]
    Resume {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name")]
        queue: String,
    },
    #[command(about = "Get queue health status")]
    Health {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Specific queue name")]
        queue: Option<String>,
    },
}

impl QueueCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

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

async fn list_queues(pool: DatabasePool) -> Result<()> {
    let query = r#"
        SELECT 
            queue_name,
            COUNT(*) as total_jobs,
            COUNT(CASE WHEN status = 'pending' THEN 1 END) as pending,
            COUNT(CASE WHEN status = 'running' THEN 1 END) as running,
            COUNT(CASE WHEN status = 'completed' THEN 1 END) as completed,
            COUNT(CASE WHEN status = 'failed' THEN 1 END) as failed,
            COUNT(CASE WHEN status = 'dead' THEN 1 END) as dead
        FROM hammerwork_jobs 
        GROUP BY queue_name 
        ORDER BY queue_name
    "#;

    let mut table = comfy_table::Table::new();
    table.set_header(vec![
        "Queue",
        "Total",
        "Pending",
        "Running",
        "Completed",
        "Failed",
        "Dead",
    ]);

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = sqlx::query(query).fetch_all(&pg_pool).await?;
            for row in rows {
                let queue_name: String = row.try_get("queue_name")?;
                let total: i64 = row.try_get("total_jobs")?;
                let pending: i64 = row.try_get("pending")?;
                let running: i64 = row.try_get("running")?;
                let completed: i64 = row.try_get("completed")?;
                let failed: i64 = row.try_get("failed")?;
                let dead: i64 = row.try_get("dead")?;

                table.add_row(vec![
                    queue_name,
                    total.to_string(),
                    pending.to_string(),
                    running.to_string(),
                    completed.to_string(),
                    failed.to_string(),
                    dead.to_string(),
                ]);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = sqlx::query(query).fetch_all(&mysql_pool).await?;
            for row in rows {
                let queue_name: String = row.try_get("queue_name")?;
                let total: i64 = row.try_get("total_jobs")?;
                let pending: i64 = row.try_get("pending")?;
                let running: i64 = row.try_get("running")?;
                let completed: i64 = row.try_get("completed")?;
                let failed: i64 = row.try_get("failed")?;
                let dead: i64 = row.try_get("dead")?;

                table.add_row(vec![
                    queue_name,
                    total.to_string(),
                    pending.to_string(),
                    running.to_string(),
                    completed.to_string(),
                    failed.to_string(),
                    dead.to_string(),
                ]);
            }
        }
    }

    println!("📋 Queue Overview");
    println!("{}", table);
    Ok(())
}

async fn show_queue_stats(pool: DatabasePool, queue: Option<String>, detailed: bool) -> Result<()> {
    if detailed {
        show_detailed_stats(pool, queue).await
    } else {
        show_basic_stats(pool, queue).await
    }
}

async fn show_basic_stats(pool: DatabasePool, queue: Option<String>) -> Result<()> {
    let mut base_query = String::from("SELECT status, COUNT(*) as count FROM hammerwork_jobs");

    if let Some(queue_name) = &queue {
        base_query.push_str(&format!(" WHERE queue_name = '{}'", queue_name));
    }

    base_query.push_str(" GROUP BY status ORDER BY status");

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Status", "Count"]);

    match &pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = sqlx::query(&base_query).fetch_all(pg_pool).await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_icon = match status.as_str() {
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
            let rows = sqlx::query(&base_query).fetch_all(mysql_pool).await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let count: i64 = row.try_get("count")?;

                let status_icon = match status.as_str() {
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
    let total_query = if let Some(queue_name) = &queue {
        format!(
            "SELECT COUNT(*) as total FROM hammerwork_jobs WHERE queue_name = '{}'",
            queue_name
        )
    } else {
        "SELECT COUNT(*) as total FROM hammerwork_jobs".to_string()
    };

    match &pool {
        DatabasePool::Postgres(pg_pool) => {
            let total_row = sqlx::query(&total_query).fetch_one(pg_pool).await?;
            let total: i64 = total_row.try_get("total")?;
            println!("\n📈 Total jobs: {}", total);
        }
        DatabasePool::MySQL(mysql_pool) => {
            let total_row = sqlx::query(&total_query).fetch_one(mysql_pool).await?;
            let total: i64 = total_row.try_get("total")?;
            println!("\n📈 Total jobs: {}", total);
        }
    }

    Ok(())
}

async fn show_detailed_stats(pool: DatabasePool, queue: Option<String>) -> Result<()> {
    let mut base_query =
        String::from("SELECT status, priority, COUNT(*) as count FROM hammerwork_jobs");

    if let Some(queue_name) = &queue {
        base_query.push_str(&format!(" WHERE queue_name = '{}'", queue_name));
    }

    base_query.push_str(" GROUP BY status, priority ORDER BY status, priority");

    let mut stats_table = StatsTable::new();

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = sqlx::query(&base_query).fetch_all(&pg_pool).await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let priority: String = row.try_get("priority")?;
                let count: i64 = row.try_get("count")?;

                stats_table.add_stats_row(&status, &priority, count);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = sqlx::query(&base_query).fetch_all(&mysql_pool).await?;
            for row in rows {
                let status: String = row.try_get("status")?;
                let priority: String = row.try_get("priority")?;
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
) -> Result<()> {
    if !confirm {
        println!(
            "⚠️  This will permanently delete jobs from queue '{}'. Use --confirm to proceed.",
            queue
        );
        return Ok(());
    }

    let condition = if pending_only {
        " AND status = 'pending'"
    } else {
        ""
    };

    let affected = match pool {
        DatabasePool::Postgres(ref pg_pool) => {
            let query = format!(
                "DELETE FROM hammerwork_jobs WHERE queue_name = $1{}",
                condition
            );
            let result = sqlx::query(&query).bind(queue).execute(pg_pool).await?;
            result.rows_affected()
        }
        DatabasePool::MySQL(ref mysql_pool) => {
            let query = format!(
                "DELETE FROM hammerwork_jobs WHERE queue_name = ?{}",
                condition
            );
            let result = sqlx::query(&query).bind(queue).execute(mysql_pool).await?;
            result.rows_affected()
        }
    };

    let job_type = if pending_only { "pending" } else { "all" };
    info!(
        "✅ Cleared {} {} jobs from queue '{}'",
        affected, job_type, queue
    );
    Ok(())
}

async fn pause_queue(_pool: DatabasePool, queue: &str) -> Result<()> {
    // Note: This is a placeholder. In a real implementation, you might:
    // 1. Add a queue_paused table or column
    // 2. Update worker logic to respect paused queues
    // 3. Store pause state in Redis or similar

    println!(
        "⏸️  Queue '{}' paused (placeholder - implement pause logic)",
        queue
    );
    info!("Queue '{}' has been paused", queue);
    Ok(())
}

async fn resume_queue(_pool: DatabasePool, queue: &str) -> Result<()> {
    // Note: This is a placeholder. In a real implementation, you would:
    // 1. Remove from paused queues table/column
    // 2. Resume worker processing for this queue

    println!(
        "▶️  Queue '{}' resumed (placeholder - implement resume logic)",
        queue
    );
    info!("Queue '{}' has been resumed", queue);
    Ok(())
}

async fn show_queue_health(pool: DatabasePool, queue: Option<String>) -> Result<()> {
    // Calculate various health metrics
    let mut health_table = comfy_table::Table::new();
    health_table.set_header(vec!["Metric", "Value", "Status"]);

    let queue_filter = if let Some(q) = &queue {
        format!(" WHERE queue_name = '{}'", q)
    } else {
        String::new()
    };

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            // Total jobs
            let total_query = format!(
                "SELECT COUNT(*) as count FROM hammerwork_jobs{}",
                queue_filter
            );
            let total_row = sqlx::query(&total_query).fetch_one(&pg_pool).await?;
            let total_jobs: i64 = total_row.try_get("count")?;

            // Failed jobs in last hour
            let failed_query = format!(
                "SELECT COUNT(*) as count FROM hammerwork_jobs{} {} failed_at > NOW() - INTERVAL '1 hour'",
                queue_filter,
                if queue_filter.is_empty() {
                    "WHERE"
                } else {
                    "AND"
                }
            );
            let failed_row = sqlx::query(&failed_query).fetch_one(&pg_pool).await?;
            let recent_failures: i64 = failed_row.try_get("count")?;

            // Long-running jobs (running > 1 hour)
            let long_running_query = format!(
                "SELECT COUNT(*) as count FROM hammerwork_jobs{} {} status = 'running' AND started_at < NOW() - INTERVAL '1 hour'",
                queue_filter,
                if queue_filter.is_empty() {
                    "WHERE"
                } else {
                    "AND"
                }
            );
            let long_running_row = sqlx::query(&long_running_query).fetch_one(&pg_pool).await?;
            let long_running: i64 = long_running_row.try_get("count")?;

            // Add health metrics
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
        }
        DatabasePool::MySQL(mysql_pool) => {
            // Similar implementation for MySQL with appropriate syntax
            let total_query = format!(
                "SELECT COUNT(*) as count FROM hammerwork_jobs{}",
                queue_filter
            );
            let total_row = sqlx::query(&total_query).fetch_one(&mysql_pool).await?;
            let total_jobs: i64 = total_row.try_get("count")?;

            let failed_query = format!(
                "SELECT COUNT(*) as count FROM hammerwork_jobs{} {} failed_at > DATE_SUB(NOW(), INTERVAL 1 HOUR)",
                queue_filter,
                if queue_filter.is_empty() {
                    "WHERE"
                } else {
                    "AND"
                }
            );
            let failed_row = sqlx::query(&failed_query).fetch_one(&mysql_pool).await?;
            let recent_failures: i64 = failed_row.try_get("count")?;

            let long_running_query = format!(
                "SELECT COUNT(*) as count FROM hammerwork_jobs{} {} status = 'running' AND started_at < DATE_SUB(NOW(), INTERVAL 1 HOUR)",
                queue_filter,
                if queue_filter.is_empty() {
                    "WHERE"
                } else {
                    "AND"
                }
            );
            let long_running_row = sqlx::query(&long_running_query)
                .fetch_one(&mysql_pool)
                .await?;
            let long_running: i64 = long_running_row.try_get("count")?;

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
        }
    }

    println!("🏥 Queue Health");
    if let Some(q) = &queue {
        println!("Queue: {}", q);
    }
    println!("{}", health_table);

    Ok(())
}
