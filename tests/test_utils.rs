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

/// A fresh in-memory [`TestQueue`](hammerwork::queue::test::TestQueue).
#[cfg(feature = "test")]
pub async fn test_queue() -> std::sync::Arc<hammerwork::queue::test::TestQueue> {
    std::sync::Arc::new(hammerwork::queue::test::TestQueue::new())
}

/// One test per scenario in module `$module`, each running `scenario($setup().await)`.
///
/// The test attribute defaults to `#[tokio::test]`; pass another one first, e.g.
/// `backend_tests!(#[tokio::test(flavor = "multi_thread")] module, ...)`.
#[allow(unused_macros)]
macro_rules! backend_tests {
    (#[$test:meta] $module:ident, $setup:path, $ignore:meta, [$($scenario:ident),* $(,)?]) => {
        mod $module {
            use super::*;
            $(
                #[$test]
                #[$ignore]
                async fn $scenario() {
                    super::$scenario($setup().await).await;
                }
            )*
        }
    };
    ($module:ident, $setup:path, $ignore:meta, [$($scenario:ident),* $(,)?]) => {
        $crate::test_utils::backend_tests!(
            #[tokio::test] $module, $setup, $ignore, [$($scenario),*]
        );
    };
}
#[allow(unused_imports)]
pub(crate) use backend_tests;

/// Poll `get_job` until `predicate` holds or `timeout` elapses; returns the last job.
pub async fn wait_for_job<Q, F>(
    queue: &std::sync::Arc<Q>,
    id: hammerwork::JobId,
    timeout: std::time::Duration,
    predicate: F,
) -> hammerwork::Job
where
    Q: hammerwork::queue::DatabaseQueue + ?Sized,
    F: Fn(&hammerwork::Job) -> bool,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let job = queue.get_job(id).await.unwrap().expect("job exists");
        if predicate(&job) || std::time::Instant::now() >= deadline {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Poll `condition` until it holds or `timeout` elapses; returns whether it held.
pub async fn eventually<F, Fut>(timeout: std::time::Duration, mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if condition().await {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// A worker on a queue of its own (named after `prefix`), so it never touches the jobs
/// under test.
#[cfg(any(feature = "postgres", feature = "mysql"))]
pub fn idle_worker<DB>(queue: &Arc<JobQueue<DB>>, prefix: &str) -> hammerwork::Worker<DB>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: hammerwork::queue::DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let handler: hammerwork::worker::JobHandler =
        Arc::new(|_job: hammerwork::Job| Box::pin(async { Ok(()) }));
    hammerwork::Worker::new(Arc::clone(queue), unique_queue(prefix), handler)
        .with_poll_interval(std::time::Duration::from_millis(50))
}

/// One ignored (it needs a database) `#[tokio::test]` per `name => scenario`, each
/// running `scenario($setup().await)`. Attributes before a name (e.g. `#[cfg(...)]`)
/// apply to that test.
#[allow(unused_macros)]
macro_rules! scenario_tests {
    ($setup:path; $($(#[$attr:meta])* $name:ident => $scenario:path),* $(,)?) => {
        $(
            $(#[$attr])*
            #[tokio::test]
            #[ignore] // Requires database connection
            async fn $name() {
                $scenario($setup().await).await;
            }
        )*
    };
}
#[allow(unused_imports)]
pub(crate) use scenario_tests;
