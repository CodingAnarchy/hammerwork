use anyhow::Result;
use clap::Subcommand;
use hammerwork::JobStatus;
use hammerwork::queue::DatabaseQueue;
use tracing::info;

use crate::config::Config;
use crate::utils::database::{DatabasePool, JobQueueWrapper};
use crate::utils::job_ops::{JobSelector, retry_many, select_job_ids};
use crate::utils::sql::{Backend, Bind, SqlParams, execute_binds, fetch_i64};

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
    #[command(
        about = "Delete finished encrypted jobs whose retention period has ended",
        long_about = "Enforce the retention policies of encrypted jobs. Deletes jobs with an \
            encrypted payload whose retention_delete_at has passed and that are finished \
            (Completed, Failed, Dead or TimedOut), and archived encrypted jobs past their \
            retention time. Pending and running jobs are never deleted. No encryption key \
            is needed. Run it periodically, e.g. from cron.\n\n\
            Example: cargo hammerwork maintenance purge-encrypted --confirm"
    )]
    PurgeEncrypted {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(long, help = "Confirm the purge")]
        confirm: bool,
        #[arg(long, help = "Dry run - count the jobs that would be deleted")]
        dry_run: bool,
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
        let pool = DatabasePool::connect_with_config(&db_url, config).await?;

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
            MaintenanceCommand::PurgeEncrypted {
                confirm, dry_run, ..
            } => {
                purge_expired_encrypted_jobs(pool, *confirm, *dry_run).await?;
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
            MaintenanceCommand::PurgeEncrypted { database_url, .. } => database_url,
        };

        url.as_ref()
            .map(|s| s.as_str())
            .or(config.get_database_url())
            .ok_or_else(|| anyhow::anyhow!("Database URL is required"))
            .map(|s| s.to_string())
    }
}

/// Queries counting the jobs `purge-encrypted` would delete, from `hammerwork_jobs` and
/// `hammerwork_jobs_archive`. Each takes the current time as its only bind parameter
/// (`placeholder`); they mirror `DatabaseQueue::purge_expired_encrypted_jobs`.
fn expired_encrypted_count_queries(placeholder: &str) -> (String, String) {
    (
        format!(
            "SELECT COUNT(*) AS count FROM hammerwork_jobs WHERE is_encrypted = true \
             AND retention_delete_at IS NOT NULL AND retention_delete_at <= {placeholder} \
             AND status IN ('Completed', 'Failed', 'Dead', 'TimedOut')"
        ),
        format!(
            "SELECT COUNT(*) AS count FROM hammerwork_jobs_archive WHERE is_encrypted = true \
             AND retention_delete_at IS NOT NULL AND retention_delete_at <= {placeholder}"
        ),
    )
}

async fn count_expired_encrypted_jobs(pool: &DatabasePool) -> Result<(i64, i64)> {
    let now = chrono::Utc::now();
    Ok(match pool {
        DatabasePool::Postgres(pg) => {
            let (jobs, archived) = expired_encrypted_count_queries("$1");
            (
                sqlx::query_scalar(&jobs).bind(now).fetch_one(pg).await?,
                sqlx::query_scalar(&archived)
                    .bind(now)
                    .fetch_one(pg)
                    .await?,
            )
        }
        DatabasePool::MySQL(my) => {
            let (jobs, archived) = expired_encrypted_count_queries("?");
            (
                sqlx::query_scalar(&jobs).bind(now).fetch_one(my).await?,
                sqlx::query_scalar(&archived)
                    .bind(now)
                    .fetch_one(my)
                    .await?,
            )
        }
    })
}

async fn purge_expired_encrypted_jobs(
    pool: DatabasePool,
    confirm: bool,
    dry_run: bool,
) -> Result<()> {
    if dry_run {
        let (jobs, archived) = count_expired_encrypted_jobs(&pool).await?;
        println!("🔐 Encrypted jobs past their retention period");
        println!("Jobs: {}", jobs);
        println!("Archived jobs: {}", archived);
        println!("\n💡 This was a dry run. Use --confirm to delete these jobs.");
        return Ok(());
    }
    if !confirm {
        println!(
            "⚠️  This permanently deletes encrypted jobs whose retention period has ended. \
             Use --confirm to proceed or --dry-run to preview."
        );
        return Ok(());
    }

    let purge = match pool.create_job_queue() {
        JobQueueWrapper::Postgres(queue) => queue.purge_expired_encrypted_jobs().await?,
        JobQueueWrapper::MySQL(queue) => queue.purge_expired_encrypted_jobs().await?,
    };
    println!(
        "✅ Deleted {} encrypted jobs past their retention period ({} jobs, {} archived)",
        purge.total(),
        purge.jobs,
        purge.archived_jobs
    );
    Ok(())
}

/// Which kind of old job a vacuum statement targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VacuumKind {
    Completed,
    Failed,
}

/// The count (`delete == false`) or delete statement for jobs of `kind` older than `cutoff`.
/// The cutoff is bound as a timestamp rather than formatted into the SQL text.
pub fn build_vacuum_query(
    backend: Backend,
    kind: VacuumKind,
    delete: bool,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> (String, Vec<Bind>) {
    let mut params = SqlParams::new(backend);
    let (status, column) = match kind {
        VacuumKind::Completed => ("Completed", "completed_at"),
        VacuumKind::Failed => ("Failed", "failed_at"),
    };
    let verb = if delete {
        "DELETE FROM hammerwork_jobs"
    } else {
        "SELECT COUNT(*) as count FROM hammerwork_jobs"
    };
    let cutoff = params.time(cutoff);
    (
        format!("{verb} WHERE status = '{status}' AND {column} < {cutoff}"),
        params.into_binds(),
    )
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
    let backend = pool.backend();
    let (completed_query, completed_binds) =
        build_vacuum_query(backend, VacuumKind::Completed, false, completed_cutoff);
    let (failed_query, failed_binds) =
        build_vacuum_query(backend, VacuumKind::Failed, false, failed_cutoff);

    let completed_count = fetch_i64(&pool, &completed_query, &completed_binds, "count").await?;
    let failed_count = fetch_i64(&pool, &failed_query, &failed_binds, "count").await?;

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
        let (delete_query, delete_binds) =
            build_vacuum_query(backend, VacuumKind::Completed, true, completed_cutoff);
        execute_binds(&pool, &delete_query, &delete_binds).await?;
        info!("Deleted {} completed jobs", completed_count);
    }

    // Delete failed jobs
    if failed_count > 0 {
        let (delete_query, delete_binds) =
            build_vacuum_query(backend, VacuumKind::Failed, true, failed_cutoff);
        execute_binds(&pool, &delete_query, &delete_binds).await?;
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
    let selector = JobSelector {
        statuses: vec![JobStatus::Running],
        started_before: Some(stale_cutoff),
        ..Default::default()
    };
    let stale_ids = select_job_ids(&pool, &selector).await?;
    let stale_count = stale_ids.len();

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

    // Mark stale jobs as dead through the library's guarded transition.
    const REASON: &str = "Job marked as dead due to inactivity";
    let mut marked = 0usize;
    match pool.clone().create_job_queue() {
        JobQueueWrapper::Postgres(queue) => {
            for id in &stale_ids {
                match queue.mark_job_dead(*id, REASON).await {
                    Ok(()) => marked += 1,
                    Err(e) => tracing::warn!("Skipped job {}: {}", id, e),
                }
            }
        }
        JobQueueWrapper::MySQL(queue) => {
            for id in &stale_ids {
                match queue.mark_job_dead(*id, REASON).await {
                    Ok(()) => marked += 1,
                    Err(e) => tracing::warn!("Skipped job {}: {}", id, e),
                }
            }
        }
    }

    println!("✅ Dead jobs cleanup completed");
    println!("   Marked {} jobs as dead", marked);

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
            // PostgreSQL index rebuilding: every Hammerwork table in the current schema
            println!("🔄 Rebuilding PostgreSQL indexes...");

            let tables: Vec<String> = sqlx::query_scalar(
                "SELECT tablename::text FROM pg_tables \
                 WHERE schemaname = current_schema() AND tablename LIKE 'hammerwork\\_%' \
                 ORDER BY tablename",
            )
            .fetch_all(pg_pool)
            .await?;
            for table in &tables {
                // Names come from the catalog; quote them anyway.
                sqlx::query(&format!("REINDEX TABLE \"{}\"", table.replace('"', "\"\"")))
                    .execute(pg_pool)
                    .await?;
            }

            println!("✅ PostgreSQL indexes rebuilt ({} tables)", tables.len());
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
    let orphaned_count = fetch_i64(&pool, orphaned_query, &[], "count").await?;

    println!("Orphaned running jobs (no start time): {}", orphaned_count);

    if fix && orphaned_count > 0 {
        let selector = JobSelector {
            statuses: vec![JobStatus::Running],
            never_started: true,
            ..Default::default()
        };
        let ids = select_job_ids(&pool, &selector).await?;
        let wrapper = pool.clone().create_job_queue();
        let result = retry_many(&wrapper, &ids).await;
        println!("✅ Fixed {} orphaned jobs", result.succeeded);
    }

    // Check for invalid priorities
    let invalid_priority_query =
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE priority NOT BETWEEN 0 AND 4";
    let invalid_priority_count = fetch_i64(&pool, invalid_priority_query, &[], "count").await?;

    println!("Jobs with invalid priority: {}", invalid_priority_count);

    if fix && invalid_priority_count > 0 {
        let fix_priority_query =
            "UPDATE hammerwork_jobs SET priority = 2 WHERE priority NOT BETWEEN 0 AND 4";
        execute_binds(&pool, fix_priority_query, &[]).await?;
        println!(
            "✅ Fixed {} jobs with invalid priority",
            invalid_priority_count
        );
    }

    // Check for negative attempts
    let negative_attempts_query =
        "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE attempts < 0";
    let negative_attempts_count = fetch_i64(&pool, negative_attempts_query, &[], "count").await?;

    println!("Jobs with negative attempts: {}", negative_attempts_count);

    if fix && negative_attempts_count > 0 {
        let fix_attempts_query = "UPDATE hammerwork_jobs SET attempts = 0 WHERE attempts < 0";
        execute_binds(&pool, fix_attempts_query, &[]).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: MaintenanceCommand,
    }

    #[test]
    fn test_purge_encrypted_arguments() {
        let cli = TestCli::try_parse_from(["test", "purge-encrypted"]).unwrap();
        match cli.command {
            MaintenanceCommand::PurgeEncrypted {
                database_url,
                confirm,
                dry_run,
            } => {
                assert!(database_url.is_none());
                assert!(!confirm);
                assert!(!dry_run);
            }
            _ => panic!("expected PurgeEncrypted"),
        }

        let cli = TestCli::try_parse_from([
            "test",
            "purge-encrypted",
            "--confirm",
            "-u",
            "mysql://localhost/db",
        ])
        .unwrap();
        match cli.command {
            MaintenanceCommand::PurgeEncrypted {
                database_url,
                confirm,
                dry_run,
            } => {
                assert_eq!(database_url.as_deref(), Some("mysql://localhost/db"));
                assert!(confirm);
                assert!(!dry_run);
            }
            _ => panic!("expected PurgeEncrypted"),
        }
    }

    #[test]
    fn test_expired_encrypted_count_queries() {
        let (jobs, archived) = expired_encrypted_count_queries("$1");
        assert!(jobs.contains("FROM hammerwork_jobs WHERE is_encrypted = true"));
        assert!(jobs.contains("retention_delete_at <= $1"));
        // Pending and running jobs are never purged
        assert!(jobs.contains("status IN ('Completed', 'Failed', 'Dead', 'TimedOut')"));
        assert!(!jobs.contains("Pending"));
        assert!(archived.contains("FROM hammerwork_jobs_archive WHERE is_encrypted = true"));
        assert!(archived.contains("retention_delete_at <= $1"));

        let (jobs, archived) = expired_encrypted_count_queries("?");
        assert!(jobs.contains("retention_delete_at <= ?"));
        assert!(archived.contains("retention_delete_at <= ?"));
    }

    use crate::utils::test_support::*;

    #[test]
    fn test_vacuum_query_binds_cutoff() {
        let cutoff = chrono::Utc::now();
        let (sql, binds) =
            build_vacuum_query(Backend::Postgres, VacuumKind::Completed, false, cutoff);
        assert_eq!(
            sql,
            "SELECT COUNT(*) as count FROM hammerwork_jobs WHERE status = 'Completed' AND completed_at < $1"
        );
        assert_eq!(binds, vec![Bind::Time(cutoff)]);

        let (sql, binds) = build_vacuum_query(Backend::MySql, VacuumKind::Failed, true, cutoff);
        assert_eq!(
            sql,
            "DELETE FROM hammerwork_jobs WHERE status = 'Failed' AND failed_at < ?"
        );
        assert_eq!(binds, vec![Bind::Time(cutoff)]);
    }

    async fn vacuum_roundtrip(pool: DatabasePool) {
        let queue = hostile_queue();
        let mut done = SeedJob::new(&queue, "Completed");
        done.completed_now = true;
        seed(&pool, &done).await;
        let mut failed = SeedJob::new(&queue, "Failed");
        failed.failed_now = true;
        seed(&pool, &failed).await;

        let backend = pool.backend();
        let future = chrono::Utc::now() + chrono::Duration::hours(1);
        let (sql, binds) = build_vacuum_query(backend, VacuumKind::Completed, false, future);
        assert!(fetch_i64(&pool, &sql, &binds, "count").await.unwrap() >= 1);
        let (sql, binds) = build_vacuum_query(backend, VacuumKind::Failed, false, future);
        assert!(fetch_i64(&pool, &sql, &binds, "count").await.unwrap() >= 1);

        // A cutoff in the distant past executes the DELETE without touching any rows
        let past = chrono::Utc::now() - chrono::Duration::days(365 * 20);
        let (sql, binds) = build_vacuum_query(backend, VacuumKind::Completed, true, past);
        assert_eq!(execute_binds(&pool, &sql, &binds).await.unwrap(), 0);
        let (sql, binds) = build_vacuum_query(backend, VacuumKind::Failed, true, past);
        assert_eq!(execute_binds(&pool, &sql, &binds).await.unwrap(), 0);

        cleanup(&pool, &[&queue]).await;
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn test_vacuum_cutoffs_are_bound_postgres() {
        vacuum_roundtrip(pg_pool().await).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn test_vacuum_cutoffs_are_bound_mysql() {
        vacuum_roundtrip(mysql_pool().await).await;
    }

    fn parse(args: &[&str]) -> MaintenanceCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_every_subcommand_with_its_flags() {
        match parse(&["vacuum"]) {
            MaintenanceCommand::Vacuum {
                keep_completed_days,
                keep_failed_days,
                confirm,
                dry_run,
                ..
            } => {
                assert_eq!((keep_completed_days, keep_failed_days), (30, 7));
                assert!(!confirm && !dry_run);
            }
            _ => panic!("expected Vacuum"),
        }
        match parse(&[
            "vacuum",
            "--keep-completed-days",
            "3",
            "--keep-failed-days",
            "1",
            "--confirm",
            "--dry-run",
        ]) {
            MaintenanceCommand::Vacuum {
                keep_completed_days,
                keep_failed_days,
                confirm,
                dry_run,
                ..
            } => {
                assert_eq!((keep_completed_days, keep_failed_days), (3, 1));
                assert!(confirm && dry_run);
            }
            _ => panic!("expected Vacuum"),
        }
        match parse(&["dead-jobs", "--stale-hours", "2", "--dry-run"]) {
            MaintenanceCommand::DeadJobs {
                stale_hours,
                confirm,
                dry_run,
                ..
            } => assert_eq!((stale_hours, confirm, dry_run), (2, false, true)),
            _ => panic!("expected DeadJobs"),
        }
        assert!(matches!(
            parse(&["dead-jobs"]),
            MaintenanceCommand::DeadJobs {
                stale_hours: 24,
                ..
            }
        ));
        assert!(matches!(
            parse(&["reindex", "--confirm"]),
            MaintenanceCommand::Reindex { confirm: true, .. }
        ));
        assert!(matches!(
            parse(&["analyze", "-u", "u"]),
            MaintenanceCommand::Analyze {
                database_url: Some(_)
            }
        ));
        assert!(matches!(
            parse(&["check", "--fix"]),
            MaintenanceCommand::Check { fix: true, .. }
        ));
    }

    #[test]
    fn database_url_comes_from_the_flag_then_the_config() {
        let config = config_for("postgres://config/db");
        for args in [
            &["vacuum"][..],
            &["dead-jobs"],
            &["reindex"],
            &["analyze"],
            &["check"],
            &["purge-encrypted"],
        ] {
            assert_eq!(
                parse(args).get_database_url(&config).unwrap(),
                "postgres://config/db"
            );
            assert!(parse(args).get_database_url(&Config::default()).is_err());
        }
        assert_eq!(
            parse(&["check", "-u", "mysql://flag/db"])
                .get_database_url(&config)
                .unwrap(),
            "mysql://flag/db"
        );
    }

    async fn maintenance_commands(base_url: String) {
        // These commands act on every queue, so they get a database of their own.
        let db = ScratchDb::create(&base_url).await;
        let pool = &db.pool;
        let config = db.config();
        let run = |args: &[&str]| {
            let cmd = parse(args);
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };
        let q = "maint";

        // --- vacuum: only jobs older than the keep window go
        let mut completed = SeedJob::new(q, "Completed");
        completed.completed_now = true;
        let old_completed = seed(pool, &completed).await;
        backdate(pool, &old_completed, "completed_at", 40).await;
        let new_completed = seed(pool, &completed).await;
        let mut failed = SeedJob::new(q, "Failed");
        failed.failed_now = true;
        let old_failed = seed(pool, &failed).await;
        backdate(pool, &old_failed, "failed_at", 10).await;
        let new_failed = seed(pool, &failed).await;
        let pending = seed(pool, &SeedJob::new(q, "Pending")).await;
        let remaining = || async { count_jobs(pool, q, None).await };
        assert_eq!(remaining().await, 5);

        run(&["vacuum"]).await.unwrap(); // refuses without --confirm
        run(&["vacuum", "--dry-run"]).await.unwrap();
        assert_eq!(remaining().await, 5, "neither deletes anything");
        run(&["vacuum", "--confirm"]).await.unwrap();
        assert_eq!(remaining().await, 3);
        for kept in [&new_completed, &new_failed, &pending] {
            assert!(job_column(pool, kept, "status").await.is_some());
        }
        for gone in [&old_completed, &old_failed] {
            assert!(job_column(pool, gone, "status").await.is_none());
        }
        run(&["vacuum", "--confirm"]).await.unwrap(); // nothing left to do
        // shorter windows catch the recent ones too
        run(&[
            "vacuum",
            "--confirm",
            "--keep-completed-days",
            "0",
            "--keep-failed-days",
            "0",
        ])
        .await
        .unwrap();
        assert_eq!(count_jobs(pool, q, Some("Pending")).await, 1);
        assert_eq!(remaining().await, 1, "pending jobs are never vacuumed");

        // --- dead-jobs: stale Running jobs are marked Dead, fresh ones are left alone
        let mut stale = SeedJob::new(q, "Running");
        stale.started_long_ago = true;
        let stale_id = seed(pool, &stale).await;
        let fresh_id = seed(pool, &SeedJob::new(q, "Running")).await;
        backdate(pool, &fresh_id, "started_at", 0).await;
        run(&["dead-jobs"]).await.unwrap(); // needs --confirm
        run(&["dead-jobs", "--dry-run", "--stale-hours", "1"])
            .await
            .unwrap();
        assert_eq!(job_status(pool, &stale_id).await, "Running");
        run(&["dead-jobs", "--confirm", "--stale-hours", "1"])
            .await
            .unwrap();
        assert_eq!(job_status(pool, &stale_id).await, "Dead");
        assert_eq!(job_status(pool, &fresh_id).await, "Running");
        assert!(
            job_column(pool, &stale_id, "error_message")
                .await
                .unwrap_or_default()
                .contains("inactivity")
        );
        run(&["dead-jobs", "--confirm", "--stale-hours", "1"])
            .await
            .unwrap(); // none left

        // --- check: reports, and --fix repairs minor corruption
        let orphan = seed(pool, &SeedJob::new(q, "Running")).await; // Running, never started
        let bad_priority = seed(pool, &SeedJob::new(q, "Pending")).await;
        exec_sql(
            pool,
            &format!("UPDATE hammerwork_jobs SET priority = 9 WHERE id = '{bad_priority}'"),
        )
        .await;
        let bad_attempts = seed(pool, &SeedJob::new(q, "Pending")).await;
        exec_sql(
            pool,
            &format!("UPDATE hammerwork_jobs SET attempts = -2 WHERE id = '{bad_attempts}'"),
        )
        .await;
        run(&["check"]).await.unwrap();
        assert_eq!(job_status(pool, &orphan).await, "Running");
        assert_eq!(
            job_column(pool, &bad_priority, "priority").await.as_deref(),
            Some("9")
        );
        run(&["check", "--fix"]).await.unwrap();
        assert_eq!(
            job_column(pool, &bad_priority, "priority").await.as_deref(),
            Some("2")
        );
        assert_eq!(
            job_column(pool, &bad_attempts, "attempts").await.as_deref(),
            Some("0")
        );
        assert_ne!(
            job_status(pool, &orphan).await,
            "Running",
            "the orphaned job is released"
        );
        run(&["check"]).await.unwrap(); // clean now

        // --- reindex / analyze
        run(&["reindex"]).await.unwrap(); // needs --confirm
        run(&["reindex", "--confirm"]).await.unwrap();
        run(&["analyze"]).await.unwrap();

        // --- purge-encrypted: only finished jobs past their retention are deleted
        let expired = seed(pool, &SeedJob::new(q, "Completed")).await;
        let not_yet = seed(pool, &SeedJob::new(q, "Completed")).await;
        let running = seed(pool, &SeedJob::new(q, "Running")).await;
        let (past, future) = match pool.backend() {
            Backend::Postgres => ("NOW() - INTERVAL '1 hour'", "NOW() + INTERVAL '1 hour'"),
            Backend::MySql => (
                "DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 HOUR)",
                "DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR)",
            ),
        };
        let blob = match pool.backend() {
            Backend::Postgres => "decode('00', 'hex')",
            Backend::MySql => "UNHEX('00')",
        };
        for (id, when) in [(&expired, past), (&not_yet, future), (&running, past)] {
            exec_sql(
                pool,
                &format!(
                    "UPDATE hammerwork_jobs SET is_encrypted = true, retention_delete_at = {when}, \
                     encrypted_payload = {blob}, encryption_nonce = {blob}, encryption_tag = {blob}, \
                     encryption_key_id = 'test-key' WHERE id = '{id}'"
                ),
            )
            .await;
        }
        let (jobs, archived) = count_expired_encrypted_jobs(pool).await.unwrap();
        assert_eq!((jobs, archived), (1, 0));
        run(&["purge-encrypted", "--dry-run"]).await.unwrap();
        run(&["purge-encrypted"]).await.unwrap(); // needs --confirm
        assert!(job_column(pool, &expired, "status").await.is_some());
        run(&["purge-encrypted", "--confirm"]).await.unwrap();
        assert!(job_column(pool, &expired, "status").await.is_none());
        assert!(job_column(pool, &not_yet, "status").await.is_some());
        assert!(job_column(pool, &running, "status").await.is_some());

        assert!(table_exists(pool).await);
        db.drop_db().await;
    }

    db_tests!(
        maintenance_commands,
        test_maintenance_commands_postgres,
        test_maintenance_commands_mysql
    );
}
