//! #111: archival and workflows.
//!
//! - Archiving a job with a retry strategy works on MySQL (the archive column was
//!   `VARCHAR(50)`), and archived jobs keep their times to the microsecond.
//! - A workflow counts its archived member jobs, so archiving a finished job does not
//!   keep the workflow from completing.
//! - Spawned children that join a workflow raise its `total_jobs`.
//! - The 1000-dependency limit is enforced the same way on every backend.
//! - `JobArchiver::run_scheduled_pass` applies per-queue policies.
//!
//! Each scenario is written once against the `DatabaseQueue` trait and runs on both
//! database backends and on `TestQueue`.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Duration as ChronoDuration;
use hammerwork::{
    HammerworkError, Job, JobId, JobStatus,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason, JobArchiver},
    job::MAX_JOB_DEPENDENCIES,
    queue::{DatabaseQueue, JobOutcome},
    retry::{JitterType, RetryStrategy},
    workflow::{FailurePolicy, JobGroup, WorkflowId, WorkflowStatus},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

/// The workflow row as stored (`status`, `total_jobs`, `completed_jobs`,
/// `failed_jobs`), to check what terminal transitions persist and not only what
/// `get_workflow_status` computes when read.
trait StoredWorkflow {
    async fn stored_workflow(&self, id: WorkflowId) -> (String, i32, i32, i32);
}

#[cfg(feature = "postgres")]
impl StoredWorkflow for hammerwork::JobQueue<sqlx::Postgres> {
    async fn stored_workflow(&self, id: WorkflowId) -> (String, i32, i32, i32) {
        sqlx::query_as(
            "SELECT status, total_jobs, completed_jobs, failed_jobs \
             FROM hammerwork_workflows WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[cfg(feature = "mysql")]
impl StoredWorkflow for hammerwork::JobQueue<sqlx::MySql> {
    async fn stored_workflow(&self, id: WorkflowId) -> (String, i32, i32, i32) {
        sqlx::query_as(
            "SELECT status, total_jobs, completed_jobs, failed_jobs \
             FROM hammerwork_workflows WHERE id = ?",
        )
        .bind(id.to_string())
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[cfg(feature = "test")]
impl StoredWorkflow for hammerwork::queue::test::TestQueue {
    async fn stored_workflow(&self, id: WorkflowId) -> (String, i32, i32, i32) {
        // TestQueue computes the workflow when it is read.
        let workflow = self.get_workflow_status(id).await.unwrap().unwrap();
        (
            workflow.status.as_str().to_string(),
            workflow.total_jobs as i32,
            workflow.completed_jobs as i32,
            workflow.failed_jobs as i32,
        )
    }
}

/// Claim the next job of `queue_name`, waiting for it to become due by the database
/// clock (and for a row lock held by another test to go away: a claim skips locked
/// rows).
async fn dequeue_one<Q>(queue: &Arc<Q>, queue_name: &str) -> Job
where
    Q: DatabaseQueue + ?Sized,
{
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(job) = queue.dequeue(queue_name).await.unwrap() {
            return job;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no job became runnable in {queue_name}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Wait until job `id` finished before `Utc::now()`. Finish times come from the
/// database clock and archival cutoffs from the application clock, so a job archived
/// "after 0 seconds" right after it finished could otherwise be skipped.
async fn finished_in_the_past<Q: DatabaseQueue + ?Sized>(queue: &Q, id: JobId) {
    let job = queue.get_job(id).await.unwrap().expect("job exists");
    let finished = job
        .completed_at
        .or(job.failed_at)
        .or(job.timed_out_at)
        .expect("job finished");
    while chrono::Utc::now() <= finished {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A policy that archives finished jobs right away and never purges.
fn archive_now() -> ArchivalPolicy {
    let mut policy = ArchivalPolicy::new()
        .archive_completed_after(ChronoDuration::seconds(0))
        .archive_failed_after(ChronoDuration::seconds(0))
        .archive_dead_after(ChronoDuration::seconds(0))
        .archive_timed_out_after(ChronoDuration::seconds(0));
    policy.purge_archived_after = None;
    policy
}

async fn archive_queue<Q: DatabaseQueue>(queue: &Q, queue_name: &str) -> u64 {
    queue
        .archive_jobs(
            Some(queue_name),
            &archive_now(),
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            Some("archive_workflow_regression_tests"),
        )
        .await
        .unwrap()
        .jobs_archived
}

/// A retry strategy whose serialized form is far longer than 50 characters.
fn long_retry_strategy() -> RetryStrategy {
    RetryStrategy::Exponential {
        base: Duration::from_secs(1),
        multiplier: 2.0,
        max_delay: Some(Duration::from_secs(600)),
        jitter: Some(JitterType::Multiplicative(0.1)),
    }
}

/// #111 (1): archiving a job with a retry strategy used to fail on strict MySQL
/// (`retry_strategy VARCHAR(50)`, error 1406), and every later pass failed on the same
/// oldest jobs. The strategy now survives archive and restore.
async fn archiving_keeps_long_retry_strategies<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("archive_retry_strategy");
    let job =
        Job::new(queue_name.clone(), json!({ "n": 1 })).with_retry_strategy(long_retry_strategy());
    let id = queue.enqueue(job).await.unwrap();
    queue.complete_job(id).await.unwrap();
    finished_in_the_past(queue.as_ref(), id).await;

    assert_eq!(archive_queue(queue.as_ref(), &queue_name).await, 1);

    let restored = queue.restore_archived_job(id).await.unwrap();
    assert_eq!(
        format!("{:?}", restored.retry_strategy),
        format!("{:?}", Some(long_retry_strategy()))
    );
    queue.delete_job(id).await.unwrap();
}

/// #111 cleanup: the MySQL archive stored second-precision timestamps, so archiving
/// rounded a job's times. Archived jobs keep them to the microsecond.
async fn archived_jobs_keep_their_times<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("archive_times");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();
    queue.complete_job(id).await.unwrap();
    finished_in_the_past(queue.as_ref(), id).await;
    let live = queue.get_job(id).await.unwrap().unwrap();

    assert_eq!(archive_queue(queue.as_ref(), &queue_name).await, 1);

    let archived = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(archived.status, JobStatus::Archived);
    assert_eq!(archived.created_at, live.created_at);
    assert_eq!(archived.scheduled_at, live.scheduled_at);
    assert_eq!(archived.completed_at, live.completed_at);
    queue
        .purge_archived_jobs_in_queue(&queue_name, chrono::Utc::now() + ChronoDuration::days(1))
        .await
        .unwrap();
}

/// #111 (2): a workflow member that is archived after it finished still counts, so
/// the workflow completes when its last live job does.
async fn archived_members_count_towards_their_workflow<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + StoredWorkflow + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let name = test_utils::unique_queue("archived_member");
    let first = Job::new(format!("{name}_first"), json!({ "step": "first" }));
    let second = Job::new(format!("{name}_second"), json!({ "step": "second" }));
    let (first_id, second_id) = (first.id, second.id);
    let workflow_id = queue
        .enqueue_workflow(
            JobGroup::new("archived_member")
                .add_job(first)
                .add_job(second),
        )
        .await
        .unwrap();

    queue.complete_job(first_id).await.unwrap();
    finished_in_the_past(queue.as_ref(), first_id).await;
    assert_eq!(
        archive_queue(queue.as_ref(), &format!("{name}_first")).await,
        1
    );
    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Running);
    assert_eq!(status.completed_jobs, 1, "the archived job still counts");

    queue.complete_job(second_id).await.unwrap();
    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Completed);
    assert_eq!((status.completed_jobs, status.failed_jobs), (2, 0));
    assert_eq!(
        queue.stored_workflow(workflow_id).await,
        ("completed".to_string(), 2, 2, 0),
        "the terminal transition persists the completion"
    );

    // The archived member is listed too, as `get_job` reports it.
    let jobs = queue.get_workflow_jobs(workflow_id).await.unwrap();
    let statuses: Vec<(JobId, JobStatus)> = jobs.iter().map(|j| (j.id, j.status)).collect();
    assert_eq!(statuses.len(), 2, "{statuses:?}");
    assert!(
        statuses.contains(&(first_id, JobStatus::Archived)),
        "{statuses:?}"
    );
    assert!(
        statuses.contains(&(second_id, JobStatus::Completed)),
        "{statuses:?}"
    );
    assert_eq!(status.jobs.len(), 2);
}

/// #111 (2): an archived failure counts as a failure.
async fn archived_failures_fail_their_workflow<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + StoredWorkflow + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let name = test_utils::unique_queue("archived_failure");
    let first = Job::new(format!("{name}_first"), json!({}));
    let second = Job::new(format!("{name}_second"), json!({}));
    let (first_id, second_id) = (first.id, second.id);
    let workflow_id = queue
        .enqueue_workflow(
            JobGroup::new("archived_failure")
                .with_failure_policy(FailurePolicy::ContinueOnFailure)
                .add_job(first)
                .add_job(second),
        )
        .await
        .unwrap();

    queue.mark_job_dead(first_id, "boom").await.unwrap();
    finished_in_the_past(queue.as_ref(), first_id).await;
    assert_eq!(
        archive_queue(queue.as_ref(), &format!("{name}_first")).await,
        1
    );
    queue.complete_job(second_id).await.unwrap();

    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Failed);
    assert_eq!((status.completed_jobs, status.failed_jobs), (1, 1));
    assert_eq!(
        queue.stored_workflow(workflow_id).await,
        ("failed".to_string(), 2, 1, 1)
    );
}

/// #111 (3): children spawned by a workflow job join the workflow and raise its
/// `total_jobs`, so it neither completes before they run nor ignores their failure.
async fn spawned_children_raise_workflow_total<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + StoredWorkflow + Send + Sync + 'static,
{
    // Serialized with the archiving tests: on MySQL a concurrent dequeue's
    // `FOR UPDATE` can lock rows they need, which their `SKIP LOCKED` then skips.
    let _serial = test_utils::serial().await;
    let name = test_utils::unique_queue("spawned_workflow");
    let children_queue = format!("{name}_children");
    let workflow = JobGroup::new("spawned_workflow")
        .with_failure_policy(FailurePolicy::ContinueOnFailure)
        .add_job(Job::new(name.clone(), json!({ "step": "parent" })));
    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();

    let run = dequeue_one(&queue, &name).await;
    // What `SpawnManager::prepare_spawn` does: depend on the parent, join its workflow.
    let child = |n: u32| {
        let mut child = Job::new(children_queue.clone(), json!({ "n": n })).depends_on(&run.id);
        child.workflow_id = run.workflow_id;
        child.workflow_name = run.workflow_name.clone();
        child
    };
    let recorded = queue
        .complete_job_run_with_children(&run, vec![child(1), child(2)])
        .await
        .unwrap()
        .expect("the run was current");
    assert_eq!(recorded.spawned.len(), 2);

    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.total_jobs, 3);
    assert_eq!(
        status.status,
        WorkflowStatus::Running,
        "children are pending"
    );
    assert_eq!(status.completed_at, None);
    assert_eq!(
        queue.stored_workflow(workflow_id).await,
        ("running".to_string(), 3, 1, 0)
    );

    let first = dequeue_one(&queue, &children_queue).await;
    queue
        .finish_job_run(&first, JobOutcome::Completed)
        .await
        .unwrap();
    assert_eq!(
        queue.stored_workflow(workflow_id).await,
        ("running".to_string(), 3, 2, 0)
    );

    let second = dequeue_one(&queue, &children_queue).await;
    queue
        .finish_job_run(
            &second,
            JobOutcome::Dead {
                error: "child failed".to_string(),
            },
        )
        .await
        .unwrap();
    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Failed);
    assert_eq!(
        queue.stored_workflow(workflow_id).await,
        ("failed".to_string(), 3, 2, 1)
    );
}

fn assert_dependency_limit_error(result: hammerwork::Result<impl std::fmt::Debug>, what: &str) {
    match result {
        Err(HammerworkError::Workflow { message }) => {
            assert!(message.contains("at most 1000"), "{what}: {message}")
        }
        other => panic!("{what}: expected the dependency limit error, got {other:?}"),
    }
}

/// #111 (4): every backend refuses a job with more than 1000 dependencies with the
/// same validation error, before writing anything.
async fn dependency_limit_is_enforced<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    // Serialized with the archiving tests: on MySQL a concurrent dequeue's
    // `FOR UPDATE` can lock rows they need, which their `SKIP LOCKED` then skips.
    let _serial = test_utils::serial().await;
    let name = test_utils::unique_queue("dependency_limit");
    let parents: Vec<JobId> = (0..=MAX_JOB_DEPENDENCIES)
        .map(|_| uuid::Uuid::new_v4())
        .collect();
    let too_many = || Job::new(name.clone(), json!({})).depends_on_jobs(&parents);

    let job = too_many();
    let id = job.id;
    assert_dependency_limit_error(queue.enqueue(job).await, "enqueue");
    assert!(
        queue.get_job(id).await.unwrap().is_none(),
        "nothing written"
    );

    let batch = hammerwork::batch::JobBatch::new("too_many").with_jobs(vec![too_many()]);
    let batch_id = batch.id;
    assert_dependency_limit_error(queue.enqueue_batch(batch).await, "enqueue_batch");
    assert!(
        queue.get_batch_status(batch_id).await.is_err(),
        "no batch written"
    );

    // A workflow job can only depend on jobs of the workflow, so build one with
    // 1001 parents.
    let roots: Vec<Job> = (0..=MAX_JOB_DEPENDENCIES)
        .map(|_| Job::new(name.clone(), json!({})))
        .collect();
    let root_ids: Vec<JobId> = roots.iter().map(|j| j.id).collect();
    let mut group = JobGroup::new("too_many");
    for root in roots {
        group = group.add_job(root);
    }
    let group = group.add_job(Job::new(name.clone(), json!({})).depends_on_jobs(&root_ids));
    let workflow_id = group.id;
    assert_dependency_limit_error(queue.enqueue_workflow(group).await, "enqueue_workflow");
    assert!(
        queue
            .get_workflow_status(workflow_id)
            .await
            .unwrap()
            .is_none()
    );

    // Spawned children: the parent's run is not completed.
    let parent = queue
        .enqueue(Job::new(name.clone(), json!({ "parent": true })))
        .await
        .unwrap();
    let run = dequeue_one(&queue, &name).await;
    assert_eq!(run.id, parent);
    let child = Job::new(format!("{name}_child"), json!({}))
        .depends_on_jobs(&parents[1..])
        .depends_on(&run.id);
    assert_dependency_limit_error(
        queue
            .complete_job_run_with_children(&run, vec![child])
            .await,
        "complete_job_run_with_children",
    );
    assert_eq!(
        queue.get_job(parent).await.unwrap().unwrap().status,
        JobStatus::Running
    );

    // Exactly the limit is accepted.
    let at_limit = Job::new(format!("{name}_limit"), json!({}))
        .depends_on_jobs(&parents[..MAX_JOB_DEPENDENCIES]);
    let at_limit_id = queue.enqueue(at_limit).await.unwrap();
    queue.delete_job(at_limit_id).await.unwrap();
    queue.delete_job(parent).await.unwrap();
}

/// An archiver for `run_scheduled_pass`, which only uses the queue it is given.
#[cfg(feature = "postgres")]
fn archiver() -> JobArchiver<sqlx::Postgres> {
    JobArchiver::new(test_utils::lazy_postgres_pool())
}

/// An archiver for `run_scheduled_pass`, which only uses the queue it is given.
#[cfg(not(feature = "postgres"))]
fn archiver() -> JobArchiver<sqlx::MySql> {
    JobArchiver::new(sqlx::MySqlPool::connect_lazy(&test_utils::mysql_url()).unwrap())
}

/// #111 (5): `run_scheduled_pass` archives (and purges) each queue with its own
/// policy, even when the default policy is disabled, and leaves the queues without a
/// policy alone.
async fn scheduled_pass_applies_queue_policies<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let with_policy = test_utils::unique_queue("scheduled_own_policy");
    let disabled = test_utils::unique_queue("scheduled_disabled_policy");
    let without_policy = test_utils::unique_queue("scheduled_no_policy");
    let mut ids = Vec::new();
    for queue_name in [&with_policy, &disabled, &without_policy] {
        let id = queue
            .enqueue(Job::new(queue_name.clone(), json!({})))
            .await
            .unwrap();
        queue.complete_job(id).await.unwrap();
        finished_in_the_past(queue.as_ref(), id).await;
        ids.push(id);
    }

    let mut archiver = archiver();
    archiver.set_default_policy(archive_now().enabled(false));
    archiver.set_policy(with_policy.as_str(), archive_now());
    archiver.set_policy(disabled.as_str(), archive_now().enabled(false));

    let pass = archiver
        .run_scheduled_pass(queue.as_ref(), Some("scheduler"), || false)
        .await
        .unwrap();
    assert_eq!(pass.jobs_archived, 1, "{pass:?}");
    assert!(!pass.stopped_early);
    let statuses: Vec<JobStatus> = {
        let mut statuses = Vec::new();
        for id in &ids {
            statuses.push(queue.get_job(*id).await.unwrap().unwrap().status);
        }
        statuses
    };
    assert_eq!(
        statuses,
        [
            JobStatus::Archived,
            JobStatus::Completed,
            JobStatus::Completed
        ]
    );

    // The queue's own `purge_archived_after` purges only that queue's archive.
    archive_queue(queue.as_ref(), &without_policy).await;
    let mut purging = archive_now();
    purging.purge_archived_after = Some(ChronoDuration::seconds(0));
    archiver.set_policy(with_policy.as_str(), purging);
    let pass = archiver
        .run_scheduled_pass(queue.as_ref(), Some("scheduler"), || false)
        .await
        .unwrap();
    assert_eq!(pass.jobs_purged, 1, "{pass:?}");
    assert!(queue.get_job(ids[0]).await.unwrap().is_none(), "purged");
    assert_eq!(
        queue.get_job(ids[2]).await.unwrap().unwrap().status,
        JobStatus::Archived,
        "another queue's archive is kept"
    );

    queue.delete_job(ids[1]).await.unwrap();
    queue
        .purge_archived_jobs_in_queue(
            &without_policy,
            chrono::Utc::now() + ChronoDuration::days(1),
        )
        .await
        .unwrap();
}

/// `list_queue_names` lists queues with live and with archived jobs.
async fn queue_names_include_archived_queues<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let live = test_utils::unique_queue("names_live");
    let archived = test_utils::unique_queue("names_archived");
    let live_id = queue
        .enqueue(Job::new(live.clone(), json!({})))
        .await
        .unwrap();
    let archived_id = queue
        .enqueue(Job::new(archived.clone(), json!({})))
        .await
        .unwrap();
    queue.complete_job(archived_id).await.unwrap();
    finished_in_the_past(queue.as_ref(), archived_id).await;
    assert_eq!(archive_queue(queue.as_ref(), &archived).await, 1);

    let names = queue.list_queue_names().await.unwrap();
    assert!(names.contains(&live), "{live} missing");
    assert!(names.contains(&archived), "{archived} missing");
    assert!(names.windows(2).all(|w| w[0] < w[1]), "sorted and unique");

    queue.delete_job(live_id).await.unwrap();
    queue
        .purge_archived_jobs_in_queue(&archived, chrono::Utc::now() + ChronoDuration::days(1))
        .await
        .unwrap();
}

/// #111 (5), with the default policy enabled: a queue without a policy of its own gets
/// the default one, a queue with its own policy keeps it. Only run on `TestQueue`: on
/// a shared database an archive-now default policy would archive other tests' jobs.
#[cfg(feature = "test")]
async fn scheduled_pass_applies_default_policy_to_other_queues<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let keep = test_utils::unique_queue("scheduled_keep");
    let other = test_utils::unique_queue("scheduled_other");
    let keep_id = queue
        .enqueue(Job::new(keep.clone(), json!({})))
        .await
        .unwrap();
    let other_id = queue
        .enqueue(Job::new(other.clone(), json!({})))
        .await
        .unwrap();
    queue.complete_job(keep_id).await.unwrap();
    queue.complete_job(other_id).await.unwrap();

    let mut archiver = archiver();
    archiver.set_default_policy(archive_now());
    archiver.set_policy(
        keep.as_str(),
        ArchivalPolicy::new().archive_completed_after(ChronoDuration::days(30)),
    );
    let pass = archiver
        .run_scheduled_pass(queue.as_ref(), None, || false)
        .await
        .unwrap();
    assert_eq!(pass.jobs_archived, 1);
    assert_eq!(
        queue.get_job(keep_id).await.unwrap().unwrap().status,
        JobStatus::Completed
    );
    assert_eq!(
        queue.get_job(other_id).await.unwrap().unwrap().status,
        JobStatus::Archived
    );
}

#[cfg(feature = "postgres")]
crate::test_utils::backend_tests!(
    postgres_tests,
    test_utils::setup_postgres_queue,
    ignore = "requires PostgreSQL: DATABASE_URL",
    [
        archiving_keeps_long_retry_strategies,
        archived_jobs_keep_their_times,
        archived_members_count_towards_their_workflow,
        archived_failures_fail_their_workflow,
        spawned_children_raise_workflow_total,
        dependency_limit_is_enforced,
        scheduled_pass_applies_queue_policies,
        queue_names_include_archived_queues,
    ]
);

#[cfg(feature = "mysql")]
crate::test_utils::backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        archiving_keeps_long_retry_strategies,
        archived_jobs_keep_their_times,
        archived_members_count_towards_their_workflow,
        archived_failures_fail_their_workflow,
        spawned_children_raise_workflow_total,
        dependency_limit_is_enforced,
        scheduled_pass_applies_queue_policies,
        queue_names_include_archived_queues,
    ]
);

#[cfg(feature = "test")]
crate::test_utils::backend_tests!(
    test_queue_tests,
    test_utils::test_queue,
    allow(unused_attributes),
    [
        archiving_keeps_long_retry_strategies,
        archived_jobs_keep_their_times,
        archived_members_count_towards_their_workflow,
        archived_failures_fail_their_workflow,
        spawned_children_raise_workflow_total,
        dependency_limit_is_enforced,
        scheduled_pass_applies_queue_policies,
        queue_names_include_archived_queues,
        scheduled_pass_applies_default_policy_to_other_queues,
    ]
);
