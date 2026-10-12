use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Subcommand;
use hammerwork::migrations::{
    MigrationManager, MigrationRunner, mysql::MySqlMigrationRunner,
    postgres::PostgresMigrationRunner,
};
use sqlx::Row;
use tracing::info;

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::sql::Backend;
use crate::utils::validation::redact_url;

#[derive(Subcommand)]
pub enum MigrationCommand {
    #[command(about = "Run database migrations")]
    Run {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
        #[arg(short, long, help = "Drop existing tables before migration")]
        drop: bool,
    },
    #[command(
        about = "Show applied and pending migrations",
        long_about = "Show which of this version's migrations the database has applied and \
            which are pending, from the hammerwork_migrations table.\n\n\
            Migrations recorded in the database that this version does not know (applied \
            by a newer Hammerwork) are listed separately."
    )]
    Status {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
    },
}

impl MigrationCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        match self {
            MigrationCommand::Run { database_url, drop } => {
                let db_url = &config.resolve_database_url(database_url.as_deref())?;

                info!("Running migrations for: {}", redact_url(db_url));
                let pool = DatabasePool::connect_with_config(db_url, config).await?;
                pool.migrate(*drop).await?;
                info!("Migrations completed successfully");
            }
            MigrationCommand::Status { database_url } => {
                let db_url = &config.resolve_database_url(database_url.as_deref())?;

                info!("Checking migration status for: {}", redact_url(db_url));
                check_migration_status(db_url, config).await?;
            }
        }
        Ok(())
    }
}

/// What `migration status` found in the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaStatus {
    /// Whether the `hammerwork_jobs` table exists.
    pub tables_exist: bool,
    /// The columns of `hammerwork_jobs`, sorted by name.
    pub columns: Vec<String>,
}

pub async fn schema_status(pool: &DatabasePool) -> Result<SchemaStatus> {
    let (exists, columns) = match pool {
        DatabasePool::Postgres(pg_pool) => {
            let exists = sqlx::query(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = current_schema() AND table_name = 'hammerwork_jobs'",
            )
            .fetch_optional(pg_pool)
            .await?
            .is_some();
            let columns: Vec<String> = sqlx::query_scalar(
                "SELECT column_name::text FROM information_schema.columns \
                 WHERE table_schema = current_schema() AND table_name = 'hammerwork_jobs' \
                 ORDER BY column_name",
            )
            .fetch_all(pg_pool)
            .await?;
            (exists, columns)
        }
        DatabasePool::MySQL(mysql_pool) => {
            let exists = sqlx::query(
                "SELECT TABLE_NAME FROM information_schema.tables \
                 WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = 'hammerwork_jobs'",
            )
            .fetch_optional(mysql_pool)
            .await?
            .is_some();
            let rows = sqlx::query(
                "SELECT CAST(COLUMN_NAME AS CHAR) AS column_name FROM information_schema.columns \
                 WHERE TABLE_NAME = 'hammerwork_jobs' AND TABLE_SCHEMA = DATABASE() \
                 ORDER BY COLUMN_NAME",
            )
            .fetch_all(mysql_pool)
            .await?;
            let columns = rows
                .iter()
                .map(|r| r.try_get::<String, _>("column_name"))
                .collect::<Result<Vec<_>, _>>()?;
            (exists, columns)
        }
    };
    Ok(SchemaStatus {
        tables_exist: exists,
        columns: if exists { columns } else { Vec::new() },
    })
}

/// One migration this version registers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationEntry {
    pub id: String,
    pub description: String,
    pub version: u32,
    /// When the database applied it; `None` while pending.
    pub applied_at: Option<DateTime<Utc>>,
}

/// The migrations of this version against the `hammerwork_migrations` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Whether the `hammerwork_migrations` table exists.
    pub table_exists: bool,
    /// The registered migrations in version order, applied or pending.
    pub migrations: Vec<MigrationEntry>,
    /// Applied migration ids this version does not register.
    pub unknown: Vec<String>,
}

impl MigrationReport {
    pub fn applied(&self) -> impl Iterator<Item = &MigrationEntry> {
        self.migrations.iter().filter(|m| m.applied_at.is_some())
    }

    pub fn pending(&self) -> impl Iterator<Item = &MigrationEntry> {
        self.migrations.iter().filter(|m| m.applied_at.is_none())
    }
}

/// Compare the registered migrations with the applied records.
fn build_report(
    table_exists: bool,
    registered: Vec<hammerwork::migrations::Migration>,
    records: Vec<(String, DateTime<Utc>)>,
) -> MigrationReport {
    let migrations: Vec<MigrationEntry> = registered
        .into_iter()
        .map(|m| MigrationEntry {
            applied_at: records
                .iter()
                .find(|(id, _)| *id == m.id)
                .map(|(_, at)| *at),
            id: m.id,
            description: m.description,
            version: m.version,
        })
        .collect();
    let mut unknown: Vec<String> = records
        .iter()
        .filter(|(id, _)| !migrations.iter().any(|m| m.id == *id))
        .map(|(id, _)| id.clone())
        .collect();
    unknown.sort();
    MigrationReport {
        table_exists,
        migrations,
        unknown,
    }
}

/// Read-only: the registered migrations and which of them the database recorded.
pub async fn migration_report(pool: &DatabasePool) -> Result<MigrationReport> {
    match pool {
        DatabasePool::Postgres(pg) => {
            let runner = PostgresMigrationRunner::new(pg.clone());
            let table_exists = runner.migration_table_exists().await?;
            let records = if table_exists {
                runner
                    .get_executed_migrations()
                    .await?
                    .into_iter()
                    .map(|r| (r.migration_id, r.executed_at))
                    .collect()
            } else {
                Vec::new()
            };
            let registered = MigrationManager::new(Box::new(runner)).registered_migrations();
            Ok(build_report(table_exists, registered, records))
        }
        DatabasePool::MySQL(my) => {
            let runner = MySqlMigrationRunner::new(my.clone());
            let table_exists = runner.migration_table_exists().await?;
            let records = if table_exists {
                runner
                    .get_executed_migrations()
                    .await?
                    .into_iter()
                    .map(|r| (r.migration_id, r.executed_at))
                    .collect()
            } else {
                Vec::new()
            };
            let registered = MigrationManager::new(Box::new(runner)).registered_migrations();
            Ok(build_report(table_exists, registered, records))
        }
    }
}

/// The text `migration status` prints.
pub fn render_report(backend: &str, report: &MigrationReport, schema: &SchemaStatus) -> String {
    let mut out = String::new();
    out.push_str(&format!("📋 {backend} migrations\n"));
    if !report.table_exists {
        out.push_str(
            "❌ No hammerwork_migrations table: no migration has been applied. \
             Run 'migration run' to create the schema.\n",
        );
    }
    let applied = report.applied().count();
    let pending = report.pending().count();
    out.push_str(&format!(
        "{applied} applied, {pending} pending ({} registered)\n",
        report.migrations.len()
    ));
    if applied > 0 {
        out.push_str("\nApplied:\n");
        for m in report.applied() {
            let at = m.applied_at.expect("applied");
            out.push_str(&format!(
                "  ✅ {} ({}) at {}\n",
                m.id,
                m.description,
                at.format("%Y-%m-%d %H:%M:%S UTC")
            ));
        }
    }
    if pending > 0 {
        out.push_str("\nPending:\n");
        for m in report.pending() {
            out.push_str(&format!("  ⏳ {} ({})\n", m.id, m.description));
        }
        out.push_str("\nRun 'migration run' to apply the pending migrations.\n");
    }
    if !report.unknown.is_empty() {
        out.push_str(
            "\n⚠️  Applied but not known to this version of Hammerwork (applied by a newer \
             version?):\n",
        );
        for id in &report.unknown {
            out.push_str(&format!("  ? {id}\n"));
        }
    }
    if schema.tables_exist {
        out.push_str(&format!(
            "\n📊 hammerwork_jobs table: {} columns\n",
            schema.columns.len()
        ));
    } else {
        out.push_str("\n❌ hammerwork_jobs table does not exist\n");
    }
    out
}

async fn check_migration_status(database_url: &str, config: &Config) -> Result<()> {
    let pool = DatabasePool::connect_with_config(database_url, config).await?;
    let backend = match pool.backend() {
        Backend::Postgres => "PostgreSQL",
        Backend::MySql => "MySQL",
    };
    let report = migration_report(&pool).await?;
    let schema = schema_status(&pool).await?;
    print!("{}", render_report(backend, &report, &schema));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::test_support::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: MigrationCommand,
    }

    fn parse(args: &[&str]) -> MigrationCommand {
        let mut argv = vec!["test"];
        argv.extend_from_slice(args);
        TestCli::try_parse_from(argv).unwrap().command
    }

    #[test]
    fn parses_run_and_status_flags() {
        match parse(&["run"]) {
            MigrationCommand::Run {
                database_url, drop, ..
            } => assert!(database_url.is_none() && !drop),
            _ => panic!("expected Run"),
        }
        match parse(&["run", "-u", "postgres://h/d", "--drop"]) {
            MigrationCommand::Run { database_url, drop } => {
                assert_eq!(database_url.as_deref(), Some("postgres://h/d"));
                assert!(drop);
            }
            _ => panic!("expected Run"),
        }
        match parse(&["run", "-d"]) {
            MigrationCommand::Run { drop, .. } => assert!(drop),
            _ => panic!("expected Run"),
        }
        assert!(matches!(
            parse(&["status", "--database-url", "mysql://h/d"]),
            MigrationCommand::Status { database_url: Some(u) } if u == "mysql://h/d"
        ));
        assert!(TestCli::try_parse_from(["test", "rollback"]).is_err());
    }

    fn registered() -> Vec<hammerwork::migrations::Migration> {
        use hammerwork::migrations::Migration;
        vec![
            Migration::new("001_initial_schema", "Initial schema", 1),
            Migration::new("002_add_priority", "Priority", 2),
            Migration::new("003_add_timeouts", "Timeouts", 3),
        ]
    }

    fn record(id: &str) -> (String, DateTime<Utc>) {
        (
            id.to_string(),
            DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
                .unwrap()
                .with_timezone(&Utc),
        )
    }

    #[test]
    fn the_report_splits_applied_pending_and_unknown_migrations() {
        let report = build_report(
            true,
            registered(),
            vec![
                record("001_initial_schema"),
                record("002_add_priority"),
                record("099_from_the_future"),
            ],
        );
        assert!(report.table_exists);
        let applied: Vec<_> = report.applied().map(|m| m.id.as_str()).collect();
        let pending: Vec<_> = report.pending().map(|m| m.id.as_str()).collect();
        assert_eq!(applied, ["001_initial_schema", "002_add_priority"]);
        assert_eq!(pending, ["003_add_timeouts"]);
        assert_eq!(report.unknown, ["099_from_the_future"]);

        let schema = SchemaStatus {
            tables_exist: true,
            columns: vec!["id".into(), "status".into()],
        };
        let text = render_report("PostgreSQL", &report, &schema);
        assert!(
            text.contains("2 applied, 1 pending (3 registered)"),
            "{text}"
        );
        assert!(text.contains("✅ 001_initial_schema"), "{text}");
        assert!(text.contains("2030-01-02 03:04:05 UTC"), "{text}");
        assert!(text.contains("⏳ 003_add_timeouts"), "{text}");
        assert!(text.contains("? 099_from_the_future"), "{text}");
        assert!(text.contains("hammerwork_jobs table: 2 columns"), "{text}");
    }

    #[test]
    fn a_database_without_the_migrations_table_has_everything_pending() {
        let report = build_report(false, registered(), Vec::new());
        assert_eq!(report.applied().count(), 0);
        assert_eq!(report.pending().count(), 3);
        let schema = SchemaStatus {
            tables_exist: false,
            columns: Vec::new(),
        };
        let text = render_report("MySQL", &report, &schema);
        assert!(text.contains("No hammerwork_migrations table"), "{text}");
        assert!(text.contains("0 applied, 3 pending"), "{text}");
        assert!(
            text.contains("hammerwork_jobs table does not exist"),
            "{text}"
        );
        assert!(!text.contains("not known to this version"), "{text}");
    }

    /// The registered list is what a real runner's manager reports, with 021 included.
    #[tokio::test]
    async fn the_library_registers_its_migrations_in_version_order() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://u:p@127.0.0.1:1/x")
            .unwrap();
        let manager = MigrationManager::new(Box::new(PostgresMigrationRunner::new(pool)));
        let migrations = manager.registered_migrations();
        assert!(migrations.len() >= 21);
        assert!(migrations.windows(2).all(|w| w[0].version < w[1].version));
        assert!(migrations.iter().any(|m| m.id.starts_with("021_")));
    }

    #[tokio::test]
    async fn a_database_url_is_required() {
        for cmd in [parse(&["run"]), parse(&["status"])] {
            let err = cmd.execute(&Config::default()).await.unwrap_err();
            assert!(
                err.to_string().contains("Database URL is required"),
                "{err}"
            );
        }
    }

    #[tokio::test]
    async fn unsupported_urls_are_errors() {
        let config = Config::default();
        let err = parse(&["run", "-u", "sqlite://x.db"])
            .execute(&config)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Unsupported database URL"),
            "{err}"
        );
    }

    /// #94: `migration status` honours `connect_timeout_secs` (default 10s) instead of
    /// connecting with the default timeout.
    #[tokio::test]
    async fn status_uses_the_configured_connect_timeout() {
        let config = Config {
            connect_timeout_secs: Some(1),
            ..Config::default()
        };
        let start = std::time::Instant::now();
        let result = parse(&["status", "-u", "postgres://u:p@10.255.255.1:5432/x"])
            .execute(&config)
            .await;
        assert!(result.is_err());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    async fn migration_lifecycle(base_url: String) {
        let db = ScratchDb::create_unmigrated(&base_url).await;
        let config = db.config();
        let run = |args: &[&str]| {
            let cmd = parse(args);
            let config = config.clone();
            async move { cmd.execute(&config).await }
        };

        // a database that does not exist is an error, not "no tables"
        let missing = format!(
            "{}/hw_no_such_database",
            base_url.rsplit_once('/').unwrap().0
        );
        assert!(run(&["status", "-u", &missing]).await.is_err());

        // before: nothing there
        let status = schema_status(&db.pool).await.unwrap();
        assert!(!status.tables_exist && status.columns.is_empty());
        run(&["status"]).await.unwrap();
        let before = migration_report(&db.pool).await.unwrap();
        assert!(!before.table_exists);
        assert_eq!(before.applied().count(), 0);
        assert!(before.pending().count() >= 21, "everything is pending");

        // run creates the schema; the key columns are there
        run(&["run"]).await.unwrap();
        let status = schema_status(&db.pool).await.unwrap();
        assert!(status.tables_exist);
        for column in [
            "id",
            "queue_name",
            "status",
            "priority",
            "cron_schedule",
            "payload",
        ] {
            assert!(status.columns.iter().any(|c| c == column), "{column}");
        }
        let mut sorted = status.columns.clone();
        sorted.sort();
        assert_eq!(status.columns, sorted);
        run(&["status"]).await.unwrap();

        // after: every registered migration is applied, including 021, none pending
        let after = migration_report(&db.pool).await.unwrap();
        assert!(after.table_exists);
        assert_eq!(after.pending().count(), 0);
        assert_eq!(after.applied().count(), after.migrations.len());
        assert!(after.applied().any(|m| m.id.starts_with("021_")));
        assert!(after.unknown.is_empty());
        // a migration this version does not register shows up as unknown, and a
        // forgotten record as pending
        let sql_for = |verb: &str, ids: &[&str]| {
            let mut params = crate::utils::sql::SqlParams::new(db.pool.backend());
            let sql = if verb == "delete" {
                format!(
                    "DELETE FROM hammerwork_migrations WHERE migration_id = {}",
                    params.text(ids[0])
                )
            } else {
                let (id, at) = (params.text(ids[0]), params.time(Utc::now()));
                format!(
                    "INSERT INTO hammerwork_migrations (migration_id, executed_at, execution_time_ms) VALUES ({id}, {at}, 1)"
                )
            };
            (sql, params.into_binds())
        };
        let last = after.migrations.last().unwrap().id.clone();
        for (verb, id) in [("delete", last.as_str()), ("insert", "999_from_the_future")] {
            let (sql, binds) = sql_for(verb, &[id]);
            crate::utils::sql::execute_binds(&db.pool, &sql, &binds)
                .await
                .unwrap();
        }
        let tampered = migration_report(&db.pool).await.unwrap();
        assert_eq!(tampered.unknown, ["999_from_the_future"]);
        let pending: Vec<_> = tampered.pending().map(|m| m.id.clone()).collect();
        assert_eq!(pending, std::slice::from_ref(&last));
        run(&["status"]).await.unwrap();
        // put the records back
        for (verb, id) in [("delete", "999_from_the_future"), ("insert", last.as_str())] {
            let (sql, binds) = sql_for(verb, &[id]);
            crate::utils::sql::execute_binds(&db.pool, &sql, &binds)
                .await
                .unwrap();
        }
        assert_eq!(
            migration_report(&db.pool).await.unwrap().pending().count(),
            0
        );

        // running again is a no-op that keeps the data
        let queue = unique_queue("migrate");
        seed(&db.pool, &SeedJob::new(&queue, "Pending")).await;
        run(&["run"]).await.unwrap();
        assert_eq!(count_jobs(&db.pool, &queue, None).await, 1);

        // --drop starts over: the data is gone, the schema is back
        run(&["run", "--drop"]).await.unwrap();
        assert!(schema_status(&db.pool).await.unwrap().tables_exist);
        assert_eq!(count_jobs(&db.pool, &queue, None).await, 0);
        seed(&db.pool, &SeedJob::new(&queue, "Pending")).await;
        assert_eq!(count_jobs(&db.pool, &queue, None).await, 1);

        db.drop_db().await;
    }

    db_tests!(
        migration_lifecycle,
        test_migration_lifecycle_postgres,
        test_migration_lifecycle_mysql
    );
}
