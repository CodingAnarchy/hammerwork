//! Job lifecycle state machine tests (#19 C3, H2, H3, H5, H6, M1, M2, M3).
//!
//! Each scenario is written once against the `DatabaseQueue` trait and run against
//! both backends. Every test takes the binary-wide serial lock because the zombie
//! worker test runs the stale-job reaper, which reclaims `Running` jobs on every queue.
#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    CronSchedule, HammerworkError, Job, JobId, JobOutcome, JobQueue, JobStatus, ResultStorage,
    RetryStrategy, Worker,
    batch::{BatchStatus, JobBatch, PartialFailureMode},
    queue::DatabaseQueue,
    worker::JobHandler,
    workflow::{DependencyStatus, FailurePolicy, JobGroup, WorkflowStatus},
};
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

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

/// Wait until no job of `ids` is `Pending` or `Running` (or `timeout` elapses).
async fn wait_until_finished<DB>(queue: &Arc<JobQueue<DB>>, ids: &[JobId], timeout: Duration)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    for id in ids {
        wait_for_job(queue, *id, timeout, |job| {
            !matches!(job.status, JobStatus::Pending | JobStatus::Running)
        })
        .await;
    }
}

/// A started worker and the means to stop it.
struct RunningWorker {
    shutdown: mpsc::Sender<()>,
    task: JoinHandle<hammerwork::Result<()>>,
}

impl RunningWorker {
    fn start<DB>(worker: Worker<DB>) -> Self
    where
        DB: sqlx::Database + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    {
        let (shutdown, shutdown_rx) = mpsc::channel(1);
        let task = tokio::spawn(async move { worker.run(shutdown_rx).await });
        Self { shutdown, task }
    }

    async fn stop(self) {
        let _ = self.shutdown.send(()).await;
        self.task.await.unwrap().unwrap();
    }
}

fn worker<DB>(queue: &Arc<JobQueue<DB>>, queue_name: &str, handler: JobHandler) -> Worker<DB>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    Worker::new(Arc::clone(queue), queue_name.to_string(), handler)
        .with_poll_interval(Duration::from_millis(20))
        .with_retry_delay(Duration::from_millis(1))
}

/// A handler that counts its runs and always fails.
fn failing_handler(runs: Arc<AtomicUsize>) -> JobHandler {
    Arc::new(move |_job: Job| {
        let runs = Arc::clone(&runs);
        Box::pin(async move {
            runs.fetch_add(1, Ordering::SeqCst);
            Err(HammerworkError::Processing("boom".to_string()))
        })
    })
}

fn assert_invalid_transition(result: hammerwork::Result<()>, what: &str) {
    match result {
        Err(HammerworkError::InvalidJobTransition { .. }) => {}
        other => panic!("{what}: expected InvalidJobTransition, got {other:?}"),
    }
}

/// H2: manual transitions only apply from valid source states, so a terminal job can
/// no longer be completed or resurrected.
async fn manual_transitions_are_guarded<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("guards");

    // A TimedOut job cannot be completed
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})))
        .await
        .unwrap();
    queue.dequeue(&queue_name).await.unwrap().expect("job");
    queue.mark_job_timed_out(id, "too slow").await.unwrap();
    assert_invalid_transition(queue.complete_job(id).await, "complete TimedOut");
    assert_invalid_transition(queue.fail_job(id, "x").await, "fail TimedOut");
    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::TimedOut);

    // A Dead job cannot be put back to Pending by retry_job...
    queue.mark_job_dead(id, "gave up").await.unwrap();
    assert_invalid_transition(queue.retry_job(id, Utc::now()).await, "retry Dead");
    assert_invalid_transition(queue.complete_job(id).await, "complete Dead");
    assert_eq!(
        queue.get_job(id).await.unwrap().unwrap().status,
        JobStatus::Dead
    );
    // ...only explicitly, through retry_dead_job
    queue.retry_dead_job(id).await.unwrap();
    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!((job.status, job.attempts), (JobStatus::Pending, 0));

    // Completed is final; timeouts only apply to running jobs
    assert_invalid_transition(queue.mark_job_timed_out(id, "x").await, "time out Pending");
    queue.complete_job(id).await.unwrap();
    assert_invalid_transition(queue.retry_dead_job(id).await, "retry_dead Completed");
    assert_invalid_transition(queue.mark_job_dead(id, "x").await, "dead Completed");
    assert_invalid_transition(
        queue.reschedule_cron_job(id, Utc::now()).await,
        "reschedule Completed",
    );

    // Missing jobs are reported as such
    let missing = uuid::Uuid::new_v4();
    assert!(matches!(
        queue.complete_job(missing).await,
        Err(HammerworkError::JobNotFound { .. })
    ));

    queue.delete_job(id).await.unwrap();
}

/// H2: a worker whose run was reclaimed by the stale-job reaper (and finished by
/// someone else) cannot overwrite the job's newer state when it finally returns.
async fn zombie_worker_cannot_overwrite_reclaimed_job<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("zombie");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})).with_max_attempts(2))
        .await
        .unwrap();

    // The zombie: a slow handler that fails once it wakes up.
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let handler: JobHandler = {
        let (started, release) = (Arc::clone(&started), Arc::clone(&release));
        Arc::new(move |_job: Job| {
            let (started, release) = (Arc::clone(&started), Arc::clone(&release));
            Box::pin(async move {
                started.notify_one();
                release.notified().await;
                Err(HammerworkError::Processing("zombie failure".to_string()))
            })
        })
    };
    let zombie = RunningWorker::start(
        worker(&queue, &queue_name, handler).with_lease_duration(Duration::ZERO),
    );
    started.notified().await;

    // The reaper reclaims the run, and another worker runs the job to completion.
    let recovery = queue.requeue_stale_jobs(Duration::ZERO).await.unwrap();
    assert!(recovery.requeued.contains(&id), "{recovery:?}");
    let rerun = queue
        .dequeue(&queue_name)
        .await
        .unwrap()
        .expect("requeued job");
    assert_eq!(rerun.attempts, 2);
    queue.complete_job(id).await.unwrap();

    // The zombie wakes up and reports its failure: it must be discarded.
    release.notify_one();
    tokio::time::sleep(Duration::from_millis(300)).await;
    zombie.stop().await;

    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Completed, "zombie overwrote the job");
    assert_eq!(job.attempts, 2);

    // The same guard at the API level: an outcome for an older run is ignored.
    let mut stale_run = rerun.clone();
    stale_run.attempts = 1;
    assert_eq!(
        queue
            .finish_job_run(
                &stale_run,
                JobOutcome::Dead {
                    error: "late".into()
                }
            )
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        queue
            .finish_job_run(&rerun, JobOutcome::Completed)
            .await
            .unwrap(),
        None,
        "a finished job's run cannot be recorded twice"
    );

    queue.delete_job(id).await.unwrap();
}

/// H3: the job's `max_attempts` is the retry limit; `with_max_retries` only caps it.
async fn job_max_attempts_is_the_retry_limit<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;

    for (max_attempts, worker_cap, expected_runs) in [(5, None, 5), (1, None, 1), (5, Some(2), 2)] {
        let queue_name = test_utils::unique_queue("retry_limit");
        let id = queue
            .enqueue(Job::new(queue_name.clone(), json!({})).with_max_attempts(max_attempts))
            .await
            .unwrap();
        let runs = Arc::new(AtomicUsize::new(0));
        let mut w = worker(&queue, &queue_name, failing_handler(Arc::clone(&runs)));
        if let Some(cap) = worker_cap {
            w = w.with_max_retries(cap);
        }
        let running = RunningWorker::start(w);
        let job = wait_for_job(&queue, id, Duration::from_secs(15), |job| {
            job.status == JobStatus::Dead
        })
        .await;
        // Give a wrongly retried job the chance to run again
        tokio::time::sleep(Duration::from_millis(200)).await;
        running.stop().await;

        let job_after = queue.get_job(id).await.unwrap().unwrap();
        assert_eq!(
            (job.status, job_after.status),
            (JobStatus::Dead, JobStatus::Dead),
            "max_attempts={max_attempts} cap={worker_cap:?}"
        );
        assert_eq!(
            runs.load(Ordering::SeqCst),
            expected_runs,
            "max_attempts={max_attempts} cap={worker_cap:?}"
        );
        assert_eq!(job_after.attempts, expected_runs as i32);
        queue.delete_job(id).await.unwrap();
    }
}

/// M1: a timed-out run is retried while attempts remain; `TimedOut` is terminal only
/// once they are exhausted.
async fn timeouts_retry_until_attempts_are_exhausted<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("timeouts");

    // Times out twice, then succeeds on its third attempt.
    let recovers = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({"slow_runs": 2}))
                .with_max_attempts(3)
                .with_timeout(Duration::from_millis(200)),
        )
        .await
        .unwrap();
    // Always too slow.
    let exhausts = queue
        .enqueue(
            Job::new(queue_name.clone(), json!({"slow_runs": 99}))
                .with_max_attempts(2)
                .with_timeout(Duration::from_millis(200)),
        )
        .await
        .unwrap();

    let runs = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let runs = Arc::clone(&runs);
        Arc::new(move |job: Job| {
            let runs = Arc::clone(&runs);
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                let slow_runs = job.payload["slow_runs"].as_i64().unwrap_or(0) as i32;
                if job.attempts <= slow_runs {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                Ok(())
            })
        })
    };
    let running = RunningWorker::start(worker(&queue, &queue_name, handler));
    wait_until_finished(&queue, &[recovers, exhausts], Duration::from_secs(20)).await;
    running.stop().await;

    let job = queue.get_job(recovers).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    assert_eq!(job.attempts, 3);
    assert!(job.timed_out_at.is_some(), "earlier timeouts are recorded");

    let job = queue.get_job(exhausts).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::TimedOut, "{job:?}");
    assert_eq!(job.attempts, 2);
    assert_eq!(runs.load(Ordering::SeqCst), 5);

    queue.delete_job(recovers).await.unwrap();
    queue.delete_job(exhausts).await.unwrap();
}

/// H5: a recurring job is rescheduled for its next run after a run that died or timed
/// out, not only after a success; the failed run's error stays visible.
async fn recurring_jobs_are_rescheduled_after_terminal_failure<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("cron_failure");
    let hourly = CronSchedule::new("0 0 * * * *").unwrap();

    let mut dies = Job::new(queue_name.clone(), json!({"slow": false}))
        .with_max_attempts(1)
        .with_cron(hourly.clone())
        .unwrap();
    dies.scheduled_at = Utc::now(); // run now rather than at the next hour
    let mut times_out = Job::new(queue_name.clone(), json!({"slow": true}))
        .with_max_attempts(1)
        .with_timeout(Duration::from_millis(200))
        .with_cron(hourly)
        .unwrap();
    times_out.scheduled_at = Utc::now();
    let dies = queue.enqueue(dies).await.unwrap();
    let times_out = queue.enqueue(times_out).await.unwrap();

    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            if job.payload["slow"].as_bool() == Some(true) {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(HammerworkError::Processing("cron run failed".to_string()))
        })
    });
    let started = Utc::now();
    let running = RunningWorker::start(worker(&queue, &queue_name, handler));
    for id in [dies, times_out] {
        wait_for_job(&queue, id, Duration::from_secs(15), |job| {
            job.status != JobStatus::Running && job.scheduled_at > started
        })
        .await;
    }
    running.stop().await;

    for (id, error) in [(dies, "cron run failed"), (times_out, "timed out")] {
        let job = queue.get_job(id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Pending, "{job:?}");
        assert!(job.recurring);
        assert_eq!(job.attempts, 0, "attempts restart for the next run");
        assert!(job.scheduled_at > Utc::now(), "scheduled for the next run");
        assert_eq!(job.next_run_at, Some(job.scheduled_at));
        assert!(
            job.error_message
                .as_deref()
                .unwrap_or_default()
                .contains(error),
            "the failed run stays visible: {job:?}"
        );
        queue.delete_job(id).await.unwrap();
    }
}

fn step(queue_name: &str, name: &str) -> Job {
    Job::new(queue_name.to_string(), json!({ "step": name }))
}

/// A handler that records the order steps ran in.
fn recording_handler(order: Arc<Mutex<Vec<String>>>) -> JobHandler {
    Arc::new(move |job: Job| {
        let order = Arc::clone(&order);
        Box::pin(async move {
            let name = job.payload["step"].as_str().unwrap_or_default().to_string();
            // Give a dependent that was released too early the chance to overtake.
            tokio::time::sleep(Duration::from_millis(20)).await;
            order.lock().unwrap().push(name);
            Ok(())
        })
    })
}

/// C3: 2- and 3-step workflows (sequential and fan-in) run to completion with real
/// workers, in dependency order, and the workflow is marked completed.
async fn workflows_complete_end_to_end<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("workflow_e2e");

    let two_step = JobGroup::new("two_step")
        .add_job(step(&queue_name, "a1"))
        .then(step(&queue_name, "a2"));
    let three_step = JobGroup::new("three_step")
        .add_job(step(&queue_name, "b1"))
        .then(step(&queue_name, "b2"))
        .then(step(&queue_name, "b3"));
    // Two parents that may complete at the same time on different workers.
    let fan_in = JobGroup::new("fan_in")
        .add_parallel_jobs(vec![step(&queue_name, "c1"), step(&queue_name, "c2")])
        .then(step(&queue_name, "c3"));

    let mut ids = Vec::new();
    let mut workflow_ids = Vec::new();
    for workflow in [two_step, three_step, fan_in] {
        ids.extend(workflow.jobs.iter().map(|job| job.id));
        workflow_ids.push(queue.enqueue_workflow(workflow).await.unwrap());
    }

    let order = Arc::new(Mutex::new(Vec::new()));
    let workers: Vec<_> = (0..3)
        .map(|_| {
            RunningWorker::start(worker(
                &queue,
                &queue_name,
                recording_handler(Arc::clone(&order)),
            ))
        })
        .collect();
    wait_until_finished(&queue, &ids, Duration::from_secs(20)).await;
    for running in workers {
        running.stop().await;
    }

    for id in &ids {
        let job = queue.get_job(*id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Completed, "{job:?}");
    }
    let order = order.lock().unwrap().clone();
    let position = |name: &str| order.iter().position(|n| n == name).unwrap();
    assert!(position("a1") < position("a2"), "{order:?}");
    assert!(position("b1") < position("b2"), "{order:?}");
    assert!(position("b2") < position("b3"), "{order:?}");
    assert!(position("c1") < position("c3"), "{order:?}");
    assert!(position("c2") < position("c3"), "{order:?}");

    for (workflow_id, total) in workflow_ids.into_iter().zip([2, 3, 3]) {
        let workflow = queue
            .get_workflow_status(workflow_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(workflow.status, WorkflowStatus::Completed);
        assert_eq!((workflow.completed_jobs, workflow.failed_jobs), (total, 0));
        assert!(workflow.completed_at.is_some());
        for id in workflow.jobs.iter().map(|job| job.id) {
            queue.delete_job(id).await.unwrap();
        }
    }
}

/// Builds `a`, `d` (independent) -> `b` (depends on a) -> `c` (depends on b).
fn policy_workflow(queue_name: &str, policy: FailurePolicy) -> (JobGroup, [JobId; 4]) {
    let a = step(queue_name, "a");
    let d = step(queue_name, "d");
    let b = step(queue_name, "b").depends_on(&a.id);
    let c = step(queue_name, "c").depends_on(&b.id);
    let ids = [a.id, b.id, c.id, d.id];
    let workflow = JobGroup::new("policy")
        .add_job(a)
        .add_job(d)
        .add_job(b)
        .add_job(c)
        .with_failure_policy(policy);
    (workflow, ids)
}

/// C3: a terminal failure is propagated according to the workflow's FailurePolicy.
async fn workflow_failure_policies<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let status = |id: JobId| {
        let queue = Arc::clone(&queue);
        async move {
            let job = queue.get_job(id).await.unwrap().unwrap();
            (job.status, job.dependency_status)
        }
    };

    // FailFast: every job that has not run yet fails with the workflow.
    let queue_name = test_utils::unique_queue("policy_fail_fast");
    let (workflow, [a, b, c, d]) = policy_workflow(&queue_name, FailurePolicy::FailFast);
    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();
    queue.mark_job_dead(a, "boom").await.unwrap();
    for id in [b, c, d] {
        assert_eq!(status(id).await.0, JobStatus::Failed);
    }
    let wf = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.status, WorkflowStatus::Failed);
    assert_eq!((wf.completed_jobs, wf.failed_jobs), (0, 4));
    assert!(wf.failed_at.is_some());
    for id in [a, b, c, d] {
        queue.delete_job(id).await.unwrap();
    }

    // ContinueOnFailure: only the failed job's dependents fail; the rest runs on.
    let queue_name = test_utils::unique_queue("policy_continue");
    let (workflow, [a, b, c, d]) = policy_workflow(&queue_name, FailurePolicy::ContinueOnFailure);
    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();
    queue.mark_job_dead(a, "boom").await.unwrap();
    assert_eq!(
        status(b).await,
        (JobStatus::Failed, DependencyStatus::Failed)
    );
    assert_eq!(
        status(c).await,
        (JobStatus::Failed, DependencyStatus::Failed)
    );
    assert_eq!(status(d).await.0, JobStatus::Pending);
    let wf = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.status, WorkflowStatus::Running);
    queue.complete_job(d).await.unwrap();
    let wf = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.status, WorkflowStatus::Failed, "finished with failures");
    assert_eq!((wf.completed_jobs, wf.failed_jobs), (1, 3));
    for id in [a, b, c, d] {
        queue.delete_job(id).await.unwrap();
    }

    // Manual: dependents keep waiting until an operator retries the failed job.
    let queue_name = test_utils::unique_queue("policy_manual");
    let (workflow, [a, b, c, d]) = policy_workflow(&queue_name, FailurePolicy::Manual);
    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();
    queue.mark_job_dead(a, "boom").await.unwrap();
    assert_eq!(
        status(b).await,
        (JobStatus::Pending, DependencyStatus::Waiting)
    );
    assert_eq!(
        status(c).await,
        (JobStatus::Pending, DependencyStatus::Waiting)
    );
    let wf = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.status, WorkflowStatus::Running);
    queue.retry_dead_job(a).await.unwrap();
    queue.complete_job(a).await.unwrap();
    assert_eq!(
        status(b).await,
        (JobStatus::Pending, DependencyStatus::Satisfied)
    );
    queue.complete_job(b).await.unwrap();
    assert_eq!(
        status(c).await,
        (JobStatus::Pending, DependencyStatus::Satisfied)
    );
    queue.complete_job(c).await.unwrap();
    queue.complete_job(d).await.unwrap();
    let wf = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wf.status, WorkflowStatus::Completed);
    assert_eq!((wf.completed_jobs, wf.failed_jobs), (4, 0));
    for id in [a, b, c, d] {
        queue.delete_job(id).await.unwrap();
    }
}

/// C3: two parents completing at the same moment both see each other, so their shared
/// child always becomes runnable.
async fn concurrent_parent_completions_release_child<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("fan_in_race");
    for _ in 0..20 {
        let p1 = step(&queue_name, "p1");
        let p2 = step(&queue_name, "p2");
        let child = step(&queue_name, "child").depends_on_jobs(&[p1.id, p2.id]);
        let ids = [p1.id, p2.id, child.id];
        for job in [p1, p2, child] {
            queue.enqueue(job).await.unwrap();
        }
        let (r1, r2) = tokio::join!(queue.complete_job(ids[0]), queue.complete_job(ids[1]));
        r1.unwrap();
        r2.unwrap();
        let child = queue.get_job(ids[2]).await.unwrap().unwrap();
        assert_eq!(child.dependency_status, DependencyStatus::Satisfied);
        for id in ids {
            queue.delete_job(id).await.unwrap();
        }
    }
}

/// H6: a FailFast batch stops running its jobs after the first terminal failure, and
/// the batch row records the outcome.
async fn batch_failure_modes_are_enforced<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    use hammerwork::priority::JobPriority;
    let _serial = test_utils::serial().await;

    let runs = Arc::new(AtomicUsize::new(0));
    let handler: JobHandler = {
        let runs = Arc::clone(&runs);
        Arc::new(move |job: Job| {
            let runs = Arc::clone(&runs);
            Box::pin(async move {
                runs.fetch_add(1, Ordering::SeqCst);
                if job.payload["fail"].as_bool() == Some(true) {
                    return Err(HammerworkError::Processing("batch job failed".into()));
                }
                Ok(())
            })
        })
    };

    // FailFast: the failing job runs first (highest priority); the others never run.
    let queue_name = test_utils::unique_queue("batch_fail_fast");
    let jobs = vec![
        Job::new(queue_name.clone(), json!({"fail": true}))
            .with_priority(JobPriority::Critical)
            .with_max_attempts(1),
        Job::new(queue_name.clone(), json!({"fail": false})),
        Job::new(queue_name.clone(), json!({"fail": false})),
    ];
    let ids: Vec<JobId> = jobs.iter().map(|job| job.id).collect();
    let batch = JobBatch::new("fail_fast")
        .with_jobs(jobs)
        .with_partial_failure_handling(PartialFailureMode::FailFast);
    let batch_id = queue.enqueue_batch(batch).await.unwrap();
    let running = RunningWorker::start(worker(&queue, &queue_name, Arc::clone(&handler)));
    wait_until_finished(&queue, &ids, Duration::from_secs(15)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    running.stop().await;

    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "remaining batch jobs must not run"
    );
    assert_eq!(
        queue.get_job(ids[0]).await.unwrap().unwrap().status,
        JobStatus::Dead
    );
    for id in &ids[1..] {
        let job = queue.get_job(*id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert!(
            job.error_message
                .unwrap_or_default()
                .contains("Batch failed")
        );
    }
    let result = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(result.status, BatchStatus::Failed);
    assert_eq!(
        (
            result.completed_jobs,
            result.failed_jobs,
            result.pending_jobs
        ),
        (0, 3, 0)
    );
    assert!(result.completed_at.is_some(), "the batch row is finalized");
    queue.delete_batch(batch_id).await.unwrap();

    // ContinueOnError: every job runs; the finished batch is persisted as completed.
    let queue_name = test_utils::unique_queue("batch_continue");
    let jobs: Vec<Job> = (0..3)
        .map(|_| Job::new(queue_name.clone(), json!({"fail": false})))
        .collect();
    let ids: Vec<JobId> = jobs.iter().map(|job| job.id).collect();
    let batch_id = queue
        .enqueue_batch(JobBatch::new("continue").with_jobs(jobs))
        .await
        .unwrap();
    let running = RunningWorker::start(worker(&queue, &queue_name, handler));
    wait_until_finished(&queue, &ids, Duration::from_secs(15)).await;
    running.stop().await;
    let result = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(result.status, BatchStatus::Completed);
    assert_eq!(result.completed_jobs, 3);
    assert!(result.completed_at.is_some(), "the batch row is finalized");
    queue.delete_batch(batch_id).await.unwrap();
}

/// M2: a job's retry strategy survives enqueue (single and batch); custom strategies,
/// which cannot be stored, are rejected instead of silently dropped.
async fn retry_strategy_is_persisted<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("retry_strategy");
    let strategy = RetryStrategy::exponential(Duration::from_secs(2), 3.0, None);

    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({})).with_retry_strategy(strategy.clone()))
        .await
        .unwrap();
    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.retry_strategy, Some(strategy.clone()));
    let dequeued = queue.dequeue(&queue_name).await.unwrap().unwrap();
    assert_eq!(dequeued.retry_strategy, Some(strategy.clone()));
    queue.delete_job(id).await.unwrap();

    let job = Job::new(queue_name.clone(), json!({})).with_retry_strategy(strategy.clone());
    let batch_job_id = job.id;
    let batch_id = queue
        .enqueue_batch(JobBatch::new("strategies").with_jobs(vec![job]))
        .await
        .unwrap();
    let job = queue.get_job(batch_job_id).await.unwrap().unwrap();
    assert_eq!(job.retry_strategy, Some(strategy));
    queue.delete_batch(batch_id).await.unwrap();

    let custom = Job::new(queue_name.clone(), json!({}))
        .with_retry_strategy(RetryStrategy::custom(|_| Duration::from_secs(1)));
    let custom_id = custom.id;
    assert!(queue.enqueue(custom).await.is_err());
    assert!(queue.get_job(custom_id).await.unwrap().is_none());
}

/// M3: `enqueue_batch` stores the same fields as `enqueue`.
async fn enqueue_batch_stores_all_fields<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("batch_fields");
    let parent = uuid::Uuid::new_v4();
    let job = Job::new(queue_name.clone(), json!({}))
        .with_result_storage(ResultStorage::Database)
        .with_result_ttl(Duration::from_secs(600))
        .with_trace_id("trace-123")
        .with_correlation_id("corr-456")
        .depends_on(&parent);
    let id = job.id;
    let batch_id = queue
        .enqueue_batch(JobBatch::new("fields").with_jobs(vec![job]))
        .await
        .unwrap();

    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(stored.batch_id, Some(batch_id));
    assert_eq!(stored.result_config.storage, ResultStorage::Database);
    assert_eq!(stored.result_config.ttl, Some(Duration::from_secs(600)));
    assert_eq!(stored.trace_id.as_deref(), Some("trace-123"));
    assert_eq!(stored.correlation_id.as_deref(), Some("corr-456"));
    assert_eq!(stored.depends_on, vec![parent]);
    assert_eq!(stored.dependency_status, DependencyStatus::Waiting);
    // The unresolved dependency keeps it from running.
    assert!(queue.dequeue(&queue_name).await.unwrap().is_none());

    queue.delete_batch(batch_id).await.unwrap();
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_manual_transitions_are_guarded() {
        manual_transitions_are_guarded(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_zombie_worker_cannot_overwrite_reclaimed_job() {
        zombie_worker_cannot_overwrite_reclaimed_job(test_utils::setup_postgres_queue().await)
            .await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_job_max_attempts_is_the_retry_limit() {
        job_max_attempts_is_the_retry_limit(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_timeouts_retry_until_attempts_are_exhausted() {
        timeouts_retry_until_attempts_are_exhausted(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_recurring_jobs_are_rescheduled_after_terminal_failure() {
        recurring_jobs_are_rescheduled_after_terminal_failure(
            test_utils::setup_postgres_queue().await,
        )
        .await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_workflows_complete_end_to_end() {
        workflows_complete_end_to_end(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_workflow_failure_policies() {
        workflow_failure_policies(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_concurrent_parent_completions_release_child() {
        concurrent_parent_completions_release_child(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_batch_failure_modes_are_enforced() {
        batch_failure_modes_are_enforced(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_retry_strategy_is_persisted() {
        retry_strategy_is_persisted(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_enqueue_batch_stores_all_fields() {
        enqueue_batch_stores_all_fields(test_utils::setup_postgres_queue().await).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_manual_transitions_are_guarded() {
        manual_transitions_are_guarded(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_zombie_worker_cannot_overwrite_reclaimed_job() {
        zombie_worker_cannot_overwrite_reclaimed_job(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_job_max_attempts_is_the_retry_limit() {
        job_max_attempts_is_the_retry_limit(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_timeouts_retry_until_attempts_are_exhausted() {
        timeouts_retry_until_attempts_are_exhausted(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_recurring_jobs_are_rescheduled_after_terminal_failure() {
        recurring_jobs_are_rescheduled_after_terminal_failure(
            test_utils::setup_mysql_queue().await,
        )
        .await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_workflows_complete_end_to_end() {
        workflows_complete_end_to_end(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_workflow_failure_policies() {
        workflow_failure_policies(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_concurrent_parent_completions_release_child() {
        concurrent_parent_completions_release_child(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_batch_failure_modes_are_enforced() {
        batch_failure_modes_are_enforced(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_retry_strategy_is_persisted() {
        retry_strategy_is_persisted(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_enqueue_batch_stores_all_fields() {
        enqueue_batch_stores_all_fields(test_utils::setup_mysql_queue().await).await;
    }
}
