//! Regressions in the database queues found after 2.2.0, run against PostgreSQL and
//! MySQL: archive listings and statistics, extreme result settings and archival
//! policies, and how a fail-fast workflow leaves the dependency state of the jobs it
//! cancels.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    Job, JobStatus,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason},
    queue::DatabaseQueue,
    workflow::{FailurePolicy, JobGroup},
};
use serde_json::json;
use std::sync::Arc;

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
    let claimed = queue.dequeue(&name).await.unwrap().unwrap();
    assert_eq!(claimed.id, id);
    queue.delete_job(id).await.unwrap();
}

/// An archival policy with an age too large to subtract from now archives nothing
/// instead of panicking.
async fn huge_archival_age_does_not_panic<Q: DatabaseQueue>(queue: &Q) {
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
    let run_a = queue.dequeue(&format!("{name}_a")).await.unwrap().unwrap();
    assert_eq!(run_a.id, a_id);
    queue.complete_job(a_id).await.unwrap();
    let run_x = queue.dequeue(&format!("{name}_x")).await.unwrap().unwrap();
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
        .dequeue(&format!("{name}_b"))
        .await
        .unwrap()
        .expect("a retried job whose dependencies completed is runnable");
    assert_eq!(retried.id, b_id);

    for id in [a_id, b_id, x_id] {
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
}
