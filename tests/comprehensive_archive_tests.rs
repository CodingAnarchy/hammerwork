mod test_utils;

use chrono::{Duration, Utc};
use hammerwork::Job;
use hammerwork::archive::{
    ArchivalConfig, ArchivalPolicy, ArchivalReason, ArchiveEvent, JobArchiver,
};
use hammerwork::queue::DatabaseQueue;
use serde_json::json;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Test ArchiveEvent serialization and deserialization
#[tokio::test]
async fn test_archive_event_serialization_comprehensive() {
    use serde_json;

    // Test all archive event types can be serialized/deserialized
    let events = vec![
        ArchiveEvent::JobArchived {
            job_id: Uuid::new_v4(),
            queue: "test_queue".to_string(),
            reason: ArchivalReason::Manual,
        },
        ArchiveEvent::JobRestored {
            job_id: Uuid::new_v4(),
            queue: "restore_queue".to_string(),
            restored_by: Some("admin".to_string()),
        },
        ArchiveEvent::BulkArchiveStarted {
            operation_id: "op_123".to_string(),
            estimated_jobs: 100,
        },
        ArchiveEvent::BulkArchiveProgress {
            operation_id: "op_123".to_string(),
            jobs_processed: 50,
            total: 100,
        },
        ArchiveEvent::BulkArchiveCompleted {
            operation_id: "op_123".to_string(),
            stats: hammerwork::archive::ArchivalStats {
                jobs_archived: 100,
                jobs_purged: 0,
                bytes_archived: 1024,
                bytes_purged: 0,
                compression_ratio: 0.8,
                operation_duration: std::time::Duration::from_secs(30),
                last_run_at: Utc::now(),
            },
        },
        ArchiveEvent::JobsPurged {
            count: 25,
            older_than: Utc::now(),
        },
    ];

    for event in events {
        // Test serialization
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.is_empty());
        assert!(serialized.contains("\""));

        // Test deserialization
        let deserialized: ArchiveEvent = serde_json::from_str(&serialized).unwrap();

        // Test round-trip consistency
        let reserialized = serde_json::to_string(&deserialized).unwrap();
        assert_eq!(serialized, reserialized);
    }
}

/// Test JobArchiver with public pool field access patterns
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_public_pool_patterns() {
    let queue = test_utils::setup_postgres_queue().await;

    // Test creating JobArchiver with public pool field
    let _archiver = JobArchiver::new(queue.pool.clone());

    // Test that we can use both the queue and archiver simultaneously
    let job = Job::new("pool_pattern_test".to_string(), json!({"test": "pattern"}));
    queue.enqueue(job.clone()).await.unwrap();
    queue.complete_job(job.id).await.unwrap();

    // Test archiving functionality
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::seconds(0))
        .enabled(true);
    let config = ArchivalConfig::new();

    let stats = queue
        .archive_jobs(
            Some("pool_pattern_test"),
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

/// Test multiple JobArchivers with shared pool from JobQueue
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_multiple_archivers_shared_pool_comprehensive() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create multiple archivers using the public pool field
    let mut archiver1 = JobArchiver::new(queue.pool.clone());
    let mut archiver2 = JobArchiver::new(queue.pool.clone());
    let mut archiver3 = JobArchiver::new(queue.pool.clone());

    // Configure different policies
    archiver1.set_policy(
        "queue_a",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .enabled(true),
    );

    archiver2.set_policy(
        "queue_b",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .with_batch_size(50)
            .enabled(true),
    );

    archiver3.set_policy(
        "queue_c",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .enabled(true),
    );

    // Create jobs in different queues
    let jobs_a: Vec<Job> = (0..3)
        .map(|i| Job::new("queue_a".to_string(), json!({"id": i})))
        .collect();
    let jobs_b: Vec<Job> = (0..4)
        .map(|i| Job::new("queue_b".to_string(), json!({"id": i})))
        .collect();
    let jobs_c: Vec<Job> = (0..2)
        .map(|i| Job::new("queue_c".to_string(), json!({"id": i})))
        .collect();

    // Enqueue and complete all jobs
    for jobs in [&jobs_a, &jobs_b, &jobs_c] {
        for job in jobs {
            queue.enqueue(job.clone()).await.unwrap();
            queue.complete_job(job.id).await.unwrap();
        }
    }

    // Test concurrent archiving with different archivers
    let queue_clone1 = Arc::clone(&queue);
    let queue_clone2 = Arc::clone(&queue);
    let queue_clone3 = Arc::clone(&queue);

    let handle1 = tokio::spawn(async move {
        archiver1
            .archive_jobs_with_progress(
                queue_clone1.as_ref(),
                Some("queue_a"),
                ArchivalReason::Manual,
                Some("archiver1"),
                None,
            )
            .await
    });

    let handle2 = tokio::spawn(async move {
        archiver2
            .archive_jobs_with_progress(
                queue_clone2.as_ref(),
                Some("queue_b"),
                ArchivalReason::Automatic,
                Some("archiver2"),
                None,
            )
            .await
    });

    let handle3 = tokio::spawn(async move {
        archiver3
            .archive_jobs_with_progress(
                queue_clone3.as_ref(),
                Some("queue_c"),
                ArchivalReason::Compliance,
                Some("archiver3"),
                None,
            )
            .await
    });

    // Wait for all operations to complete
    let result1 = handle1.await.unwrap().unwrap();
    let result2 = handle2.await.unwrap().unwrap();
    let result3 = handle3.await.unwrap().unwrap();

    // Verify all operations succeeded
    let (op_id1, stats1) = result1;
    let (op_id2, stats2) = result2;
    let (op_id3, stats3) = result3;

    assert_eq!(stats1.jobs_archived, 3);
    assert_eq!(stats2.jobs_archived, 4);
    assert_eq!(stats3.jobs_archived, 2);

    // Ensure all operation IDs are unique
    assert_ne!(op_id1, op_id2);
    assert_ne!(op_id2, op_id3);
    assert_ne!(op_id1, op_id3);

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// Test JobArchiver event publishing functionality
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_event_publishing_comprehensive() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create archiver using public pool field
    let mut archiver = JobArchiver::new(queue.pool.clone());

    // Set up comprehensive event tracking
    let events: Arc<Mutex<Vec<ArchiveEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = Arc::clone(&events);

    // Create test jobs with various payloads
    let jobs: Vec<Job> = vec![
        Job::new("event_test".to_string(), json!({"type": "simple", "id": 1})),
        Job::new(
            "event_test".to_string(),
            json!({"type": "complex", "data": {"nested": {"value": 42}}}),
        ),
        Job::new(
            "event_test".to_string(),
            json!({"type": "large", "payload": "x".repeat(1000)}),
        ),
    ];

    for job in &jobs {
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();
    }

    // Configure archiver with comprehensive policy
    archiver.set_policy(
        "event_test",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .with_batch_size(2) // Test batching
            .enabled(true),
    );

    // Test event publishing with comprehensive tracking
    let (operation_id, stats) = archiver
        .archive_jobs_with_events(
            queue.as_ref(),
            Some("event_test"),
            ArchivalReason::Manual,
            Some("comprehensive_test"),
            |event| {
                // Store event for analysis
                events_clone.lock().unwrap().push(event);
            },
        )
        .await
        .unwrap();

    // Verify operation succeeded
    assert_eq!(stats.jobs_archived, 3);
    assert!(!operation_id.is_empty());

    // Analyze published events
    let published_events = events.lock().unwrap();
    assert_eq!(published_events.len(), 2); // BulkArchiveStarted + BulkArchiveCompleted

    // Verify event structure and content
    match &published_events[0] {
        ArchiveEvent::BulkArchiveStarted {
            operation_id: op_id,
            estimated_jobs,
        } => {
            assert_eq!(op_id, &operation_id);
            assert!(estimated_jobs > &0);
            assert!(estimated_jobs >= &3); // Should estimate at least the jobs we created
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
            assert!(event_stats.bytes_archived > 0);
            assert!(event_stats.operation_duration.as_millis() > 0);
        }
        _ => panic!("Expected BulkArchiveCompleted event"),
    }

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// Test JobArchiver progress tracking functionality
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_jobarchiver_progress_tracking_comprehensive() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create many jobs for meaningful progress tracking
    let job_count = 25;
    let jobs: Vec<Job> = (0..job_count)
        .map(|i| Job::new("progress_test".to_string(), json!({"batch": i})))
        .collect();

    for job in &jobs {
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();
    }

    // Set up detailed progress tracking
    let progress_updates: Arc<Mutex<Vec<(u64, u64, std::time::Instant)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let progress_clone = Arc::clone(&progress_updates);

    let mut archiver = JobArchiver::new(queue.pool.clone());
    archiver.set_policy(
        "progress_test",
        ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .with_batch_size(10) // Test batching progress
            .enabled(true),
    );

    let start_time = std::time::Instant::now();

    // Run archive operation with detailed progress tracking
    let (operation_id, stats) = archiver
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some("progress_test"),
            ArchivalReason::Automatic,
            Some("progress_test"),
            Some(Box::new(move |current, total| {
                let timestamp = std::time::Instant::now();
                progress_clone
                    .lock()
                    .unwrap()
                    .push((current, total, timestamp));
            })),
        )
        .await
        .unwrap();

    let operation_duration = start_time.elapsed();

    // Verify operation succeeded
    assert_eq!(stats.jobs_archived, job_count as u64);
    assert!(!operation_id.is_empty());

    // Analyze progress updates
    let updates = progress_updates.lock().unwrap();
    assert!(!updates.is_empty());
    assert!(updates.len() >= 2); // At least start and end

    // Verify progress consistency
    let mut last_current = 0;
    let mut last_timestamp = start_time;

    for &(current, total, timestamp) in updates.iter() {
        // Progress should only increase
        assert!(
            current >= last_current,
            "Progress should not decrease: {} -> {}",
            last_current,
            current
        );

        // Total should remain consistent
        assert_eq!(total, updates[0].1, "Total should remain consistent");

        // Timestamps should advance
        assert!(timestamp >= last_timestamp, "Timestamps should advance");

        // Current should never exceed total
        assert!(
            current <= total,
            "Current ({}) should not exceed total ({})",
            current,
            total
        );

        last_current = current;
        last_timestamp = timestamp;
    }

    // Verify final progress state
    let (final_current, final_total, _) = updates[updates.len() - 1];
    assert_eq!(final_current, stats.jobs_archived);
    assert_eq!(final_total, updates[0].1); // Total should be consistent

    println!(
        "Progress tracking: {} updates over {:?} for {} jobs",
        updates.len(),
        operation_duration,
        job_count
    );

    // Clean up
    let cutoff = Utc::now() + Duration::seconds(1);
    queue.purge_archived_jobs(cutoff).await.unwrap();
}

/// Test error handling and edge cases in archive operations
#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore] // Requires database connection
async fn test_archive_error_handling_comprehensive() {
    let queue = test_utils::setup_postgres_queue().await;

    let archiver = JobArchiver::new(queue.pool.clone());

    // Test archiving with no matching jobs
    let (operation_id, stats) = archiver
        .archive_jobs_with_progress(
            queue.as_ref(),
            Some("nonexistent_queue"),
            ArchivalReason::Manual,
            Some("error_test"),
            None,
        )
        .await
        .unwrap();

    assert_eq!(stats.jobs_archived, 0);
    assert!(!operation_id.is_empty());
    assert_eq!(stats.bytes_archived, 0);

    // Test with empty queue name
    let (operation_id2, stats2) = archiver
        .archive_jobs_with_events(
            queue.as_ref(),
            Some(""),
            ArchivalReason::Manual,
            Some("error_test"),
            |_event| {
                // Event handler that does nothing
            },
        )
        .await
        .unwrap();

    assert_eq!(stats2.jobs_archived, 0);
    assert!(!operation_id2.is_empty());
    assert_ne!(operation_id, operation_id2); // Should have different operation IDs

    // Test with None queue name (all queues)
    let (operation_id3, stats3) = archiver
        .archive_jobs_with_progress(
            queue.as_ref(),
            None, // All queues
            ArchivalReason::Automatic,
            None, // No archived_by
            Some(Box::new(|_current, _total| {
                // Progress callback that handles zero progress
            })),
        )
        .await
        .unwrap();

    assert_eq!(stats3.jobs_archived, 0); // No jobs to archive
    assert!(!operation_id3.is_empty());
    assert_ne!(operation_id2, operation_id3);
}

/// Test ArchiveEvent edge cases and special values
#[test]
fn test_archive_event_edge_cases() {
    use serde_json;

    // Test events with edge case values
    let edge_case_events = vec![
        // Zero values
        ArchiveEvent::BulkArchiveProgress {
            operation_id: "zero_test".to_string(),
            jobs_processed: 0,
            total: 0,
        },
        // Maximum values
        ArchiveEvent::BulkArchiveProgress {
            operation_id: "max_test".to_string(),
            jobs_processed: u64::MAX,
            total: u64::MAX,
        },
        // Empty strings
        ArchiveEvent::JobArchived {
            job_id: Uuid::new_v4(),
            queue: "".to_string(),
            reason: ArchivalReason::Manual,
        },
        // Very long strings
        ArchiveEvent::JobArchived {
            job_id: Uuid::new_v4(),
            queue: "x".repeat(1000),
            reason: ArchivalReason::Automatic,
        },
        // Unicode characters
        ArchiveEvent::JobRestored {
            job_id: Uuid::new_v4(),
            queue: "队列_🚀_💻_test".to_string(),
            restored_by: Some("用户_admin_👤".to_string()),
        },
        // None values
        ArchiveEvent::JobRestored {
            job_id: Uuid::new_v4(),
            queue: "test".to_string(),
            restored_by: None,
        },
    ];

    for event in edge_case_events {
        // Test serialization handles edge cases
        let serialized = serde_json::to_string(&event).unwrap();
        assert!(!serialized.is_empty());

        // Test deserialization preserves edge case values
        let deserialized: ArchiveEvent = serde_json::from_str(&serialized).unwrap();

        // Test round-trip consistency
        let reserialized = serde_json::to_string(&deserialized).unwrap();
        assert_eq!(serialized, reserialized);
    }
}

/// Test JobArchiver configuration management with public pool
#[test]
fn test_jobarchiver_configuration_comprehensive() {
    // Test that JobArchiver can be configured properly using builder patterns

    #[cfg(feature = "postgres")]
    {
        // This is a compile-time test for API usage patterns
        async fn test_configuration_patterns() -> Result<(), Box<dyn std::error::Error>> {
            let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
            let queue = Arc::new(hammerwork::JobQueue::new(pool.clone()));

            // Test creating archiver with public pool field
            let mut archiver = JobArchiver::new(queue.pool.clone());

            // Test comprehensive policy configuration
            let policy = ArchivalPolicy::new()
                .archive_completed_after(Duration::days(7))
                .archive_failed_after(Duration::days(30))
                .archive_dead_after(Duration::days(14))
                .archive_timed_out_after(Duration::days(21))
                .purge_archived_after(Duration::days(365))
                .with_batch_size(500)
                .compress_archived_payloads(true)
                .enabled(true);

            archiver.set_policy("comprehensive_queue", policy);

            // Test configuration management
            let config = ArchivalConfig::new()
                .with_compression_level(9)
                .with_max_payload_size(2048)
                .with_compression_verification(true);

            archiver.set_config(config);

            // Test that configurations are properly set
            let retrieved_policy = archiver.get_policy("comprehensive_queue").unwrap();
            assert_eq!(retrieved_policy.batch_size, 500);
            assert!(retrieved_policy.compress_payloads);
            assert!(retrieved_policy.enabled);

            let retrieved_config = archiver.get_config();
            assert_eq!(retrieved_config.compression_level, 9);
            assert_eq!(retrieved_config.max_payload_size, 2048);
            assert!(retrieved_config.verify_compression);

            Ok(())
        }

        // Test that the configuration API compiles correctly
        let _ = test_configuration_patterns; // Reference to avoid unused function warning
    }

    // Always pass this test since it's primarily a compilation test
}
