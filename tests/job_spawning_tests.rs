//! Spawned child jobs are enqueued in the same transaction that completes their parent
//! (#64 C2): a parent is never `Completed` without its children, and a spawn failure
//! fails the parent's run so it is retried. Each scenario runs on both backends.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    HammerworkError, Job, JobId, JobQueue, JobStatus, Worker,
    queue::DatabaseQueue,
    retry::RetryStrategy,
    spawn::{ClosureSpawnHandler, JobSpawnExt, SpawnConfig, SpawnContext, SpawnManager},
    worker::JobHandler,
    workflow::DependencyStatus,
};
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

/// Poll `get_job` until `predicate` holds or 15 seconds pass; returns the last job.
async fn wait_for_job<Q, F>(queue: &Arc<Q>, id: JobId, predicate: F) -> Job
where
    Q: DatabaseQueue + ?Sized,
    F: Fn(&Job) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let job = queue.get_job(id).await.unwrap().expect("job exists");
        if predicate(&job) || Instant::now() >= deadline {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Every job of `queue_name`, by claiming them all.
async fn drain<Q: DatabaseQueue + ?Sized>(queue: &Arc<Q>, queue_name: &str) -> Vec<Job> {
    let mut jobs = Vec::new();
    while let Some(job) = queue.dequeue(queue_name).await.unwrap() {
        jobs.push(job);
    }
    jobs
}

/// Run a worker for `queue_name` with `manager` until `done` holds for job `id`.
async fn run_worker<DB, F>(
    queue: &Arc<JobQueue<DB>>,
    queue_name: &str,
    manager: SpawnManager<DB>,
    id: JobId,
    done: F,
) -> Job
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    F: Fn(&Job) -> bool,
{
    let handler: JobHandler = Arc::new(|_job: Job| Box::pin(async { Ok(()) }));
    let worker = Worker::new(Arc::clone(queue), queue_name.to_string(), handler)
        .with_poll_interval(Duration::from_millis(50))
        .with_retry_delay(Duration::ZERO)
        .with_spawn_manager(Arc::new(manager));
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let task = tokio::spawn(async move { worker.run(shutdown_rx).await });
    let job = wait_for_job(queue, id, done).await;
    shutdown_tx.send(()).await.unwrap();
    task.await.unwrap().unwrap();
    job
}

/// A spawn handler producing `count` children on `child_queue`; the child at index
/// `bad_index` (if any) carries a closure retry strategy, which cannot be stored, so
/// enqueueing it fails inside the completing transaction.
fn children<DB: sqlx::Database + Send + Sync>(
    child_queue: &str,
    count: usize,
    bad_index: Option<usize>,
) -> ClosureSpawnHandler<
    impl Fn(SpawnContext<DB>) -> hammerwork::Result<Vec<Job>> + Send + Sync + 'static,
    DB,
> {
    let child_queue = child_queue.to_string();
    ClosureSpawnHandler::new(move |_context: SpawnContext<DB>| {
        Ok((0..count)
            .map(|i| {
                let child = Job::new(child_queue.clone(), json!({ "i": i }));
                if Some(i) == bad_index {
                    child.with_retry_strategy(RetryStrategy::custom(|_| Duration::from_secs(1)))
                } else {
                    child
                }
            })
            .collect())
    })
}

/// Children keep their own retry strategy, so the injected bad child stays bad.
fn spawn_config() -> SpawnConfig {
    SpawnConfig {
        inherit_retry_strategy: false,
        ..SpawnConfig::default()
    }
}

/// The parent completes and its children exist, released by the completion.
async fn children_are_enqueued_with_the_completion<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("spawn_ok");
    let child_queue = format!("{queue_name}_child");
    let mut manager: SpawnManager<DB> = SpawnManager::new();
    manager.register_handler(queue_name.clone(), children::<DB>(&child_queue, 3, None));
    let id = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({}))
                .with_spawn_config(spawn_config())
                .unwrap(),
        )
        .await
        .unwrap();

    let parent = run_worker(&queue, &queue_name, manager, id, |job| {
        job.status == JobStatus::Completed
    })
    .await;
    assert_eq!(parent.status, JobStatus::Completed);
    assert_eq!(parent.attempts, 1);

    // The children could be claimed: their dependency on the parent is met.
    let spawned = drain(&queue, &child_queue).await;
    assert_eq!(spawned.len(), 3);
    for child in &spawned {
        assert_eq!(child.depends_on, vec![id]);
        assert_ne!(child.dependency_status, DependencyStatus::Waiting);
        queue.delete_job(child.id).await.unwrap();
    }
    queue.delete_job(id).await.unwrap();
}

/// A failing spawn handler fails the parent's run: it is retried, and the retry
/// spawns the children once.
async fn failing_spawn_handler_retries_the_parent<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("spawn_retry");
    let child_queue = format!("{queue_name}_child");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut manager: SpawnManager<DB> = SpawnManager::new();
    {
        let (calls, child_queue) = (Arc::clone(&calls), child_queue.clone());
        manager.register_handler(
            queue_name.clone(),
            ClosureSpawnHandler::new(move |_context: SpawnContext<DB>| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(HammerworkError::Processing("spawn backend down".into()));
                }
                Ok(vec![
                    Job::new(child_queue.clone(), json!({ "i": 0 })),
                    Job::new(child_queue.clone(), json!({ "i": 1 })),
                ])
            }),
        );
    }
    let id = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({}))
                .with_max_attempts(3)
                .with_spawning()
                .unwrap(),
        )
        .await
        .unwrap();

    let parent = run_worker(&queue, &queue_name, manager, id, |job| {
        job.status == JobStatus::Completed
    })
    .await;
    assert_eq!(parent.status, JobStatus::Completed, "{parent:?}");
    assert_eq!(parent.attempts, 2, "the failed spawn retried the parent");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    let spawned = drain(&queue, &child_queue).await;
    assert_eq!(spawned.len(), 2, "children are spawned exactly once");
    for child in spawned {
        queue.delete_job(child.id).await.unwrap();
    }
    queue.delete_job(id).await.unwrap();
}

/// Enqueueing a child fails after the first one was written: the whole completion is
/// rolled back, so the parent is not `Completed` and no child exists.
async fn child_enqueue_failure_rolls_back_the_completion<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("spawn_rollback");
    let child_queue = format!("{queue_name}_child");
    let mut manager: SpawnManager<DB> = SpawnManager::new();
    manager.register_handler(queue_name.clone(), children::<DB>(&child_queue, 3, Some(2)));
    let id = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({}))
                .with_max_attempts(2)
                .with_spawn_config(spawn_config())
                .unwrap(),
        )
        .await
        .unwrap();

    // Both runs fail to spawn, so the parent ends Dead instead of Completed.
    let parent = run_worker(&queue, &queue_name, manager, id, |job| {
        job.status == JobStatus::Dead
    })
    .await;
    assert_eq!(parent.status, JobStatus::Dead, "{parent:?}");
    assert_eq!(parent.attempts, 2);
    assert!(
        parent
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("retry strategy"),
        "{parent:?}"
    );
    assert!(
        drain(&queue, &child_queue).await.is_empty(),
        "no child of a failed completion may exist"
    );
    let stats = queue.get_queue_stats(&child_queue).await.unwrap();
    assert_eq!(stats.pending_count + stats.running_count, 0, "{stats:?}");
    queue.delete_job(id).await.unwrap();
}

/// `complete_job_run_with_children` at the API level: atomic, and guarded by the run.
async fn complete_with_children_is_atomic_and_guarded<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("spawn_api");
    let child_queue = format!("{queue_name}_child");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();
    let run = queue.dequeue(&queue_name).await.unwrap().unwrap();
    let child = |i: u32| Job::new(child_queue.clone(), json!({ "i": i })).depends_on(&id);

    // A failing child rolls back the completion and the children before it.
    let bad = child(1).with_retry_strategy(RetryStrategy::custom(|_| Duration::from_secs(1)));
    assert!(
        queue
            .complete_job_run_with_children(&run, vec![child(0), bad])
            .await
            .is_err()
    );
    assert_eq!(
        queue.get_job(id).await.unwrap().unwrap().status,
        JobStatus::Running
    );
    assert!(drain(&queue, &child_queue).await.is_empty());

    // A stale run writes nothing.
    let mut stale = run.clone();
    stale.attempts += 1;
    assert!(
        queue
            .complete_job_run_with_children(&stale, vec![child(2)])
            .await
            .unwrap()
            .is_none()
    );
    assert!(drain(&queue, &child_queue).await.is_empty());

    // The current run completes and its children are released.
    let recorded = queue
        .complete_job_run_with_children(&run, vec![child(3), child(4)])
        .await
        .unwrap()
        .expect("current run");
    assert_eq!(recorded.status, JobStatus::Completed);
    assert_eq!(recorded.spawned.len(), 2);
    assert_eq!(
        queue.get_job(id).await.unwrap().unwrap().status,
        JobStatus::Completed
    );
    let spawned = drain(&queue, &child_queue).await;
    let mut ids: Vec<JobId> = spawned.iter().map(|job| job.id).collect();
    let mut expected = recorded.spawned.clone();
    ids.sort();
    expected.sort();
    assert_eq!(ids, expected);
    for job in spawned {
        queue.delete_job(job.id).await.unwrap();
    }
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
        children_are_enqueued_with_the_completion,
        failing_spawn_handler_retries_the_parent,
        child_enqueue_failure_rolls_back_the_completion,
        complete_with_children_is_atomic_and_guarded,
    ]
);

#[cfg(feature = "mysql")]
backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        children_are_enqueued_with_the_completion,
        failing_spawn_handler_retries_the_parent,
        child_enqueue_failure_rolls_back_the_completion,
        complete_with_children_is_atomic_and_guarded,
    ]
);
