//! End-to-end tests for workers and worker pools built from configuration.
//!
//! Each scenario builds a worker (or pool) from a `HammerworkConfig` or the worker
//! builder methods, runs it against a real database and checks that it behaves as
//! configured: pool size, job timeout, retry strategy, priority weights, throttles,
//! autoscaling, pausing, error backoff and job hooks. Each scenario is written once and
//! run against both backends.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    HammerworkConfig, HammerworkError, Job, JobId, JobQueue, JobStatus, Worker, WorkerPool,
    priority::{JobPriority, PriorityWeights},
    queue::DatabaseQueue,
    rate_limit::ThrottleConfig,
    retry::RetryStrategy,
    worker::{JobHandler, JobHookEvent},
};
use serde_json::json;
use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

/// Poll `condition` until it holds or `timeout` elapses; returns whether it held.
async fn eventually<F, Fut>(timeout: Duration, mut condition: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if condition().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until every job in `ids` matches `predicate`.
async fn wait_for_jobs<DB, P>(
    queue: &Arc<JobQueue<DB>>,
    ids: &[JobId],
    timeout: Duration,
    predicate: P,
) -> bool
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    P: Fn(&Job) -> bool,
{
    eventually(timeout, || async {
        for id in ids {
            match queue.get_job(*id).await.unwrap() {
                Some(job) if predicate(&job) => {}
                _ => return false,
            }
        }
        true
    })
    .await
}

fn completed(job: &Job) -> bool {
    job.status == JobStatus::Completed
}

/// Run `pool` until `until` finishes, then shut it down gracefully.
async fn run_pool_until<DB, Fut>(pool: &mut WorkerPool<DB>, until: Fut)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    Fut: Future<Output = ()>,
{
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        () = until => {}
    }
    pool.shutdown().await.unwrap();
}

/// Counts handlers running at once and remembers the highest count.
#[derive(Clone, Default)]
struct Concurrency {
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl Concurrency {
    /// A handler that sleeps for `work` while counted as running.
    fn handler(&self, work: Duration) -> JobHandler {
        let this = self.clone();
        Arc::new(move |_job: Job| {
            let this = this.clone();
            Box::pin(async move {
                let now = this.running.fetch_add(1, Ordering::SeqCst) + 1;
                this.peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(work).await;
                this.running.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        })
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn running(&self) -> usize {
        self.running.load(Ordering::SeqCst)
    }

    fn reset_peak(&self) {
        self.peak.store(self.running(), Ordering::SeqCst);
    }
}

async fn enqueue_many<DB>(queue: &Arc<JobQueue<DB>>, queue_name: &str, count: usize) -> Vec<JobId>
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let mut ids = Vec::new();
    for i in 0..count {
        ids.push(
            queue
                .enqueue(Job::new(queue_name.to_string(), json!({ "i": i })))
                .await
                .unwrap(),
        );
    }
    ids
}

async fn delete_jobs<DB>(queue: &Arc<JobQueue<DB>>, ids: &[JobId])
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    for id in ids {
        queue.delete_job(*id).await.unwrap();
    }
}

/// A pool built with `WorkerPool::from_hammerwork_config` runs `worker.pool_size`
/// workers at once and no more.
async fn pool_runs_pool_size_workers<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_pool_size");
    let ids = enqueue_many(&queue, &queue_name, 9).await;

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 3;
    config.worker.polling_interval = Duration::from_millis(20);
    let concurrency = Concurrency::default();
    let worker = Worker::new(
        Arc::clone(&queue),
        queue_name.clone(),
        concurrency.handler(Duration::from_millis(300)),
    )
    .with_hammerwork_config(&config);
    let mut pool = WorkerPool::from_hammerwork_config(worker, &config)
        .unwrap()
        .without_stale_job_reaper();

    let started = Instant::now();
    run_pool_until(&mut pool, async {
        assert!(wait_for_jobs(&queue, &ids, Duration::from_secs(30), completed).await);
    })
    .await;
    let elapsed = started.elapsed();
    assert_eq!(concurrency.peak(), 3, "pool_size workers run at once");
    // 9 jobs of 300ms on 3 workers take at least 3 rounds.
    assert!(elapsed >= Duration::from_millis(850), "took {elapsed:?}");
    delete_jobs(&queue, &ids).await;
}

/// `worker.job_timeout` and `worker.retry_strategy` from the configuration apply to
/// jobs without their own timeout or strategy; a job's own settings win.
async fn config_timeout_and_retry_strategy_apply<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_timeout_retry");
    let slow = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "action": "sleep" })).with_max_attempts(3))
        .await
        .unwrap();
    let failing = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "action": "fail" })).with_max_attempts(3))
        .await
        .unwrap();
    // Its own strategy (a 2 minute delay) overrides the worker default (1 hour).
    let own_strategy = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({ "action": "fail" }))
                .with_max_attempts(3)
                .with_retry_strategy(RetryStrategy::fixed(Duration::from_secs(120))),
        )
        .await
        .unwrap();

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 1;
    config.worker.polling_interval = Duration::from_millis(20);
    config.worker.job_timeout = Duration::from_millis(200);
    config.worker.retry_strategy = RetryStrategy::fixed(Duration::from_secs(3600));

    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            match job.payload["action"].as_str() {
                Some("sleep") => {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok(())
                }
                _ => Err(HammerworkError::Worker {
                    message: "configured failure".to_string(),
                }),
            }
        })
    });
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_hammerwork_config(&config);
    let mut pool = WorkerPool::from_hammerwork_config(worker, &config)
        .unwrap()
        .without_stale_job_reaper();

    let before = Utc::now();
    let ids = [slow, failing, own_strategy];
    run_pool_until(&mut pool, async {
        assert!(
            wait_for_jobs(&queue, &ids, Duration::from_secs(20), |job| job.attempts
                == 1
                && job.status != JobStatus::Running)
            .await
        );
    })
    .await;

    let slow = queue.get_job(slow).await.unwrap().unwrap();
    assert_eq!(slow.status, JobStatus::Pending, "rescheduled for a retry");
    let error = slow.error_message.clone().unwrap_or_default();
    assert!(error.contains("timed out after 200ms"), "error: {error}");
    let delay = slow.scheduled_at - before;
    assert!(
        delay > chrono::Duration::minutes(59) && delay < chrono::Duration::minutes(61),
        "a timed-out job is retried with the configured strategy, got {delay}"
    );

    let failing = queue.get_job(failing).await.unwrap().unwrap();
    assert_eq!(
        failing.status,
        JobStatus::Pending,
        "rescheduled for a retry"
    );
    let delay = failing.scheduled_at - before;
    assert!(
        delay > chrono::Duration::minutes(59) && delay < chrono::Duration::minutes(61),
        "a failed job is retried with the configured strategy, got {delay}"
    );

    let own = queue.get_job(own_strategy).await.unwrap().unwrap();
    let delay = own.scheduled_at - before;
    assert!(
        delay > chrono::Duration::seconds(110) && delay < chrono::Duration::seconds(130),
        "the job's own strategy wins, got {delay}"
    );
    delete_jobs(&queue, &ids).await;
}

/// `worker.priority_weights = strict` makes the worker always take the highest
/// priority job first.
async fn config_strict_priority_orders_jobs<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_strict_priority");
    let mut ids = Vec::new();
    for priority in [
        JobPriority::Background,
        JobPriority::Low,
        JobPriority::Normal,
        JobPriority::High,
        JobPriority::Critical,
    ] {
        ids.push(
            queue
                .enqueue(
                    Job::new(
                        queue_name.clone(),
                        json!({ "priority": priority.to_string() }),
                    )
                    .with_priority(priority),
                )
                .await
                .unwrap(),
        );
    }

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 1;
    config.worker.polling_interval = Duration::from_millis(20);
    config.worker.priority_weights = PriorityWeights::strict();

    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&order);
    let handler: JobHandler = Arc::new(move |job: Job| {
        seen.lock().unwrap().push(job.priority);
        Box::pin(async { Ok(()) })
    });
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_hammerwork_config(&config);
    let mut pool = WorkerPool::from_hammerwork_config(worker, &config)
        .unwrap()
        .without_stale_job_reaper();
    run_pool_until(&mut pool, async {
        assert!(wait_for_jobs(&queue, &ids, Duration::from_secs(20), completed).await);
    })
    .await;

    assert_eq!(
        *order.lock().unwrap(),
        vec![
            JobPriority::Critical,
            JobPriority::High,
            JobPriority::Normal,
            JobPriority::Low,
            JobPriority::Background,
        ]
    );
    delete_jobs(&queue, &ids).await;
}

/// A queue throttle's `max_concurrent` caps how many jobs a whole pool runs at once.
async fn throttle_max_concurrent_caps_the_pool<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_max_concurrent");
    let ids = enqueue_many(&queue, &queue_name, 8).await;

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 4;
    config.worker.polling_interval = Duration::from_millis(20);
    config.rate_limiting.enabled = true;
    config
        .rate_limiting
        .queue_throttles
        .insert(queue_name.clone(), ThrottleConfig::new().max_concurrent(2));

    let concurrency = Concurrency::default();
    let worker = Worker::new(
        Arc::clone(&queue),
        queue_name.clone(),
        concurrency.handler(Duration::from_millis(200)),
    )
    .with_hammerwork_config(&config);
    let mut pool = WorkerPool::from_hammerwork_config(worker, &config)
        .unwrap()
        .without_stale_job_reaper();
    run_pool_until(&mut pool, async {
        assert!(wait_for_jobs(&queue, &ids, Duration::from_secs(30), completed).await);
    })
    .await;
    assert_eq!(
        concurrency.peak(),
        2,
        "4 workers, but the throttle allows 2 jobs at once"
    );
    delete_jobs(&queue, &ids).await;
}

/// A queue throttle's `rate_per_minute` limits how fast a pool takes jobs. The bucket
/// holds a minute's worth of tokens, so the first `rate` jobs run at once and the rest
/// at the configured rate.
async fn throttle_rate_limits_the_pool<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_rate_limit");
    // 60 per minute: 60 tokens up front, then one per second.
    let ids = enqueue_many(&queue, &queue_name, 62).await;

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 2;
    config.worker.polling_interval = Duration::from_millis(20);
    config.rate_limiting.enabled = true;
    config.rate_limiting.default_throttle = ThrottleConfig::new().rate_per_minute(60);

    let concurrency = Concurrency::default();
    let worker = Worker::new(
        Arc::clone(&queue),
        queue_name.clone(),
        concurrency.handler(Duration::ZERO),
    )
    .with_hammerwork_config(&config);
    let mut pool = WorkerPool::from_hammerwork_config(worker, &config)
        .unwrap()
        .without_stale_job_reaper();

    let started = Instant::now();
    run_pool_until(&mut pool, async {
        assert!(wait_for_jobs(&queue, &ids, Duration::from_secs(60), completed).await);
    })
    .await;
    let elapsed = started.elapsed();
    // Jobs 61 and 62 each wait for a new token (one per second).
    assert!(
        elapsed >= Duration::from_millis(1500),
        "62 jobs at 60/minute finished in {elapsed:?}"
    );
    delete_jobs(&queue, &ids).await;
}

/// An autoscaling pool starts workers from its template while the queue is deep (up
/// to `max_workers`) and retires them down to `min_workers` once it drains.
async fn autoscaling_adds_and_retires_workers<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_autoscale");
    let ids = enqueue_many(&queue, &queue_name, 24).await;

    let concurrency = Concurrency::default();
    let worker = Worker::new(
        Arc::clone(&queue),
        queue_name.clone(),
        concurrency.handler(Duration::from_millis(150)),
    )
    .with_poll_interval(Duration::from_millis(20));
    let config = hammerwork::config::WorkerConfig {
        pool_size: 1,
        autoscaling_enabled: true,
        min_workers: 1,
        max_workers: 3,
        ..Default::default()
    };
    let mut pool = WorkerPool::from_config(worker, &config)
        .with_autoscaling(
            config
                .autoscale_config()
                .with_scale_up_threshold(2)
                .with_scale_down_threshold(1)
                .with_cooldown_period(Duration::ZERO)
                .with_evaluation_window(Duration::from_millis(100)),
        )
        .without_stale_job_reaper();

    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        () = async {
            assert!(wait_for_jobs(&queue, &ids, Duration::from_secs(30), completed).await);
        } => {}
    }
    assert_eq!(
        concurrency.peak(),
        3,
        "the pool grows from 1 to max_workers (3) and no further"
    );

    // With the queue empty it scales back down to min_workers.
    let scaled_down = eventually(Duration::from_secs(10), || async {
        pool.get_autoscale_metrics().active_workers == 1
    })
    .await;
    assert!(scaled_down, "metrics: {:?}", pool.get_autoscale_metrics());
    let metrics = pool.get_autoscale_metrics();
    assert_eq!(metrics.current_queue_depth, 0);
    assert!(metrics.last_scale_time.is_some());

    // The retired workers stopped: two jobs (2 per worker, not above the scale-up
    // threshold) now run one at a time.
    concurrency.reset_peak();
    let more = enqueue_many(&queue, &queue_name, 2).await;
    assert!(wait_for_jobs(&queue, &more, Duration::from_secs(10), completed).await);
    assert_eq!(concurrency.peak(), 1, "only min_workers remain");

    pool.shutdown().await.unwrap();
    delete_jobs(&queue, &ids).await;
    delete_jobs(&queue, &more).await;
}

/// A worker leaves a paused queue alone and picks its jobs up once it is resumed.
async fn worker_waits_while_queue_is_paused<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_paused");
    queue.pause_queue(&queue_name, Some("test")).await.unwrap();
    let ids = enqueue_many(&queue, &queue_name, 2).await;

    let handled = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&handled);
    let handler: JobHandler = Arc::new(move |_job: Job| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    });
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(20));
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let task = tokio::spawn(async move { worker.run(shutdown_rx).await });

    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        handled.load(Ordering::SeqCst),
        0,
        "paused queue is not processed"
    );
    for id in &ids {
        assert_eq!(
            queue.get_job(*id).await.unwrap().unwrap().status,
            JobStatus::Pending
        );
    }

    queue.resume_queue(&queue_name, Some("test")).await.unwrap();
    assert!(wait_for_jobs(&queue, &ids, Duration::from_secs(10), completed).await);
    assert_eq!(handled.load(Ordering::SeqCst), 2);

    shutdown_tx.send(()).await.unwrap();
    task.await.unwrap().unwrap();
    delete_jobs(&queue, &ids).await;
}

/// The job hooks registered with `on_job_*` fire for each outcome.
async fn job_hooks_fire_for_each_outcome<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cfg_hooks");
    let ok = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "action": "ok" })))
        .await
        .unwrap();
    let retried = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "action": "fail" })).with_max_attempts(2))
        .await
        .unwrap();
    let failed = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "action": "fail" })).with_max_attempts(1))
        .await
        .unwrap();
    let timed_out = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({ "action": "sleep" }))
                .with_max_attempts(1)
                .with_timeout(Duration::from_millis(100)),
        )
        .await
        .unwrap();

    type Log = Arc<Mutex<Vec<(&'static str, JobId, Option<String>)>>>;
    let log: Log = Arc::default();
    let record = |name: &'static str, log: &Log| {
        let log = Arc::clone(log);
        move |event: JobHookEvent| {
            log.lock().unwrap().push((name, event.job.id, event.error));
        }
    };
    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            match job.payload["action"].as_str() {
                Some("ok") => Ok(()),
                Some("sleep") => {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    Ok(())
                }
                _ => Err(HammerworkError::Worker {
                    message: "hook failure".to_string(),
                }),
            }
        })
    });
    let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
        .with_poll_interval(Duration::from_millis(20))
        .with_retry_delay(Duration::from_secs(3600))
        .on_job_start(record("start", &log))
        .on_job_complete(record("complete", &log))
        .on_job_fail(record("fail", &log))
        .on_job_retry(record("retry", &log))
        .on_job_timeout(record("timeout", &log));
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let task = tokio::spawn(async move { worker.run(shutdown_rx).await });

    let ids = [ok, retried, failed, timed_out];
    assert!(
        wait_for_jobs(&queue, &ids, Duration::from_secs(20), |job| {
            job.attempts >= 1 && job.status != JobStatus::Running
        })
        .await
    );
    shutdown_tx.send(()).await.unwrap();
    task.await.unwrap().unwrap();

    let log = log.lock().unwrap().clone();
    let fired = |name: &str, id: JobId| log.iter().any(|(n, i, _)| *n == name && *i == id);
    for id in ids {
        assert!(fired("start", id), "start hook for every job: {log:?}");
    }
    assert!(fired("complete", ok));
    assert!(fired("retry", retried));
    assert!(
        !fired("fail", retried),
        "a retried job has not failed for good"
    );
    assert!(fired("fail", failed));
    assert!(fired("timeout", timed_out));
    assert!(!fired("complete", failed) && !fired("complete", timed_out));
    let retry_error = log
        .iter()
        .find(|(n, i, _)| *n == "retry" && *i == retried)
        .and_then(|(_, _, error)| error.clone())
        .unwrap_or_default();
    assert!(retry_error.contains("hook failure"), "error: {retry_error}");
    delete_jobs(&queue, &ids).await;
}

/// A worker whose database is unreachable keeps running (backing off between polls)
/// and still stops promptly when shut down during a long backoff.
async fn worker_backs_off_on_errors_and_stops_promptly<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let handled = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&handled);
    let handler: JobHandler = Arc::new(move |_job: Job| {
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    });
    let worker = Worker::new(
        Arc::clone(&queue),
        test_utils::unique_queue("cfg_backoff"),
        handler,
    )
    .with_poll_interval(Duration::from_millis(10))
    .with_throttle_config(ThrottleConfig::new().backoff_on_error(Duration::from_secs(60)));

    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let task = tokio::spawn(async move { worker.run(shutdown_rx).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!task.is_finished(), "dequeue errors do not stop the worker");

    let stopping = Instant::now();
    shutdown_tx.send(()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("shutdown interrupts the error backoff")
        .unwrap()
        .unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(5));
    assert_eq!(handled.load(Ordering::SeqCst), 0);
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    async fn queue() -> Arc<JobQueue<sqlx::Postgres>> {
        test_utils::setup_postgres_queue().await
    }

    /// A queue whose pool is closed: every query fails.
    async fn broken_queue() -> Arc<JobQueue<sqlx::Postgres>> {
        let pool = sqlx::PgPool::connect(&test_utils::postgres_url())
            .await
            .unwrap();
        pool.close().await;
        Arc::new(JobQueue::new(pool))
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_pool_runs_pool_size_workers() {
        pool_runs_pool_size_workers(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_config_timeout_and_retry_strategy_apply() {
        config_timeout_and_retry_strategy_apply(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_config_strict_priority_orders_jobs() {
        config_strict_priority_orders_jobs(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_throttle_max_concurrent_caps_the_pool() {
        throttle_max_concurrent_caps_the_pool(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_throttle_rate_limits_the_pool() {
        throttle_rate_limits_the_pool(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_autoscaling_adds_and_retires_workers() {
        autoscaling_adds_and_retires_workers(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_worker_waits_while_queue_is_paused() {
        worker_waits_while_queue_is_paused(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_job_hooks_fire_for_each_outcome() {
        job_hooks_fire_for_each_outcome(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_worker_backs_off_on_errors_and_stops_promptly() {
        worker_backs_off_on_errors_and_stops_promptly(broken_queue().await).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    async fn queue() -> Arc<JobQueue<sqlx::MySql>> {
        test_utils::setup_mysql_queue().await
    }

    /// A queue whose pool is closed: every query fails.
    async fn broken_queue() -> Arc<JobQueue<sqlx::MySql>> {
        let pool = sqlx::MySqlPool::connect(&test_utils::mysql_url())
            .await
            .unwrap();
        pool.close().await;
        Arc::new(JobQueue::new(pool))
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_pool_runs_pool_size_workers() {
        pool_runs_pool_size_workers(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_config_timeout_and_retry_strategy_apply() {
        config_timeout_and_retry_strategy_apply(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_config_strict_priority_orders_jobs() {
        config_strict_priority_orders_jobs(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_throttle_max_concurrent_caps_the_pool() {
        throttle_max_concurrent_caps_the_pool(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_throttle_rate_limits_the_pool() {
        throttle_rate_limits_the_pool(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_autoscaling_adds_and_retires_workers() {
        autoscaling_adds_and_retires_workers(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_worker_waits_while_queue_is_paused() {
        worker_waits_while_queue_is_paused(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_job_hooks_fire_for_each_outcome() {
        job_hooks_fire_for_each_outcome(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_worker_backs_off_on_errors_and_stops_promptly() {
        worker_backs_off_on_errors_and_stops_promptly(broken_queue().await).await;
    }
}
