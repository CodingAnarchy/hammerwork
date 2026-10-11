//! `JobArchiver` tests. The archiver holds no database connection; every operation
//! takes the queue it works on.

mod test_utils;

#[cfg(feature = "postgres")]
use chrono::{Duration, Utc};
#[cfg(feature = "postgres")]
use hammerwork::{
    Job,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason, JobArchiver},
    queue::DatabaseQueue,
};
#[cfg(feature = "postgres")]
use serde_json::json;

/// An archiver created with `JobArchiver::new()` works alongside the queue it is given.
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_new_with_queue() {
    let queue = test_utils::setup_postgres_queue().await;
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("pool_test");

    // The archiver needs no pool: it works on the queue passed to each operation.
    let archiver = JobArchiver::new();

    // Verify the archiver was created successfully
    assert!(archiver.get_config().compression_level > 0);

    // Test that we can use both the queue and archiver
    let job = Job::new(queue_name.clone(), json!({"test": "data"}));
    queue.enqueue(job.clone()).await.unwrap();
    queue.complete_job(job.id).await.unwrap();

    // Create a simple archival policy
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::seconds(0))
        .enabled(true);
    let config = ArchivalConfig::new();

    // Archive through the queue the archiver would work on
    let stats = queue
        .archive_jobs(
            Some(queue_name.as_str()),
            &policy,
            &config,
            ArchivalReason::Manual,
            Some("test"),
        )
        .await
        .unwrap();

    assert_eq!(stats.jobs_archived, 1);

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// Several archivers, each with its own policies, working on the same queue
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_multiple_archivers_same_queue() {
    let queue = test_utils::setup_postgres_queue().await;
    let _serial = test_utils::serial().await;
    let queue1_name = test_utils::unique_queue("queue1");
    let queue2_name = test_utils::unique_queue("queue2");

    // Create multiple independent archivers
    let mut archiver1 = JobArchiver::new();
    let mut archiver2 = JobArchiver::new();

    // Configure different policies for each archiver
    archiver1.set_policy(
        queue1_name.as_str(),
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .enabled(true),
    );

    archiver2.set_policy(
        queue2_name.as_str(),
        ArchivalPolicy::new()
            .archive_completed_after(Duration::minutes(1))
            .enabled(true),
    );

    // Create jobs in different queues
    let job1 = Job::new(queue1_name.clone(), json!({"archiver": 1}));
    let job2 = Job::new(queue2_name.clone(), json!({"archiver": 2}));

    queue.enqueue(job1.clone()).await.unwrap();
    queue.enqueue(job2.clone()).await.unwrap();
    queue.complete_job(job1.id).await.unwrap();
    queue.complete_job(job2.id).await.unwrap();

    // Each archiver works independently on the same queue
    let (op_id1, stats1) = archiver1
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some(queue1_name.as_str()),
            ArchivalReason::Manual,
            Some("archiver1"),
            None,
        )
        .await
        .unwrap();

    let (op_id2, stats2) = archiver2
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some(queue2_name.as_str()),
            ArchivalReason::Manual,
            Some("archiver2"),
            None,
        )
        .await
        .unwrap();

    // Each archiver applied its own policy: queue1 archives immediately, while
    // queue2 only archives jobs completed more than a minute ago.
    assert_eq!(stats1.jobs_archived, 1);
    assert_eq!(stats2.jobs_archived, 0);
    assert_ne!(op_id1, op_id2); // Should have different operation IDs

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// JobArchiver policy management
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_policy_management() {
    let queue = test_utils::setup_postgres_queue().await;
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("policy_test");

    // Create the archiver
    let mut archiver = JobArchiver::new();

    // Test policy management
    let policy1 = ArchivalPolicy::new()
        .archive_completed_after(Duration::days(1))
        .enabled(true);

    let policy2 = ArchivalPolicy::new()
        .archive_completed_after(Duration::days(7))
        .enabled(false);

    // Set policies for different queues
    archiver.set_policy("fast_queue", policy1.clone());
    archiver.set_policy("slow_queue", policy2.clone());

    // Test retrieving policies
    let retrieved_policy1 = archiver.get_policy("fast_queue").unwrap();
    let retrieved_policy2 = archiver.get_policy("slow_queue").unwrap();

    assert_eq!(
        retrieved_policy1.archive_completed_after,
        policy1.archive_completed_after
    );
    assert_eq!(retrieved_policy2.enabled, policy2.enabled);

    // Test removing a policy
    let removed_policy = archiver.remove_policy("fast_queue");
    assert!(removed_policy.is_some());
    assert!(archiver.get_policy("fast_queue").is_none());

    // Test that the archiver still works with the queue after policy changes
    let job = Job::new(queue_name.clone(), json!({"test": "policy"}));
    queue.enqueue(job.clone()).await.unwrap();
    queue.complete_job(job.id).await.unwrap();

    // Should work fine
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::seconds(0))
        .enabled(true);
    let config = ArchivalConfig::new();

    let stats = queue
        .archive_jobs(
            Some(queue_name.as_str()),
            &policy,
            &config,
            ArchivalReason::Manual,
            Some("test"),
        )
        .await
        .unwrap();

    assert_eq!(stats.jobs_archived, 1);

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// JobArchiver configuration management (no database required)
#[cfg(feature = "postgres")]
#[test]
fn test_jobarchiver_config_management() {
    // Create the archiver
    let mut archiver = JobArchiver::new();

    // Test default configuration
    let default_config = archiver.get_config();
    assert_eq!(default_config.compression_level, 6);
    assert!(default_config.verify_compression);

    // Test setting custom configuration
    let custom_config = ArchivalConfig::new()
        .with_compression_level(9)
        .with_compression_verification(false);

    archiver.set_config(custom_config);

    let updated_config = archiver.get_config();
    assert_eq!(updated_config.compression_level, 9);
    assert!(!updated_config.verify_compression);
}

/// Benchmark test for JobArchiver
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_performance() {
    let queue = test_utils::setup_postgres_queue().await;
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("perf_test");

    // Create many jobs for performance testing
    let job_count = 100;
    let jobs: Vec<Job> = (0..job_count)
        .map(|i| Job::new(queue_name.clone(), json!({"batch": i})))
        .collect();

    // Enqueue and complete all jobs
    for job in &jobs {
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();
    }

    // Create the archiver
    let mut archiver = JobArchiver::new();
    archiver.set_policy(
        queue_name.as_str(),
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .with_batch_size(50) // Test batching
            .enabled(true),
    );

    // Measure archival performance
    let start = std::time::Instant::now();

    let (operation_id, stats) = archiver
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some(queue_name.as_str()),
            ArchivalReason::Manual,
            Some(queue_name.as_str()),
            Some(Box::new(|current, total| {
                // Progress callback for performance testing
                if current % 25 == 0 || current == total {
                    println!("Progress: {}/{}", current, total);
                }
            })),
        )
        .await
        .unwrap();

    let duration = start.elapsed();

    // Verify operation succeeded
    assert_eq!(stats.jobs_archived, job_count as u64);
    assert!(!operation_id.is_empty());

    // Basic performance assertions (these are lenient for CI environments)
    assert!(
        duration.as_secs() < 30,
        "Archival took too long: {:?}",
        duration
    );
    assert!(
        stats.operation_duration.as_secs() < 30,
        "Operation duration too long"
    );

    println!(
        "Archived {} jobs in {:?} (operation duration: {:?})",
        job_count, duration, stats.operation_duration
    );

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// `JobArchiver::new()` takes no arguments and starts with no per-queue policies, the
/// default policy and the default configuration, like `JobArchiver::default()`.
#[test]
fn test_jobarchiver_new_has_defaults() {
    use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, JobArchiver};

    let archiver = JobArchiver::new();
    assert!(archiver.get_policy("any_queue").is_none());
    let default_policy = ArchivalPolicy::default();
    assert_eq!(
        archiver.default_policy().archive_completed_after,
        default_policy.archive_completed_after
    );
    assert_eq!(archiver.default_policy().enabled, default_policy.enabled);
    let config = ArchivalConfig::default();
    assert_eq!(
        archiver.get_config().compression_level,
        config.compression_level
    );
    assert_eq!(
        archiver.get_config().verify_compression,
        config.verify_compression
    );
    assert_eq!(
        format!("{archiver:?}"),
        format!("{:?}", JobArchiver::default())
    );
}
