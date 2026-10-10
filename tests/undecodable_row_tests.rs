//! #71: listings skip rows they cannot decode instead of failing as a whole, while reads
//! of a single job keep returning the error. The corrupt rows are written with raw SQL
//! (a negative `timeout_seconds`, as older versions could store) and removed afterwards.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Duration;
use hammerwork::{
    Job, JobId,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason},
    cron::CronSchedule,
    queue::DatabaseQueue,
};
use serde_json::json;
use std::collections::HashSet;
use std::future::Future;

fn ids<'a>(ids: impl IntoIterator<Item = &'a JobId>) -> HashSet<JobId> {
    ids.into_iter().copied().collect()
}

/// Every listing of `queue` returns the healthy jobs and skips the corrupt ones.
///
/// `sql` runs one statement on the queue's database. The statements only contain ids and
/// queue names this test generated.
async fn listings_skip_undecodable_rows<Q, S, F>(queue: &Q, sql: S)
where
    Q: DatabaseQueue + Send + Sync,
    S: Fn(String) -> F,
    F: Future<Output = ()>,
{
    let name = test_utils::unique_queue("undecodable");
    let archive_name = test_utils::unique_queue("undecodable_archive");
    let corrupt = |id: JobId, table: &str| {
        let column = if table == "hammerwork_jobs" {
            "timeout_seconds"
        } else {
            "original_payload_size"
        };
        sql(format!(
            "UPDATE {table} SET {column} = -1 WHERE id = '{id}'"
        ))
    };
    let enqueue = |queue_name: &str| queue.enqueue(Job::new(queue_name.to_string(), json!({})));

    // Ready jobs
    let ready = enqueue(&name).await.unwrap();
    let bad_ready = enqueue(&name).await.unwrap();
    corrupt(bad_ready, "hammerwork_jobs").await;
    let listed = queue.get_ready_jobs(&name, 10).await.unwrap();
    assert_eq!(ids(listed.iter().map(|j| &j.id)), ids([&ready]));

    // A single read still reports the corrupt row.
    let err = queue.get_job(bad_ready).await.unwrap_err();
    assert!(err.to_string().contains("timeout_seconds"), "{err}");

    // Dead jobs, per queue and overall
    let dead = enqueue(&name).await.unwrap();
    queue.mark_job_dead(dead, "gone").await.unwrap();
    let bad_dead = enqueue(&name).await.unwrap();
    queue.mark_job_dead(bad_dead, "gone").await.unwrap();
    corrupt(bad_dead, "hammerwork_jobs").await;
    let listed = queue
        .get_dead_jobs_by_queue(&name, Some(10), Some(0))
        .await
        .unwrap();
    assert_eq!(ids(listed.iter().map(|j| &j.id)), ids([&dead]));
    let all_dead = ids(queue
        .get_dead_jobs(Some(10_000), Some(0))
        .await
        .unwrap()
        .iter()
        .map(|j| &j.id));
    assert!(all_dead.contains(&dead) && !all_dead.contains(&bad_dead));

    // Recurring jobs
    let hourly = CronSchedule::new("0 0 * * * *").unwrap();
    let cron_job = || Job::with_cron_schedule(name.clone(), json!({}), hourly.clone()).unwrap();
    let recurring = queue.enqueue_cron_job(cron_job()).await.unwrap();
    let bad_recurring = queue.enqueue_cron_job(cron_job()).await.unwrap();
    corrupt(bad_recurring, "hammerwork_jobs").await;
    let listed = queue.get_recurring_jobs(&name).await.unwrap();
    assert_eq!(ids(listed.iter().map(|j| &j.id)), ids([&recurring]));

    // Archived jobs
    let archived = enqueue(&archive_name).await.unwrap();
    let bad_archived = enqueue(&archive_name).await.unwrap();
    for id in [archived, bad_archived] {
        queue.mark_job_dead(id, "gone").await.unwrap();
    }
    let policy = ArchivalPolicy::new()
        .archive_dead_after(Duration::seconds(0))
        .enabled(true);
    let stats = queue
        .archive_jobs(
            Some(&archive_name),
            &policy,
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            None,
        )
        .await
        .unwrap();
    assert_eq!(stats.jobs_archived, 2);
    corrupt(bad_archived, "hammerwork_jobs_archive").await;
    let listed = queue
        .list_archived_jobs(Some(&archive_name), Some(10), Some(0))
        .await
        .unwrap();
    assert_eq!(ids(listed.iter().map(|j| &j.id)), ids([&archived]));

    for (table, queue_name) in [
        ("hammerwork_jobs", &name),
        ("hammerwork_jobs_archive", &archive_name),
    ] {
        sql(format!(
            "DELETE FROM {table} WHERE queue_name = '{queue_name}'"
        ))
        .await;
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires PostgreSQL: DATABASE_URL"]
async fn postgres_listings_skip_undecodable_rows() {
    let queue = test_utils::setup_postgres_queue().await;
    let pool = sqlx::PgPool::connect(&test_utils::postgres_url())
        .await
        .unwrap();
    listings_skip_undecodable_rows(queue.as_ref(), |statement| {
        let pool = pool.clone();
        async move {
            sqlx::query(&statement).execute(&pool).await.unwrap();
        }
    })
    .await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
async fn mysql_listings_skip_undecodable_rows() {
    let queue = test_utils::setup_mysql_queue().await;
    let pool = sqlx::MySqlPool::connect(&test_utils::mysql_url())
        .await
        .unwrap();
    listings_skip_undecodable_rows(queue.as_ref(), |statement| {
        let pool = pool.clone();
        async move {
            sqlx::query(&statement).execute(&pool).await.unwrap();
        }
    })
    .await;
}
