//! Regressions in the database queues found after 2.2.0, run against PostgreSQL and
//! MySQL: archive listings and statistics, extreme result settings and archival
//! policies, how a fail-fast workflow leaves the dependency state of the jobs it
//! cancels, and how archiving, restoring and retrying treat workflows and dependencies
//! (#102).

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    HammerworkError, Job, JobStatus,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason},
    queue::DatabaseQueue,
    workflow::{DependencyStatus, FailurePolicy, JobGroup},
};
use serde_json::json;
use std::sync::Arc;

/// `purge_archived_jobs` purges every queue's archived jobs, so the tests here that
/// archive and then purge run one at a time: a purge must not delete the jobs another
/// test has just archived and is about to list.
static ARCHIVE_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Archive every finished job of `queue_name` now.
async fn archive_now<Q: DatabaseQueue>(
    queue: &Q,
    queue_name: &str,
    policy: ArchivalPolicy,
) -> hammerwork::archive::ArchivalStats {
    queue
        .archive_jobs(
            Some(queue_name),
            &policy.enabled(true),
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            None,
        )
        .await
        .unwrap()
}

/// `list_archived_jobs` with an offset and no limit skips the first jobs. MySQL does not
/// accept `OFFSET` without `LIMIT`, so this used to be a syntax error there.
async fn archived_listing_offset_without_limit<Q: DatabaseQueue>(queue: &Q) {
    let _serial = ARCHIVE_TESTS.lock().await;
    let name = test_utils::unique_queue("archived_offset");
    for i in 0..3 {
        let id = queue
            .enqueue(Job::new(name.clone(), json!({ "i": i })))
            .await
            .unwrap();
        queue.complete_job(id).await.unwrap();
    }
    let policy = ArchivalPolicy::new().archive_completed_after(chrono::Duration::seconds(0));
    assert_eq!(archive_now(queue, &name, policy).await.jobs_archived, 3);

    let all = queue
        .list_archived_jobs(Some(&name), None, None)
        .await
        .unwrap();
    assert_eq!(all.len(), 3);
    let rest = queue
        .list_archived_jobs(Some(&name), None, Some(1))
        .await
        .expect("an offset without a limit is valid");
    assert_eq!(rest.len(), 2);
    assert_eq!(
        rest.iter().map(|job| job.id).collect::<Vec<_>>(),
        all[1..].iter().map(|job| job.id).collect::<Vec<_>>()
    );

    queue
        .purge_archived_jobs(chrono::Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
}

/// A job with a result TTL or size limit too large for the `BIGINT` columns is stored
/// with the largest value instead of wrapping negative, which made the row unreadable
/// (`get_job` and claims failed on it) although the enqueue succeeded.
async fn huge_result_settings_stay_readable<Q: DatabaseQueue>(queue: &Q) {
    let name = test_utils::unique_queue("huge_result_ttl");
    let mut job = Job::new(name.clone(), json!({})).with_result_ttl(std::time::Duration::MAX);
    job.result_config.max_size_bytes = Some(usize::MAX);
    let id = queue.enqueue(job).await.unwrap();

    let stored = queue
        .get_job(id)
        .await
        .expect("a huge result TTL must not make the job unreadable")
        .unwrap();
    assert_eq!(
        stored.result_config.ttl,
        Some(std::time::Duration::from_secs(i64::MAX as u64))
    );
    assert_eq!(
        stored.result_config.max_size_bytes,
        Some(usize::try_from(i64::MAX).unwrap_or(usize::MAX))
    );
    let claimed = queue.dequeue_one(&name).await.unwrap().unwrap();
    assert_eq!(claimed.id, id);
    queue.delete_job(id).await.unwrap();
}

/// An archival policy with an age too large to subtract from now archives nothing
/// instead of panicking.
async fn huge_archival_age_does_not_panic<Q: DatabaseQueue>(queue: &Q) {
    let _serial = ARCHIVE_TESTS.lock().await;
    let name = test_utils::unique_queue("huge_archival_age");
    let id = queue
        .enqueue(Job::new(name.clone(), json!({})))
        .await
        .unwrap();
    queue.complete_job(id).await.unwrap();

    let policy = ArchivalPolicy::new().archive_completed_after(chrono::Duration::MAX);
    assert_eq!(archive_now(queue, &name, policy).await.jobs_archived, 0);
    let policy = ArchivalPolicy::new().archive_completed_after(chrono::Duration::MIN);
    assert_eq!(archive_now(queue, &name, policy).await.jobs_archived, 1);

    queue
        .purge_archived_jobs(chrono::Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
}

/// `ArchivalStats::compression_ratio` (original size / stored size) counts the jobs
/// whose payloads were stored uncompressed as 1.0 instead of 0, so it is never below 1
/// when nothing grew.
async fn compression_ratio_counts_uncompressed_jobs<Q: DatabaseQueue>(queue: &Q) {
    let _serial = ARCHIVE_TESTS.lock().await;
    let name = test_utils::unique_queue("compression_ratio");
    // Random hex compresses about 2:1; the tiny payloads do not compress at all.
    let hex: String = (0..40)
        .map(|_| uuid::Uuid::new_v4().simple().to_string())
        .collect();
    let mut ids = vec![
        queue
            .enqueue(Job::new(name.clone(), json!({ "data": hex })))
            .await
            .unwrap(),
    ];
    for _ in 0..9 {
        ids.push(
            queue
                .enqueue(Job::new(name.clone(), json!({})))
                .await
                .unwrap(),
        );
    }
    for id in &ids {
        queue.complete_job(*id).await.unwrap();
    }

    let policy = ArchivalPolicy::new()
        .archive_completed_after(chrono::Duration::seconds(0))
        .compress_archived_payloads(true);
    let stats = archive_now(queue, &name, policy).await;
    assert_eq!(stats.jobs_archived, 10);
    assert!(
        stats.compression_ratio > 1.0,
        "compression ratio {} below 1 although only one payload was compressed",
        stats.compression_ratio
    );

    queue
        .purge_archived_jobs(chrono::Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
}

/// A fail-fast workflow fails its pending jobs without marking the dependencies of the
/// ones that were already satisfied as failed, on both backends, so such a job retried
/// by hand runs. MySQL marked them failed, leaving the retried job `Pending` forever.
async fn fail_fast_keeps_satisfied_dependencies<Q>(queue: Arc<Q>)
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let name = test_utils::unique_queue("fail_fast_satisfied");
    let step = |step: &str| Job::new(format!("{name}_{step}"), json!({ "step": step }));
    let a = step("a");
    let b = step("b").depends_on(&a.id);
    let x = step("x").with_max_attempts(1);
    let (a_id, b_id, x_id) = (a.id, b.id, x.id);
    queue
        .enqueue_workflow(
            JobGroup::new("fail-fast-satisfied")
                .with_failure_policy(FailurePolicy::FailFast)
                .add_job(a)
                .add_job(b)
                .add_job(x),
        )
        .await
        .unwrap();

    // a completes, so b's dependencies are satisfied, then x fails the workflow.
    let run_a = queue
        .dequeue_one(&format!("{name}_a"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run_a.id, a_id);
    queue.complete_job(a_id).await.unwrap();
    let run_x = queue
        .dequeue_one(&format!("{name}_x"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run_x.id, x_id);
    queue.fail_job(x_id, "boom").await.unwrap();
    assert_eq!(
        queue.get_job(b_id).await.unwrap().unwrap().status,
        JobStatus::Failed
    );

    queue
        .retry_job(b_id, chrono::Utc::now() - chrono::Duration::hours(1))
        .await
        .unwrap();
    let retried = queue
        .dequeue_one(&format!("{name}_b"))
        .await
        .unwrap()
        .expect("a retried job whose dependencies completed is runnable");
    assert_eq!(retried.id, b_id);

    for id in [a_id, b_id, x_id] {
        queue.delete_job(id).await.unwrap();
    }
}

/// Archiving keeps a job's workflow and restoring puts it back (#102).
async fn archive_keeps_workflow<Q: DatabaseQueue>(queue: &Q) {
    let _serial = ARCHIVE_TESTS.lock().await;
    let name = test_utils::unique_queue("archive_workflow");
    let job = Job::new(name.clone(), json!({}));
    let id = job.id;
    let workflow_id = queue
        .enqueue_workflow(JobGroup::new("archived-pipeline").add_job(job))
        .await
        .unwrap();
    queue.complete_job(id).await.unwrap();
    let policy = ArchivalPolicy::new().archive_completed_after(chrono::Duration::seconds(0));
    assert_eq!(archive_now(queue, &name, policy).await.jobs_archived, 1);

    let archived = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(archived.status, JobStatus::Archived);
    assert_eq!(archived.workflow_id, Some(workflow_id));
    assert_eq!(archived.workflow_name.as_deref(), Some("archived-pipeline"));

    let restored = queue.restore_archived_job(id).await.unwrap();
    assert_eq!(restored.workflow_id, Some(workflow_id));
    assert_eq!(restored.workflow_name.as_deref(), Some("archived-pipeline"));
    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Pending);
    assert_eq!(stored.workflow_id, Some(workflow_id));
    assert_eq!(stored.workflow_name.as_deref(), Some("archived-pipeline"));

    queue.delete_job(id).await.unwrap();
}

/// Whether `result` is the error for a job whose dependency `dependency` cannot complete.
fn is_dependency_refusal<T: std::fmt::Debug>(
    result: &hammerwork::Result<T>,
    dependency: hammerwork::JobId,
) -> bool {
    matches!(result, Err(HammerworkError::Workflow { message })
        if message.contains(&dependency.to_string()))
}

/// `retry_job` re-evaluates the dependencies of a job failed because a dependency
/// failed: it refuses while the dependency is still failed, and afterwards puts the job
/// back to wait for it instead of leaving it `Pending` forever (#102).
async fn retry_reevaluates_dependencies<Q: DatabaseQueue>(queue: &Q) {
    let name = test_utils::unique_queue("retry_deps");
    let parent = Job::new(format!("{name}_parent"), json!({}));
    let child = Job::new(format!("{name}_child"), json!({})).depends_on(&parent.id);
    let (parent_id, child_id) = (parent.id, child.id);
    queue.enqueue(parent).await.unwrap();
    queue.enqueue(child).await.unwrap();

    queue.fail_job(parent_id, "boom").await.unwrap();
    let failed = queue.get_job(child_id).await.unwrap().unwrap();
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(failed.dependency_status, DependencyStatus::Failed);

    let past = chrono::Utc::now() - chrono::Duration::hours(1);
    let refused = queue.retry_job(child_id, past).await;
    assert!(is_dependency_refusal(&refused, parent_id), "{refused:?}");
    assert_eq!(
        queue.get_job(child_id).await.unwrap().unwrap().status,
        JobStatus::Failed,
        "a refused retry changes nothing"
    );

    // Once the dependency is retried, the job can be retried and waits for it.
    queue.retry_job(parent_id, past).await.unwrap();
    queue.retry_job(child_id, past).await.unwrap();
    let waiting = queue.get_job(child_id).await.unwrap().unwrap();
    assert_eq!(waiting.status, JobStatus::Pending);
    assert_eq!(waiting.dependency_status, DependencyStatus::Waiting);
    assert!(
        queue
            .dequeue_one(&format!("{name}_child"))
            .await
            .unwrap()
            .is_none()
    );

    let run = queue
        .dequeue_one(&format!("{name}_parent"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.id, parent_id);
    queue.complete_job(parent_id).await.unwrap();
    let run = queue
        .dequeue_one(&format!("{name}_child"))
        .await
        .unwrap()
        .expect("released when its dependency completed");
    assert_eq!(run.id, child_id);

    // A job whose dependencies all completed is satisfied straight away.
    queue.fail_job(child_id, "child failed").await.unwrap();
    queue.retry_job(child_id, past).await.unwrap();
    assert_eq!(
        queue
            .get_job(child_id)
            .await
            .unwrap()
            .unwrap()
            .dependency_status,
        DependencyStatus::Satisfied
    );

    // A satisfied job stays retryable after its dependency is gone (purged).
    queue.delete_job(parent_id).await.unwrap();
    queue.fail_job(child_id, "failed again").await.unwrap();
    queue.retry_job(child_id, past).await.unwrap();
    assert_eq!(
        queue
            .get_job(child_id)
            .await
            .unwrap()
            .unwrap()
            .dependency_status,
        DependencyStatus::Satisfied
    );

    queue.delete_job(child_id).await.unwrap();
}

/// `restore_archived_job` re-evaluates the dependencies of the restored job the same
/// way (#102).
async fn restore_reevaluates_dependencies<Q: DatabaseQueue>(queue: &Q) {
    let _serial = ARCHIVE_TESTS.lock().await;
    let name = test_utils::unique_queue("restore_deps");
    let parent = Job::new(name.clone(), json!({}));
    let child = Job::new(name.clone(), json!({})).depends_on(&parent.id);
    let (parent_id, child_id) = (parent.id, child.id);
    queue.enqueue(parent).await.unwrap();
    queue.enqueue(child).await.unwrap();
    queue.fail_job(parent_id, "boom").await.unwrap();
    let policy = ArchivalPolicy::new().archive_failed_after(chrono::Duration::seconds(0));
    assert_eq!(archive_now(queue, &name, policy).await.jobs_archived, 2);

    // The dependency is archived as Failed: restoring the dependent is refused.
    let refused = queue.restore_archived_job(child_id).await;
    assert!(is_dependency_refusal(&refused, parent_id), "{refused:?}");
    assert_eq!(
        queue.get_job(child_id).await.unwrap().unwrap().status,
        JobStatus::Archived,
        "a refused restore leaves the job archived"
    );

    queue.restore_archived_job(parent_id).await.unwrap();
    let restored = queue.restore_archived_job(child_id).await.unwrap();
    assert_eq!(restored.dependency_status, DependencyStatus::Waiting);
    assert_eq!(
        queue
            .get_job(child_id)
            .await
            .unwrap()
            .unwrap()
            .dependency_status,
        DependencyStatus::Waiting
    );

    let run = queue.dequeue_one(&name).await.unwrap().unwrap();
    assert_eq!(run.id, parent_id, "the dependent waits for its dependency");
    queue.complete_job(parent_id).await.unwrap();
    let run = queue
        .dequeue_one(&name)
        .await
        .unwrap()
        .expect("released when its dependency completed");
    assert_eq!(run.id, child_id);

    for id in [parent_id, child_id] {
        queue.delete_job(id).await.unwrap();
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn archived_listing_offset_without_limit_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        let _serial = test_utils::serial().await;
        archived_listing_offset_without_limit(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn huge_result_settings_stay_readable_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        huge_result_settings_stay_readable(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn huge_archival_age_does_not_panic_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        let _serial = test_utils::serial().await;
        huge_archival_age_does_not_panic(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn compression_ratio_counts_uncompressed_jobs_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        let _serial = test_utils::serial().await;
        compression_ratio_counts_uncompressed_jobs(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn fail_fast_keeps_satisfied_dependencies_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        fail_fast_keeps_satisfied_dependencies(queue).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn archive_keeps_workflow_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        archive_keeps_workflow(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn retry_reevaluates_dependencies_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        retry_reevaluates_dependencies(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn restore_reevaluates_dependencies_postgres() {
        let queue = test_utils::setup_postgres_queue().await;
        restore_reevaluates_dependencies(queue.as_ref()).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn archived_listing_offset_without_limit_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        let _serial = test_utils::serial().await;
        archived_listing_offset_without_limit(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn huge_result_settings_stay_readable_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        huge_result_settings_stay_readable(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn huge_archival_age_does_not_panic_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        let _serial = test_utils::serial().await;
        huge_archival_age_does_not_panic(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn compression_ratio_counts_uncompressed_jobs_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        let _serial = test_utils::serial().await;
        compression_ratio_counts_uncompressed_jobs(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn fail_fast_keeps_satisfied_dependencies_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        fail_fast_keeps_satisfied_dependencies(queue).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn archive_keeps_workflow_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        archive_keeps_workflow(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn retry_reevaluates_dependencies_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        retry_reevaluates_dependencies(queue.as_ref()).await;
    }

    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn restore_reevaluates_dependencies_mysql() {
        let queue = test_utils::setup_mysql_queue().await;
        restore_reevaluates_dependencies(queue.as_ref()).await;
    }
}

/// The in-memory `TestQueue` re-evaluates dependencies like the database backends.
#[cfg(feature = "test-queue")]
mod test_queue_tests {
    use super::*;
    use hammerwork::queue::test::TestQueue;

    #[tokio::test]
    async fn retry_reevaluates_dependencies_test_queue() {
        retry_reevaluates_dependencies(&TestQueue::new()).await;
    }

    #[tokio::test]
    async fn restore_reevaluates_dependencies_test_queue() {
        restore_reevaluates_dependencies(&TestQueue::new()).await;
    }
}
