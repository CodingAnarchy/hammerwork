//! #71: listings skip rows they cannot decode instead of failing as a whole, while reads
//! of a single job keep returning the error. The corrupt rows are written with raw SQL
//! (a negative `timeout_seconds`, as older versions could store) and removed afterwards.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Duration;
use hammerwork::queue::DequeueOptions;
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
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .unwrap();
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
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .unwrap();
        }
    })
    .await;
}

/// The stale job reaper reclaims a job whose row cannot be decoded (requeued, or dead
/// once out of attempts) instead of failing, which would leave every stale job of every
/// queue unreclaimed for as long as the corrupt row stays `Running`.
async fn reaper_reclaims_undecodable_rows<Q, S, F>(queue: &Q, sql: S)
where
    Q: DatabaseQueue + Send + Sync,
    S: Fn(String) -> F,
    F: Future<Output = ()>,
{
    let name = test_utils::unique_queue("undecodable_reaper");
    let enqueue = |max_attempts: i32| {
        queue.enqueue(Job::new(name.clone(), json!({})).with_max_attempts(max_attempts))
    };
    let healthy = enqueue(3).await.unwrap();
    let bad_requeued = enqueue(3).await.unwrap();
    let bad_dead = enqueue(1).await.unwrap();

    // Claim all three with a lease that has expired by the time the reaper runs.
    let claimed = queue
        .dequeue(
            &name,
            &DequeueOptions::batch(10).with_lease_duration(std::time::Duration::from_millis(1)),
        )
        .await
        .unwrap();
    assert_eq!(claimed.len(), 3);
    for id in [bad_requeued, bad_dead] {
        sql(format!(
            "UPDATE hammerwork_jobs SET timeout_seconds = -1 WHERE id = '{id}'"
        ))
        .await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Another test's reaper may reclaim some of them first; either way they end up
    // reclaimed and this call succeeds.
    queue
        .requeue_stale_jobs(None)
        .await
        .expect("an undecodable stale row must not fail the reaper");
    let counts = queue.get_job_counts_by_status(&name).await.unwrap();
    assert_eq!(counts.get("Pending"), Some(&2), "{counts:?}");
    assert_eq!(counts.get("Dead"), Some(&1), "{counts:?}");
    assert_eq!(
        queue.get_job(healthy).await.unwrap().unwrap().status,
        hammerwork::JobStatus::Pending
    );

    sql(format!(
        "DELETE FROM hammerwork_jobs WHERE queue_name = '{name}'"
    ))
    .await;
}

/// A job whose row cannot be decoded does not block its queue: a claim takes it out of
/// the way (it stays `Running` until its lease expires, then the reaper handles it) and
/// the next claim gets the jobs behind it, for single and batch claims alike.
async fn undecodable_rows_do_not_block_claims<Q, S, F>(queue: &Q, sql: S)
where
    Q: DatabaseQueue + Send + Sync,
    S: Fn(String) -> F,
    F: Future<Output = ()>,
{
    use hammerwork::priority::JobPriority;

    for batch in [false, true] {
        let name = test_utils::unique_queue("undecodable_claim");
        let bad = queue
            .enqueue(Job::new(name.clone(), json!({})).with_priority(JobPriority::Critical))
            .await
            .unwrap();
        let healthy = queue
            .enqueue(Job::new(name.clone(), json!({})))
            .await
            .unwrap();
        sql(format!(
            "UPDATE hammerwork_jobs SET timeout_seconds = -1 WHERE id = '{bad}'"
        ))
        .await;

        // The first claim takes the corrupt job (an error for a single claim, nothing
        // for a batch); the healthy job behind it is claimed next.
        let mut claimed: Vec<JobId> = Vec::new();
        for _ in 0..3 {
            let jobs = if batch {
                queue.dequeue(&name, &DequeueOptions::batch(1)).await
            } else {
                queue
                    .dequeue_one(&name)
                    .await
                    .map(|job| job.into_iter().collect())
            };
            if let Ok(jobs) = jobs {
                claimed.extend(jobs.into_iter().map(|job| job.id));
            }
            if !claimed.is_empty() {
                break;
            }
        }
        assert_eq!(
            claimed,
            vec![healthy],
            "the corrupt job blocked the queue (batch: {batch})"
        );

        sql(format!(
            "DELETE FROM hammerwork_jobs WHERE queue_name = '{name}'"
        ))
        .await;
    }
}

#[cfg(feature = "postgres")]
fn postgres_sql(
    pool: sqlx::PgPool,
) -> impl Fn(String) -> std::pin::Pin<Box<dyn Future<Output = ()>>> {
    move |statement| {
        let pool = pool.clone();
        Box::pin(async move {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .unwrap();
        })
    }
}

#[cfg(feature = "mysql")]
fn mysql_sql(
    pool: sqlx::MySqlPool,
) -> impl Fn(String) -> std::pin::Pin<Box<dyn Future<Output = ()>>> {
    move |statement| {
        let pool = pool.clone();
        Box::pin(async move {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
                .unwrap();
        })
    }
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires PostgreSQL: DATABASE_URL"]
async fn postgres_reaper_reclaims_undecodable_rows() {
    let queue = test_utils::setup_postgres_queue().await;
    let pool = sqlx::PgPool::connect(&test_utils::postgres_url())
        .await
        .unwrap();
    reaper_reclaims_undecodable_rows(queue.as_ref(), postgres_sql(pool)).await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
async fn mysql_reaper_reclaims_undecodable_rows() {
    let queue = test_utils::setup_mysql_queue().await;
    let pool = sqlx::MySqlPool::connect(&test_utils::mysql_url())
        .await
        .unwrap();
    reaper_reclaims_undecodable_rows(queue.as_ref(), mysql_sql(pool)).await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires PostgreSQL: DATABASE_URL"]
async fn postgres_undecodable_rows_do_not_block_claims() {
    let queue = test_utils::setup_postgres_queue().await;
    let pool = sqlx::PgPool::connect(&test_utils::postgres_url())
        .await
        .unwrap();
    undecodable_rows_do_not_block_claims(queue.as_ref(), postgres_sql(pool)).await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
async fn mysql_undecodable_rows_do_not_block_claims() {
    let queue = test_utils::setup_mysql_queue().await;
    let pool = sqlx::MySqlPool::connect(&test_utils::mysql_url())
        .await
        .unwrap();
    undecodable_rows_do_not_block_claims(queue.as_ref(), mysql_sql(pool)).await;
}

/// A job whose row cannot be decoded does not stop the archiver: the healthy jobs of the
/// pass are archived and the corrupt row is skipped (and left in `hammerwork_jobs`),
/// instead of the whole pass failing and rolling back on every run.
async fn undecodable_rows_do_not_block_archival<Q, S, F>(queue: &Q, sql: S)
where
    Q: DatabaseQueue + Send + Sync,
    S: Fn(String) -> F,
    F: Future<Output = ()>,
{
    let name = test_utils::unique_queue("undecodable_archiver");
    // The corrupt jobs are the oldest and fill a whole batch, so they come first.
    let mut bad = Vec::new();
    for _ in 0..3 {
        bad.push(
            queue
                .enqueue(Job::new(name.clone(), json!({})))
                .await
                .unwrap(),
        );
    }
    let healthy = queue
        .enqueue(Job::new(name.clone(), json!({})))
        .await
        .unwrap();
    for id in bad.iter().chain([&healthy]) {
        queue.complete_job(*id).await.unwrap();
    }
    for id in &bad {
        sql(format!(
            "UPDATE hammerwork_jobs SET timeout_seconds = -1 WHERE id = '{id}'"
        ))
        .await;
    }

    // One batch: the undecodable rows are skipped and the batch topped up.
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::seconds(0))
        .with_batch_size(2)
        .enabled(true);
    let stats = queue
        .archive_jobs(
            Some(&name),
            &policy,
            &ArchivalConfig::new(),
            ArchivalReason::Automatic,
            None,
        )
        .await
        .expect("an undecodable row must not fail the archiver");
    assert_eq!(stats.jobs_archived, 1);
    let archived = queue
        .list_archived_jobs(Some(&name), Some(10), None)
        .await
        .unwrap();
    assert_eq!(
        archived.iter().map(|job| job.id).collect::<Vec<_>>(),
        vec![healthy]
    );

    sql(format!(
        "DELETE FROM hammerwork_jobs WHERE queue_name = '{name}'"
    ))
    .await;
    sql(format!(
        "DELETE FROM hammerwork_jobs_archive WHERE queue_name = '{name}'"
    ))
    .await;
}

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires PostgreSQL: DATABASE_URL"]
async fn postgres_undecodable_rows_do_not_block_archival() {
    let queue = test_utils::setup_postgres_queue().await;
    let pool = sqlx::PgPool::connect(&test_utils::postgres_url())
        .await
        .unwrap();
    undecodable_rows_do_not_block_archival(queue.as_ref(), postgres_sql(pool)).await;
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
async fn mysql_undecodable_rows_do_not_block_archival() {
    let queue = test_utils::setup_mysql_queue().await;
    let pool = sqlx::MySqlPool::connect(&test_utils::mysql_url())
        .await
        .unwrap();
    undecodable_rows_do_not_block_archival(queue.as_ref(), mysql_sql(pool)).await;
}
