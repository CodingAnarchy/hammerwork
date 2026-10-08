//! Shared helpers for the database-backed `#[ignore]` tests.

use crate::utils::database::DatabasePool;
use crate::utils::sql::{Bind, bind_mysql, bind_pg};
use sqlx::Row;

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
                VALUES (?, ?, CAST(? AS JSON), ?, 0, 3, NOW(), NOW(), \
                IF(?, DATE_SUB(NOW(), INTERVAL 2 HOUR), NULL), \
                IF(?, NOW(), NULL), \
                IF(?, NOW(), NULL), \
                IF(?, '0 0 * * * *', NULL), ?, \
                IF(?, DATE_ADD(NOW(), INTERVAL 1 HOUR), NULL), \
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
