use anyhow::Result;
use clap::Subcommand;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::db_helpers::*;

#[derive(Subcommand)]
pub enum MaintenanceCommand {
    #[command(about = "Clean up old completed and failed jobs")]
    Vacuum {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, default_value = "30", help = "Days to keep completed jobs")]
        keep_completed_days: u32,
        #[arg(long, default_value = "7", help = "Days to keep failed jobs")]
        keep_failed_days: u32,
        #[arg(long, help = "Confirm the vacuum operation")]
        confirm: bool,
        #[arg(long, help = "Dry run - show what would be deleted")]
        dry_run: bool,
    },
    #[command(about = "Clean up dead/stale jobs")]
    DeadJobs {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(
            long,
            default_value = "24",
            help = "Hours without update to consider dead"
        )]
        stale_hours: u32,
        #[arg(long, help = "Confirm the cleanup operation")]
        confirm: bool,
        #[arg(long, help = "Dry run - show what would be cleaned")]
        dry_run: bool,
    },
    #[command(about = "Rebuild database indexes for optimal performance")]
    Reindex {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Confirm the reindex operation")]
        confirm: bool,
    },
    #[command(about = "Update database statistics for query optimization")]
    Analyze {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
    },
    #[command(about = "Check database integrity and job consistency")]
    Check {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Fix minor issues automatically")]
        fix: bool,
    },
}

impl MaintenanceCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        let db_url = self.get_database_url(config)?;
        let pool = DatabasePool::connect(&db_url, config.get_connection_pool_size()).await?;

        match self {
            MaintenanceCommand::Vacuum {
                keep_completed_days,
                keep_failed_days,
                confirm,
                dry_run,
                ..
            } => {
                vacuum_jobs(
                    pool,
                    *keep_completed_days,
                    *keep_failed_days,
                    *confirm,
                    *dry_run,
                )
                .await?;
            }
            MaintenanceCommand::DeadJobs {
                stale_hours,
                confirm,
                dry_run,
                ..
            } => {
                cleanup_dead_jobs(pool, *stale_hours, *confirm, *dry_run).await?;
            }
            MaintenanceCommand::Reindex { confirm, .. } => {
                reindex_database(pool, *confirm).await?;
            }
            MaintenanceCommand::Analyze { .. } => {
                analyze_database(pool).await?;
            }
            MaintenanceCommand::Check { fix, .. } => {
                check_database(pool, *fix).await?;
            }
        }
        Ok(())
    }

    fn get_database_url(&self, config: &Config) -> Result<String> {
        let url = match self {
            MaintenanceCommand::Vacuum { database_url, .. } => database_url,
            MaintenanceCommand::DeadJobs { database_url, .. } => database_url,
            MaintenanceCommand::Reindex { database_url, .. } => database_url,
            MaintenanceCommand::Analyze { database_url, .. } => database_url,
            MaintenanceCommand::Check { database_url, .. } => database_url,
        };

        url.as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
            .map(|s| s.to_string())
    }
}

async fn vacuum_jobs(
    pool: DatabasePool,
    keep_completed_days: u32,
    keep_failed_days: u32,
    confirm: bool,
    dry_run: bool,
) -> Result<()> {
    if !confirm && !dry_run {
        println!(
            "⚠️  This will permanently delete old jobs. Use --confirm to proceed or --dry-run to preview."
        );
        return Ok(());
    }

    let completed_cutoff = chrono::Utc::now() - chrono::Duration::days(keep_completed_days as i64);
    let failed_cutoff = chrono::Utc::now() - chrono::Duration::days(keep_failed_days as i64);

    // Count jobs to delete
    let completed_query = format!(
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Completed' AND completed_at < '{}'",
        completed_cutoff.format("%Y-%m-%d %H:%M:%S")
    );
    let failed_query = format!(
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Failed' AND failed_at < '{}'",
        failed_cutoff.format("%Y-%m-%d %H:%M:%S")
    );

    let completed_count = execute_count_query(&pool, &completed_query).await?;
    let failed_count = execute_count_query(&pool, &failed_query).await?;

    println!("🧹 Vacuum Analysis");
    println!("════════════════════");
    println!(
        "Completed jobs older than {} days: {}",
        keep_completed_days, completed_count
    );
    println!(
        "Failed jobs older than {} days: {}",
        keep_failed_days, failed_count
    );
    println!("Total jobs to delete: {}", completed_count + failed_count);

    if dry_run {
        println!("\n💡 This was a dry run. Use --confirm to actually delete these jobs.");
        return Ok(());
    }

    if completed_count == 0 && failed_count == 0 {
        println!("✨ No old jobs found. Database is clean!");
        return Ok(());
    }

    info!("Starting vacuum operation");

    // Delete completed jobs
    if completed_count > 0 {
        let delete_completed_query = format!(
            "DELETE FROM hammerwork_jobs WHERE status = 'Completed' AND completed_at < '{}'",
            completed_cutoff.format("%Y-%m-%d %H:%M:%S")
        );
        execute_update_query(&pool, &delete_completed_query).await?;
        info!("Deleted {} completed jobs", completed_count);
    }

    // Delete failed jobs
    if failed_count > 0 {
        let delete_failed_query = format!(
            "DELETE FROM hammerwork_jobs WHERE status = 'Failed' AND failed_at < '{}'",
            failed_cutoff.format("%Y-%m-%d %H:%M:%S")
        );
        execute_update_query(&pool, &delete_failed_query).await?;
        info!("Deleted {} failed jobs", failed_count);
    }

    println!("✅ Vacuum completed successfully");
    println!("   Deleted {} completed jobs", completed_count);
    println!("   Deleted {} failed jobs", failed_count);

    Ok(())
}

async fn cleanup_dead_jobs(
    pool: DatabasePool,
    stale_hours: u32,
    confirm: bool,
    dry_run: bool,
) -> Result<()> {
    if !confirm && !dry_run {
        println!(
            "⚠️  This will mark stale jobs as dead. Use --confirm to proceed or --dry-run to preview."
        );
        return Ok(());
    }

    let stale_cutoff = chrono::Utc::now() - chrono::Duration::hours(stale_hours as i64);

    // Find stale running jobs
    let query = format!(
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Running' AND started_at < '{}'",
        stale_cutoff.format("%Y-%m-%d %H:%M:%S")
    );
    let stale_count = execute_count_query(&pool, &query).await?;

    println!("🔍 Dead Jobs Analysis");
    println!("═══════════════════════");
    println!(
        "Stale running jobs (no update for {} hours): {}",
        stale_hours, stale_count
    );

    if dry_run {
        println!("\n💡 This was a dry run. Use --confirm to actually mark these jobs as dead.");
        return Ok(());
    }

    if stale_count == 0 {
        println!("✨ No stale jobs found!");
        return Ok(());
    }

    info!("Marking {} stale jobs as dead", stale_count);

    // Mark stale jobs as dead
    let update_query = format!(
        "UPDATE hammerwork_jobs SET status = 'Dead', error_message = 'Job marked as dead due to inactivity' WHERE status = 'Running' AND started_at < '{}'",
        stale_cutoff.format("%Y-%m-%d %H:%M:%S")
    );
    execute_update_query(&pool, &update_query).await?;

    println!("✅ Dead jobs cleanup completed");
    println!("   Marked {} jobs as dead", stale_count);

    Ok(())
}

async fn reindex_database(pool: DatabasePool, confirm: bool) -> Result<()> {
    if !confirm {
        println!("⚠️  This will rebuild database indexes. Use --confirm to proceed.");
        return Ok(());
    }

    info!("Starting database reindex operation");

    match &pool {
        DatabasePool::Postgres(pg_pool) => {
            // PostgreSQL index rebuilding
            println!("🔄 Rebuilding PostgreSQL indexes...");

            sqlx::query("REINDEX INDEX idx_hammerwork_queue_priority")
                .execute(pg_pool)
                .await?;
            sqlx::query("REINDEX INDEX idx_hammerwork_status")
                .execute(pg_pool)
                .await?;
            sqlx::query("REINDEX INDEX idx_hammerwork_scheduled")
                .execute(pg_pool)
                .await?;
            sqlx::query("REINDEX INDEX idx_hammerwork_cron")
                .execute(pg_pool)
                .await?;
            sqlx::query("REINDEX INDEX idx_hammerwork_batch")
                .execute(pg_pool)
                .await?;
            sqlx::query("REINDEX INDEX idx_hammerwork_failed_at")
                .execute(pg_pool)
                .await?;
            sqlx::query("REINDEX INDEX idx_hammerwork_completed_at")
                .execute(pg_pool)
                .await?;

            println!("✅ PostgreSQL indexes rebuilt");
        }
        DatabasePool::MySQL(mysql_pool) => {
            // MySQL doesn't have REINDEX, but we can optimize tables
            println!("🔄 Optimizing MySQL tables...");

            sqlx::query("OPTIMIZE TABLE hammerwork_jobs")
                .execute(mysql_pool)
                .await?;

            println!("✅ MySQL tables optimized");
        }
    }

    info!("Database reindex completed successfully");
    Ok(())
}

async fn analyze_database(pool: DatabasePool) -> Result<()> {
    info!("Analyzing database statistics");

    match &pool {
        DatabasePool::Postgres(pg_pool) => {
            println!("📊 Updating PostgreSQL statistics...");
            sqlx::query("ANALYZE hammerwork_jobs")
                .execute(pg_pool)
                .await?;
            println!("✅ PostgreSQL statistics updated");
        }
        DatabasePool::MySQL(mysql_pool) => {
            println!("📊 Updating MySQL statistics...");
            sqlx::query("ANALYZE TABLE hammerwork_jobs")
                .execute(mysql_pool)
                .await?;
            println!("✅ MySQL statistics updated");
        }
    }

    Ok(())
}

async fn check_database(pool: DatabasePool, fix: bool) -> Result<()> {
    info!("Checking database integrity");

    println!("🔍 Database Integrity Check");
    println!("═══════════════════════════");

    // Check for orphaned jobs
    let orphaned_query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Running' AND started_at IS NULL";
    let orphaned_count = execute_count_query(&pool, orphaned_query).await?;

    println!("Orphaned running jobs (no start time): {}", orphaned_count);

    if fix && orphaned_count > 0 {
        let fix_query = "UPDATE hammerwork_jobs SET status = 'Pending' WHERE status = 'Running' AND started_at IS NULL";
        execute_update_query(&pool, fix_query).await?;
        println!("✅ Fixed {} orphaned jobs", orphaned_count);
    }

    // Check for invalid priorities
    let invalid_priority_query = "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE priority NOT IN ('background', 'low', 'normal', 'high', 'critical')";
    let invalid_priority_count = execute_count_query(&pool, invalid_priority_query).await?;

    println!("Jobs with invalid priority: {}", invalid_priority_count);

    if fix && invalid_priority_count > 0 {
        let fix_priority_query = "UPDATE hammerwork_jobs SET priority = 'normal' WHERE priority NOT IN ('background', 'low', 'normal', 'high', 'critical')";
        execute_update_query(&pool, fix_priority_query).await?;
        println!(
            "✅ Fixed {} jobs with invalid priority",
            invalid_priority_count
        );
    }

    // Check for negative attempts
    let negative_attempts_query =
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE attempts < 0";
    let negative_attempts_count = execute_count_query(&pool, negative_attempts_query).await?;

    println!("Jobs with negative attempts: {}", negative_attempts_count);

    if fix && negative_attempts_count > 0 {
        let fix_attempts_query = "UPDATE hammerwork_jobs SET attempts = 0 WHERE attempts < 0";
        execute_update_query(&pool, fix_attempts_query).await?;
        println!(
            "✅ Fixed {} jobs with negative attempts",
            negative_attempts_count
        );
    }

    let total_issues = orphaned_count + invalid_priority_count + negative_attempts_count;
    if total_issues == 0 {
        println!("\n✨ Database integrity check passed! No issues found.");
    } else if fix {
        println!("\n✅ Database check completed with fixes applied");
    } else {
        println!(
            "\n⚠️  Found {} issues. Use --fix to automatically repair them.",
            total_issues
        );
    }

    Ok(())
}
