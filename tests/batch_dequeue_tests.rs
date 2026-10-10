//! Batch claims (`DatabaseQueue::dequeue_batch_leased`) and batch-mode workers
//! (`Worker::with_batch_size`).
//!
//! The queue scenarios run against PostgreSQL, MySQL and `TestQueue`, so the in-memory
//! queue is held to the same behaviour; the worker scenarios run against both database
//! backends. Each claimed job must get the same guarantees as a single claim: lease,
//! attempt count, pause/dependency/recurring checks, priority order and exactly-once
//! claiming under concurrency.

#![cfg(any(feature = "postgres", feature = "mysql", feature = "test"))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    Job, JobId, JobPriority, JobStatus, PriorityWeights, cron::CronSchedule, queue::DatabaseQueue,
    workflow::JobGroup,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

/// A job of `queue_name` with `priority`, due `age_secs` seconds ago (so claim order
/// within a priority level is known, on TestQueue's mock clock too).
fn aged_job(queue_name: &str, priority: JobPriority, age_secs: i64) -> Job {
    let mut job =
        Job::new(queue_name.to_string(), json!({ "age": age_secs })).with_priority(priority);
    job.scheduled_at = Utc::now() - chrono::Duration::seconds(age_secs);
    job
}

/// A batch claims up to N jobs, highest priority first then oldest, each `Running`
/// with its attempt counted.
async fn batch_claims_in_priority_order<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("batch_order");
    let specs = [
        (JobPriority::Normal, 50),
        (JobPriority::Critical, 10),
        (JobPriority::Low, 90),
        (JobPriority::Normal, 70),
        (JobPriority::High, 20),
        (JobPriority::Critical, 30),
    ];
    let mut ids = HashMap::new();
    for (priority, age) in specs {
        let id = queue
            .enqueue(aged_job(&queue_name, priority, age))
            .await
            .unwrap();
        ids.insert(id, (priority, age));
    }

    assert!(
        queue
            .dequeue_batch(&queue_name, 0)
            .await
            .unwrap()
            .is_empty()
    );

    let first = queue.dequeue_batch(&queue_name, 4).await.unwrap();
    let order: Vec<(JobPriority, i64)> = first.iter().map(|job| ids[&job.id]).collect();
    assert_eq!(
        order,
        [
            (JobPriority::Critical, 30),
            (JobPriority::Critical, 10),
            (JobPriority::High, 20),
            (JobPriority::Normal, 70),
        ]
    );
    for job in &first {
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.attempts, 1);
        assert!(job.started_at.is_some());
        let stored = queue.get_job(job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, JobStatus::Running);
        assert_eq!(stored.attempts, 1);
    }

    // The rest, then nothing.
    let rest = queue.dequeue_batch(&queue_name, 10).await.unwrap();
    let order: Vec<(JobPriority, i64)> = rest.iter().map(|job| ids[&job.id]).collect();
    assert_eq!(order, [(JobPriority::Normal, 50), (JobPriority::Low, 90)]);
    assert!(
        queue
            .dequeue_batch(&queue_name, 10)
            .await
            .unwrap()
            .is_empty()
    );

    // Strict weights claim in the same strict order.
    let strict_queue = test_utils::unique_queue("batch_strict");
    for (priority, age) in specs {
        queue
            .enqueue(aged_job(&strict_queue, priority, age))
            .await
            .unwrap();
    }
    let strict = queue
        .dequeue_batch_leased(
            &strict_queue,
            Some(&PriorityWeights::strict()),
            Duration::from_secs(60),
            3,
        )
        .await
        .unwrap();
    let priorities: Vec<JobPriority> = strict.iter().map(|job| job.priority).collect();
    assert_eq!(
        priorities,
        [
            JobPriority::Critical,
            JobPriority::Critical,
            JobPriority::High
        ]
    );
}

/// With weighted priorities a batch is filled from one level picked by weight, then
/// topped up from the other levels when that level runs out.
async fn weighted_batch_fills_from_picked_level<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("batch_weighted");
    for age in 0..5 {
        queue
            .enqueue(aged_job(&queue_name, JobPriority::High, 100 - age))
            .await
            .unwrap();
        queue
            .enqueue(aged_job(&queue_name, JobPriority::Low, 200 - age))
            .await
            .unwrap();
    }
    let weights = PriorityWeights::new();
    let lease = Duration::from_secs(60);

    let first = queue
        .dequeue_batch_leased(&queue_name, Some(&weights), lease, 3)
        .await
        .unwrap();
    assert_eq!(first.len(), 3);
    let level = first[0].priority;
    assert!(
        first.iter().all(|job| job.priority == level),
        "a batch smaller than the picked level comes from that level only"
    );
    // Oldest first within the level.
    assert!(
        first
            .windows(2)
            .all(|pair| pair[0].scheduled_at <= pair[1].scheduled_at)
    );

    // Seven are left (2 of the picked level, 5 of the other): a batch of 7 takes all.
    let rest = queue
        .dequeue_batch_leased(&queue_name, Some(&weights), lease, 10)
        .await
        .unwrap();
    assert_eq!(rest.len(), 7);
    let all: HashSet<JobId> = first.iter().chain(&rest).map(|job| job.id).collect();
    assert_eq!(all.len(), 10);
}

/// Paused queues, unfinished dependencies and disabled recurring jobs hold their jobs,
/// as for single claims.
async fn batch_honours_pause_dependencies_and_recurring<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("batch_holds");

    // A workflow: `child` waits for `parent`.
    let parent = Job::new(queue_name.clone(), json!({ "step": "parent" }));
    let child = Job::new(queue_name.clone(), json!({ "step": "child" })).depends_on(&parent.id);
    let (parent_id, child_id) = (parent.id, child.id);
    queue
        .enqueue_workflow(JobGroup::new("batch").add_job(parent).then(child))
        .await
        .unwrap();

    // A disabled recurring job's pending occurrence.
    let mut cron_job = Job::new(queue_name.clone(), json!({}))
        .with_cron(CronSchedule::new("0 0 * * * *").unwrap())
        .unwrap();
    cron_job.scheduled_at = Utc::now() - chrono::Duration::hours(1);
    cron_job.next_run_at = Some(cron_job.scheduled_at);
    let held_cron = queue.enqueue(cron_job).await.unwrap();
    queue.disable_recurring_job(held_cron).await.unwrap();

    let plain = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();

    // Paused: nothing is claimed.
    queue.pause_queue(&queue_name, Some("test")).await.unwrap();
    assert!(
        queue
            .dequeue_batch(&queue_name, 10)
            .await
            .unwrap()
            .is_empty()
    );
    queue.resume_queue(&queue_name, Some("test")).await.unwrap();

    let claimed = queue.dequeue_batch(&queue_name, 10).await.unwrap();
    let ids: HashSet<JobId> = claimed.iter().map(|job| job.id).collect();
    assert_eq!(ids, HashSet::from([parent_id, plain]));

    // Completing the parent releases the child.
    let parent_run = claimed.iter().find(|job| job.id == parent_id).unwrap();
    queue
        .finish_job_run(parent_run, hammerwork::JobOutcome::Completed)
        .await
        .unwrap()
        .expect("the parent run is recorded");
    let next = queue.dequeue_batch(&queue_name, 10).await.unwrap();
    let ids: Vec<JobId> = next.iter().map(|job| job.id).collect();
    assert_eq!(ids, [child_id]);
    assert_eq!(
        queue.get_job(held_cron).await.unwrap().unwrap().status,
        JobStatus::Pending,
        "a disabled recurring job ran"
    );
}

/// Every job of a batch holds its own lease from the claim: a reaper with any
/// staleness window leaves it alone, and the lease can be renewed per job.
async fn batch_claims_hold_leases<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    // The reaper looks at every queue.
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("batch_leases");
    for _ in 0..3 {
        queue
            .enqueue(Job::new(queue_name.clone(), json!({})))
            .await
            .unwrap();
    }
    let claimed = queue
        .dequeue_batch_leased(&queue_name, None, Duration::from_secs(600), 3)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 3);

    let recovery = queue.requeue_stale_jobs(Duration::ZERO).await.unwrap();
    for job in &claimed {
        assert!(
            !recovery.requeued.contains(&job.id) && !recovery.dead.contains(&job.id),
            "a job claimed in a batch had no lease"
        );
        assert!(
            queue
                .heartbeat_job(job, Duration::from_secs(600))
                .await
                .unwrap()
        );
    }
}

/// `release_job_run` hands a claimed job back as if it had not been claimed, once.
async fn release_returns_unstarted_job<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("batch_release");
    for age in [20, 10] {
        queue
            .enqueue(aged_job(&queue_name, JobPriority::Normal, age))
            .await
            .unwrap();
    }
    let claimed = queue.dequeue_batch(&queue_name, 2).await.unwrap();
    assert_eq!(claimed.len(), 2);
    let released = &claimed[0];

    assert!(queue.release_job_run(released).await.unwrap());
    let stored = queue.get_job(released.id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Pending);
    assert_eq!(stored.attempts, 0, "the claim's attempt is taken back");
    assert_eq!(stored.started_at, None);
    // Not Running this run any more: nothing to release, and no heartbeat.
    assert!(!queue.release_job_run(released).await.unwrap());
    assert!(
        !queue
            .heartbeat_job(released, Duration::from_secs(60))
            .await
            .unwrap()
    );

    // It keeps its place and is claimed again.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let again = queue.dequeue_batch(&queue_name, 5).await.unwrap();
    let ids: Vec<JobId> = again.iter().map(|job| job.id).collect();
    assert_eq!(ids, [released.id]);
    assert_eq!(again[0].attempts, 1);
    // A stale run (the first claim) cannot release the new one. Runs are told apart by
    // their start time, which TestQueue's mock clock does not move on its own.
    if again[0].started_at != released.started_at {
        assert!(!queue.release_job_run(released).await.unwrap());
        assert_eq!(
            queue.get_job(released.id).await.unwrap().unwrap().status,
            JobStatus::Running
        );
    }
}

/// Several tasks claiming batches (strict and weighted) and single jobs at once never
/// claim the same job twice, and together claim every job.
async fn concurrent_batch_claims_never_double_claim<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    const JOBS: usize = 300;
    let queue_name = test_utils::unique_queue("batch_concurrent");
    let priorities = [
        JobPriority::Background,
        JobPriority::Low,
        JobPriority::Normal,
        JobPriority::High,
        JobPriority::Critical,
    ];
    let jobs: Vec<Job> = (0..JOBS)
        .map(|i| {
            Job::new(queue_name.clone(), json!({ "i": i }))
                .with_priority(priorities[i % priorities.len()])
        })
        .collect();
    let expected: HashSet<JobId> = jobs.iter().map(|job| job.id).collect();
    for job in jobs {
        queue.enqueue(job).await.unwrap();
    }

    let mut tasks = Vec::new();
    for task in 0..8usize {
        let queue = Arc::clone(&queue);
        let queue_name = queue_name.clone();
        tasks.push(tokio::spawn(async move {
            let weights = PriorityWeights::new();
            let mut claimed = Vec::new();
            let mut empty_polls = 0;
            while empty_polls < 3 {
                let batch = match task % 4 {
                    0 => queue
                        .dequeue_leased(&queue_name, None, Duration::from_secs(600))
                        .await
                        .unwrap()
                        .into_iter()
                        .collect(),
                    1 => queue
                        .dequeue_batch_leased(
                            &queue_name,
                            Some(&weights),
                            Duration::from_secs(600),
                            5 + task,
                        )
                        .await
                        .unwrap(),
                    _ => queue
                        .dequeue_batch_leased(&queue_name, None, Duration::from_secs(600), 7)
                        .await
                        .unwrap(),
                };
                if batch.is_empty() {
                    empty_polls += 1;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                } else {
                    empty_polls = 0;
                    claimed.extend(batch.into_iter().map(|job: Job| job.id));
                }
            }
            claimed
        }));
    }

    let mut seen = HashSet::new();
    let mut total = 0;
    for task in tasks {
        for id in task.await.unwrap() {
            total += 1;
            assert!(seen.insert(id), "job {id} was claimed twice");
        }
    }
    assert_eq!(total, JOBS);
    assert_eq!(seen, expected);
    for id in expected.iter().take(20) {
        assert_eq!(queue.get_job(*id).await.unwrap().unwrap().attempts, 1);
    }
}

/// One `#[tokio::test]` per scenario, run against the queue returned by `$setup`.
macro_rules! backend_tests {
    ($module:ident, $setup:path, $ignore:meta, [$($scenario:ident),* $(,)?]) => {
        mod $module {
            use super::*;
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
        batch_claims_in_priority_order,
        weighted_batch_fills_from_picked_level,
        batch_honours_pause_dependencies_and_recurring,
        batch_claims_hold_leases,
        release_returns_unstarted_job,
        concurrent_batch_claims_never_double_claim,
    ]
);

#[cfg(feature = "mysql")]
backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        batch_claims_in_priority_order,
        weighted_batch_fills_from_picked_level,
        batch_honours_pause_dependencies_and_recurring,
        batch_claims_hold_leases,
        release_returns_unstarted_job,
        concurrent_batch_claims_never_double_claim,
    ]
);

#[cfg(feature = "test")]
async fn test_queue() -> Arc<hammerwork::queue::test::TestQueue> {
    Arc::new(hammerwork::queue::test::TestQueue::new())
}

#[cfg(feature = "test")]
backend_tests!(
    test_queue_tests,
    test_queue,
    allow(unused_attributes),
    [
        batch_claims_in_priority_order,
        weighted_batch_fills_from_picked_level,
        batch_honours_pause_dependencies_and_recurring,
        batch_claims_hold_leases,
        release_returns_unstarted_job,
        concurrent_batch_claims_never_double_claim,
    ]
);

/// Worker scenarios: they need a `JobQueue`, so they run on the database backends only.
#[cfg(any(feature = "postgres", feature = "mysql"))]
mod worker {
    use super::*;
    use hammerwork::{
        HammerworkError, JobQueue, RateLimit, ThrottleConfig, Worker, worker::JobHandler,
    };
    use std::{
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Instant,
    };
    use tokio::sync::mpsc;

    async fn enqueue_jobs<DB>(
        queue: &Arc<JobQueue<DB>>,
        queue_name: &str,
        count: usize,
    ) -> Vec<JobId>
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        let mut ids = Vec::with_capacity(count);
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

    /// Wait until `queue_name` has `count` jobs in `status`, or panic after `timeout`.
    async fn wait_for_status<DB>(
        queue: &Arc<JobQueue<DB>>,
        queue_name: &str,
        status: &str,
        count: u64,
        timeout: Duration,
    ) where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        let deadline = Instant::now() + timeout;
        loop {
            let counts = queue.get_job_counts_by_status(queue_name).await.unwrap();
            let have = counts.get(status).copied().unwrap_or(0);
            if have == count {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {count} {status} jobs: {counts:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A handler that counts runs per job id, sleeping `delay` in each.
    fn counting_handler(runs: Arc<Mutex<HashMap<JobId, usize>>>, delay: Duration) -> JobHandler {
        Arc::new(move |job: Job| {
            let runs = Arc::clone(&runs);
            Box::pin(async move {
                *runs.lock().unwrap().entry(job.id).or_default() += 1;
                tokio::time::sleep(delay).await;
                Ok::<(), HammerworkError>(())
            })
        })
    }

    /// A pool of batch-mode workers runs every job exactly once.
    pub async fn batch_worker_processes_everything_once<DB>(queue: Arc<JobQueue<DB>>)
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        const JOBS: usize = 150;
        let queue_name = test_utils::unique_queue("batch_worker");
        let ids = enqueue_jobs(&queue, &queue_name, JOBS).await;
        let runs = Arc::new(Mutex::new(HashMap::new()));

        let mut tasks = Vec::new();
        let mut senders = Vec::new();
        for _ in 0..3 {
            let worker = Worker::new(
                Arc::clone(&queue),
                queue_name.clone(),
                counting_handler(Arc::clone(&runs), Duration::from_millis(2)),
            )
            .with_poll_interval(Duration::from_millis(20))
            .with_batch_size(8)
            .with_batch_concurrency(3);
            let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
            senders.push(shutdown_tx);
            tasks.push(tokio::spawn(async move { worker.run(shutdown_rx).await }));
        }

        wait_for_status(
            &queue,
            &queue_name,
            "Completed",
            JOBS as u64,
            Duration::from_secs(60),
        )
        .await;
        let runs = runs.lock().unwrap().clone();
        assert_eq!(runs.len(), JOBS);
        assert!(runs.values().all(|count| *count == 1), "a job ran twice");
        assert!(ids.iter().all(|id| runs.contains_key(id)));
        for id in ids.iter().take(20) {
            assert_eq!(queue.get_job(*id).await.unwrap().unwrap().attempts, 1);
        }

        for sender in senders {
            sender.send(()).await.unwrap();
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    }

    /// Jobs waiting their turn in a batch keep their leases: with a lease much shorter
    /// than the batch takes, a reaper running throughout reclaims none of them.
    pub async fn held_jobs_are_not_reaped_while_waiting<DB>(queue: Arc<JobQueue<DB>>)
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        let _serial = test_utils::serial().await;
        let queue_name = test_utils::unique_queue("batch_held");
        let ids = enqueue_jobs(&queue, &queue_name, 4).await;
        let runs = Arc::new(Mutex::new(HashMap::new()));

        // Lease 1.5s, heartbeat every 500ms; the batch runs ~4s one job at a time, so the
        // last jobs wait well past the lease. The margin leaves room for slow CI runners.
        let worker = Worker::new(
            Arc::clone(&queue),
            queue_name.clone(),
            counting_handler(Arc::clone(&runs), Duration::from_millis(1000)),
        )
        .with_poll_interval(Duration::from_millis(20))
        .with_lease_duration(Duration::from_millis(1500))
        .with_batch_size(4)
        .with_batch_concurrency(1);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let worker_task = tokio::spawn(async move { worker.run(shutdown_rx).await });

        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let recovery = queue
                .requeue_stale_jobs(Duration::from_secs(3600))
                .await
                .unwrap();
            for id in &ids {
                assert!(
                    !recovery.requeued.contains(id) && !recovery.dead.contains(id),
                    "job {id} was reaped while held by a live batch worker"
                );
            }
            let counts = queue.get_job_counts_by_status(&queue_name).await.unwrap();
            if counts.get("Completed") == Some(&4) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "batch did not finish: {counts:?}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let runs = runs.lock().unwrap().clone();
        assert_eq!(runs.len(), 4);
        assert!(runs.values().all(|count| *count == 1));
        for id in &ids {
            assert_eq!(queue.get_job(*id).await.unwrap().unwrap().attempts, 1);
        }

        shutdown_tx.send(()).await.unwrap();
        worker_task.await.unwrap().unwrap();
    }

    /// On shutdown, the claimed jobs that have not started go straight back to
    /// `Pending` (attempt not counted) while the running one finishes.
    pub async fn shutdown_releases_unstarted_jobs<DB>(queue: Arc<JobQueue<DB>>)
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        let queue_name = test_utils::unique_queue("batch_shutdown");
        let ids = enqueue_jobs(&queue, &queue_name, 5).await;
        let runs = Arc::new(Mutex::new(HashMap::new()));

        let worker = Worker::new(
            Arc::clone(&queue),
            queue_name.clone(),
            counting_handler(Arc::clone(&runs), Duration::from_millis(800)),
        )
        .with_poll_interval(Duration::from_millis(20))
        .with_shutdown_grace_period(Duration::from_secs(10))
        .with_batch_size(5)
        .with_batch_concurrency(1);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let worker_task = tokio::spawn(async move { worker.run(shutdown_rx).await });

        // All five are claimed in one batch, and the first starts.
        wait_for_status(&queue, &queue_name, "Running", 5, Duration::from_secs(10)).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        while runs.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "no job started");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let requested = Instant::now();
        shutdown_tx.send(()).await.unwrap();
        worker_task.await.unwrap().unwrap();
        assert!(requested.elapsed() < Duration::from_secs(5));

        let started: Vec<JobId> = runs.lock().unwrap().keys().copied().collect();
        assert_eq!(started.len(), 1, "only the first job of the batch started");
        for id in &ids {
            let job = queue.get_job(*id).await.unwrap().unwrap();
            if started.contains(id) {
                assert_eq!(job.status, JobStatus::Completed, "the running job finished");
            } else {
                assert_eq!(
                    job.status,
                    JobStatus::Pending,
                    "an unstarted job was not released"
                );
                assert_eq!(job.attempts, 0);
                assert_eq!(job.started_at, None);
            }
        }

        // The released jobs run normally afterwards.
        let claimed = queue.dequeue_batch(&queue_name, 10).await.unwrap();
        assert_eq!(claimed.len(), 4);
    }

    /// A throttle's `max_concurrent` bounds the jobs a batch-mode pool holds (claims or
    /// runs) at once, whatever the batch size and concurrency.
    pub async fn batch_worker_respects_throttle<DB>(queue: Arc<JobQueue<DB>>)
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        const JOBS: usize = 12;
        let queue_name = test_utils::unique_queue("batch_throttle");
        enqueue_jobs(&queue, &queue_name, JOBS).await;

        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicUsize::new(0));
        let handler: JobHandler = {
            let (active, peak, done) = (Arc::clone(&active), Arc::clone(&peak), Arc::clone(&done));
            Arc::new(move |_job: Job| {
                let (active, peak, done) =
                    (Arc::clone(&active), Arc::clone(&peak), Arc::clone(&done));
                Box::pin(async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                    done.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), HammerworkError>(())
                })
            })
        };
        let template = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
            .with_poll_interval(Duration::from_millis(20))
            .with_throttle_config(ThrottleConfig::new().max_concurrent(2))
            .with_batch_size(10)
            .with_batch_concurrency(10);
        // Two clones share the throttle.
        let workers = [template.clone(), template];
        let mut tasks = Vec::new();
        let mut senders = Vec::new();
        for worker in workers {
            let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
            senders.push(shutdown_tx);
            tasks.push(tokio::spawn(async move { worker.run(shutdown_rx).await }));
        }

        let deadline = Instant::now() + Duration::from_secs(30);
        while done.load(Ordering::SeqCst) < JOBS {
            let counts = queue.get_job_counts_by_status(&queue_name).await.unwrap();
            let running = counts.get("Running").copied().unwrap_or(0);
            assert!(
                running <= 2,
                "{running} jobs held at once with max_concurrent 2"
            );
            assert!(Instant::now() < deadline, "jobs did not finish");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(peak.load(Ordering::SeqCst) <= 2);
        wait_for_status(
            &queue,
            &queue_name,
            "Completed",
            JOBS as u64,
            Duration::from_secs(10),
        )
        .await;

        for sender in senders {
            sender.send(()).await.unwrap();
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
    }

    /// A rate limit caps how many jobs a batch claims: with a burst of 3 and 3 jobs a
    /// second, a batch size of 10 claims at most the tokens available.
    pub async fn batch_worker_respects_rate_limit<DB>(queue: Arc<JobQueue<DB>>)
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        const JOBS: usize = 9;
        let queue_name = test_utils::unique_queue("batch_rate");
        enqueue_jobs(&queue, &queue_name, JOBS).await;

        let starts = Arc::new(Mutex::new(Vec::new()));
        let handler: JobHandler = {
            let starts = Arc::clone(&starts);
            Arc::new(move |_job: Job| {
                let starts = Arc::clone(&starts);
                Box::pin(async move {
                    starts.lock().unwrap().push(Instant::now());
                    Ok::<(), HammerworkError>(())
                })
            })
        };
        let worker = Worker::new(Arc::clone(&queue), queue_name.clone(), handler)
            .with_poll_interval(Duration::from_millis(20))
            .with_rate_limit(RateLimit::per_second(3))
            .with_batch_size(10)
            .with_batch_concurrency(10);
        let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
        let began = Instant::now();
        let worker_task = tokio::spawn(async move { worker.run(shutdown_rx).await });

        // The first poll can take the 3-token burst only; the queue keeps the rest.
        let deadline = Instant::now() + Duration::from_secs(10);
        while starts.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let counts = queue.get_job_counts_by_status(&queue_name).await.unwrap();
        assert!(
            counts.get("Pending").copied().unwrap_or(0) >= (JOBS - 3) as u64,
            "a batch claimed more jobs than it had rate limit tokens: {counts:?}"
        );

        wait_for_status(
            &queue,
            &queue_name,
            "Completed",
            JOBS as u64,
            Duration::from_secs(20),
        )
        .await;
        // A burst of 3, then one token every 333ms: the k-th job (from 1) cannot start
        // before (k - 3) / 3 seconds.
        let mut starts = starts.lock().unwrap().clone();
        starts.sort();
        assert_eq!(starts.len(), JOBS);
        for (index, start) in starts.iter().enumerate() {
            let earliest = Duration::from_millis(333 * (index as u64 + 1).saturating_sub(3));
            let at = start.duration_since(began);
            assert!(
                at + Duration::from_millis(100) >= earliest,
                "job {} started at {at:?}, before its rate limit token ({earliest:?})",
                index + 1
            );
        }

        shutdown_tx.send(()).await.unwrap();
        worker_task.await.unwrap().unwrap();
    }

    /// Informative benchmark, not a gate: jobs per second claimed one at a time versus
    /// in batches, and processed by a single-job versus a batch-mode worker. Run with
    /// `--nocapture` to see the numbers.
    pub async fn batch_vs_single_claim_throughput<DB>(queue: Arc<JobQueue<DB>>)
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        if std::env::var_os("HAMMERWORK_BENCH").is_none() {
            eprintln!("set HAMMERWORK_BENCH=1 to run the batch claim benchmark");
            return;
        }
        const JOBS: usize = 2000;
        let backend = std::any::type_name::<DB>();

        // Claims only.
        let single_queue = test_utils::unique_queue("bench_single");
        enqueue_jobs(&queue, &single_queue, JOBS).await;
        let began = Instant::now();
        let mut claimed = 0;
        while queue.dequeue(&single_queue).await.unwrap().is_some() {
            claimed += 1;
        }
        let single = claimed as f64 / began.elapsed().as_secs_f64();

        let mut batch_rates = Vec::new();
        for size in [10, 50, 100] {
            let batch_queue = test_utils::unique_queue("bench_batch");
            enqueue_jobs(&queue, &batch_queue, JOBS).await;
            let began = Instant::now();
            let mut claimed = 0;
            loop {
                let batch = queue.dequeue_batch(&batch_queue, size).await.unwrap();
                if batch.is_empty() {
                    break;
                }
                claimed += batch.len();
            }
            batch_rates.push((size, claimed as f64 / began.elapsed().as_secs_f64()));
        }
        eprintln!("[{backend}] claim throughput: single {single:.0} jobs/s");
        for (size, rate) in &batch_rates {
            eprintln!(
                "[{backend}] claim throughput: batch of {size} {rate:.0} jobs/s ({:.1}x)",
                rate / single
            );
        }

        // End to end: one worker, no-op handler.
        let mut worker_rates = Vec::new();
        for (label, size, concurrency) in [
            ("single-job worker", 0, 1),
            ("batch worker (10, sequential)", 10, 1),
            ("batch worker (50, concurrency 10)", 50, 10),
        ] {
            let worker_queue = test_utils::unique_queue("bench_worker");
            enqueue_jobs(&queue, &worker_queue, JOBS).await;
            let handler: JobHandler = Arc::new(|_job: Job| Box::pin(async { Ok(()) }));
            let mut worker = Worker::new(Arc::clone(&queue), worker_queue.clone(), handler)
                .with_poll_interval(Duration::from_millis(10));
            if size > 0 {
                worker = worker
                    .with_batch_size(size)
                    .with_batch_concurrency(concurrency);
            }
            let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
            let began = Instant::now();
            let task = tokio::spawn(async move { worker.run(shutdown_rx).await });
            wait_for_status(
                &queue,
                &worker_queue,
                "Completed",
                JOBS as u64,
                Duration::from_secs(600),
            )
            .await;
            worker_rates.push((label, JOBS as f64 / began.elapsed().as_secs_f64()));
            shutdown_tx.send(()).await.unwrap();
            task.await.unwrap().unwrap();
        }
        for (label, rate) in &worker_rates {
            eprintln!("[{backend}] worker throughput: {label} {rate:.0} jobs/s");
        }
    }

    #[cfg(feature = "postgres")]
    backend_tests!(
        postgres_tests,
        test_utils::setup_postgres_queue,
        ignore = "requires PostgreSQL: DATABASE_URL",
        [
            batch_worker_processes_everything_once,
            held_jobs_are_not_reaped_while_waiting,
            shutdown_releases_unstarted_jobs,
            batch_worker_respects_throttle,
            batch_worker_respects_rate_limit,
            batch_vs_single_claim_throughput,
        ]
    );

    #[cfg(feature = "mysql")]
    backend_tests!(
        mysql_tests,
        test_utils::setup_mysql_queue,
        ignore = "requires MySQL: MYSQL_DATABASE_URL",
        [
            batch_worker_processes_everything_once,
            held_jobs_are_not_reaped_while_waiting,
            shutdown_releases_unstarted_jobs,
            batch_worker_respects_throttle,
            batch_worker_respects_rate_limit,
            batch_vs_single_claim_throughput,
        ]
    );
}
