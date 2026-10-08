//! Worker robustness and stale job recovery tests.
//!
//! Covers handler panics, graceful shutdown with in-flight jobs, job leases and the
//! stale job reaper (`DatabaseQueue::requeue_stale_jobs`). Each scenario is written
//! once against the `DatabaseQueue` trait and run against both backends.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    HammerworkError, Job, JobId, JobQueue, JobStatus, Worker, WorkerPool, queue::DatabaseQueue,
    worker::JobHandler,
};
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

/// Poll `get_job` until `predicate` holds or `timeout` elapses; returns the last job.
async fn wait_for_job<DB, F>(
    queue: &Arc<JobQueue<DB>>,
    id: JobId,
    timeout: Duration,
    predicate: F,
) -> Job
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    F: Fn(&Job) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let job = queue.get_job(id).await.unwrap().expect("job exists");
        if predicate(&job) || Instant::now() >= deadline {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A panicking handler must fail the job through the normal path (not leave it
/// `Running`) and must not kill the worker.
async fn handler_panic_fails_job_and_worker_survives<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    // The reaper tests reclaim stale jobs across all queues; keep them apart.
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("panic_handler");
    let panicking = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "panic": true })).with_max_attempts(1))
        .await
        .unwrap();

    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            if job.payload["panic"].as_bool() == Some(true) {
                panic!("handler exploded");
            }
            Ok(())
        })
    });
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(50))
        .with_max_retries(1);

    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let worker_task = tokio::spawn(async move { worker.run(shutdown_rx).await });

    let job = wait_for_job(&queue, panicking, Duration::from_secs(10), |job| {
        job.status != JobStatus::Running && job.status != JobStatus::Pending
    })
    .await;
    assert_eq!(job.status, JobStatus::Dead, "panicked job must be failed");
    let error = job.error_message.unwrap_or_default();
    assert!(
        error.contains("Job handler panicked: handler exploded"),
        "unexpected error message: {error}"
    );

    // The same worker keeps processing jobs after the panic.
    let healthy = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "panic": false })))
        .await
        .unwrap();
    let job = wait_for_job(&queue, healthy, Duration::from_secs(10), |job| {
        job.status == JobStatus::Completed
    })
    .await;
    assert_eq!(job.status, JobStatus::Completed, "worker died after panic");
    assert!(
        !worker_task.is_finished(),
        "worker task must still be running"
    );

    shutdown_tx.send(()).await.unwrap();
    worker_task.await.unwrap().unwrap();

    queue.delete_job(panicking).await.unwrap();
    queue.delete_job(healthy).await.unwrap();
}

/// A panicking handler with retries left must be rescheduled, not left `Running`.
async fn handler_panic_is_retried<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    // The reaper tests reclaim stale jobs across all queues; keep them apart.
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("panic_retry");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})).with_max_attempts(5))
        .await
        .unwrap();

    let handler: JobHandler =
        Arc::new(|_job: Job| Box::pin(async move { panic!("transient explosion") }));
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(50))
        .with_max_retries(5)
        .with_retry_delay(Duration::from_secs(3600));

    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let worker_task = tokio::spawn(async move { worker.run(shutdown_rx).await });

    let job = wait_for_job(&queue, id, Duration::from_secs(10), |job| {
        job.attempts >= 1 && job.status == JobStatus::Pending
    })
    .await;
    assert_eq!(
        job.status,
        JobStatus::Pending,
        "panicked job must be retried"
    );
    assert_eq!(job.attempts, 1);
    assert!(job.scheduled_at > chrono::Utc::now() + chrono::Duration::minutes(30));

    shutdown_tx.send(()).await.unwrap();
    worker_task.await.unwrap().unwrap();
    queue.delete_job(id).await.unwrap();
}

/// `WorkerPool::shutdown` must let an in-flight job finish (within the grace period)
/// and record its outcome, instead of cancelling it mid-run.
async fn graceful_shutdown_completes_in_flight_job<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    // The reaper tests reclaim stale jobs across all queues; keep them apart.
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("graceful_shutdown");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();

    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let handler: JobHandler = {
        let started = Arc::clone(&started);
        let finished = Arc::clone(&finished);
        Arc::new(move |_job: Job| {
            let started = Arc::clone(&started);
            let finished = Arc::clone(&finished);
            Box::pin(async move {
                started.store(true, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_secs(2)).await;
                finished.store(true, Ordering::SeqCst);
                Ok(())
            })
        })
    };

    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(50))
        .with_shutdown_grace_period(Duration::from_secs(20));
    let mut pool = WorkerPool::new()
        .without_autoscaling()
        .without_stale_job_reaper();
    pool.add_worker(worker);

    // Run the pool until the job has started, then drop the `start` future: the
    // workers keep running in their own tasks.
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        _ = async {
            while !started.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        } => {}
    }
    assert!(!finished.load(Ordering::SeqCst));

    let shutdown_started = Instant::now();
    pool.shutdown().await.unwrap();
    assert!(
        finished.load(Ordering::SeqCst),
        "shutdown must wait for the in-flight job"
    );
    assert!(shutdown_started.elapsed() < Duration::from_secs(15));

    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(
        job.status,
        JobStatus::Completed,
        "in-flight job must complete"
    );
    queue.delete_job(id).await.unwrap();
}

/// Enqueue a job and claim it, leaving it `Running` as a crashed worker would.
async fn start_job<DB>(queue: &Arc<JobQueue<DB>>, job: Job) -> JobId
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = job.queue_name.clone();
    let id = queue.enqueue(job).await.unwrap();
    let claimed = queue
        .dequeue(&queue_name)
        .await
        .unwrap()
        .expect("job claimed");
    assert_eq!(claimed.id, id);
    id
}

/// Jobs whose lease expired go back to `Pending` (or `Dead` when out of attempts);
/// jobs with a live lease, or without a lease but younger than `older_than`, stay.
async fn reaper_requeues_expired_leases<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let hour = Duration::from_secs(3600);

    let expiring = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_expiring"), json!({})).with_max_attempts(3),
    )
    .await;
    let exhausted = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_exhausted"), json!({})).with_max_attempts(1),
    )
    .await;
    let live = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_live"), json!({})),
    )
    .await;
    let unleased = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_unleased"), json!({})),
    )
    .await;

    assert!(
        queue
            .heartbeat_job(expiring, Duration::from_millis(500))
            .await
            .unwrap()
    );
    assert!(
        queue
            .heartbeat_job(exhausted, Duration::from_millis(500))
            .await
            .unwrap()
    );
    assert!(queue.heartbeat_job(live, hour).await.unwrap());

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let recovery = queue.requeue_stale_jobs(hour).await.unwrap();
    assert!(recovery.requeued.contains(&expiring), "{recovery:?}");
    assert!(recovery.dead.contains(&exhausted), "{recovery:?}");
    for id in [live, unleased] {
        assert!(
            !recovery.requeued.contains(&id) && !recovery.dead.contains(&id),
            "job {id} must not be reclaimed: {recovery:?}"
        );
    }

    let job = queue.get_job(expiring).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Pending);
    assert_eq!(job.attempts, 1, "the interrupted run counts as an attempt");
    assert!(job.started_at.is_none());
    assert!(job.error_message.unwrap().contains("lease expired"));
    // The requeued job can be claimed again.
    let reclaimed = queue.dequeue(&job.queue_name).await.unwrap().unwrap();
    assert_eq!(reclaimed.id, expiring);
    assert_eq!(reclaimed.attempts, 2);

    let job = queue.get_job(exhausted).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Dead);
    assert!(job.failed_at.is_some());
    // A worker that lost its lease finds out on its next heartbeat.
    assert!(
        !queue
            .heartbeat_job(exhausted, Duration::from_secs(1))
            .await
            .unwrap()
    );

    // Without a lease, `older_than` (measured from `started_at`) decides.
    let recovery = queue
        .requeue_stale_jobs(Duration::from_millis(500))
        .await
        .unwrap();
    assert!(recovery.requeued.contains(&unleased), "{recovery:?}");
    assert!(!recovery.requeued.contains(&live), "{recovery:?}");

    for id in [expiring, exhausted, live, unleased] {
        queue.delete_job(id).await.unwrap();
    }
}

/// Concurrent reapers must reclaim every stale job exactly once.
async fn concurrent_reapers_never_double_requeue<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("reaper_concurrency");
    let mut stale = HashSet::new();
    for i in 0..40 {
        let id = start_job(
            &queue,
            Job::new(queue_name.clone(), json!({ "index": i })).with_max_attempts(3),
        )
        .await;
        assert!(
            queue
                .heartbeat_job(id, Duration::from_millis(200))
                .await
                .unwrap()
        );
        stale.insert(id);
    }
    tokio::time::sleep(Duration::from_millis(800)).await;

    let mut reapers = Vec::new();
    for _ in 0..8 {
        let queue = Arc::clone(&queue);
        reapers.push(tokio::spawn(async move {
            queue
                .requeue_stale_jobs(Duration::from_secs(3600))
                .await
                .unwrap()
        }));
    }

    let mut reclaimed = HashSet::new();
    for reaper in reapers {
        let recovery = reaper.await.unwrap();
        for id in recovery.requeued.into_iter().chain(recovery.dead) {
            if stale.contains(&id) {
                assert!(reclaimed.insert(id), "job {id} was reclaimed twice");
            }
        }
    }
    assert_eq!(reclaimed, stale, "every stale job must be reclaimed");

    for id in stale {
        let job = queue.get_job(id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 1);
        queue.delete_job(id).await.unwrap();
    }
}

/// A worker heartbeats long-running jobs, so a reaper never reclaims a live job even
/// when the job outlives its original lease.
async fn worker_heartbeat_keeps_long_job_leased<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("heartbeat");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();

    let runs = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let runs = Arc::clone(&runs);
        Arc::new(move |_job: Job| {
            let runs = Arc::clone(&runs);
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2500)).await;
                Ok::<(), HammerworkError>(())
            })
        })
    };
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(50))
        .with_lease_duration(Duration::from_millis(900));
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let worker_task = tokio::spawn(async move { worker.run(shutdown_rx).await });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let recovery = queue
            .requeue_stale_jobs(Duration::from_secs(3600))
            .await
            .unwrap();
        assert!(
            !recovery.requeued.contains(&id) && !recovery.dead.contains(&id),
            "a live, heartbeating job was reclaimed"
        );
        let job = queue.get_job(id).await.unwrap().unwrap();
        if job.status == JobStatus::Completed || Instant::now() >= deadline {
            assert_eq!(job.status, JobStatus::Completed);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);

    shutdown_tx.send(()).await.unwrap();
    worker_task.await.unwrap().unwrap();
    queue.delete_job(id).await.unwrap();
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_handler_panic_fails_job_and_worker_survives() {
        handler_panic_fails_job_and_worker_survives(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_handler_panic_is_retried() {
        handler_panic_is_retried(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_graceful_shutdown_completes_in_flight_job() {
        graceful_shutdown_completes_in_flight_job(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_reaper_requeues_expired_leases() {
        reaper_requeues_expired_leases(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_concurrent_reapers_never_double_requeue() {
        concurrent_reapers_never_double_requeue(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_worker_heartbeat_keeps_long_job_leased() {
        worker_heartbeat_keeps_long_job_leased(test_utils::setup_postgres_queue().await).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_handler_panic_fails_job_and_worker_survives() {
        handler_panic_fails_job_and_worker_survives(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_handler_panic_is_retried() {
        handler_panic_is_retried(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_graceful_shutdown_completes_in_flight_job() {
        graceful_shutdown_completes_in_flight_job(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_reaper_requeues_expired_leases() {
        reaper_requeues_expired_leases(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_concurrent_reapers_never_double_requeue() {
        concurrent_reapers_never_double_requeue(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_worker_heartbeat_keeps_long_job_leased() {
        worker_heartbeat_keeps_long_job_leased(test_utils::setup_mysql_queue().await).await;
    }
}
