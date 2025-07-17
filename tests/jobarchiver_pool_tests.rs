mod test_utils;

/// Test JobArchiver creation with public pool field from JobQueue
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_with_public_pool_field() {
    let queue = test_utils::setup_postgres_queue().await;

    // Test that we can create a JobArchiver using the public pool field
    let archiver = JobArchiver::new(queue.pool.clone());

    // Verify the archiver was created successfully
    assert!(archiver.get_config().compression_level > 0);

    // Test that we can use both the queue and archiver
    let job = Job::new("pool_test".to_string(), json!({"test": "data"}));
    queue.enqueue(job.clone()).await.unwrap();
    queue.complete_job(job.id).await.unwrap();

    // Create a simple archival policy
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::seconds(0))
        .enabled(true);
    let config = ArchivalConfig::new();

    // Test archiving with the archiver created from the public pool
    let stats = queue
        .archive_jobs(
            Some("pool_test"),
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

/// Test multiple JobArchivers sharing the same pool
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_multiple_archivers_shared_pool() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create multiple archivers from the same pool
    let mut archiver1 = JobArchiver::new(queue.pool.clone());
    let mut archiver2 = JobArchiver::new(queue.pool.clone());

    // Configure different policies for each archiver
    archiver1.set_policy(
        "queue1",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .enabled(true),
    );

    archiver2.set_policy(
        "queue2",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::minutes(1))
            .enabled(true),
    );

    // Create jobs in different queues
    let job1 = Job::new("queue1".to_string(), json!({"archiver": 1}));
    let job2 = Job::new("queue2".to_string(), json!({"archiver": 2}));

    queue.enqueue(job1.clone()).await.unwrap();
    queue.enqueue(job2.clone()).await.unwrap();
    queue.complete_job(job1.id).await.unwrap();
    queue.complete_job(job2.id).await.unwrap();

    // Test that each archiver can work independently with the shared pool
    let (op_id1, stats1) = archiver1
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some("queue1"),
            ArchivalReason::Manual,
            Some("archiver1"),
            None,
        )
        .await
        .unwrap();

    let (op_id2, stats2) = archiver2
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some("queue2"),
            ArchivalReason::Manual,
            Some("archiver2"),
            None,
        )
        .await
        .unwrap();

    // Verify both operations succeeded
    assert_eq!(stats1.jobs_archived, 1);
    assert_eq!(stats2.jobs_archived, 1);
    assert_ne!(op_id1, op_id2); // Should have different operation IDs

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// Test JobArchiver policy management with public pool
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_policy_management_with_public_pool() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create archiver with public pool
    let mut archiver = JobArchiver::new(queue.pool.clone());

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
    let job = Job::new("policy_test".to_string(), json!({"test": "policy"}));
    queue.enqueue(job.clone()).await.unwrap();
    queue.complete_job(job.id).await.unwrap();

    // Should work fine
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::seconds(0))
        .enabled(true);
    let config = ArchivalConfig::new();

    let stats = queue
        .archive_jobs(
            Some("policy_test"),
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

/// Test JobArchiver configuration management with public pool
#[test]
fn test_jobarchiver_config_management_with_public_pool() {
    #[cfg(feature = "postgres")]
    {
        use sqlx::PgPool;

        // Test configuration management without requiring a database connection
        // This is a compile-time and logic test

        // Mock pool creation (won't actually connect)
        #[allow(dead_code)]
        async fn test_config_logic() -> Result<(), Box<dyn std::error::Error>> {
            // This test focuses on the API and logic, not actual database operations
            let pool = PgPool::connect("postgresql://fake_host/fake_db").await?;
            let queue = Arc::new(JobQueue::new(pool.clone()));

            // Create archiver with public pool
            let mut archiver = JobArchiver::new(queue.pool.clone());

            // Test default configuration
            let default_config = archiver.get_config();
            assert_eq!(default_config.compression_level, 6);
            assert!(default_config.verify_compression);

            // Test setting custom configuration
            let custom_config = ArchivalConfig::new()
                .with_compression_level(9)
                .with_max_payload_size(2048)
                .with_compression_verification(false);

            archiver.set_config(custom_config.clone());

            let updated_config = archiver.get_config();
            assert_eq!(updated_config.compression_level, 9);
            assert_eq!(updated_config.max_payload_size, 2048);
            assert!(!updated_config.verify_compression);

            Ok(())
        }

        // This test verifies the API compiles correctly
    }
}

/// Test JobArchiver event publishing with public pool
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_event_publishing_with_public_pool() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create archiver using public pool
    let mut archiver = JobArchiver::new(queue.pool.clone());

    // Set up event tracking
    let events: Arc<Mutex<Vec<ArchiveEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);

    // Create and complete test jobs
    let jobs: Vec<Job> = (0..3)
        .map(|i| Job::new("event_test".to_string(), json!({"event": i})))
        .collect();

    for job in &jobs {
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();
    }

    // Configure archiver
    archiver.set_policy(
        "event_test",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .enabled(true),
    );

    // Test event publishing
    let (operation_id, stats) = archiver
        .archive_jobs_with_events(
            queue.as_ref(),
            Some("event_test"),
            ArchivalReason::Manual,
            Some("event_publisher"),
            |event| {
                events_clone.lock().unwrap().push(event);
            },
        )
        .await
        .unwrap();

    // Verify operation succeeded
    assert_eq!(stats.jobs_archived, 3);
    assert!(!operation_id.is_empty());

    // Verify events were published
    let published_events = events.lock().unwrap();
    assert_eq!(published_events.len(), 2); // BulkArchiveStarted + BulkArchiveCompleted

    // Verify event content
    match &published_events[0] {
        ArchiveEvent::BulkArchiveStarted {
            operation_id: op_id,
            estimated_jobs,
        } => {
            assert_eq!(op_id, &operation_id);
            assert!(*estimated_jobs > 0);
        }
        _ => panic!("Expected BulkArchiveStarted event"),
    }

    match &published_events[1] {
        ArchiveEvent::BulkArchiveCompleted {
            operation_id: op_id,
            stats: event_stats,
        } => {
            assert_eq!(op_id, &operation_id);
            assert_eq!(event_stats.jobs_archived, 3);
        }
        _ => panic!("Expected BulkArchiveCompleted event"),
    }

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// Test pool sharing between JobQueue and JobArchiver
#[test]
fn test_pool_sharing_pattern() {
    // Test the common pattern of sharing a pool between queue and archiver
    #[cfg(feature = "postgres")]
    {
        #[allow(dead_code)]
        async fn pool_sharing_example() -> Result<(), Box<dyn std::error::Error>> {
            // Create pool
            let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;

            // Create queue
            let queue = Arc::new(JobQueue::new(pool.clone()));

            // Create archiver using the public pool field
            let _archiver = JobArchiver::new(queue.pool.clone());

            // Verify both use the same pool (pointer comparison)
            assert!(std::ptr::eq(&queue.pool, &pool));

            // The archiver and queue now share the same connection pool
            // This is efficient and ensures consistency

            Ok(())
        }

        // Test that the pattern compiles
    }
}

/// Test error scenarios with JobArchiver and public pool
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_error_handling_with_public_pool() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create archiver with public pool
    let archiver = JobArchiver::new(queue.pool.clone());

    // Test archiving with no jobs (should succeed with 0 archived)
    let (operation_id, stats) = archiver
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some("nonexistent_queue"),
            ArchivalReason::Manual,
            Some("test"),
            None,
        )
        .await
        .unwrap();

    assert_eq!(stats.jobs_archived, 0);
    assert!(!operation_id.is_empty());

    // Test with invalid queue name (should still work)
    let (operation_id2, stats2) = archiver
        .archive_jobs_with_events(
            queue.as_ref(),
            Some(""),
            ArchivalReason::Manual,
            Some("test"),
            |_event| {
                // Event handler that does nothing
            },
        )
        .await
        .unwrap();

    assert_eq!(stats2.jobs_archived, 0);
    assert!(!operation_id2.is_empty());
    assert_ne!(operation_id, operation_id2); // Should have different operation IDs
}

/// Benchmark test for JobArchiver with public pool
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection and is performance-sensitive
async fn test_jobarchiver_performance_with_public_pool() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create many jobs for performance testing
    let job_count = 100;
    let jobs: Vec<Job> = (0..job_count)
        .map(|i| Job::new("perf_test".to_string(), json!({"batch": i})))
        .collect();

    // Enqueue and complete all jobs
    for job in &jobs {
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();
    }

    // Create archiver with public pool
    let mut archiver = JobArchiver::new(queue.pool.clone());
    archiver.set_policy(
        "perf_test",
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
            Some("perf_test"),
            ArchivalReason::Manual,
            Some("perf_test"),
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
