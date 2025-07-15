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
    #[command(about = "List all paused queues")]
    Paused {
        #[arg(short, long, help = "Database connection URL")]
        database_url: Option<String>,
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

async fn list_queues(pool: DatabasePool) -> Result<()> {
    let query = r#"
        SELECT 
            j.queue_name,
            COUNT(j.id) as total_jobs,
            COUNT(CASE WHEN j.status = 'pending' THEN 1 END) as pending,
            COUNT(CASE WHEN j.status = 'running' THEN 1 END) as running,
            COUNT(CASE WHEN j.status = 'completed' THEN 1 END) as completed,
            COUNT(CASE WHEN j.status = 'failed' THEN 1 END) as failed,
            COUNT(CASE WHEN j.status = 'dead' THEN 1 END) as dead,
            CASE WHEN p.queue_name IS NOT NULL THEN true ELSE false END as is_paused
        FROM hammerwork_jobs j
        LEFT JOIN hammerwork_queue_pause p ON j.queue_name = p.queue_name
        GROUP BY j.queue_name, p.queue_name
        ORDER BY j.queue_name
    "#;

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
                let is_paused: bool = row.try_get("is_paused")?;

                let status = if is_paused {
                    "⏸️ Paused"
                } else {
                    "▶️ Active"
                };

                table.add_row(vec![
                    queue_name,
                    status.to_string(),
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
                let is_paused: bool = row.try_get("is_paused")?;

                let status = if is_paused {
                    "⏸️ Paused"
                } else {
                    "▶️ Active"
                };

                table.add_row(vec![
                    queue_name,
                    status.to_string(),
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

async fn pause_queue(pool: DatabasePool, queue: &str) -> Result<()> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let result = sqlx::query(
                r#"
                INSERT INTO hammerwork_queue_pause (queue_name, paused_by, paused_at, created_at, updated_at)
                VALUES ($1, $2, NOW(), NOW(), NOW())
                ON CONFLICT (queue_name) 
                DO UPDATE SET 
                    paused_by = EXCLUDED.paused_by,
                    paused_at = NOW(),
                    updated_at = NOW()
                "#,
            )
            .bind(queue)
            .bind("cli")
            .execute(&pg_pool)
            .await?;

            if result.rows_affected() > 0 {
                println!("⏸️  Queue '{}' has been paused", queue);
                info!("Queue '{}' has been paused via CLI", queue);
            } else {
                println!("⚠️  Failed to pause queue '{}'", queue);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query(
                r#"
                INSERT INTO hammerwork_queue_pause (queue_name, paused_by, paused_at, created_at, updated_at)
                VALUES (?, ?, NOW(), NOW(), NOW())
                ON DUPLICATE KEY UPDATE 
                    paused_by = VALUES(paused_by),
                    paused_at = NOW(),
                    updated_at = NOW()
                "#,
            )
            .bind(queue)
            .bind("cli")
            .execute(&mysql_pool)
            .await?;

            if result.rows_affected() > 0 {
                println!("⏸️  Queue '{}' has been paused", queue);
                info!("Queue '{}' has been paused via CLI", queue);
            } else {
                println!("⚠️  Failed to pause queue '{}'", queue);
            }
        }
    }

    Ok(())
}

async fn resume_queue(pool: DatabasePool, queue: &str) -> Result<()> {
    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let result = sqlx::query("DELETE FROM hammerwork_queue_pause WHERE queue_name = $1")
                .bind(queue)
                .execute(&pg_pool)
                .await?;

            if result.rows_affected() > 0 {
                println!("▶️  Queue '{}' has been resumed", queue);
                info!("Queue '{}' has been resumed via CLI", queue);
            } else {
                println!("ℹ️  Queue '{}' was not paused", queue);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query("DELETE FROM hammerwork_queue_pause WHERE queue_name = ?")
                .bind(queue)
                .execute(&mysql_pool)
                .await?;

            if result.rows_affected() > 0 {
                println!("▶️  Queue '{}' has been resumed", queue);
                info!("Queue '{}' has been resumed via CLI", queue);
            } else {
                println!("ℹ️  Queue '{}' was not paused", queue);
            }
        }
    }

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

async fn list_paused_queues(pool: DatabasePool) -> Result<()> {
    let query = r#"
        SELECT 
            queue_name,
            paused_at,
            paused_by,
            reason
        FROM hammerwork_queue_pause 
        ORDER BY paused_at DESC
    "#;

    let mut table = comfy_table::Table::new();
    table.set_header(vec!["Queue Name", "Paused At", "Paused By", "Reason"]);

    match pool {
        DatabasePool::Postgres(pg_pool) => {
            let rows = sqlx::query(query).fetch_all(&pg_pool).await?;

            if rows.is_empty() {
                println!("✅ No paused queues found - all queues are active");
                return Ok(());
            }

            for row in rows {
                let queue_name: String = row.try_get("queue_name")?;
                let paused_at: chrono::DateTime<chrono::Utc> = row.try_get("paused_at")?;
                let paused_by: Option<String> = row.try_get("paused_by")?;
                let reason: Option<String> = row.try_get("reason")?;

                table.add_row(vec![
                    queue_name,
                    paused_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    paused_by.unwrap_or_else(|| "Unknown".to_string()),
                    reason.unwrap_or_else(|| "-".to_string()),
                ]);
            }
        }
        DatabasePool::MySQL(mysql_pool) => {
            let rows = sqlx::query(query).fetch_all(&mysql_pool).await?;

            if rows.is_empty() {
                println!("✅ No paused queues found - all queues are active");
                return Ok(());
            }

            for row in rows {
                let queue_name: String = row.try_get("queue_name")?;
                let paused_at: chrono::DateTime<chrono::Utc> = row.try_get("paused_at")?;
                let paused_by: Option<String> = row.try_get("paused_by")?;
                let reason: Option<String> = row.try_get("reason")?;

                table.add_row(vec![
                    queue_name,
                    paused_at.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    paused_by.unwrap_or_else(|| "Unknown".to_string()),
                    reason.unwrap_or_else(|| "-".to_string()),
                ]);
            }
        }
    }

    println!("⏸️  Paused Queues");
    println!("{}", table);
    Ok(())
}
