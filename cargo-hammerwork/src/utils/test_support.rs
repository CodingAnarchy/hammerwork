//! Shared helpers for the database-backed `#[ignore]` tests.

use crate::config::Config;
use crate::utils::database::DatabasePool;
use crate::utils::sql::{
    Backend, Bind, IntervalUnit, SqlParams, bind_mysql, bind_pg, execute_binds,
};
use sqlx::Row;
use tokio::sync::{Mutex, MutexGuard};

static SERIAL: Mutex<()> = Mutex::const_new(());

/// Lock for tests that touch process-global state (environment variables, the config file
/// path). Hold the guard for the whole test.
pub async fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().await
}

/// [`serial`] for synchronous tests.
pub fn serial_blocking() -> MutexGuard<'static, ()> {
    SERIAL.blocking_lock()
}

/// Define a `_postgres` and a `_mysql` test that both run `scenario(url)`.
macro_rules! db_tests {
    ($scenario:ident, $pg:ident, $my:ident) => {
        #[tokio::test]
        #[ignore = "requires DATABASE_URL (PostgreSQL)"]
        async fn $pg() {
            $scenario($crate::utils::test_support::pg_url()).await;
        }

        #[tokio::test]
        #[ignore = "requires MYSQL_DATABASE_URL"]
        async fn $my() {
            $scenario($crate::utils::test_support::mysql_url()).await;
        }
    };
}
pub(crate) use db_tests;

pub fn pg_url() -> String {
    std::env::var("DATABASE_URL").expect("DATABASE_URL")
}

pub fn mysql_url() -> String {
    std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL")
}

/// A CLI config pointing at `url`.
pub fn config_for(url: &str) -> Config {
    Config {
        database_url: Some(url.to_string()),
        connection_pool_size: Some(2),
        ..Config::default()
    }
}

/// A unique queue name.
pub fn unique_queue(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// A throwaway database (created on the server `base_url` points at, migrated, dropped by
/// [`ScratchDb::drop_db`]) for tests that act on every queue or drop tables.
pub struct ScratchDb {
    pub url: String,
    pub pool: DatabasePool,
    admin: DatabasePool,
    name: String,
}

impl ScratchDb {
    pub async fn create(base_url: &str) -> Self {
        let name = format!("hw_scratch_{}", uuid::Uuid::new_v4().simple());
        let (prefix, query) = match base_url.split_once('?') {
            Some((p, q)) => (p, format!("?{q}")),
            None => (base_url, String::new()),
        };
        let root = prefix.rsplit_once('/').unwrap().0;
        let admin_db = if base_url.starts_with("mysql") {
            "mysql"
        } else {
            "postgres"
        };
        let admin = DatabasePool::connect(&format!("{root}/{admin_db}{query}"), 1)
            .await
            .unwrap();
        match &admin {
            DatabasePool::Postgres(p) => {
                sqlx::query(&format!("CREATE DATABASE {name}"))
                    .execute(p)
                    .await
                    .unwrap();
            }
            DatabasePool::MySQL(p) => {
                sqlx::query(&format!("CREATE DATABASE {name}"))
                    .execute(p)
                    .await
                    .unwrap();
            }
        }
        let url = format!("{root}/{name}{query}");
        let pool = DatabasePool::connect(&url, 4).await.unwrap();
        pool.migrate(false).await.unwrap();
        Self {
            url,
            pool,
            admin,
            name,
        }
    }

    /// An empty scratch database: created but not migrated.
    pub async fn create_unmigrated(base_url: &str) -> Self {
        // Reuse `create`, then drop the tables it migrated.
        let db = Self::create(base_url).await;
        match &db.pool {
            DatabasePool::Postgres(p) => {
                sqlx::query("DROP SCHEMA public CASCADE")
                    .execute(p)
                    .await
                    .unwrap();
                sqlx::query("CREATE SCHEMA public")
                    .execute(p)
                    .await
                    .unwrap();
            }
            DatabasePool::MySQL(p) => {
                let tables: Vec<String> = sqlx::query_scalar(
                    "SELECT CAST(TABLE_NAME AS CHAR) FROM information_schema.tables WHERE TABLE_SCHEMA = DATABASE()",
                )
                .fetch_all(p)
                .await
                .unwrap();
                sqlx::query("SET FOREIGN_KEY_CHECKS = 0")
                    .execute(p)
                    .await
                    .ok();
                for t in tables {
                    sqlx::query(&format!("DROP TABLE `{t}`"))
                        .execute(p)
                        .await
                        .unwrap();
                }
            }
        }
        db
    }

    pub fn config(&self) -> Config {
        config_for(&self.url)
    }

    pub async fn drop_db(self) {
        match (&self.pool, &self.admin) {
            (DatabasePool::Postgres(p), DatabasePool::Postgres(a)) => {
                p.close().await;
                sqlx::query(&format!(
                    "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                    self.name
                ))
                .execute(a)
                .await
                .unwrap();
            }
            (DatabasePool::MySQL(p), DatabasePool::MySQL(a)) => {
                p.close().await;
                sqlx::query(&format!("DROP DATABASE IF EXISTS {}", self.name))
                    .execute(a)
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
    }
}

/// Move `column` of job `id` to `days` days ago (database clock).
pub async fn backdate(pool: &DatabasePool, id: &str, column: &str, days: u32) {
    let mut params = SqlParams::new(pool.backend());
    let ago = params.ago(days, IntervalUnit::Day);
    let id = params.uuid(id);
    let sql = format!("UPDATE hammerwork_jobs SET {column} = {ago} WHERE id = {id}");
    let n = execute_binds(pool, &sql, params.binds()).await.unwrap();
    assert_eq!(n, 1, "backdate {column} of {id}");
}

/// Read one text column of one job.
pub async fn job_column(pool: &DatabasePool, id: &str, column: &str) -> Option<String> {
    let mut params = SqlParams::new(pool.backend());
    let id = params.uuid(id);
    let cast = match pool.backend() {
        Backend::Postgres => format!("CAST({column} AS TEXT)"),
        Backend::MySql => format!("CAST({column} AS CHAR)"),
    };
    let sql = format!("SELECT {cast} AS v FROM hammerwork_jobs WHERE id = {id}");
    match pool {
        DatabasePool::Postgres(p) => bind_pg(sqlx::query(&sql), params.binds())
            .fetch_optional(p)
            .await
            .unwrap()
            .and_then(|r| r.try_get::<Option<String>, _>("v").unwrap()),
        DatabasePool::MySQL(p) => bind_mysql(sqlx::query(&sql), params.binds())
            .fetch_optional(p)
            .await
            .unwrap()
            .and_then(|r| r.try_get::<Option<String>, _>("v").unwrap()),
    }
}

/// Number of jobs in `queue`, optionally only those with `status`.
pub async fn count_jobs(pool: &DatabasePool, queue: &str, status: Option<&str>) -> i64 {
    let mut params = SqlParams::new(pool.backend());
    let mut sql = format!(
        "SELECT COUNT(*) AS count FROM hammerwork_jobs WHERE queue_name = {}",
        params.text(queue)
    );
    if let Some(status) = status {
        sql.push_str(&format!(" AND status = {}", params.text(status)));
    }
    crate::utils::sql::fetch_i64(pool, &sql, params.binds(), "count")
        .await
        .unwrap()
}

/// A queue name full of SQL metacharacters.
pub const HOSTILE_QUEUE: &str = "it's; DROP TABLE x --";

/// A hostile queue name that is unique per call, so concurrent tests never share rows.
pub fn hostile_queue() -> String {
    format!("{HOSTILE_QUEUE} {}", uuid::Uuid::new_v4().simple())
}

pub async fn pg_pool() -> DatabasePool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    DatabasePool::connect(&url, 2).await.unwrap()
}

pub async fn mysql_pool() -> DatabasePool {
    let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
    DatabasePool::connect(&url, 2).await.unwrap()
}

/// A job row to insert directly (bypassing the library) for query tests.
pub struct SeedJob<'a> {
    pub queue: &'a str,
    pub status: &'a str,
    pub payload: &'a str,
    /// Marks the job as a recurring cron job due in one hour.
    pub cron: bool,
    /// Sets `failed_at` to now.
    pub failed_now: bool,
    /// Sets `started_at` two hours ago (a long-running job when `Running`).
    pub started_long_ago: bool,
    /// Sets `completed_at` to now.
    pub completed_now: bool,
    /// Parent job id this job depends on.
    pub depends_on: Option<&'a str>,
}

impl<'a> SeedJob<'a> {
    pub fn new(queue: &'a str, status: &'a str) -> Self {
        Self {
            queue,
            status,
            payload: "{}",
            cron: false,
            failed_now: false,
            started_long_ago: false,
            completed_now: false,
            depends_on: None,
        }
    }
}

/// Insert `job` and return its id.
pub async fn seed(pool: &DatabasePool, job: &SeedJob<'_>) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    match pool {
        DatabasePool::Postgres(p) => {
            let sql = "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, attempts, max_attempts, \
                created_at, scheduled_at, started_at, completed_at, failed_at, cron_schedule, recurring, next_run_at, depends_on) \
                VALUES ($1::uuid, $2, $3::jsonb, $4, 0, 3, NOW(), NOW(), \
                CASE WHEN $5 THEN NOW() - INTERVAL '2 hours' ELSE NULL END, \
                CASE WHEN $6 THEN NOW() ELSE NULL END, \
                CASE WHEN $7 THEN NOW() ELSE NULL END, \
                CASE WHEN $8 THEN '0 0 * * * *' ELSE NULL END, $8, \
                CASE WHEN $8 THEN NOW() + INTERVAL '1 hour' ELSE NULL END, \
                CASE WHEN $9::text IS NULL THEN '{}'::uuid[] ELSE ARRAY[$9::uuid] END)";
            sqlx::query(sql)
                .bind(&id)
                .bind(job.queue)
                .bind(job.payload)
                .bind(job.status)
                .bind(job.started_long_ago)
                .bind(job.completed_now)
                .bind(job.failed_now)
                .bind(job.cron)
                .bind(job.depends_on)
                .execute(p)
                .await
                .unwrap();
        }
        DatabasePool::MySQL(p) => {
            let sql = "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, attempts, max_attempts, \
                created_at, scheduled_at, started_at, completed_at, failed_at, cron_schedule, recurring, next_run_at, depends_on) \
                VALUES (?, ?, CAST(? AS JSON), ?, 0, 3, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), \
                IF(?, DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 2 HOUR), NULL), \
                IF(?, UTC_TIMESTAMP(6), NULL), \
                IF(?, UTC_TIMESTAMP(6), NULL), \
                IF(?, '0 0 * * * *', NULL), ?, \
                IF(?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), NULL), \
                IF(? IS NULL, JSON_ARRAY(), JSON_ARRAY(?)))";
            sqlx::query(sql)
                .bind(&id)
                .bind(job.queue)
                .bind(job.payload)
                .bind(job.status)
                .bind(job.started_long_ago)
                .bind(job.completed_now)
                .bind(job.failed_now)
                .bind(job.cron)
                .bind(job.cron)
                .bind(job.cron)
                .bind(job.depends_on)
                .bind(job.depends_on)
                .execute(p)
                .await
                .unwrap();
        }
    }
    id
}

/// Remove every job in the given queues.
pub async fn cleanup(pool: &DatabasePool, queues: &[&str]) {
    for queue in queues {
        match pool {
            DatabasePool::Postgres(p) => {
                sqlx::query("DELETE FROM hammerwork_jobs WHERE queue_name = $1")
                    .bind(*queue)
                    .execute(p)
                    .await
                    .unwrap();
            }
            DatabasePool::MySQL(p) => {
                sqlx::query("DELETE FROM hammerwork_jobs WHERE queue_name = ?")
                    .bind(*queue)
                    .execute(p)
                    .await
                    .unwrap();
            }
        }
    }
}

/// Run `sql` with `binds` and return column `column` of every row as text.
pub async fn column_strings(
    pool: &DatabasePool,
    sql: &str,
    binds: &[Bind],
    column: &str,
) -> Vec<String> {
    match pool {
        DatabasePool::Postgres(p) => bind_pg(sqlx::query(sql), binds)
            .fetch_all(p)
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String, _>(column).unwrap())
            .collect(),
        DatabasePool::MySQL(p) => bind_mysql(sqlx::query(sql), binds)
            .fetch_all(p)
            .await
            .unwrap()
            .iter()
            .map(|r| r.try_get::<String, _>(column).unwrap())
            .collect(),
    }
}

/// Whether the `hammerwork_jobs` table still exists (the injection payload must not drop it).
pub async fn table_exists(pool: &DatabasePool) -> bool {
    match pool {
        DatabasePool::Postgres(p) => sqlx::query("SELECT 1 FROM hammerwork_jobs LIMIT 1")
            .fetch_all(p)
            .await
            .is_ok(),
        DatabasePool::MySQL(p) => sqlx::query("SELECT 1 FROM hammerwork_jobs LIMIT 1")
            .fetch_all(p)
            .await
            .is_ok(),
    }
}

/// Run a raw statement (test fixtures only; never build these from user input).
pub async fn exec_sql(pool: &DatabasePool, sql: &str) -> u64 {
    match pool {
        DatabasePool::Postgres(p) => sqlx::query(sql).execute(p).await.unwrap().rows_affected(),
        DatabasePool::MySQL(p) => sqlx::query(sql).execute(p).await.unwrap().rows_affected(),
    }
}

/// The status of job `id`.
pub async fn job_status(pool: &DatabasePool, id: &str) -> String {
    job_column(pool, id, "status").await.expect("job exists")
}

/// Environment variables set for the duration of a test. Holds [`serial`] so no other
/// test sees them, and removes them again on drop.
pub struct ScopedEnv {
    _lock: MutexGuard<'static, ()>,
    keys: Vec<String>,
}

impl ScopedEnv {
    pub async fn set(vars: &[(&str, &str)]) -> Self {
        let lock = serial().await;
        for (key, value) in vars {
            // SAFETY: every test that reads or writes these variables holds `serial`.
            unsafe { std::env::set_var(key, value) };
        }
        Self {
            _lock: lock,
            keys: vars.iter().map(|(k, _)| k.to_string()).collect(),
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        for key in &self.keys {
            // SAFETY: see `set`; the lock is still held here.
            unsafe { std::env::remove_var(key) };
        }
    }
}
