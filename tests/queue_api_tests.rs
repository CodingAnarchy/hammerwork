//! `DatabaseQueue` operations not covered elsewhere, on both backends and on
//! `TestQueue` (so the in-memory queue is held to the same behaviour): dead job
//! management, statistics queries, recurring jobs, throttle configuration, pause
//! information and workflow dependency helpers.

#![cfg(any(feature = "postgres", feature = "mysql", feature = "test"))]

mod test_utils;

use chrono::{Duration, Utc};
use hammerwork::{
    Job, JobId, JobStatus, cron::CronSchedule, queue::DatabaseQueue, rate_limit::ThrottleConfig,
    workflow::JobGroup,
};
use serde_json::json;
use std::sync::Arc;

async fn enqueue<Q>(queue: &Arc<Q>, queue_name: &str) -> JobId
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    queue
        .enqueue(Job::new(queue_name.to_string(), json!({})))
        .await
        .unwrap()
}

/// Listing, summarising, retrying and purging dead jobs.
async fn dead_job_management<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    // Purging and the summary cover every queue.
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("dead_jobs");
    let error = format!("boom-{queue_name}");
    let first = enqueue(&queue, &queue_name).await;
    let second = enqueue(&queue, &queue_name).await;
    let alive = enqueue(&queue, &queue_name).await;
    queue.mark_job_dead(first, &error).await.unwrap();
    queue.mark_job_dead(second, &error).await.unwrap();

    let dead = queue
        .get_dead_jobs_by_queue(&queue_name, None, None)
        .await
        .unwrap();
    let mut ids: Vec<JobId> = dead.iter().map(|job| job.id).collect();
    ids.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(ids, expected);
    assert!(dead.iter().all(|job| job.status == JobStatus::Dead));
    assert!(
        dead.iter()
            .all(|job| job.error_message.as_deref() == Some(error.as_str()))
    );
    let page = queue
        .get_dead_jobs_by_queue(&queue_name, Some(1), Some(1))
        .await
        .unwrap();
    assert_eq!(page.len(), 1);
    assert!(expected.contains(&page[0].id));

    let all_dead = queue.get_dead_jobs(Some(100_000), None).await.unwrap();
    assert!(all_dead.iter().any(|job| job.id == first));
    assert!(!all_dead.iter().any(|job| job.id == alive));
    assert_eq!(queue.get_dead_jobs(Some(1), None).await.unwrap().len(), 1);

    let summary = queue.get_dead_job_summary().await.unwrap();
    assert_eq!(summary.dead_jobs_by_queue.get(&queue_name), Some(&2));
    assert_eq!(summary.error_patterns.get(&error), Some(&2));
    assert!(summary.total_dead_jobs >= 2);
    assert!(summary.oldest_dead_job.is_some() && summary.newest_dead_job.is_some());

    // Retrying puts a dead job back in the queue.
    queue.retry_dead_job(first).await.unwrap();
    let retried = queue.get_job(first).await.unwrap().unwrap();
    assert_eq!(retried.status, JobStatus::Pending);
    assert!(queue.retry_dead_job(alive).await.is_err(), "not dead");

    // Purging only removes dead jobs that failed before the cutoff.
    queue
        .purge_dead_jobs(Utc::now() - Duration::hours(1))
        .await
        .unwrap();
    assert!(queue.get_job(second).await.unwrap().is_some());
    let purged = queue
        .purge_dead_jobs(Utc::now() + Duration::minutes(1))
        .await
        .unwrap();
    assert!(purged >= 1);
    assert!(queue.get_job(second).await.unwrap().is_none());
    assert!(queue.get_job(alive).await.unwrap().is_some(), "not dead");

    queue.delete_job(first).await.unwrap();
    queue.delete_job(alive).await.unwrap();
}

/// Status counts, error frequencies, processing times and completion ranges.
async fn statistics_queries<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("stats_queries");
    let since = Utc::now() - Duration::minutes(1);
    let pending = enqueue(&queue, &queue_name).await;
    let failed = enqueue(&queue, &queue_name).await;
    let done = enqueue(&queue, &queue_name).await;
    let error = format!("bad-{queue_name}");
    queue.fail_job(failed, &error).await.unwrap();
    // Complete one job through the normal path so it has timings.
    let job = loop {
        let job = queue.dequeue(&queue_name).await.unwrap().expect("a job");
        if job.id == done {
            break job;
        }
        // Put others back.
        queue.retry_job(job.id, Utc::now()).await.unwrap();
    };
    queue.complete_job(job.id).await.unwrap();

    let counts = queue.get_job_counts_by_status(&queue_name).await.unwrap();
    assert_eq!(counts.get("Completed"), Some(&1));
    assert_eq!(counts.get("Failed"), Some(&1));
    assert_eq!(counts.get("Pending"), Some(&1));

    let frequencies = queue
        .get_error_frequencies(Some(&queue_name), since)
        .await
        .unwrap();
    assert_eq!(frequencies.get(&error), Some(&1));
    assert_eq!(frequencies.len(), 1);
    let everywhere = queue.get_error_frequencies(None, since).await.unwrap();
    assert_eq!(everywhere.get(&error), Some(&1));
    let later = queue
        .get_error_frequencies(Some(&queue_name), Utc::now() + Duration::minutes(1))
        .await
        .unwrap();
    assert!(later.is_empty());

    let times = queue
        .get_processing_times(&queue_name, since)
        .await
        .unwrap();
    assert_eq!(times.len(), 1);
    assert!(times[0] >= 0);

    let completed = queue
        .get_jobs_completed_in_range(
            Some(&queue_name),
            since,
            Utc::now() + Duration::minutes(1),
            Some(10),
        )
        .await
        .unwrap();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].id, done);
    let none = queue
        .get_jobs_completed_in_range(Some(&queue_name), since, since, None)
        .await
        .unwrap();
    assert!(none.is_empty());

    let stats = queue.get_queue_stats(&queue_name).await.unwrap();
    assert_eq!(stats.queue_name, queue_name);
    assert_eq!(stats.pending_count, 1);
    assert_eq!(stats.completed_count, 1);
    let all = queue.get_all_queue_stats().await.unwrap();
    assert!(all.iter().any(|s| s.queue_name == queue_name));

    for id in [pending, failed, done] {
        queue.delete_job(id).await.unwrap();
    }
}

/// Recurring jobs can be listed, disabled and enabled again.
async fn recurring_jobs<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("recurring");
    let schedule = CronSchedule::new("0 0 * * * *").unwrap();
    let job = Job::with_cron_schedule(queue_name.clone(), json!({}), schedule).unwrap();
    let id = queue.enqueue_cron_job(job).await.unwrap();
    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert!(stored.recurring);
    assert_eq!(stored.cron_schedule.as_deref(), Some("0 0 * * * *"));
    assert!(stored.next_run_at.is_some());

    let recurring = queue.get_recurring_jobs(&queue_name).await.unwrap();
    assert_eq!(recurring.iter().map(|j| j.id).collect::<Vec<_>>(), [id]);

    queue.disable_recurring_job(id).await.unwrap();
    assert!(!queue.get_job(id).await.unwrap().unwrap().recurring);
    assert!(
        queue
            .get_recurring_jobs(&queue_name)
            .await
            .unwrap()
            .is_empty()
    );

    queue.enable_recurring_job(id).await.unwrap();
    assert!(queue.get_job(id).await.unwrap().unwrap().recurring);
    assert_eq!(
        queue.get_recurring_jobs(&queue_name).await.unwrap().len(),
        1
    );

    // A job without a (valid) schedule is not a cron job, as with `TestQueue`.
    let plain = Job::new(queue_name.clone(), json!({}));
    let err = queue.enqueue_cron_job(plain).await.unwrap_err();
    assert!(
        err.to_string().contains("must have a cron schedule"),
        "{err}"
    );
    let mut invalid = Job::new(queue_name.clone(), json!({}));
    invalid.cron_schedule = Some("every tuesday".to_string());
    let err = queue.enqueue_cron_job(invalid).await.unwrap_err();
    assert!(err.to_string().contains("Invalid cron schedule"), "{err}");

    // A schedule without a computed next run is scheduled for its next execution.
    let mut bare = Job::new(queue_name.clone(), json!({}));
    bare.cron_schedule = Some("0 0 * * * *".to_string());
    let bare_id = queue.enqueue_cron_job(bare).await.unwrap();
    let bare = queue.get_job(bare_id).await.unwrap().unwrap();
    assert!(bare.recurring);
    let next = bare.next_run_at.expect("next run computed");
    assert!(next > Utc::now() && next <= Utc::now() + Duration::hours(1));
    // TestQueue re-bases schedules on its mock clock, so allow for clock skew.
    assert!(
        (bare.scheduled_at - next).num_seconds().abs() <= 1,
        "{bare:?}"
    );
    queue.delete_job(bare_id).await.unwrap();

    queue.delete_job(id).await.unwrap();
}

/// #64 M1: disabling a recurring job stops its pending occurrence, and enabling it
/// resumes the schedule, also for a job whose last run finished while disabled.
async fn disable_and_enable_recurring_jobs<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("recurring_toggle");
    let hourly = CronSchedule::new("0 0 * * * *").unwrap();
    let due_cron_job = || {
        let mut job = Job::new(queue_name.clone(), json!({}))
            .with_cron(hourly.clone())
            .unwrap();
        // Due now (an hour ago, so it is due on TestQueue's mock clock too).
        job.scheduled_at = Utc::now() - Duration::hours(1);
        job.next_run_at = Some(job.scheduled_at);
        job
    };

    // A pending occurrence is held while the job is disabled.
    let held = queue.enqueue(due_cron_job()).await.unwrap();
    queue.disable_recurring_job(held).await.unwrap();
    queue.disable_recurring_job(held).await.unwrap(); // idempotent
    let job = queue.get_job(held).await.unwrap().unwrap();
    assert!(!job.recurring);
    assert_eq!(job.next_run_at, None);
    assert_eq!(job.status, JobStatus::Pending);
    assert!(
        queue.dequeue(&queue_name).await.unwrap().is_none(),
        "a disabled recurring job ran"
    );

    // Enabling resumes it at the next occurrence (missed ones are not run).
    queue.enable_recurring_job(held).await.unwrap();
    let job = queue.get_job(held).await.unwrap().unwrap();
    assert!(job.recurring);
    assert_eq!(job.status, JobStatus::Pending);
    assert!(
        job.scheduled_at > Utc::now() - Duration::minutes(5),
        "{job:?}"
    );
    assert_eq!(job.next_run_at, Some(job.scheduled_at));
    assert!(queue.dequeue(&queue_name).await.unwrap().is_none());
    queue.delete_job(held).await.unwrap();

    // A run in progress finishes; without `recurring` it is not rescheduled.
    let finished = queue.enqueue(due_cron_job()).await.unwrap();
    let run = queue.dequeue(&queue_name).await.unwrap().expect("due job");
    assert_eq!(run.id, finished);
    queue.disable_recurring_job(finished).await.unwrap();
    queue
        .finish_job_run(&run, hammerwork::queue::JobOutcome::Completed)
        .await
        .unwrap()
        .expect("current run");
    assert_eq!(
        queue.get_job(finished).await.unwrap().unwrap().status,
        JobStatus::Completed
    );

    // Enabling revives the finished job.
    queue.enable_recurring_job(finished).await.unwrap();
    let job = queue.get_job(finished).await.unwrap().unwrap();
    assert!(job.recurring);
    assert_eq!(job.status, JobStatus::Pending, "{job:?}");
    assert_eq!(job.attempts, 0);
    assert!(job.completed_at.is_none());
    assert_eq!(job.next_run_at, Some(job.scheduled_at));
    queue.delete_job(finished).await.unwrap();

    // Errors: a missing job, and a job without a cron schedule.
    let missing = uuid::Uuid::new_v4();
    assert!(matches!(
        queue.disable_recurring_job(missing).await,
        Err(hammerwork::HammerworkError::JobNotFound { .. })
    ));
    assert!(matches!(
        queue.enable_recurring_job(missing).await,
        Err(hammerwork::HammerworkError::JobNotFound { .. })
    ));
    let plain = enqueue(&queue, &queue_name).await;
    assert!(queue.disable_recurring_job(plain).await.is_err());
    assert!(queue.enable_recurring_job(plain).await.is_err());
    // A plain job (no cron schedule) is not held.
    assert_eq!(
        queue.dequeue(&queue_name).await.unwrap().map(|job| job.id),
        Some(plain)
    );
    queue.delete_job(plain).await.unwrap();
}

/// The `DatabaseQueue` throttle methods use the queue's throttle registry.
async fn throttle_configuration<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("throttle");
    assert!(
        DatabaseQueue::get_throttle_config(queue.as_ref(), &queue_name)
            .await
            .unwrap()
            .is_none()
    );
    DatabaseQueue::set_throttle_config(
        queue.as_ref(),
        &queue_name,
        ThrottleConfig::new().max_concurrent(3),
    )
    .await
    .unwrap();
    let config = DatabaseQueue::get_throttle_config(queue.as_ref(), &queue_name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(config.max_concurrent, Some(3));
    assert!(
        DatabaseQueue::get_all_throttle_configs(queue.as_ref())
            .await
            .unwrap()
            .contains_key(&queue_name)
    );
    DatabaseQueue::remove_throttle_config(queue.as_ref(), &queue_name)
        .await
        .unwrap();
    assert!(
        DatabaseQueue::get_throttle_config(queue.as_ref(), &queue_name)
            .await
            .unwrap()
            .is_none()
    );
}

/// Pause information records who paused a queue, until it is resumed.
async fn pause_information<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("pause_info");
    assert!(
        queue
            .get_queue_pause_info(&queue_name)
            .await
            .unwrap()
            .is_none()
    );
    queue.pause_queue(&queue_name, Some("ops")).await.unwrap();
    let info = queue
        .get_queue_pause_info(&queue_name)
        .await
        .unwrap()
        .expect("paused");
    assert_eq!(info.queue_name, queue_name);
    assert_eq!(info.paused_by.as_deref(), Some("ops"));
    assert!(info.paused_at <= Utc::now() + Duration::seconds(5));
    assert!(
        queue
            .get_paused_queues()
            .await
            .unwrap()
            .iter()
            .any(|i| i.queue_name == queue_name)
    );
    queue.resume_queue(&queue_name, Some("ops")).await.unwrap();
    assert!(
        queue
            .get_queue_pause_info(&queue_name)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !queue
            .get_paused_queues()
            .await
            .unwrap()
            .iter()
            .any(|i| i.queue_name == queue_name)
    );
}

/// `get_ready_jobs` only returns jobs whose dependencies are satisfied, and the
/// explicit resolve / fail helpers update dependents.
async fn workflow_dependency_helpers<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("ready_jobs");
    let a = Job::new(queue_name.clone(), json!({ "step": "a" }));
    let b = Job::new(queue_name.clone(), json!({ "step": "b" })).depends_on(&a.id);
    let c = Job::new(queue_name.clone(), json!({ "step": "c" })).depends_on(&b.id);
    let (a_id, b_id, c_id) = (a.id, b.id, c.id);
    let workflow_id = queue
        .enqueue_workflow(JobGroup::new("ready").add_job(a).then(b).then(c))
        .await
        .unwrap();

    let ready: Vec<JobId> = queue
        .get_ready_jobs(&queue_name, 10)
        .await
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(ready, [a_id]);
    assert!(
        queue
            .get_ready_jobs(&queue_name, 0)
            .await
            .unwrap()
            .is_empty()
    );

    // Completing `a` (as a worker would) and resolving makes `b` ready.
    let job = queue.dequeue(&queue_name).await.unwrap().unwrap();
    assert_eq!(job.id, a_id);
    queue.complete_job(a_id).await.unwrap();
    queue.resolve_job_dependencies(a_id).await.unwrap();
    let ready: Vec<JobId> = queue
        .get_ready_jobs(&queue_name, 10)
        .await
        .unwrap()
        .iter()
        .map(|job| job.id)
        .collect();
    assert_eq!(ready, [b_id]);

    // A failure of `b` fails its dependents.
    queue.mark_job_dead(b_id, "b failed").await.unwrap();
    queue.fail_job_dependencies(b_id).await.unwrap();
    let c = queue.get_job(c_id).await.unwrap().unwrap();
    assert_eq!(c.status, JobStatus::Failed);
    assert!(c.dependencies_failed(), "{:?}", c.dependency_status);
    assert!(
        queue
            .get_ready_jobs(&queue_name, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let jobs = queue.get_workflow_jobs(workflow_id).await.unwrap();
    assert_eq!(jobs.len(), 3);
    for id in [a_id, b_id, c_id] {
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
        dead_job_management,
        statistics_queries,
        recurring_jobs,
        disable_and_enable_recurring_jobs,
        throttle_configuration,
        pause_information,
        workflow_dependency_helpers,
    ]
);

#[cfg(feature = "mysql")]
backend_tests!(
    mysql_tests,
    test_utils::setup_mysql_queue,
    ignore = "requires MySQL: MYSQL_DATABASE_URL",
    [
        dead_job_management,
        statistics_queries,
        recurring_jobs,
        disable_and_enable_recurring_jobs,
        throttle_configuration,
        pause_information,
        workflow_dependency_helpers,
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
        dead_job_management,
        statistics_queries,
        recurring_jobs,
        disable_and_enable_recurring_jobs,
        throttle_configuration,
        pause_information,
        workflow_dependency_helpers,
    ]
);
