//! Automatic archival run by `WorkerPool::with_archival` (and `from_hammerwork_config`
//! with `[archive] enabled = true`): the pool archives eligible jobs and purges old
//! archived jobs on its interval, stops on shutdown, and concurrent archivers archive
//! each job exactly once. Each scenario runs against both backends.
//!
//! The pool archives every queue, so each test holds `test_utils::serial()`.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    HammerworkConfig, Job, JobId, JobQueue, JobStatus, Worker, WorkerPool,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason, JobArchiver},
    queue::DatabaseQueue,
    worker::JobHandler,
};
use serde_json::json;
use std::{
    collections::HashSet,
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

/// Archive completed jobs at once; keep archived jobs for an hour.
fn policy() -> ArchivalPolicy {
    ArchivalPolicy {
        archive_completed_after: Some(chrono::Duration::zero()),
        archive_failed_after: None,
        archive_dead_after: None,
        archive_timed_out_after: None,
        purge_archived_after: Some(chrono::Duration::hours(1)),
        compress_payloads: true,
        batch_size: 2,
        enabled: true,
    }
}

async fn completed_job<DB>(queue: &JobQueue<DB>, queue_name: &str) -> JobId
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB>,
{
    let id = queue
        .enqueue(Job::new(
            queue_name.to_string(),
            json!({"data": "x".repeat(200)}),
        ))
        .await
        .unwrap();
    queue.complete_job(id).await.unwrap();
    id
}

async fn archived_ids<DB>(queue: &JobQueue<DB>, queue_name: &str) -> HashSet<JobId>
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB>,
{
    queue
        .list_archived_jobs(Some(queue_name), None, None)
        .await
        .unwrap()
        .into_iter()
        .map(|job| job.id)
        .collect()
}

/// Polls `condition` every 50ms until it holds or 20 seconds pass.
async fn eventually<F, Fut>(condition: F) -> bool
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if condition().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A worker on a queue of its own, so it never touches the jobs under test.
fn idle_worker<DB>(queue: &Arc<JobQueue<DB>>) -> Worker<DB>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let handler: JobHandler = Arc::new(|_job: Job| Box::pin(async { Ok(()) }));
    Worker::new(
        Arc::clone(queue),
        test_utils::unique_queue("archival_idle"),
        handler,
    )
    .with_poll_interval(Duration::from_millis(50))
}

/// The pool archives eligible jobs (in several batches), leaves pending jobs alone,
/// purges archived jobs older than `purge_archived_after` and keeps newer ones, and
/// stops archiving once it is shut down.
async fn pool_archives_and_purges<DB, B, BFut>(queue: Arc<JobQueue<DB>>, backdate: B)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    B: Fn(Arc<JobQueue<DB>>, JobId, chrono::DateTime<Utc>) -> BFut,
    BFut: Future<Output = ()>,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("pool_archival");

    // Archived two hours ago: past purge_archived_after.
    let old = completed_job(&queue, &queue_name).await;
    let stats = queue
        .archive_jobs(
            Some(&queue_name),
            &policy(),
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            Some("test"),
        )
        .await
        .unwrap();
    assert_eq!(stats.jobs_archived, 1);
    backdate(
        Arc::clone(&queue),
        old,
        Utc::now() - chrono::Duration::hours(2),
    )
    .await;

    // Five eligible jobs: three batches of two.
    let mut eligible = Vec::new();
    for _ in 0..5 {
        eligible.push(completed_job(&queue, &queue_name).await);
    }
    let pending = queue
        .enqueue(Job::new(queue_name.clone(), json!({"keep": true})))
        .await
        .unwrap();

    let mut pool = WorkerPool::new()
        .without_autoscaling()
        .without_stale_job_reaper()
        .with_archival(policy(), ArchivalConfig::new(), Duration::from_millis(100));
    pool.add_worker(idle_worker(&queue));

    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        done = eventually(|| async {
            let archived = archived_ids(&queue, &queue_name).await;
            eligible.iter().all(|id| archived.contains(id)) && !archived.contains(&old)
        }) => assert!(done, "the pool archived the eligible jobs and purged the old one"),
    }
    pool.shutdown().await.unwrap();

    let archived = archived_ids(&queue, &queue_name).await;
    assert_eq!(
        archived,
        eligible.iter().copied().collect::<HashSet<_>>(),
        "recently archived jobs are kept"
    );
    for id in &eligible {
        let listing = queue
            .list_archived_jobs(Some(&queue_name), None, None)
            .await
            .unwrap();
        let job = listing.iter().find(|job| job.id == *id).unwrap();
        assert_eq!(job.status, JobStatus::Completed);
        assert_eq!(job.archived_by.as_deref(), Some("worker_pool"));
        assert_eq!(job.archival_reason, ArchivalReason::Automatic);
        assert!(job.payload_compressed);
    }
    let pending_job = queue.get_job(pending).await.unwrap().unwrap();
    assert_eq!(pending_job.status, JobStatus::Pending);

    // After shutdown nothing is archived any more.
    let late = completed_job(&queue, &queue_name).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!archived_ids(&queue, &queue_name).await.contains(&late));
    let late_job = queue.get_job(late).await.unwrap().unwrap();
    assert_eq!(late_job.status, JobStatus::Completed);

    // Clean up
    queue.delete_job(late).await.unwrap();
    queue.delete_job(pending).await.unwrap();
    for id in eligible {
        queue.restore_archived_job(id).await.unwrap();
        queue.delete_job(id).await.unwrap();
    }
}

/// `from_hammerwork_config` with `[archive] enabled = true` runs archival on
/// `check_interval`; with archiving disabled the pool does not archive.
async fn configured_pool_archives<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("cfg_archival");

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 1;
    config.archive.enabled = false;
    config.archive.archive_after = chrono::Duration::zero();
    config.archive.check_interval = Duration::from_millis(100);

    let job = completed_job(&queue, &queue_name).await;
    let mut pool = WorkerPool::from_hammerwork_config(idle_worker(&queue), &config)
        .unwrap()
        .without_stale_job_reaper();
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(500)) => {}
    }
    pool.shutdown().await.unwrap();
    assert!(
        archived_ids(&queue, &queue_name).await.is_empty(),
        "archiving is off unless enabled"
    );

    config.archive.enabled = true;
    let mut pool = WorkerPool::from_hammerwork_config(idle_worker(&queue), &config)
        .unwrap()
        .without_stale_job_reaper();
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        done = eventually(|| async { archived_ids(&queue, &queue_name).await.contains(&job) }) => {
            assert!(done, "the configured pool archived the job");
        }
    }
    pool.shutdown().await.unwrap();

    queue.restore_archived_job(job).await.unwrap();
    queue.delete_job(job).await.unwrap();
}

/// Two archivers running a scheduled pass at the same time (as two pools or processes
/// would) archive every eligible job exactly once and neither fails.
async fn concurrent_passes_archive_each_job_once<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("concurrent_archival");
    let mut ids = HashSet::new();
    for _ in 0..40 {
        ids.insert(completed_job(&queue, &queue_name).await);
    }

    let archiver = || {
        JobArchiver::new(queue.pool.clone())
            .with_default_policy(ArchivalPolicy {
                purge_archived_after: None,
                batch_size: 5,
                ..policy()
            })
            .with_config(ArchivalConfig::new())
    };
    let (first, second) = (archiver(), archiver());
    let (a, b) = tokio::join!(
        first.run_scheduled_pass(queue.as_ref(), Some("a"), || false),
        second.run_scheduled_pass(queue.as_ref(), Some("b"), || false),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(a.jobs_archived + b.jobs_archived >= ids.len() as u64);
    assert!(!a.stopped_early && !b.stopped_early);

    let listing = queue
        .list_archived_jobs(Some(&queue_name), None, None)
        .await
        .unwrap();
    assert_eq!(listing.len(), ids.len(), "each job archived exactly once");
    assert_eq!(
        listing.iter().map(|job| job.id).collect::<HashSet<_>>(),
        ids
    );
    for id in &ids {
        assert!(
            queue
                .get_job(*id)
                .await
                .unwrap()
                .is_none_or(|job| job.status == JobStatus::Archived),
            "the job left hammerwork_jobs"
        );
    }

    // A pass asked to stop archives nothing.
    let more = completed_job(&queue, &queue_name).await;
    let stopped = archiver()
        .run_scheduled_pass(queue.as_ref(), None, || true)
        .await
        .unwrap();
    assert!(stopped.stopped_early);
    assert_eq!(stopped.jobs_archived, 0);
    assert_eq!(stopped.jobs_purged, 0);
    assert!(!archived_ids(&queue, &queue_name).await.contains(&more));

    queue.delete_job(more).await.unwrap();
    for id in ids {
        queue.restore_archived_job(id).await.unwrap();
        queue.delete_job(id).await.unwrap();
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    async fn queue() -> Arc<JobQueue<sqlx::Postgres>> {
        test_utils::setup_postgres_queue().await
    }

    async fn backdate(queue: Arc<JobQueue<sqlx::Postgres>>, id: JobId, at: chrono::DateTime<Utc>) {
        sqlx::query("UPDATE hammerwork_jobs_archive SET archived_at = $1 WHERE id = $2")
            .bind(at)
            .bind(id)
            .execute(&queue.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_pool_archives_and_purges() {
        pool_archives_and_purges(queue().await, backdate).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_configured_pool_archives() {
        configured_pool_archives(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_concurrent_passes_archive_each_job_once() {
        concurrent_passes_archive_each_job_once(queue().await).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    async fn queue() -> Arc<JobQueue<sqlx::MySql>> {
        test_utils::setup_mysql_queue().await
    }

    async fn backdate(queue: Arc<JobQueue<sqlx::MySql>>, id: JobId, at: chrono::DateTime<Utc>) {
        sqlx::query("UPDATE hammerwork_jobs_archive SET archived_at = ? WHERE id = ?")
            .bind(at)
            .bind(id.to_string())
            .execute(&queue.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_pool_archives_and_purges() {
        pool_archives_and_purges(queue().await, backdate).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_configured_pool_archives() {
        configured_pool_archives(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_concurrent_passes_archive_each_job_once() {
        concurrent_passes_archive_each_job_once(queue().await).await;
    }
}
