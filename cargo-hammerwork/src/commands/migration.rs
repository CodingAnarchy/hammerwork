use anyhow::Result;
use clap::Subcommand;
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
    #[command(about = "Check migration status")]
    Status {
        #[arg(short = 'u', long, help = "Database connection URL")]
        database_url: Option<String>,
    },
}

impl MigrationCommand {
    pub async fn execute(&self, config: &Config) -> Result<()> {
        match self {
            MigrationCommand::Run { database_url, drop } => {
                let db_url = database_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;

                info!("Running migrations for: {}", redact_url(db_url));
                let pool = DatabasePool::connect(db_url, config.get_connection_pool_size()).await?;
                pool.migrate(*drop).await?;
                info!("Migrations completed successfully");
            }
            MigrationCommand::Status { database_url } => {
                let db_url = database_url
                    .as_ref()
                    .map(|s| s.as_str())
                    .or(config.get_database_url())
                    .ok_or_else(|| anyhow::anyhow!("Database URL is required"))?;

                info!("Checking migration status for: {}", redact_url(db_url));
                check_migration_status(db_url).await?;
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

async fn check_migration_status(database_url: &str) -> Result<()> {
    let pool = DatabasePool::connect(database_url, 1).await?;
    let backend = match pool.backend() {
        Backend::Postgres => "PostgreSQL",
        Backend::MySql => "MySQL",
    };
    let status = schema_status(&pool).await?;

    if status.tables_exist {
        println!("✅ {backend} tables exist");
        println!("📊 Schema columns: {}", status.columns.len());
        for column in &status.columns {
            println!("  - {}", column);
        }
    } else {
        println!("❌ {backend} tables do not exist. Run 'migration run' to create them.");
    }

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
