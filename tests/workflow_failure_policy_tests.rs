//! How a job's terminal failure is applied, on both backends and on `TestQueue`:
//! `fail_job` is terminal, and a workflow's `FailurePolicy` decides what happens to
//! the other jobs (`fail_job`, a worker's `finish_job_run` and the explicit
//! `fail_job_dependencies` helper all apply it the same way).

#![cfg(any(feature = "postgres", feature = "mysql", feature = "test"))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    Job, JobId, JobStatus,
    queue::{DatabaseQueue, JobOutcome},
    workflow::{FailurePolicy, JobGroup, WorkflowId, WorkflowStatus},
};
use serde_json::json;
use std::sync::Arc;

/// A workflow `a -> b -> c` plus an independent job `x`. Each job has its own queue,
/// so the scenarios dequeue exactly the job they mean.
struct Graph {
    workflow: WorkflowId,
    queue: String,
    a: JobId,
    b: JobId,
    c: JobId,
    x: JobId,
}

impl Graph {
    fn queue_of(&self, step: &str) -> String {
        format!("{}_{step}", self.queue)
    }

    fn ids(&self) -> [JobId; 4] {
        [self.a, self.b, self.c, self.x]
    }
}

async fn enqueue_graph<Q>(queue: &Arc<Q>, policy: FailurePolicy) -> Graph
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let name = test_utils::unique_queue("failure_policy");
    let job = |step: &str| Job::new(format!("{name}_{step}"), json!({ "step": step }));
    let a = job("a");
    let x = job("x");
    let b = job("b").depends_on(&a.id);
    let c = job("c").depends_on(&b.id);
    let (a_id, b_id, c_id, x_id) = (a.id, b.id, c.id, x.id);
    let workflow = queue
        .enqueue_workflow(
            JobGroup::new(format!("policy-{}", policy.as_str()))
                .with_failure_policy(policy)
                .add_job(a)
                .add_job(x)
                .add_job(b)
                .add_job(c),
        )
        .await
        .unwrap();
    Graph {
        workflow,
        queue: name,
        a: a_id,
        b: b_id,
        c: c_id,
        x: x_id,
    }
}

async fn status_of<Q>(queue: &Arc<Q>, id: JobId) -> JobStatus
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    queue.get_job(id).await.unwrap().expect("job exists").status
}

async fn workflow_status<Q>(queue: &Arc<Q>, graph: &Graph) -> WorkflowStatus
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    queue
        .get_workflow_status(graph.workflow)
        .await
        .unwrap()
        .expect("workflow exists")
        .status
}

/// Dequeue the job of `step`, asserting it is `expected`.
async fn run<Q>(queue: &Arc<Q>, graph: &Graph, step: &str, expected: JobId) -> Job
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let job = queue
        .dequeue(&graph.queue_of(step))
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("job {step} is runnable"));
    assert_eq!(job.id, expected);
    job
}

/// Whether `step`'s queue has a runnable job.
async fn runnable<Q>(queue: &Arc<Q>, graph: &Graph, step: &str) -> bool
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    !queue
        .get_ready_jobs(&graph.queue_of(step), 10)
        .await
        .unwrap()
        .is_empty()
}

/// A time that is already due on every backend (`TestQueue`'s mock clock starts at
/// its creation and does not advance on its own).
fn past() -> chrono::DateTime<Utc> {
    Utc::now() - chrono::Duration::hours(1)
}

fn sorted(mut ids: Vec<JobId>) -> Vec<JobId> {
    ids.sort();
    ids
}

async fn cleanup<Q>(queue: &Arc<Q>, graph: &Graph)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    for id in graph.ids() {
        queue.delete_job(id).await.unwrap();
    }
}

/// How the failure of `a` is recorded.
#[derive(Clone, Copy, Debug)]
enum Trigger {
    /// `fail_job`, the manual terminal failure.
    FailJob,
    /// A worker's last attempt failing: `finish_job_run` with a `Dead` outcome.
    WorkerDead,
    /// `fail_job_dependencies` called directly, while `a` is still running.
    Explicit,
}

/// Fail `a` with `trigger`. Returns the jobs the queue reports as failed with it,
/// when the trigger reports them.
async fn fail_a<Q>(queue: &Arc<Q>, graph: &Graph, trigger: Trigger) -> Option<Vec<JobId>>
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let run_a = run(queue, graph, "a", graph.a).await;
    match trigger {
        Trigger::FailJob => {
            queue.fail_job(graph.a, "a failed").await.unwrap();
            assert_eq!(status_of(queue, graph.a).await, JobStatus::Failed);
            None
        }
        Trigger::WorkerDead => {
            let outcome = JobOutcome::Dead {
                error: "a failed".to_string(),
            };
            let recorded = queue
                .finish_job_run(&run_a, outcome)
                .await
                .unwrap()
                .expect("the run is current");
            assert_eq!(recorded.status, JobStatus::Dead);
            assert_eq!(status_of(queue, graph.a).await, JobStatus::Dead);
            Some(recorded.cancelled)
        }
        Trigger::Explicit => {
            let failed = queue.fail_job_dependencies(graph.a).await.unwrap();
            assert_eq!(status_of(queue, graph.a).await, JobStatus::Running);
            Some(failed)
        }
    }
}

/// `fail_job` is a terminal `Failed`: no automatic retry, `attempts` unchanged, and
/// `retry_job` re-runs it.
async fn fail_job_is_terminal<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("fail_job_terminal");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})).with_max_attempts(3))
        .await
        .unwrap();
    let job = queue.dequeue(&queue_name).await.unwrap().unwrap();
    assert_eq!(job.attempts, 1);
    queue.fail_job(id, "permanent").await.unwrap();

    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Failed);
    assert_eq!(job.attempts, 1);
    assert_eq!(job.error_message.as_deref(), Some("permanent"));
    assert!(job.failed_at.is_some());
    assert!(queue.dequeue(&queue_name).await.unwrap().is_none());
    assert!(
        queue.fail_job(id, "again").await.is_err(),
        "a Failed job cannot fail again"
    );

    queue.retry_job(id, past()).await.unwrap();
    let job = queue.dequeue(&queue_name).await.unwrap().unwrap();
    assert_eq!(job.id, id);
    assert_eq!(job.attempts, 2);
    queue.delete_job(id).await.unwrap();
}

/// `FailFast`: the whole workflow stops.
async fn fail_fast<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    for trigger in [Trigger::FailJob, Trigger::WorkerDead, Trigger::Explicit] {
        let graph = enqueue_graph(&queue, FailurePolicy::FailFast).await;
        let reported = fail_a(&queue, &graph, trigger).await;
        if let Some(reported) = reported {
            assert_eq!(
                sorted(reported),
                sorted(vec![graph.b, graph.c, graph.x]),
                "{trigger:?}"
            );
        }
        for id in [graph.b, graph.c, graph.x] {
            let job = queue.get_job(id).await.unwrap().unwrap();
            assert_eq!(job.status, JobStatus::Failed, "{trigger:?}");
            assert!(job.error_message.is_some(), "{trigger:?}");
        }
        for step in ["b", "c", "x"] {
            assert!(!runnable(&queue, &graph, step).await, "{trigger:?} {step}");
        }
        assert_eq!(
            workflow_status(&queue, &graph).await,
            WorkflowStatus::Failed,
            "{trigger:?}"
        );
        // Applying the failure again changes nothing.
        assert!(
            queue
                .fail_job_dependencies(graph.a)
                .await
                .unwrap()
                .is_empty(),
            "{trigger:?}"
        );
        cleanup(&queue, &graph).await;
    }
}

/// `ContinueOnFailure`: the failed job's dependents cannot run, the independent
/// branch continues, and the workflow fails once everything has finished.
async fn continue_on_failure<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    for trigger in [Trigger::FailJob, Trigger::WorkerDead, Trigger::Explicit] {
        let graph = enqueue_graph(&queue, FailurePolicy::ContinueOnFailure).await;
        let reported = fail_a(&queue, &graph, trigger).await;
        if let Some(reported) = reported {
            assert_eq!(
                sorted(reported),
                sorted(vec![graph.b, graph.c]),
                "{trigger:?}"
            );
        }
        for id in [graph.b, graph.c] {
            let job = queue.get_job(id).await.unwrap().unwrap();
            assert_eq!(job.status, JobStatus::Failed, "{trigger:?}");
            assert!(job.dependencies_failed(), "{trigger:?}");
        }
        assert_eq!(status_of(&queue, graph.x).await, JobStatus::Pending);
        assert_eq!(
            workflow_status(&queue, &graph).await,
            WorkflowStatus::Running,
            "{trigger:?}"
        );

        // The independent branch still runs.
        run(&queue, &graph, "x", graph.x).await;
        if matches!(trigger, Trigger::Explicit) {
            // `a` is still running in this scenario; finish it.
            queue.fail_job(graph.a, "a failed").await.unwrap();
        }
        queue.complete_job(graph.x).await.unwrap();
        assert_eq!(status_of(&queue, graph.x).await, JobStatus::Completed);
        assert_eq!(
            workflow_status(&queue, &graph).await,
            WorkflowStatus::Failed,
            "{trigger:?}: every job finished, one failed"
        );
        cleanup(&queue, &graph).await;
    }
}

/// `Manual`: nothing is failed automatically. The dependents wait until an operator
/// re-runs the failed job, whose completion releases them.
async fn manual<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    for trigger in [Trigger::FailJob, Trigger::WorkerDead, Trigger::Explicit] {
        let graph = enqueue_graph(&queue, FailurePolicy::Manual).await;
        let reported = fail_a(&queue, &graph, trigger).await;
        if let Some(reported) = reported {
            assert!(reported.is_empty(), "{trigger:?}: {reported:?}");
        }
        for id in [graph.b, graph.c, graph.x] {
            assert_eq!(
                status_of(&queue, id).await,
                JobStatus::Pending,
                "{trigger:?}"
            );
        }
        for id in [graph.b, graph.c] {
            let job = queue.get_job(id).await.unwrap().unwrap();
            assert!(!job.dependencies_failed(), "{trigger:?}");
        }
        assert!(!runnable(&queue, &graph, "b").await, "{trigger:?}");
        assert!(runnable(&queue, &graph, "x").await, "{trigger:?}");
        assert_eq!(
            workflow_status(&queue, &graph).await,
            WorkflowStatus::Running,
            "{trigger:?}"
        );

        // The operator re-runs `a`; its completion releases `b`.
        match trigger {
            Trigger::FailJob => queue.retry_job(graph.a, past()).await.unwrap(),
            Trigger::WorkerDead => queue.retry_dead_job(graph.a).await.unwrap(),
            // `a` is still running in this scenario.
            Trigger::Explicit => {}
        }
        if !matches!(trigger, Trigger::Explicit) {
            run(&queue, &graph, "a", graph.a).await;
        }
        queue.complete_job(graph.a).await.unwrap();
        queue.resolve_job_dependencies(graph.a).await.unwrap();
        run(&queue, &graph, "b", graph.b).await;
        assert_eq!(
            workflow_status(&queue, &graph).await,
            WorkflowStatus::Running,
            "{trigger:?}"
        );
        cleanup(&queue, &graph).await;
    }
}

/// Jobs that depend on a failed job outside any workflow can no longer run.
async fn dependents_outside_workflows<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("no_workflow");
    let parent = Job::new(queue_name.clone(), json!({ "step": "parent" }));
    let child = Job::new(format!("{queue_name}_child"), json!({})).depends_on(&parent.id);
    let (parent_id, child_id) = (parent.id, child.id);
    // A one-job group without a policy would still be a workflow; enqueue both jobs
    // as plain jobs instead.
    queue.enqueue(parent).await.unwrap();
    queue.enqueue(child).await.unwrap();

    queue.dequeue(&queue_name).await.unwrap().unwrap();
    let failed = queue.fail_job_dependencies(parent_id).await.unwrap();
    assert_eq!(failed, [child_id]);
    let child = queue.get_job(child_id).await.unwrap().unwrap();
    assert_eq!(child.status, JobStatus::Failed);
    assert!(child.dependencies_failed());
    for id in [parent_id, child_id] {
        queue.delete_job(id).await.unwrap();
    }
}

/// One `#[tokio::test]` per scenario, run against the queue returned by `$setup`.
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
        fail_job_is_terminal,
        fail_fast,
        continue_on_failure,
        manual,
        dependents_outside_workflows,
    ]
);

#[cfg(feature = "mysql")]
backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        fail_job_is_terminal,
        fail_fast,
        continue_on_failure,
        manual,
        dependents_outside_workflows,
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
        fail_job_is_terminal,
        fail_fast,
        continue_on_failure,
        manual,
        dependents_outside_workflows,
    ]
);
