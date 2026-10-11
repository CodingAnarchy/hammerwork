//! #112: `TestQueue` behaves like the database backends.
//!
//! - Restoring an archived job resets its attempts, times, error and result.
//! - Batch status is tallied the same way, counting archived members.
//! - `get_ready_jobs` honours the dependency status and orders by priority, then
//!   schedule.
//! - `cancel_workflow` fails every unfinished job and fails the dependencies of the
//!   jobs still waiting on them.
//!
//! Each scenario is written once against the `DatabaseQueue` trait and runs on both
//! database backends and on `TestQueue`.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::{Duration as ChronoDuration, Utc};
use hammerwork::{
    Job, JobId, JobPriority, JobStatus,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason},
    batch::{BatchStatus, JobBatch, PartialFailureMode},
    queue::DatabaseQueue,
    workflow::{DependencyStatus, JobGroup, WorkflowStatus},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

/// Wait until the wall clock is past the time `id` finished, so a zero-age archival
/// policy selects it on every backend.
async fn finished_in_the_past<Q: DatabaseQueue + ?Sized>(queue: &Q, id: JobId) {
    let job = queue.get_job(id).await.unwrap().expect("job exists");
    let finished = job
        .completed_at
        .or(job.failed_at)
        .or(job.timed_out_at)
        .expect("job finished");
    while Utc::now() <= finished {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Archive every finished job of `queue_name` now; returns how many were archived.
async fn archive_queue<Q: DatabaseQueue>(queue: &Q, queue_name: &str) -> u64 {
    let mut policy = ArchivalPolicy::new()
        .archive_completed_after(ChronoDuration::seconds(0))
        .archive_failed_after(ChronoDuration::seconds(0))
        .archive_dead_after(ChronoDuration::seconds(0))
        .archive_timed_out_after(ChronoDuration::seconds(0));
    policy.purge_archived_after = None;
    queue
        .archive_jobs(
            Some(queue_name),
            &policy,
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            Some("test_queue_parity_tests"),
        )
        .await
        .unwrap()
        .jobs_archived
}

async fn purge_archive<Q: DatabaseQueue>(queue: &Q, queue_name: &str) {
    queue
        .purge_archived_jobs_in_queue(queue_name, Utc::now() + ChronoDuration::days(1))
        .await
        .unwrap();
}

/// #112 (1): a restored job starts over. It used to keep its attempts, times and
/// error on `TestQueue`, so a restored dead job died again after one run.
async fn restore_resets_the_job<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("restore_reset");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})).with_max_attempts(1))
        .await
        .unwrap();
    let run = queue
        .dequeue_one(&queue_name)
        .await
        .unwrap()
        .expect("job runs");
    assert_eq!(run.id, id);
    queue.mark_job_dead(id, "out of attempts").await.unwrap();
    finished_in_the_past(queue.as_ref(), id).await;
    assert_eq!(archive_queue(queue.as_ref(), &queue_name).await, 1);

    let restored = queue.restore_archived_job(id).await.unwrap();
    for job in [restored, queue.get_job(id).await.unwrap().unwrap()] {
        assert_eq!(job.status, JobStatus::Pending);
        assert_eq!(job.attempts, 0, "attempts reset");
        assert_eq!(job.started_at, None);
        assert_eq!(job.completed_at, None);
        assert_eq!(job.failed_at, None);
        assert_eq!(job.timed_out_at, None);
        assert_eq!(job.error_message, None);
        assert_eq!(job.result_data, None);
    }

    let run = queue
        .dequeue_one(&queue_name)
        .await
        .unwrap()
        .expect("the restored job runs again");
    assert_eq!((run.id, run.attempts), (id, 1));
    queue.delete_job(id).await.unwrap();
}

/// #112 (2): a batch nothing of which has finished is `Pending`, and one whose jobs
/// all failed is `Failed` (not `PartiallyFailed`), whatever its failure mode.
async fn batch_status_follows_its_jobs<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("batch_parity");
    let batch = JobBatch::new("batch_parity")
        .with_jobs(
            (0..2)
                .map(|i| Job::new(queue_name.clone(), json!({ "i": i })))
                .collect(),
        )
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError);
    let batch_id = queue.enqueue_batch(batch).await.unwrap();
    let jobs = queue.get_batch_jobs(batch_id).await.unwrap();

    let status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(status.status, BatchStatus::Pending);
    assert_eq!(
        (
            status.pending_jobs,
            status.completed_jobs,
            status.failed_jobs
        ),
        (2, 0, 0)
    );
    assert!(status.completed_at.is_none());
    assert!(status.error_summary.is_none());

    for job in &jobs {
        queue.fail_job(job.id, "boom").await.unwrap();
    }
    let status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(status.status, BatchStatus::Failed);
    assert_eq!(
        (
            status.pending_jobs,
            status.completed_jobs,
            status.failed_jobs
        ),
        (0, 0, 2)
    );
    assert!(status.completed_at.is_some());
    assert!(status.error_summary.is_some());
    assert_eq!(status.job_errors.len(), 2);

    queue.delete_batch(batch_id).await.unwrap();
}

/// #112 (2): archived members of a batch still count, with the status they were
/// archived with.
async fn batch_status_counts_archived_jobs<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("batch_archived");
    let batch = JobBatch::new("batch_archived")
        .with_jobs(
            (0..2)
                .map(|i| Job::new(queue_name.clone(), json!({ "i": i })))
                .collect(),
        )
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError);
    let batch_id = queue.enqueue_batch(batch).await.unwrap();
    let jobs = queue.get_batch_jobs(batch_id).await.unwrap();

    queue.complete_job(jobs[0].id).await.unwrap();
    finished_in_the_past(queue.as_ref(), jobs[0].id).await;
    assert_eq!(archive_queue(queue.as_ref(), &queue_name).await, 1);

    let status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(status.status, BatchStatus::Processing);
    assert_eq!(
        (
            status.pending_jobs,
            status.completed_jobs,
            status.failed_jobs
        ),
        (1, 1, 0),
        "the archived job still counts as completed"
    );

    queue.fail_job(jobs[1].id, "boom").await.unwrap();
    let status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(status.status, BatchStatus::PartiallyFailed);
    assert_eq!(
        (
            status.pending_jobs,
            status.completed_jobs,
            status.failed_jobs
        ),
        (0, 1, 1)
    );

    queue.delete_batch(batch_id).await.unwrap();
    purge_archive(queue.as_ref(), &queue_name).await;
}

/// #112 (3): `get_ready_jobs` leaves out jobs waiting on their dependencies and lists
/// the rest by priority, then by schedule.
async fn ready_jobs_honour_dependencies_and_order<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("ready_jobs");
    let due = |seconds_ago: i64, job: Job| {
        let mut job = job;
        job.scheduled_at = job.created_at - ChronoDuration::seconds(seconds_ago);
        job
    };

    // Enqueued in an order matching neither priority nor schedule.
    let low = due(
        30,
        Job::new(queue_name.clone(), json!({ "j": "low" })).as_low_priority(),
    );
    let normal_late = due(
        10,
        Job::new(queue_name.clone(), json!({ "j": "normal_late" })),
    );
    let high = due(
        5,
        Job::new(queue_name.clone(), json!({ "j": "high" })).as_high_priority(),
    );
    let normal_early = due(
        20,
        Job::new(queue_name.clone(), json!({ "j": "normal_early" })),
    );
    // A dependency on a job in another queue that has not run yet.
    let dependency = Job::new(format!("{queue_name}_dep"), json!({ "j": "dependency" }));
    let waiting = due(
        60,
        Job::new(queue_name.clone(), json!({ "j": "waiting" }))
            .with_priority(JobPriority::Critical)
            .depends_on(&dependency.id),
    );
    let future = Job::with_delay(
        queue_name.clone(),
        json!({ "j": "future" }),
        ChronoDuration::hours(1),
    )
    .with_priority(JobPriority::Critical);

    let expected = [high.id, normal_early.id, normal_late.id, low.id];
    let waiting_id = waiting.id;
    let mut ids = Vec::new();
    for job in [
        dependency,
        low,
        normal_late,
        high,
        normal_early,
        waiting,
        future,
    ] {
        ids.push(queue.enqueue(job).await.unwrap());
    }
    assert_eq!(
        queue
            .get_job(waiting_id)
            .await
            .unwrap()
            .unwrap()
            .dependency_status,
        DependencyStatus::Waiting
    );

    let ready: Vec<JobId> = queue
        .get_ready_jobs(&queue_name, 10)
        .await
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(ready, expected);

    let limited: Vec<JobId> = queue
        .get_ready_jobs(&queue_name, 2)
        .await
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(limited, expected[..2]);

    for id in ids {
        queue.delete_job(id).await.unwrap();
    }
}

/// #112 (4): cancelling a workflow fails its unfinished jobs, running ones included,
/// marks the dependencies of the jobs still waiting as failed and leaves finished jobs
/// alone.
async fn cancel_workflow_fails_unfinished_jobs<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let name = test_utils::unique_queue("cancel_workflow");
    let running = Job::new(format!("{name}_running"), json!({ "j": "running" }));
    let waiting =
        Job::new(format!("{name}_waiting"), json!({ "j": "waiting" })).depends_on(&running.id);
    let pending = Job::new(format!("{name}_pending"), json!({ "j": "pending" }));
    let done = Job::new(format!("{name}_done"), json!({ "j": "done" }));
    let (running_id, waiting_id, pending_id, done_id) =
        (running.id, waiting.id, pending.id, done.id);
    let workflow_id = queue
        .enqueue_workflow(
            JobGroup::new("cancel_workflow")
                .add_job(running)
                .add_job(waiting)
                .add_job(pending)
                .add_job(done),
        )
        .await
        .unwrap();

    let run = queue
        .dequeue_one(&format!("{name}_running"))
        .await
        .unwrap()
        .expect("job runs");
    assert_eq!(run.id, running_id);
    queue.complete_job(done_id).await.unwrap();

    queue.cancel_workflow(workflow_id).await.unwrap();

    for id in [running_id, waiting_id, pending_id] {
        let job = queue.get_job(id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed, "{id}");
        assert_eq!(job.error_message.as_deref(), Some("Workflow cancelled"));
        assert!(job.failed_at.is_some());
    }
    assert_eq!(
        queue
            .get_job(waiting_id)
            .await
            .unwrap()
            .unwrap()
            .dependency_status,
        DependencyStatus::Failed,
        "the job waiting on its dependencies records that they failed"
    );
    assert_eq!(
        queue
            .get_job(running_id)
            .await
            .unwrap()
            .unwrap()
            .dependency_status,
        DependencyStatus::None
    );
    let done = queue.get_job(done_id).await.unwrap().unwrap();
    assert_eq!(done.status, JobStatus::Completed);
    assert_eq!(done.error_message, None);

    let workflow = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workflow.status, WorkflowStatus::Cancelled);
    assert!(workflow.failed_at.is_some());

    for id in [running_id, waiting_id, pending_id, done_id] {
        queue.delete_job(id).await.unwrap();
    }
}

#[cfg(feature = "postgres")]
crate::test_utils::backend_tests!(
    postgres_tests,
    test_utils::setup_postgres_queue,
    ignore = "requires PostgreSQL: DATABASE_URL",
    [
        restore_resets_the_job,
        batch_status_follows_its_jobs,
        batch_status_counts_archived_jobs,
        ready_jobs_honour_dependencies_and_order,
        cancel_workflow_fails_unfinished_jobs,
    ]
);

#[cfg(feature = "mysql")]
crate::test_utils::backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        restore_resets_the_job,
        batch_status_follows_its_jobs,
        batch_status_counts_archived_jobs,
        ready_jobs_honour_dependencies_and_order,
        cancel_workflow_fails_unfinished_jobs,
    ]
);

#[cfg(feature = "test-queue")]
crate::test_utils::backend_tests!(
    test_queue_tests,
    test_utils::test_queue,
    allow(unused_attributes),
    [
        restore_resets_the_job,
        batch_status_follows_its_jobs,
        batch_status_counts_archived_jobs,
        ready_jobs_honour_dependencies_and_order,
        cancel_workflow_fails_unfinished_jobs,
    ]
);
