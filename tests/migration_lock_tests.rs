//! Concurrent migration runs (several replicas starting with `auto_migrate`, or the CLI
//! while the application starts) must all succeed (#64 H3): the run holds a migration
//! lock, re-reads the applied migrations once it has it, and runs each migration once.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Utc;
use hammerwork::migrations::{Migration, MigrationManager, MigrationRunner};

/// How many runs race each other.
const RUNS: usize = 6;

/// A migration only this test knows, whose SQL fails if it runs twice: a run that does
/// not wait for the others would replay it and fail.
fn probe_migration() -> (Migration, String) {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let migration = Migration {
        id: format!("900_lock_probe_{suffix}"),
        description: "Concurrency probe (test only)".to_string(),
        version: 900_000,
        created_at: Utc::now(),
    };
    (migration, format!("hw_lock_probe_{suffix}"))
}

/// Build `RUNS` managers (one runner each) that know the built-in migrations plus the
/// probe creating `table`, and run them all at once. Every run must succeed.
async fn race<DB, F>(make_runner: F, migration: &Migration, table: &str)
where
    DB: sqlx::Database,
    F: Fn() -> Box<dyn MigrationRunner<DB> + Send + Sync>,
{
    let sql = format!("CREATE TABLE {table} (id INT)");
    let mut runs = tokio::task::JoinSet::new();
    for _ in 0..RUNS {
        let mut manager = MigrationManager::new(make_runner());
        manager.register_migration(migration.clone(), sql.clone(), sql.clone());
        runs.spawn(async move { manager.run_migrations().await });
    }
    while let Some(run) = runs.join_next().await {
        run.unwrap()
            .expect("every concurrent migration run must succeed");
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;
    use hammerwork::migrations::postgres::PostgresMigrationRunner;
    use sqlx::{
        Connection, Executor, PgConnection, PgPool,
        postgres::{PgConnectOptions, PgPoolOptions},
    };
    use std::str::FromStr;

    async fn pool(options: &PgConnectOptions) -> PgPool {
        PgPoolOptions::new()
            .max_connections(3)
            .connect_with(options.clone())
            .await
            .unwrap()
    }

    /// Concurrent runs against a migrated database each apply a new migration once.
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn concurrent_runs_apply_a_new_migration_once() {
        let options = PgConnectOptions::from_str(&test_utils::postgres_url()).unwrap();
        let (migration, table) = probe_migration();
        let pools: Vec<PgPool> = futures_pools(&options).await;
        let next = std::sync::atomic::AtomicUsize::new(0);
        race::<sqlx::Postgres, _>(
            || {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::new(PostgresMigrationRunner::new(pools[i % pools.len()].clone()))
            },
            &migration,
            &table,
        )
        .await;

        let pool = &pools[0];
        let recorded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hammerwork_migrations WHERE migration_id = $1",
        )
        .bind(&migration.id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(recorded, 1);
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(pool)
            .await
            .expect("the probe migration created its table");
        sqlx::query("DELETE FROM hammerwork_migrations WHERE migration_id = $1")
            .bind(&migration.id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn futures_pools(options: &PgConnectOptions) -> Vec<PgPool> {
        let mut pools = Vec::new();
        for _ in 0..RUNS {
            pools.push(pool(options).await);
        }
        pools
    }

    /// A fresh deploy: several replicas migrate an empty database at once.
    #[tokio::test]
    #[ignore = "requires a PostgreSQL server on which the user may CREATE DATABASE"]
    async fn concurrent_runs_migrate_a_fresh_database() {
        let base = PgConnectOptions::from_str(&test_utils::postgres_url()).unwrap();
        let scratch = format!("hw_lock_{}", uuid::Uuid::new_v4().simple());
        let mut admin = PgConnection::connect_with(&base).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE \"{scratch}\"").as_str())
            .await
            .unwrap();

        let options = base.clone().database(&scratch);
        let outcome = tokio::spawn(async move {
            let pools = futures_pools(&options).await;
            let mut runs = tokio::task::JoinSet::new();
            for pool in &pools {
                let runner = Box::new(PostgresMigrationRunner::new(pool.clone()));
                runs.spawn(async move { MigrationManager::new(runner).run_migrations().await });
            }
            while let Some(run) = runs.join_next().await {
                run.unwrap()
                    .expect("every replica's migration run must succeed");
            }
            let status =
                MigrationManager::new(Box::new(PostgresMigrationRunner::new(pools[0].clone())))
                    .get_migration_status()
                    .await
                    .unwrap();
            assert!(status.iter().all(|(_, applied)| *applied), "{status:?}");
            for pool in pools {
                pool.close().await;
            }
        })
        .await;

        admin
            .execute(format!("DROP DATABASE IF EXISTS \"{scratch}\" WITH (FORCE)").as_str())
            .await
            .unwrap();
        outcome.unwrap();
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;
    use hammerwork::migrations::mysql::MySqlMigrationRunner;
    use sqlx::{
        Connection, Executor, MySqlConnection, MySqlPool,
        mysql::{MySqlConnectOptions, MySqlPoolOptions},
    };
    use std::str::FromStr;

    async fn pools(options: &MySqlConnectOptions) -> Vec<MySqlPool> {
        let mut pools = Vec::new();
        for _ in 0..RUNS {
            pools.push(
                MySqlPoolOptions::new()
                    .max_connections(3)
                    .connect_with(options.clone())
                    .await
                    .unwrap(),
            );
        }
        pools
    }

    /// Concurrent runs against a migrated database each apply a new migration once.
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn mysql_concurrent_runs_apply_a_new_migration_once() {
        let options = MySqlConnectOptions::from_str(&test_utils::mysql_url()).unwrap();
        let (migration, table) = probe_migration();
        let pools = pools(&options).await;
        let next = std::sync::atomic::AtomicUsize::new(0);
        race::<sqlx::MySql, _>(
            || {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Box::new(MySqlMigrationRunner::new(pools[i % pools.len()].clone()))
            },
            &migration,
            &table,
        )
        .await;

        let pool = &pools[0];
        let recorded: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM hammerwork_migrations WHERE migration_id = ?")
                .bind(&migration.id)
                .fetch_one(pool)
                .await
                .unwrap();
        assert_eq!(recorded, 1);
        sqlx::query(&format!("DROP TABLE {table}"))
            .execute(pool)
            .await
            .expect("the probe migration created its table");
        sqlx::query("DELETE FROM hammerwork_migrations WHERE migration_id = ?")
            .bind(&migration.id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// A fresh deploy: several replicas migrate an empty database at once.
    #[tokio::test]
    #[ignore = "requires a MySQL server on which the user may CREATE DATABASE"]
    async fn mysql_concurrent_runs_migrate_a_fresh_database() {
        let base = MySqlConnectOptions::from_str(&test_utils::mysql_url()).unwrap();
        let scratch = format!("hw_lock_{}", uuid::Uuid::new_v4().simple());
        let mut admin = MySqlConnection::connect_with(&base).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE `{scratch}`").as_str())
            .await
            .unwrap();

        let options = base.clone().database(&scratch);
        let outcome = tokio::spawn(async move {
            let pools = pools(&options).await;
            let mut runs = tokio::task::JoinSet::new();
            for pool in &pools {
                let runner = Box::new(MySqlMigrationRunner::new(pool.clone()));
                runs.spawn(async move { MigrationManager::new(runner).run_migrations().await });
            }
            while let Some(run) = runs.join_next().await {
                run.unwrap()
                    .expect("every replica's migration run must succeed");
            }
            let status =
                MigrationManager::new(Box::new(MySqlMigrationRunner::new(pools[0].clone())))
                    .get_migration_status()
                    .await
                    .unwrap();
            assert!(status.iter().all(|(_, applied)| *applied), "{status:?}");
            for pool in pools {
                pool.close().await;
            }
        })
        .await;

        admin
            .execute(format!("DROP DATABASE IF EXISTS `{scratch}`").as_str())
            .await
            .unwrap();
        outcome.unwrap();
    }
}
