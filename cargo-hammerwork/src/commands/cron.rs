use anyhow::Result;
use clap::Subcommand;
use serde_json::Value;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::db_helpers::*;

#[derive(Subcommand)]
pub enum CronCommand {
    #[command(about = "List scheduled/recurring jobs")]
    List {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name filter")]
        queue: Option<String>,
        #[arg(long, help = "Show only active cron jobs")]
        active_only: bool,
        #[arg(long, help = "Show detailed schedule information")]
        detailed: bool,
    },
    #[command(about = "Create a new cron-scheduled job")]
    Create {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short = 'n', long, help = "Queue name")]
        queue: String,
        #[arg(short = 'j', long, help = "Job payload as JSON")]
        payload: String,
        #[arg(short = 's', long, help = "Cron schedule expression")]
        schedule: String,
        #[arg(short = 'z', long, help = "Timezone (e.g., UTC, America/New_York)")]
        timezone: Option<String>,
        #[arg(short = 'r', long, help = "Job priority")]
        priority: Option<String>,
        #[arg(long, help = "Job description")]
        description: Option<String>,
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
        #[arg(short = 'n', long, help = "Queue name filter")]
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
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

        match self {
            CronCommand::List {
                queue,
                active_only,
                detailed,
                ..
            } => {
                list_cron_jobs(pool, queue.clone(), *active_only, *detailed).await?;
            }
            CronCommand::Create {
                queue,
                payload,
                schedule,
                timezone,
                priority,
                description,
                ..
            } => {
                create_cron_job(
                    &pool,
                    queue,
                    payload,
                    schedule,
                    timezone.clone(),
                    priority.clone(),
                    description.clone(),
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
                show_next_executions(pool, queue.clone(), *count, *hours).await?;
            }
            CronCommand::Update {
                job_id,
                schedule,
                timezone,
                priority,
                ..
            } => {
                update_cron_job(
                    pool,
                    job_id,
                    schedule.clone(),
                    timezone.clone(),
                    priority.clone(),
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
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
            .map(|s| s.to_string())
    }
}

async fn list_cron_jobs(
    pool: DatabasePool,
    queue: Option<String>,
    active_only: bool,
    detailed: bool,
) -> Result<()> {
    println!("📅 Cron Jobs");
    println!("═══════════");

    // Simplified implementation - cron functionality coming soon
    let mut query =
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE cron_schedule IS NOT NULL".to_string();

    if let Some(queue_name) = &queue {
        query = format!("{} AND queue_name = '{}'", query, queue_name);
    }

    if active_only {
        query = format!("{} AND recurring = true", query);
    }

    let count = execute_count_query(&pool, &query).await?;

    if count == 0 {
        println!("📅 No cron jobs found");
        if let Some(q) = queue {
            println!("   Queue filter: {}", q);
        }
        if active_only {
            println!("   Filter: Active only");
        }
    } else {
        println!("Found {} cron jobs", count);
        println!("💡 Detailed cron job listing coming soon!");
        if detailed {
            println!("   Note: Detailed view requested");
        }
    }

    Ok(())
}

async fn create_cron_job(
    pool: &DatabasePool,
    queue: &str,
    payload: &str,
    schedule: &str,
    timezone: Option<String>,
    priority: Option<String>,
    description: Option<String>,
) -> Result<()> {
    // Validate cron expression (basic validation)
    if !is_valid_cron_expression(schedule) {
        return Err(anyhow::anyhow!("Invalid cron expression: {}", schedule));
    }

    // Parse payload JSON
    let payload_json: Value = serde_json::from_str(payload)?;

    // Calculate next run time (simplified - in production you'd use a cron library)
    let next_run_at = chrono::Utc::now() + chrono::Duration::minutes(1); // Placeholder

    let job_id = uuid::Uuid::new_v4().to_string();
    let priority_val = priority.unwrap_or_else(|| "normal".to_string());
    let timezone_val = timezone.unwrap_or_else(|| "UTC".to_string());

    info!("Creating cron job with schedule: {}", schedule);

    match &pool {
        DatabasePool::Postgres(pg_pool) => {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs (
                    id, queue_name, payload, status, priority, attempts, max_attempts,
                    created_at, scheduled_at, cron_schedule, next_run_at, recurring, timezone
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
            )
            .bind(&job_id)
            .bind(queue)
            .bind(&payload_json)
            .bind("Pending")
            .bind(&priority_val)
            .bind(0)
            .bind(3)
            .bind(chrono::Utc::now())
            .bind(next_run_at)
            .bind(schedule)
            .bind(next_run_at)
            .bind(true)
            .bind(&timezone_val)
            .execute(pg_pool)
            .await?;
        }
        DatabasePool::MySQL(mysql_pool) => {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs (
                    id, queue_name, payload, status, priority, attempts, max_attempts,
                    created_at, scheduled_at, cron_schedule, next_run_at, recurring, timezone
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            )
            .bind(&job_id)
            .bind(queue)
            .bind(&payload_json)
            .bind("Pending")
            .bind(&priority_val)
            .bind(0)
            .bind(3)
            .bind(chrono::Utc::now())
            .bind(next_run_at)
            .bind(schedule)
            .bind(next_run_at)
            .bind(true)
            .bind(&timezone_val)
            .execute(mysql_pool)
            .await?;
        }
    }

    println!("✅ Cron job created successfully");
    println!("   Job ID: {}", job_id);
    println!("   Queue: {}", queue);
    println!("   Schedule: {}", schedule);
    println!("   Priority: {}", priority_val);
    println!("   Timezone: {}", timezone_val);
    if let Some(desc) = description {
        println!("   Description: {}", desc);
    }

    info!("Created cron job: {}", job_id);
    Ok(())
}

async fn toggle_cron_job(pool: &DatabasePool, job_id: &str, enable: bool) -> Result<()> {
    let status = if enable { "enabled" } else { "disabled" };

    let updated = match pool {
        DatabasePool::Postgres(pg_pool) => {
            let result = sqlx::query("UPDATE hammerwork_jobs SET recurring = $1 WHERE id = $2 AND cron_schedule IS NOT NULL")
                .bind(enable)
                .bind(job_id)
                .execute(pg_pool)
                .await?;
            result.rows_affected()
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query("UPDATE hammerwork_jobs SET recurring = ? WHERE id = ? AND cron_schedule IS NOT NULL")
                .bind(enable)
                .bind(job_id)
                .execute(mysql_pool)
                .await?;
            result.rows_affected()
        }
    };

    if updated == 0 {
        return Err(anyhow::anyhow!("Cron job not found: {}", job_id));
    }

    println!("✅ Cron job {} successfully", status);
    println!("   Job ID: {}", job_id);

    info!("Cron job {} {}", job_id, status);
    Ok(())
}

async fn show_next_executions(
    pool: DatabasePool,
    queue: Option<String>,
    count: u32,
    hours: Option<u32>,
) -> Result<()> {
    println!("⏰ Upcoming Cron Job Executions");
    println!("═══════════════════════════════");

    // Simplified implementation
    let mut query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE cron_schedule IS NOT NULL AND recurring = true".to_string();

    if let Some(queue_name) = &queue {
        query = format!("{} AND queue_name = '{}'", query, queue_name);
    }

    if let Some(hour_limit) = hours {
        let cutoff = chrono::Utc::now() + chrono::Duration::hours(hour_limit as i64);
        query = format!(
            "{} AND next_run_at <= '{}'",
            query,
            cutoff.format("%Y-%m-%d %H:%M:%S")
        );
    }

    let total_jobs = execute_count_query(&pool, &query).await?;

    if total_jobs == 0 {
        println!("📅 No upcoming cron job executions found");
    } else {
        println!("Found {} active cron jobs", total_jobs);
        println!("💡 Detailed execution schedule display coming soon!");
        println!("   Requested count: {}", count);
        if let Some(h) = hours {
            println!("   Time window: {} hours", h);
        }
    }

    Ok(())
}

async fn update_cron_job(
    pool: DatabasePool,
    job_id: &str,
    schedule: Option<String>,
    timezone: Option<String>,
    priority: Option<String>,
) -> Result<()> {
    // Build update query dynamically
    let mut updates = Vec::new();

    if let Some(new_schedule) = &schedule {
        if !is_valid_cron_expression(new_schedule) {
            return Err(anyhow::anyhow!("Invalid cron expression: {}", new_schedule));
        }
        updates.push("cron_schedule".to_string());
    }

    if timezone.is_some() {
        updates.push("timezone".to_string());
    }

    if priority.is_some() {
        updates.push("priority".to_string());
    }

    if updates.is_empty() {
        return Err(anyhow::anyhow!("No updates specified"));
    }

    // Simplified update - just check if job exists
    let exists_query =
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE id = ? AND cron_schedule IS NOT NULL";
    let exists = execute_count_query_with_param(&pool, exists_query, job_id).await? > 0;

    if !exists {
        return Err(anyhow::anyhow!("Cron job not found: {}", job_id));
    }

    println!("✅ Cron job update would be applied successfully");
    println!("   Job ID: {}", job_id);
    if let Some(s) = schedule {
        println!("   New Schedule: {}", s);
    }
    if let Some(tz) = timezone {
        println!("   New Timezone: {}", tz);
    }
    if let Some(p) = priority {
        println!("   New Priority: {}", p);
    }
    println!("💡 Actual cron job update implementation coming soon!");

    info!("Cron job update requested: {}", job_id);
    Ok(())
}

async fn delete_cron_job(pool: &DatabasePool, job_id: &str, confirm: bool) -> Result<()> {
    if !confirm {
        println!("⚠️  This will permanently delete the cron job. Use --confirm to proceed.");
        return Ok(());
    }

    let deleted = match &pool {
        DatabasePool::Postgres(pg_pool) => {
            let result = sqlx::query(
                "DELETE FROM hammerwork_jobs WHERE id = $1 AND cron_schedule IS NOT NULL",
            )
            .bind(job_id)
            .execute(pg_pool)
            .await?;
            result.rows_affected()
        }
        DatabasePool::MySQL(mysql_pool) => {
            let result = sqlx::query(
                "DELETE FROM hammerwork_jobs WHERE id = ? AND cron_schedule IS NOT NULL",
            )
            .bind(job_id)
            .execute(mysql_pool)
            .await?;
            result.rows_affected()
        }
    };

    if deleted == 0 {
        return Err(anyhow::anyhow!("Cron job not found: {}", job_id));
    }

    println!("✅ Cron job deleted successfully");
    println!("   Job ID: {}", job_id);

    info!("Deleted cron job: {}", job_id);
    Ok(())
}

fn is_valid_cron_expression(expr: &str) -> bool {
    // Basic validation - should have 5 or 6 parts separated by spaces
    let parts: Vec<&str> = expr.split_whitespace().collect();
    parts.len() >= 5 && parts.len() <= 6
}
