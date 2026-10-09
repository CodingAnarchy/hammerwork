//! Queue backend follow-ups (#31, #17): late dependents, pause enforcement in the
//! dequeue queries, weighted selection across every priority, the database clock,
//! workflow cancellation of running jobs, cron catch-up, processing-time statistics
//! and errors for corrupt rows.
//!
//! Each scenario is written once against the `DatabaseQueue` trait and run against
//! both backends; backend-specific checks live in the `postgres_tests` and
//! `mysql_tests` modules.
#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::{DateTime, Duration as ChronoDuration, DurationRound, Utc};
use hammerwork::{
    CronSchedule, Job, JobId, JobOutcome, JobQueue, JobStatus, PriorityWeights,
    batch::JobBatch,
    priority::JobPriority,
    queue::DatabaseQueue,
    workflow::{DependencyStatus, FailurePolicy, JobGroup, WorkflowStatus},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn job(queue_name: &str, name: &str) -> Job {
    Job::new(queue_name.to_string(), json!({ "name": name }))
}

async fn dependency_state<DB>(queue: &JobQueue<DB>, id: JobId) -> (JobStatus, DependencyStatus)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB>,
{
    let job = queue.get_job(id).await.unwrap().expect("job exists");
    (job.status, job.dependency_status)
}

/// #31: a job enqueued after its dependencies finished gets the dependency state it
/// would have had if it had existed when they finished, instead of waiting forever.
async fn late_dependents_are_settled_at_enqueue<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("late_dependents");
    let done = queue.enqueue(job(&queue_name, "done")).await.unwrap();
    queue.complete_job(done).await.unwrap();
    let dead = queue.enqueue(job(&queue_name, "dead")).await.unwrap();
    queue.mark_job_dead(dead, "boom").await.unwrap();
    let pending = queue.enqueue(job(&queue_name, "pending")).await.unwrap();

    // All dependencies completed: runnable right away
    let satisfied = queue
        .enqueue(job(&queue_name, "satisfied").depends_on(&done))
        .await
        .unwrap();
    assert_eq!(
        dependency_state(&queue, satisfied).await,
        (JobStatus::Pending, DependencyStatus::Satisfied)
    );

    // A dependency that has not finished keeps it waiting
    let waiting = queue
        .enqueue(job(&queue_name, "waiting").depends_on_jobs(&[done, pending]))
        .await
        .unwrap();
    assert_eq!(
        dependency_state(&queue, waiting).await,
        (JobStatus::Pending, DependencyStatus::Waiting)
    );

    // A failed dependency fails it, like dependents that existed when it failed
    let failed = queue
        .enqueue(job(&queue_name, "failed").depends_on_jobs(&[done, dead]))
        .await
        .unwrap();
    assert_eq!(
        dependency_state(&queue, failed).await,
        (JobStatus::Failed, DependencyStatus::Failed)
    );
    let failed_job = queue.get_job(failed).await.unwrap().unwrap();
    assert!(failed_job.failed_at.is_some());
    assert!(
        failed_job
            .error_message
            .unwrap_or_default()
            .contains(&dead.to_string())
    );

    // Batches settle their jobs the same way, including chains inside the batch
    let in_batch = job(&queue_name, "batch_satisfied").depends_on(&done);
    let doomed = job(&queue_name, "batch_doomed").depends_on(&dead);
    let downstream = job(&queue_name, "batch_downstream").depends_on(&doomed.id);
    let batch_ids = [in_batch.id, doomed.id, downstream.id];
    let batch_id = queue
        .enqueue_batch(JobBatch::new("late").with_jobs(vec![in_batch, doomed, downstream]))
        .await
        .unwrap();
    assert_eq!(
        dependency_state(&queue, batch_ids[0]).await,
        (JobStatus::Pending, DependencyStatus::Satisfied)
    );
    for id in &batch_ids[1..] {
        assert_eq!(
            dependency_state(&queue, *id).await,
            (JobStatus::Failed, DependencyStatus::Failed)
        );
    }

    // The satisfied jobs are dequeued; nothing else is
    let mut dequeued = Vec::new();
    while let Some(job) = queue.dequeue(&queue_name).await.unwrap() {
        dequeued.push(job.id);
    }
    dequeued.sort_unstable();
    let mut expected = vec![pending, satisfied, batch_ids[0]];
    expected.sort_unstable();
    assert_eq!(dequeued, expected);

    queue.delete_batch(batch_id).await.unwrap();
    for id in [done, dead, pending, satisfied, waiting, failed] {
        queue.delete_job(id).await.unwrap();
    }
}

/// #31: under the `Manual` failure policy the dependents of a failed job wait for an
/// operator; a late dependent does too.
async fn late_dependents_respect_manual_policy<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("late_manual");
    let parent = job(&queue_name, "parent");
    let parent_id = parent.id;
    let workflow = JobGroup::new("manual")
        .add_job(parent)
        .with_failure_policy(FailurePolicy::Manual);
    queue.enqueue_workflow(workflow).await.unwrap();
    queue.mark_job_dead(parent_id, "boom").await.unwrap();

    let child = queue
        .enqueue(job(&queue_name, "child").depends_on(&parent_id))
        .await
        .unwrap();
    assert_eq!(
        dependency_state(&queue, child).await,
        (JobStatus::Pending, DependencyStatus::Waiting)
    );
    for id in [parent_id, child] {
        queue.delete_job(id).await.unwrap();
    }
}

/// #31: a dependent enqueued while its dependency completes is never left waiting,
/// whichever transaction commits first.
async fn late_dependents_race_with_completion<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("late_race");
    for _ in 0..25 {
        let parent = queue.enqueue(job(&queue_name, "parent")).await.unwrap();
        let run = queue.dequeue(&queue_name).await.unwrap().expect("parent");
        let child = job(&queue_name, "child").depends_on(&parent);
        let (finished, enqueued) = tokio::join!(
            queue.finish_job_run(&run, JobOutcome::Completed),
            queue.enqueue(child)
        );
        assert!(finished.unwrap().is_some());
        let child = enqueued.unwrap();
        assert_eq!(
            dependency_state(&queue, child).await,
            (JobStatus::Pending, DependencyStatus::Satisfied)
        );
        for id in [parent, child] {
            queue.delete_job(id).await.unwrap();
        }
    }
}

/// #31: a paused queue hands out no jobs, even to callers that skip the worker's
/// pause check.
async fn paused_queue_is_not_dequeued<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("paused");
    let id = queue.enqueue(job(&queue_name, "paused")).await.unwrap();

    queue.pause_queue(&queue_name, Some("test")).await.unwrap();
    assert!(queue.dequeue(&queue_name).await.unwrap().is_none());
    assert!(
        queue
            .dequeue_with_priority_weights(&queue_name, &PriorityWeights::new())
            .await
            .unwrap()
            .is_none()
    );
    // Pausing another queue does not affect this one
    queue.resume_queue(&queue_name, None).await.unwrap();
    let other = test_utils::unique_queue("paused_other");
    queue.pause_queue(&other, None).await.unwrap();
    assert_eq!(queue.dequeue(&queue_name).await.unwrap().unwrap().id, id);

    queue.resume_queue(&other, None).await.unwrap();
    queue.delete_job(id).await.unwrap();
}

/// #31 M4: weighted selection considers every priority level, not only the jobs that
/// happen to be among the highest-priority candidates.
async fn weighted_dequeue_reaches_every_priority<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("weighted_starvation");
    let mut ids = Vec::new();
    for i in 0..30 {
        let critical =
            job(&queue_name, &format!("critical {i}")).with_priority(JobPriority::Critical);
        ids.push(queue.enqueue(critical).await.unwrap());
    }
    let background = queue
        .enqueue(job(&queue_name, "background").with_priority(JobPriority::Background))
        .await
        .unwrap();
    ids.push(background);

    // Background is 1000 times more likely than Critical: it must come up right away,
    // although 30 Critical jobs are ahead of it.
    let weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 1)
        .with_weight(JobPriority::Background, 1000);
    let mut picked = Vec::new();
    for _ in 0..3 {
        let job = queue
            .dequeue_with_priority_weights(&queue_name, &weights)
            .await
            .unwrap()
            .expect("a job");
        picked.push(job.priority);
    }
    assert!(
        picked.contains(&JobPriority::Background),
        "background job starved: {picked:?}"
    );

    // Every job is still handed out exactly once
    let mut remaining = 0;
    while queue
        .dequeue_with_priority_weights(&queue_name, &weights)
        .await
        .unwrap()
        .is_some()
    {
        remaining += 1;
    }
    assert_eq!(remaining + picked.len(), ids.len());
    for id in ids {
        queue.delete_job(id).await.unwrap();
    }
}

/// #31 M6: due times are compared with the database clock. `queue` runs on a
/// connection pool whose database clock is one hour ahead of the application's.
async fn due_times_use_the_database_clock<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("db_clock");
    // Due in 30 minutes by the application clock, so already due by the database's
    let mut delayed = job(&queue_name, "delayed");
    delayed.scheduled_at = Utc::now() + ChronoDuration::minutes(30);
    let id = queue.enqueue(delayed).await.unwrap();

    let run = queue
        .dequeue(&queue_name)
        .await
        .unwrap()
        .expect("due by the database clock");
    assert_eq!(run.id, id);
    let started_at = run.started_at.expect("started_at");
    assert!(
        started_at > Utc::now() + ChronoDuration::minutes(50),
        "started_at comes from the database clock: {started_at}"
    );

    // A retry keeps its backoff on the database clock: due in 30s from the database's
    // point of view, i.e. not yet.
    queue
        .finish_job_run(
            &run,
            JobOutcome::Retry {
                retry_at: Utc::now() + ChronoDuration::seconds(30),
                error: "again".to_string(),
                timed_out: false,
            },
        )
        .await
        .unwrap()
        .expect("recorded");
    assert!(queue.dequeue(&queue_name).await.unwrap().is_none());
    let job = queue.get_job(id).await.unwrap().unwrap();
    assert!(job.scheduled_at > Utc::now() + ChronoDuration::minutes(50));

    // Cron jobs are due by the database clock too
    let mut cron = Job::new(queue_name.clone(), json!({}))
        .with_cron(CronSchedule::new("0 0 * * * *").unwrap())
        .unwrap();
    cron.next_run_at = Some(Utc::now() + ChronoDuration::minutes(30));
    cron.scheduled_at = cron.next_run_at.unwrap();
    let cron_id = queue.enqueue(cron).await.unwrap();
    let due = queue.get_due_cron_jobs(Some(&queue_name)).await.unwrap();
    assert_eq!(due.iter().map(|job| job.id).collect::<Vec<_>>(), [cron_id]);

    for id in [id, cron_id] {
        queue.delete_job(id).await.unwrap();
    }
}

/// #31: cancelling a workflow also cancels its running jobs; their workers' outcomes
/// are discarded.
async fn cancel_workflow_discards_running_jobs<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cancel_running");
    let a = job(&queue_name, "a");
    let b = job(&queue_name, "b").depends_on(&a.id);
    let (a_id, b_id) = (a.id, b.id);
    let workflow_id = queue
        .enqueue_workflow(JobGroup::new("cancel").add_job(a).add_job(b))
        .await
        .unwrap();
    let run = queue.dequeue(&queue_name).await.unwrap().expect("a runs");
    assert_eq!(run.id, a_id);

    queue.cancel_workflow(workflow_id).await.unwrap();

    for id in [a_id, b_id] {
        let job = queue.get_job(id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Failed, "{job:?}");
        assert_eq!(job.error_message.as_deref(), Some("Workflow cancelled"));
    }
    // The worker running `a` loses its lease and its outcome is not recorded
    assert!(
        !queue
            .heartbeat_job(&run, Duration::from_secs(30))
            .await
            .unwrap()
    );
    assert_eq!(
        queue
            .finish_job_run(&run, JobOutcome::Completed)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        queue.get_job(a_id).await.unwrap().unwrap().status,
        JobStatus::Failed
    );
    assert_eq!(
        dependency_state(&queue, b_id).await,
        (JobStatus::Failed, DependencyStatus::Failed)
    );
    let workflow = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(workflow.status, WorkflowStatus::Cancelled);

    for id in [a_id, b_id] {
        queue.delete_job(id).await.unwrap();
    }
}

/// #31: the next run of a recurring job is computed from the run's slot. Missed slots
/// are coalesced into one immediate catch-up run, then the schedule continues.
async fn cron_catches_up_missed_slots_once<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("cron_catch_up");
    let today: DateTime<Utc> = Utc::now().duration_trunc(ChronoDuration::days(1)).unwrap();
    let tomorrow = today + ChronoDuration::days(1);

    // A daily job whose run was due three days ago (e.g. workers were down)
    let mut daily = Job::new(queue_name.clone(), json!({}))
        .with_cron(CronSchedule::new("0 0 0 * * *").unwrap())
        .unwrap();
    daily.next_run_at = Some(today - ChronoDuration::days(3));
    daily.scheduled_at = today - ChronoDuration::days(3);
    let id = queue.enqueue(daily).await.unwrap();

    // The late run is followed by one catch-up run for the missed slots, due now...
    let run = queue.dequeue(&queue_name).await.unwrap().expect("due");
    let recorded = queue
        .finish_job_run(&run, JobOutcome::Completed)
        .await
        .unwrap()
        .expect("recorded");
    assert_eq!(recorded.status, JobStatus::Pending);
    assert_eq!(recorded.next_run_at, Some(today));
    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!((job.scheduled_at, job.next_run_at), (today, Some(today)));

    // ...and after it, the job is back on schedule.
    let run = queue
        .dequeue(&queue_name)
        .await
        .unwrap()
        .expect("catch-up run");
    let recorded = queue
        .finish_job_run(&run, JobOutcome::Completed)
        .await
        .unwrap()
        .expect("recorded");
    assert_eq!(recorded.next_run_at, Some(tomorrow));
    assert!(queue.dequeue(&queue_name).await.unwrap().is_none());

    queue.delete_job(id).await.unwrap();
}

/// #17 H9: processing-time statistics are computed (and DB errors are reported, not
/// replaced with empty statistics); completed jobs are found by time range.
async fn processing_time_statistics<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("processing_stats");
    let id = queue
        .enqueue(job(&queue_name, "timed").with_priority(JobPriority::High))
        .await
        .unwrap();
    let run = queue.dequeue(&queue_name).await.unwrap().expect("job");
    tokio::time::sleep(Duration::from_millis(30)).await;
    queue
        .finish_job_run(&run, JobOutcome::Completed)
        .await
        .unwrap()
        .expect("recorded");

    let stats = queue.get_priority_stats(&queue_name).await.unwrap();
    let average = stats
        .avg_processing_times
        .get(&JobPriority::High)
        .copied()
        .expect("average processing time for High");
    assert!(average >= 0.0, "{average}");

    let since = Utc::now() - ChronoDuration::hours(1);
    let times = queue
        .get_processing_times(&queue_name, since)
        .await
        .unwrap();
    assert_eq!(times.len(), 1);
    assert!(times[0] >= 0, "{times:?}");

    let completed = queue
        .get_jobs_completed_in_range(
            Some(&queue_name),
            since,
            Utc::now() + ChronoDuration::hours(1),
            None,
        )
        .await
        .unwrap();
    assert_eq!(completed.iter().map(|job| job.id).collect::<Vec<_>>(), [id]);

    queue.delete_job(id).await.unwrap();
}

/// Resolving many dependents of one job at once (set-based resolution).
async fn fan_out_dependents_are_released_together<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("fan_out");
    let parent = queue.enqueue(job(&queue_name, "parent")).await.unwrap();
    let other = queue.enqueue(job(&queue_name, "other")).await.unwrap();
    let mut ready = Vec::new();
    for i in 0..40 {
        ready.push(
            queue
                .enqueue(job(&queue_name, &format!("child {i}")).depends_on(&parent))
                .await
                .unwrap(),
        );
    }
    // Also waits on `other`, which has not completed
    let blocked = queue
        .enqueue(job(&queue_name, "blocked").depends_on_jobs(&[parent, other]))
        .await
        .unwrap();

    let run = queue.dequeue(&queue_name).await.unwrap().expect("parent");
    let recorded = queue
        .finish_job_run(&run, JobOutcome::Completed)
        .await
        .unwrap()
        .expect("recorded");
    let mut unblocked = recorded.unblocked.clone();
    unblocked.sort_unstable();
    ready.sort_unstable();
    assert_eq!(unblocked, ready);
    assert_eq!(
        dependency_state(&queue, blocked).await,
        (JobStatus::Pending, DependencyStatus::Waiting)
    );
    assert_eq!(queue.get_queue_depth(&queue_name).await.unwrap(), 42);

    for id in ready.into_iter().chain([parent, other, blocked]) {
        queue.delete_job(id).await.unwrap();
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;

    /// A queue whose database clock (`now()`) runs one hour ahead: a schema-local
    /// `now()` shadows `pg_catalog.now()` through the search path.
    async fn queue_with_clock_ahead() -> Arc<JobQueue<sqlx::Postgres>> {
        let base = test_utils::setup_postgres_queue().await;
        let pool = base.get_pool();
        sqlx::query("CREATE SCHEMA IF NOT EXISTS hw_test_clock_ahead")
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE OR REPLACE FUNCTION hw_test_clock_ahead.now() RETURNS timestamptz \
             LANGUAGE sql STABLE AS 'SELECT pg_catalog.now() + interval ''1 hour'''",
        )
        .execute(pool)
        .await
        .unwrap();
        let pool = PgPoolOptions::new()
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path = hw_test_clock_ahead, pg_catalog, public")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&test_utils::postgres_url())
            .await
            .unwrap();
        Arc::new(JobQueue::new(pool))
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_late_dependents_are_settled_at_enqueue() {
        late_dependents_are_settled_at_enqueue(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_late_dependents_respect_manual_policy() {
        late_dependents_respect_manual_policy(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_late_dependents_race_with_completion() {
        late_dependents_race_with_completion(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_paused_queue_is_not_dequeued() {
        paused_queue_is_not_dequeued(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_weighted_dequeue_reaches_every_priority() {
        weighted_dequeue_reaches_every_priority(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_due_times_use_the_database_clock() {
        due_times_use_the_database_clock(queue_with_clock_ahead().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_cancel_workflow_discards_running_jobs() {
        cancel_workflow_discards_running_jobs(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_cron_catches_up_missed_slots_once() {
        cron_catches_up_missed_slots_once(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_processing_time_statistics() {
        processing_time_statistics(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_fan_out_dependents_are_released_together() {
        fan_out_dependents_are_released_together(test_utils::setup_postgres_queue().await).await;
    }

    /// #17 M13: corrupt column values are reported instead of being replaced with
    /// defaults or wrapped into huge numbers.
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_corrupt_rows_are_errors() {
        let queue = test_utils::setup_postgres_queue().await;
        let queue_name = test_utils::unique_queue("corrupt");
        for (column, value) in [
            ("timeout_seconds", "-1"),
            ("retry_strategy", "'{\"NoSuchStrategy\": {}}'::jsonb"),
        ] {
            let id = queue.enqueue(job(&queue_name, column)).await.unwrap();
            sqlx::query(&format!(
                "UPDATE hammerwork_jobs SET {column} = {value} WHERE id = $1"
            ))
            .bind(id)
            .execute(queue.get_pool())
            .await
            .unwrap();
            assert!(
                queue.get_job(id).await.is_err(),
                "corrupt {column} must be an error"
            );
            queue.delete_job(id).await.unwrap();
        }
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;
    use hammerwork::HammerworkError;
    use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions};
    use std::str::FromStr;

    /// A queue whose database clock runs one hour ahead (`SET timestamp` moves the
    /// session's `NOW()` and `UTC_TIMESTAMP()`).
    async fn mysql_queue_with_clock_ahead() -> Arc<JobQueue<sqlx::MySql>> {
        test_utils::setup_mysql_queue().await;
        let pool = MySqlPoolOptions::new()
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET timestamp = UNIX_TIMESTAMP(NOW(6)) + 3600")
                        .execute(conn)
                        .await?;
                    Ok(())
                })
            })
            .connect(&test_utils::mysql_url())
            .await
            .unwrap();
        Arc::new(JobQueue::new(pool))
    }

    /// A queue whose sessions use a non-UTC `time_zone`.
    async fn mysql_queue_in_time_zone(time_zone: &str) -> Arc<JobQueue<sqlx::MySql>> {
        test_utils::setup_mysql_queue().await;
        let options = MySqlConnectOptions::from_str(&test_utils::mysql_url())
            .unwrap()
            .timezone(Some(time_zone.to_string()));
        let pool = MySqlPoolOptions::new().connect_with(options).await.unwrap();
        Arc::new(JobQueue::new(pool))
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_late_dependents_are_settled_at_enqueue() {
        late_dependents_are_settled_at_enqueue(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_late_dependents_respect_manual_policy() {
        late_dependents_respect_manual_policy(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_late_dependents_race_with_completion() {
        late_dependents_race_with_completion(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_paused_queue_is_not_dequeued() {
        paused_queue_is_not_dequeued(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_weighted_dequeue_reaches_every_priority() {
        weighted_dequeue_reaches_every_priority(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_due_times_use_the_database_clock() {
        due_times_use_the_database_clock(mysql_queue_with_clock_ahead().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_cancel_workflow_discards_running_jobs() {
        cancel_workflow_discards_running_jobs(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_cron_catches_up_missed_slots_once() {
        cron_catches_up_missed_slots_once(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_processing_time_statistics() {
        processing_time_statistics(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_fan_out_dependents_are_released_together() {
        fan_out_dependents_are_released_together(test_utils::setup_mysql_queue().await).await;
    }

    /// #17 M13: corrupt column values are reported instead of being replaced with
    /// defaults or wrapped into huge numbers.
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_corrupt_rows_are_errors() {
        let queue = test_utils::setup_mysql_queue().await;
        let queue_name = test_utils::unique_queue("corrupt");
        for (column, value) in [
            ("timeout_seconds", "-1"),
            ("depends_on", "JSON_OBJECT('not', 'a list')"),
            ("dependents", "JSON_ARRAY(1, 2)"),
            ("pii_fields", "JSON_OBJECT('a', 1)"),
            (
                "retry_strategy",
                "JSON_OBJECT('NoSuchStrategy', JSON_OBJECT())",
            ),
        ] {
            let id = queue.enqueue(job(&queue_name, column)).await.unwrap();
            sqlx::query(&format!(
                "UPDATE hammerwork_jobs SET {column} = {value} WHERE id = ?"
            ))
            .bind(id.to_string())
            .execute(queue.get_pool())
            .await
            .unwrap();
            assert!(
                queue.get_job(id).await.is_err(),
                "corrupt {column} must be an error"
            );
            queue.delete_job(id).await.unwrap();
        }
    }

    /// #31 M7: MySQL `TIMESTAMP` columns end at 2038-01-19 03:14:07 UTC. Times beyond
    /// it are rejected with a clear error instead of a database error.
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_times_after_2038_are_rejected() {
        let queue = test_utils::setup_mysql_queue().await;
        let queue_name = test_utils::unique_queue("y2038");
        let after_2038: DateTime<Utc> = "2040-01-01T00:00:00Z".parse().unwrap();

        let mut late = job(&queue_name, "late");
        late.scheduled_at = after_2038;
        match queue.enqueue(late).await {
            Err(HammerworkError::InvalidJobPayload { message }) => {
                assert!(message.contains("2038"), "{message}")
            }
            other => panic!("expected InvalidJobPayload, got {other:?}"),
        }

        let id = queue.enqueue(job(&queue_name, "on time")).await.unwrap();
        let run = queue.dequeue(&queue_name).await.unwrap().expect("job");
        assert!(matches!(
            queue.retry_job(id, after_2038).await,
            Err(HammerworkError::InvalidJobPayload { .. })
        ));
        assert!(matches!(
            queue
                .store_job_result(id, json!({}), Some(after_2038))
                .await,
            Err(HammerworkError::InvalidJobPayload { .. })
        ));
        // A lease longer than the column can hold is capped, not an error
        assert!(queue.heartbeat_job(&run, Duration::MAX).await.unwrap());
        // A worker's retry far in the future is capped at the latest storable time
        queue
            .finish_job_run(
                &run,
                JobOutcome::Retry {
                    retry_at: after_2038,
                    error: "later".to_string(),
                    timed_out: false,
                },
            )
            .await
            .unwrap()
            .expect("recorded");
        let job = queue.get_job(id).await.unwrap().unwrap();
        assert!(job.scheduled_at < after_2038);
        assert!(job.scheduled_at > "2038-01-19T03:14:06Z".parse::<DateTime<Utc>>().unwrap());

        queue.delete_job(id).await.unwrap();
    }

    /// Times are UTC whatever the session `time_zone`: written and compared with
    /// `UTC_TIMESTAMP()`, never the session-local `NOW()`.
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_non_utc_session_time_zone() {
        let queue = mysql_queue_in_time_zone("+05:00").await;
        let queue_name = test_utils::unique_queue("session_tz");

        let mut delayed = job(&queue_name, "delayed");
        delayed.scheduled_at = Utc::now() + ChronoDuration::hours(1);
        let delayed = queue.enqueue(delayed).await.unwrap();
        let now = queue.enqueue(job(&queue_name, "now")).await.unwrap();

        let run = queue.dequeue(&queue_name).await.unwrap().expect("due job");
        assert_eq!(run.id, now);
        let started_at = run.started_at.unwrap();
        assert!(
            (started_at - Utc::now()).num_seconds().abs() < 60,
            "{started_at}"
        );
        assert!(queue.dequeue(&queue_name).await.unwrap().is_none());

        queue.pause_queue(&queue_name, None).await.unwrap();
        let paused_at = queue
            .get_queue_pause_info(&queue_name)
            .await
            .unwrap()
            .unwrap()
            .paused_at;
        assert!(
            (paused_at - Utc::now()).num_seconds().abs() < 60,
            "{paused_at}"
        );
        queue.resume_queue(&queue_name, None).await.unwrap();

        for id in [delayed, now] {
            queue.delete_job(id).await.unwrap();
        }
    }
}
