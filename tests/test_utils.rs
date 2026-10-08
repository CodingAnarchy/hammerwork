//! Test utilities for setting up databases with migrations.
//!
//! Connection URLs come from `DATABASE_URL` (PostgreSQL) and `MYSQL_DATABASE_URL`
//! (MySQL), defaulting to the databases created by `scripts/setup-test-databases.sh`.
//!
//! Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

#[cfg(any(feature = "postgres", feature = "mysql"))]
use hammerwork::{JobQueue, migrations::MigrationManager};
#[cfg(any(feature = "postgres", feature = "mysql"))]
use std::sync::Arc;

/// Default PostgreSQL URL used when `DATABASE_URL` is unset.
pub const DEFAULT_POSTGRES_URL: &str = "postgres://postgres:hammerwork@localhost:5433/hammerwork";

/// Default MySQL URL used when `MYSQL_DATABASE_URL` is unset.
pub const DEFAULT_MYSQL_URL: &str = "mysql://root:hammerwork@localhost:3307/hammerwork";

/// PostgreSQL connection URL for integration tests.
pub fn postgres_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_POSTGRES_URL.to_string())
}

/// MySQL connection URL for integration tests.
pub fn mysql_url() -> String {
    std::env::var("MYSQL_DATABASE_URL").unwrap_or_else(|_| DEFAULT_MYSQL_URL.to_string())
}

/// A PostgreSQL pool that does not connect until first used.
///
/// Useful for tests that need a `Pool` value (e.g. to construct a `JobArchiver`)
/// but never touch the database.
#[cfg(feature = "postgres")]
pub fn lazy_postgres_pool() -> sqlx::PgPool {
    sqlx::PgPool::connect_lazy(&postgres_url()).expect("Invalid PostgreSQL URL")
}

#[cfg(feature = "postgres")]
pub async fn setup_postgres_queue() -> Arc<JobQueue<sqlx::Postgres>> {
    use hammerwork::migrations::postgres::PostgresMigrationRunner;
    use sqlx::{Pool, Postgres};

    let pool = Pool::<Postgres>::connect(&postgres_url())
        .await
        .expect("Failed to connect to Postgres");

    let queue = Arc::new(JobQueue::new(pool.clone()));

    // Run migrations to set up tables - the migration system handles duplicates gracefully
    let runner = Box::new(PostgresMigrationRunner::new(pool));
    let manager = MigrationManager::new(runner);
    manager
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    queue
}

#[cfg(feature = "mysql")]
pub async fn setup_mysql_queue() -> Arc<JobQueue<sqlx::MySql>> {
    use hammerwork::migrations::mysql::MySqlMigrationRunner;
    use sqlx::{MySql, Pool};

    let pool = Pool::<MySql>::connect(&mysql_url())
        .await
        .expect("Failed to connect to MySQL");

    let queue = Arc::new(JobQueue::new(pool.clone()));

    // Run migrations to set up tables - the migration system handles duplicates gracefully
    let runner = Box::new(MySqlMigrationRunner::new(pool));
    let manager = MigrationManager::new(runner);
    manager
        .run_migrations()
        .await
        .expect("Failed to run migrations");

    queue
}

/// A queue name that no other test (or earlier test run) uses.
///
/// The test databases are shared and not reset between runs, so tests that
/// dequeue or archive by queue name must not see each other's jobs.
pub fn unique_queue(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serialize database tests within one test binary.
///
/// Needed by tests that operate on every queue at once (e.g. archiving with no
/// queue filter or purging the whole archive table), which would otherwise race
/// with the other tests in the same binary.
pub async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL.lock().await
}

/// Whether to skip a test that is `#[ignore]`d because it exposes a known library bug.
///
/// CI runs the suite with `--include-ignored`, which would also run these tests.
/// They return early unless `HAMMERWORK_TEST_KNOWN_BUGS` is set, so the bug can be
/// reproduced with `HAMMERWORK_TEST_KNOWN_BUGS=1 cargo test ... -- --include-ignored`.
pub fn skip_known_bug() -> bool {
    if std::env::var_os("HAMMERWORK_TEST_KNOWN_BUGS").is_some() {
        return false;
    }
    eprintln!("skipping known-bug test; set HAMMERWORK_TEST_KNOWN_BUGS=1 to run it");
    true
}
