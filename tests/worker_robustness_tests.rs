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

/// Enqueue a job and claim it with `lease`, leaving it `Running` as a crashed worker
/// would. Returns the run.
async fn start_job<DB>(queue: &Arc<JobQueue<DB>>, job: Job, lease: Duration) -> Job
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = job.queue_name.clone();
    let id = queue.enqueue(job).await.unwrap();
    let claimed = queue
        .dequeue_leased(&queue_name, None, lease)
        .await
        .unwrap()
        .expect("job claimed");
    assert_eq!(claimed.id, id);
    claimed
}

/// Raw access the generic scenarios need to simulate rows written by older versions.
#[async_trait::async_trait]
trait LegacyRows {
    /// Remove the lease of a `Running` job, as left by a version that claimed jobs
    /// without writing one.
    async fn forget_lease(&self, id: JobId);
}

#[cfg(feature = "postgres")]
#[async_trait::async_trait]
impl LegacyRows for JobQueue<sqlx::Postgres> {
    async fn forget_lease(&self, id: JobId) {
        sqlx::query(
            "UPDATE hammerwork_jobs SET last_heartbeat_at = NULL, lease_expires_at = NULL \
             WHERE id = $1",
        )
        .bind(id)
        .execute(self.get_pool())
        .await
        .unwrap();
    }
}

#[cfg(feature = "mysql")]
#[async_trait::async_trait]
impl LegacyRows for JobQueue<sqlx::MySql> {
    async fn forget_lease(&self, id: JobId) {
        sqlx::query(
            "UPDATE hammerwork_jobs SET last_heartbeat_at = NULL, lease_expires_at = NULL \
             WHERE id = ?",
        )
        .bind(id.to_string())
        .execute(self.get_pool())
        .await
        .unwrap();
    }
}

/// Jobs whose lease expired go back to `Pending` (or `Dead` when out of attempts);
/// jobs with a live lease, or without a lease but younger than `older_than`, stay.
async fn reaper_requeues_expired_leases<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + LegacyRows + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let hour = Duration::from_secs(3600);
    let short = Duration::from_millis(500);

    let expiring = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_expiring"), json!({})).with_max_attempts(3),
        short,
    )
    .await;
    let exhausted = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_exhausted"), json!({})).with_max_attempts(1),
        short,
    )
    .await;
    let live = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_live"), json!({})),
        short,
    )
    .await;
    let unleased = start_job(
        &queue,
        Job::new(test_utils::unique_queue("reaper_unleased"), json!({})),
        short,
    )
    .await;
    queue.forget_lease(unleased.id).await;

    assert!(queue.heartbeat_job(&live, hour).await.unwrap());

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let recovery = queue.requeue_stale_jobs(hour).await.unwrap();
    assert!(recovery.requeued.contains(&expiring.id), "{recovery:?}");
    assert!(recovery.dead.contains(&exhausted.id), "{recovery:?}");
    for id in [live.id, unleased.id] {
        assert!(
            !recovery.requeued.contains(&id) && !recovery.dead.contains(&id),
            "job {id} must not be reclaimed: {recovery:?}"
        );
    }

    let job = queue.get_job(expiring.id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Pending);
    assert_eq!(job.attempts, 1, "the interrupted run counts as an attempt");
    assert!(job.started_at.is_none());
    assert!(job.error_message.unwrap().contains("lease expired"));
    // The requeued job can be claimed again.
    let reclaimed = queue.dequeue(&job.queue_name).await.unwrap().unwrap();
    assert_eq!(reclaimed.id, expiring.id);
    assert_eq!(reclaimed.attempts, 2);

    let job = queue.get_job(exhausted.id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Dead);
    assert!(job.failed_at.is_some());
    // A worker that lost its lease finds out on its next heartbeat.
    assert!(
        !queue
            .heartbeat_job(&exhausted, Duration::from_secs(1))
            .await
            .unwrap()
    );

    // Without a lease (claimed by an older version), `older_than` (measured from
    // `started_at`) decides.
    let recovery = queue
        .requeue_stale_jobs(Duration::from_millis(500))
        .await
        .unwrap();
    assert!(recovery.requeued.contains(&unleased.id), "{recovery:?}");
    assert!(!recovery.requeued.contains(&live.id), "{recovery:?}");

    for id in [expiring.id, exhausted.id, live.id, unleased.id] {
        queue.delete_job(id).await.unwrap();
    }
}

/// #64 C1: a job is leased from the moment it is claimed, so a reaper with a short
/// staleness window (another pool's, or `cargo hammerwork job requeue-stale
/// --older-than-secs 1`) never reclaims it while its lease is valid, even before the
/// worker's first heartbeat. Once the lease expires it is reclaimed.
async fn claim_lease_protects_job_from_short_reaper_window<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;

    // A worker with a long lease (its first heartbeat would come after 20 minutes),
    // and one using the default lease of a plain `dequeue`.
    let long = start_job(
        &queue,
        Job::new(test_utils::unique_queue("lease_long"), json!({})),
        Duration::from_secs(3600),
    )
    .await;
    let queue_name = test_utils::unique_queue("lease_default");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();
    let default = queue.dequeue(&queue_name).await.unwrap().unwrap();
    assert_eq!(default.id, id);
    let short = start_job(
        &queue,
        Job::new(test_utils::unique_queue("lease_short"), json!({})),
        Duration::from_millis(400),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(1200)).await;

    // Reapers with any window, down to zero, leave valid leases alone.
    for older_than in [
        Duration::ZERO,
        Duration::from_millis(1),
        Duration::from_secs(1),
    ] {
        let recovery = queue.requeue_stale_jobs(older_than).await.unwrap();
        for run in [&long, &default] {
            assert!(
                !recovery.requeued.contains(&run.id) && !recovery.dead.contains(&run.id),
                "job {} was reclaimed while its lease is valid: {recovery:?}",
                run.id
            );
        }
        if older_than.is_zero() {
            assert!(recovery.requeued.contains(&short.id), "{recovery:?}");
        }
    }
    for run in [&long, &default] {
        let job = queue.get_job(run.id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.attempts, 1);
        // The worker still holds the run, so it can extend and finish it.
        assert!(
            queue
                .heartbeat_job(run, Duration::from_secs(60))
                .await
                .unwrap()
        );
    }
    assert_eq!(
        queue.get_job(short.id).await.unwrap().unwrap().status,
        JobStatus::Pending
    );

    for id in [long.id, default.id, short.id] {
        queue.delete_job(id).await.unwrap();
    }
}

/// #64 C1: after a run is reclaimed and claimed again, the first run's worker (still
/// alive, still heartbeating) cannot extend the new run's lease.
async fn stale_heartbeat_cannot_extend_new_run<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let lease = Duration::from_millis(400);
    let first = start_job(
        &queue,
        Job::new(test_utils::unique_queue("lease_rerun"), json!({})).with_max_attempts(5),
        lease,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    let recovery = queue
        .requeue_stale_jobs(Duration::from_secs(3600))
        .await
        .unwrap();
    assert!(recovery.requeued.contains(&first.id), "{recovery:?}");

    let second = queue
        .dequeue_leased(&first.queue_name, None, lease)
        .await
        .unwrap()
        .expect("requeued job");
    assert_eq!(second.id, first.id);
    assert_eq!(second.attempts, 2);

    // The old run's heartbeats report the lost lease and change nothing.
    for _ in 0..3 {
        assert!(
            !queue
                .heartbeat_job(&first, Duration::from_secs(3600))
                .await
                .unwrap()
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // So the new run's lease expired on schedule.
    let recovery = queue
        .requeue_stale_jobs(Duration::from_secs(3600))
        .await
        .unwrap();
    assert!(
        recovery.requeued.contains(&second.id),
        "the stale heartbeat extended the new run's lease: {recovery:?}"
    );
    assert!(
        !queue
            .heartbeat_job(&second, Duration::from_secs(60))
            .await
            .unwrap()
    );
    queue.delete_job(first.id).await.unwrap();
}

/// A zero lease never expires: the reaper never reclaims the job.
async fn zero_lease_is_never_reclaimed<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let run = start_job(
        &queue,
        Job::new(test_utils::unique_queue("lease_zero"), json!({})),
        Duration::ZERO,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let recovery = queue.requeue_stale_jobs(Duration::ZERO).await.unwrap();
    assert!(
        !recovery.requeued.contains(&run.id) && !recovery.dead.contains(&run.id),
        "{recovery:?}"
    );
    assert!(queue.heartbeat_job(&run, Duration::ZERO).await.unwrap());
    let recovery = queue.requeue_stale_jobs(Duration::ZERO).await.unwrap();
    assert!(!recovery.requeued.contains(&run.id), "{recovery:?}");
    assert_eq!(
        queue.get_job(run.id).await.unwrap().unwrap().status,
        JobStatus::Running
    );
    queue.delete_job(run.id).await.unwrap();
}

/// #64 C1 scenario A: a worker with a long lease runs a job while another pool's reaper
/// uses a tiny staleness window. The job runs once.
async fn other_pools_reaper_does_not_rerun_live_job<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("lease_two_pools");
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
                tokio::time::sleep(Duration::from_millis(1500)).await;
                Ok::<(), HammerworkError>(())
            })
        })
    };
    // Service A: two workers with a one hour lease, so no heartbeat during the job.
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(50))
        .with_lease_duration(Duration::from_secs(3600));
    let mut service_a = WorkerPool::new().without_stale_job_reaper();
    service_a.add_worker(worker.clone());
    service_a.add_worker(worker);
    // Service B: no workers on this queue, only an aggressive reaper.
    let idle_handler: JobHandler = Arc::new(|_job: Job| Box::pin(async { Ok(()) }));
    let mut service_b = WorkerPool::new()
        .with_stale_job_reaper(Duration::from_millis(100), Duration::from_millis(1));
    service_b.add_worker(Worker::new(
        Arc::clone(&queue),
        test_utils::unique_queue("lease_other_service"),
        idle_handler,
    ));

    tokio::select! {
        result = async { tokio::join!(service_a.start(), service_b.start()) } => {
            panic!("pools stopped early: {result:?}")
        }
        _ = wait_for_job(&queue, id, Duration::from_secs(10), |job| {
            job.status == JobStatus::Completed
        }) => {}
    }
    // Let the reaper run a few more passes, then stop both pools.
    tokio::time::sleep(Duration::from_millis(300)).await;
    service_a.shutdown().await.unwrap();
    service_b.shutdown().await.unwrap();

    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(job.attempts, 1, "the job was reclaimed and run again");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the job ran twice");
    queue.delete_job(id).await.unwrap();
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
        let run = start_job(
            &queue,
            Job::new(queue_name.clone(), json!({ "index": i })).with_max_attempts(3),
            Duration::from_millis(200),
        )
        .await;
        stale.insert(run.id);
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

macro_rules! backend_tests {
    ($module:ident, $setup:path, $ignore:meta, [$($scenario:ident),* $(,)?]) => {
        mod $module {
            use super::*;
            $(
                #[tokio::test]
                #[$ignore]
                async fn $scenario() {
                    super::$scenario($setup().await).await;
                }
            )*
        }
    };
}

#[cfg(feature = "postgres")]
backend_tests!(
    postgres_tests,
    test_utils::setup_postgres_queue,
    ignore = "requires PostgreSQL: DATABASE_URL",
    [
        handler_panic_fails_job_and_worker_survives,
        handler_panic_is_retried,
        graceful_shutdown_completes_in_flight_job,
        reaper_requeues_expired_leases,
        claim_lease_protects_job_from_short_reaper_window,
        stale_heartbeat_cannot_extend_new_run,
        zero_lease_is_never_reclaimed,
        other_pools_reaper_does_not_rerun_live_job,
        concurrent_reapers_never_double_requeue,
        worker_heartbeat_keeps_long_job_leased,
    ]
);

#[cfg(feature = "mysql")]
backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        handler_panic_fails_job_and_worker_survives,
        handler_panic_is_retried,
        graceful_shutdown_completes_in_flight_job,
        reaper_requeues_expired_leases,
        claim_lease_protects_job_from_short_reaper_window,
        stale_heartbeat_cannot_extend_new_run,
        zero_lease_is_never_reclaimed,
        other_pools_reaper_does_not_rerun_live_job,
        concurrent_reapers_never_double_requeue,
        worker_heartbeat_keeps_long_job_leased,
    ]
);
