use anyhow::Result;
use clap::Subcommand;
use sqlx::Row;
use tracing::info;

use crate::commands::job::priority_display;
use crate::commands::monitor::build_status_counts_query;
use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::display::StatsTable;
use crate::utils::sql::{Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg, fetch_i64};

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
            COUNT(CASE WHEN j.status = 'Pending' THEN 1 END) as pending,
            COUNT(CASE WHEN j.status = 'Running' THEN 1 END) as running,
            COUNT(CASE WHEN j.status = 'Completed' THEN 1 END) as completed,
            COUNT(CASE WHEN j.status = 'Failed' THEN 1 END) as failed,
            COUNT(CASE WHEN j.status = 'Dead' THEN 1 END) as dead,
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
) -> Result<()> {
    if !confirm {
        println!(
            "⚠️  This will permanently delete jobs from queue '{}'. Use --confirm to proceed.",
            queue
        );
        return Ok(());
    }

    let condition = if pending_only {
        " AND status = 'Pending'"
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
            "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE queue_name = ? AND status = 'Running' AND started_at < DATE_SUB(NOW(), INTERVAL ? HOUR)"
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
}
