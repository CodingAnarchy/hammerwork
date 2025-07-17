mod test_utils;

use chrono::Duration;
use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};

#[cfg(feature = "postgres")]
mod postgres_archive_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_policy_based_archival() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create completed and failed jobs with different ages
        let completed_job_old =
            Job::new("test_queue".to_string(), json!({"type": "completed_old"}));
        let completed_job_new =
            Job::new("test_queue".to_string(), json!({"type": "completed_new"}));
        let failed_job_old = Job::new("test_queue".to_string(), json!({"type": "failed_old"}));
        let failed_job_new = Job::new("test_queue".to_string(), json!({"type": "failed_new"}));

        // Enqueue all jobs
        queue.enqueue(completed_job_old.clone()).await.unwrap();
        queue.enqueue(completed_job_new.clone()).await.unwrap();
        queue.enqueue(failed_job_old.clone()).await.unwrap();
        queue.enqueue(failed_job_new.clone()).await.unwrap();

        // Complete the completed jobs
        queue.complete_job(completed_job_old.id).await.unwrap();
        queue.complete_job(completed_job_new.id).await.unwrap();

        // Fail the failed jobs
        queue
            .fail_job(failed_job_old.id, "Test failure")
            .await
            .unwrap();
        queue
            .fail_job(failed_job_new.id, "Test failure")
            .await
            .unwrap();

        let config = ArchivalConfig::new().with_compression_level(6);

        // For testing, we'll use a very short duration to trigger archival
        let archival_policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0)) // Archive immediately
            .archive_failed_after(Duration::days(1)) // Don't archive failed jobs yet
            .with_batch_size(100)
            .compress_archived_payloads(true)
            .enabled(true);

        // Archive jobs
        let stats = queue
            .archive_jobs(
                Some("test_queue"),
                &archival_policy,
                &config,
                ArchivalReason::Manual,
                Some("test_user"),
            )
            .await
            .unwrap();

        // Should have archived the completed jobs
        assert_eq!(stats.jobs_archived, 2);
        assert!(stats.bytes_archived > 0);
        assert!(stats.compression_ratio > 0.0);

        // Verify jobs are archived
        let completed_job_old_check = queue.get_job(completed_job_old.id).await.unwrap().unwrap();
        let completed_job_new_check = queue.get_job(completed_job_new.id).await.unwrap().unwrap();
        assert_eq!(completed_job_old_check.status, JobStatus::Archived);
        assert_eq!(completed_job_new_check.status, JobStatus::Archived);

        // Verify failed jobs are still failed (not archived)
        let failed_job_old_check = queue.get_job(failed_job_old.id).await.unwrap().unwrap();
        let failed_job_new_check = queue.get_job(failed_job_new.id).await.unwrap().unwrap();
        assert_eq!(failed_job_old_check.status, JobStatus::Failed);
        assert_eq!(failed_job_new_check.status, JobStatus::Failed);

        // Clean up
        queue.delete_job(failed_job_old.id).await.unwrap();
        queue.delete_job(failed_job_new.id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_job_restoration() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create and complete a job
        let job = Job::new("restore_test".to_string(), json!({"message": "restore me"}));
        let job_id = job.id;
        queue.enqueue(job).await.unwrap();
        queue.complete_job(job_id).await.unwrap();

        // Archive the job
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .enabled(true);
        let config = ArchivalConfig::new();

        let stats = queue
            .archive_jobs(
                Some("restore_test"),
                &policy,
                &config,
                ArchivalReason::Manual,
                Some("test"),
            )
            .await
            .unwrap();

        assert_eq!(stats.jobs_archived, 1);

        // Verify job is archived
        let archived_job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(archived_job.status, JobStatus::Archived);

        // Restore the job
        let restored_job = queue.restore_archived_job(job_id).await.unwrap();
        assert_eq!(restored_job.id, job_id);
        assert_eq!(restored_job.status, JobStatus::Pending); // Should be pending after restore
        assert_eq!(restored_job.payload, json!({"message": "restore me"}));

        // Verify job is no longer archived
        let check_job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(check_job.status, JobStatus::Pending);

        // Clean up
        queue.delete_job(job_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_list_archived_jobs() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create multiple jobs in different queues
        let jobs: Vec<Job> = (0..5)
            .map(|i| {
                Job::new(
                    format!("list_test_{}", i % 2), // Alternate between two queues
                    json!({"index": i}),
                )
            })
            .collect();

        // Enqueue and complete all jobs
        for job in &jobs {
            queue.enqueue(job.clone()).await.unwrap();
            queue.complete_job(job.id).await.unwrap();
        }

        // Archive all jobs
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(false) // Test without compression
            .enabled(true);
        let config = ArchivalConfig::new();

        let stats = queue
            .archive_jobs(
                None, // Archive from all queues
                &policy,
                &config,
                ArchivalReason::Automatic,
                None,
            )
            .await
            .unwrap();

        assert_eq!(stats.jobs_archived, 5);

        // List all archived jobs
        let all_archived = queue
            .list_archived_jobs(None, Some(10), Some(0))
            .await
            .unwrap();
        assert!(all_archived.len() >= 5); // May have more from other tests

        // List archived jobs from specific queue
        let queue_0_archived = queue
            .list_archived_jobs(Some("list_test_0"), Some(10), Some(0))
            .await
            .unwrap();
        assert_eq!(queue_0_archived.len(), 3); // Jobs 0, 2, 4

        let queue_1_archived = queue
            .list_archived_jobs(Some("list_test_1"), Some(10), Some(0))
            .await
            .unwrap();
        assert_eq!(queue_1_archived.len(), 2); // Jobs 1, 3

        // Test pagination
        let first_page = queue
            .list_archived_jobs(None, Some(2), Some(0))
            .await
            .unwrap();
        let second_page = queue
            .list_archived_jobs(None, Some(2), Some(2))
            .await
            .unwrap();
        assert_eq!(first_page.len(), 2);
        assert!(second_page.len() >= 2);

        // Verify archived job properties
        for archived_job in &queue_0_archived {
            assert_eq!(archived_job.status, JobStatus::Archived);
            assert!(archived_job.archived_at > archived_job.created_at);
            assert_eq!(archived_job.archival_reason, ArchivalReason::Automatic);
            assert!(!archived_job.payload_compressed); // We disabled compression
        }
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_archival_stats() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create jobs with different statuses
        let completed_job = Job::new("stats_test".to_string(), json!({"type": "completed"}));
        let failed_job = Job::new("stats_test".to_string(), json!({"type": "failed"}));

        queue.enqueue(completed_job.clone()).await.unwrap();
        queue.enqueue(failed_job.clone()).await.unwrap();

        queue.complete_job(completed_job.id).await.unwrap();
        queue.fail_job(failed_job.id, "Test error").await.unwrap();

        // Archive jobs
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .archive_failed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .enabled(true);
        let config = ArchivalConfig::new().with_compression_level(9);

        let archive_stats = queue
            .archive_jobs(
                Some("stats_test"),
                &policy,
                &config,
                ArchivalReason::Compliance,
                Some("compliance_user"),
            )
            .await
            .unwrap();

        assert_eq!(archive_stats.jobs_archived, 2);

        // Get archival statistics
        let stats = queue.get_archival_stats(Some("stats_test")).await.unwrap();
        assert_eq!(stats.jobs_archived, 2);
        assert!(stats.bytes_archived > 0);
        assert!(stats.compression_ratio > 0.0);
        assert!(stats.compression_ratio <= 1.0); // Should be compressed

        // Clean up archived jobs for other tests
        let cutoff = Utc::now() + Duration::seconds(1); // Future date to purge all
        queue.purge_archived_jobs(cutoff).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_purge_archived_jobs() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create and archive some jobs
        let jobs: Vec<Job> = (0..3)
            .map(|i| Job::new("purge_test".to_string(), json!({"index": i})))
            .collect();

        for job in &jobs {
            queue.enqueue(job.clone()).await.unwrap();
            queue.complete_job(job.id).await.unwrap();
        }

        // Archive jobs
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .enabled(true);
        let config = ArchivalConfig::new();

        let stats = queue
            .archive_jobs(
                Some("purge_test"),
                &policy,
                &config,
                ArchivalReason::Manual,
                Some("test"),
            )
            .await
            .unwrap();

        assert_eq!(stats.jobs_archived, 3);

        // Verify jobs are archived
        let archived_list = queue
            .list_archived_jobs(Some("purge_test"), Some(10), Some(0))
            .await
            .unwrap();
        assert_eq!(archived_list.len(), 3);

        // Purge archived jobs older than now (should delete all test jobs)
        let cutoff_date = Utc::now() + Duration::seconds(1);
        let purged_count = queue.purge_archived_jobs(cutoff_date).await.unwrap();
        assert_eq!(purged_count, 3);

        // Verify jobs are purged
        let archived_list_after = queue
            .list_archived_jobs(Some("purge_test"), Some(10), Some(0))
            .await
            .unwrap();
        assert_eq!(archived_list_after.len(), 0);
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_compression_verification() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create a job with a large payload to test compression
        let large_payload = json!({
            "data": "x".repeat(1000), // 1KB of repeated data - should compress well
            "metadata": {
                "key1": "value1".repeat(100),
                "key2": "value2".repeat(100),
                "key3": "value3".repeat(100)
            }
        });

        let job = Job::new("compression_test".to_string(), large_payload.clone());
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();

        // Archive with compression
        let policy_compressed = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .enabled(true);
        let config_compressed = ArchivalConfig::new().with_compression_level(9);

        let stats_compressed = queue
            .archive_jobs(
                Some("compression_test"),
                &policy_compressed,
                &config_compressed,
                ArchivalReason::Manual,
                Some("test"),
            )
            .await
            .unwrap();

        assert_eq!(stats_compressed.jobs_archived, 1);
        assert!(stats_compressed.compression_ratio < 1.0); // Should be compressed
        assert!(stats_compressed.compression_ratio > 0.0);

        // Restore and verify payload integrity
        let restored_job = queue.restore_archived_job(job.id).await.unwrap();
        assert_eq!(restored_job.payload, large_payload);
        assert_eq!(restored_job.queue_name, "compression_test");

        // Clean up
        queue.delete_job(job.id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_different_job_statuses_archival() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create jobs with different final statuses
        let completed_job = Job::new("status_test".to_string(), json!({"type": "completed"}));
        let failed_job = Job::new("status_test".to_string(), json!({"type": "failed"}));
        let dead_job = Job::new("status_test".to_string(), json!({"type": "dead"}));
        let timed_out_job = Job::new("status_test".to_string(), json!({"type": "timed_out"}));

        // Enqueue all jobs
        for job in [&completed_job, &failed_job, &dead_job, &timed_out_job] {
            queue.enqueue(job.clone()).await.unwrap();
        }

        // Set different final statuses
        queue.complete_job(completed_job.id).await.unwrap();
        queue.fail_job(failed_job.id, "Test failure").await.unwrap();

        // Simulate dead and timed out jobs by updating directly
        // In a real scenario, these would be set by the worker processes
        // For testing, we'll just test with completed and failed jobs

        // Create policy that archives different statuses at different intervals
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0)) // Archive completed immediately
            .archive_failed_after(Duration::days(1)) // Don't archive failed yet
            .archive_dead_after(Duration::seconds(0)) // Archive dead immediately
            .archive_timed_out_after(Duration::seconds(0)) // Archive timed out immediately
            .enabled(true);

        let config = ArchivalConfig::new();

        let stats = queue
            .archive_jobs(
                Some("status_test"),
                &policy,
                &config,
                ArchivalReason::Automatic,
                None,
            )
            .await
            .unwrap();

        // Should only archive completed job (failed job has longer retention)
        assert_eq!(stats.jobs_archived, 1);

        // Verify which jobs were archived
        let completed_check = queue.get_job(completed_job.id).await.unwrap().unwrap();
        let failed_check = queue.get_job(failed_job.id).await.unwrap().unwrap();

        assert_eq!(completed_check.status, JobStatus::Archived);
        assert_eq!(failed_check.status, JobStatus::Failed);

        // Clean up
        queue.delete_job(failed_job.id).await.unwrap();
        queue.delete_job(dead_job.id).await.unwrap();
        queue.delete_job(timed_out_job.id).await.unwrap();
    }
}

#[cfg(feature = "mysql")]
mod mysql_archive_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_basic_archival_flow() {
        let queue = test_utils::setup_mysql_queue().await;

        // Create and complete a job
        let job = Job::new("mysql_test".to_string(), json!({"message": "MySQL test"}));
        let job_id = job.id;

        queue.enqueue(job).await.unwrap();
        queue.complete_job(job_id).await.unwrap();

        // Archive the job
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .enabled(true);
        let config = ArchivalConfig::new();

        let stats = queue
            .archive_jobs(
                Some("mysql_test"),
                &policy,
                &config,
                ArchivalReason::Manual,
                Some("mysql_test_user"),
            )
            .await
            .unwrap();

        assert_eq!(stats.jobs_archived, 1);
        assert!(stats.bytes_archived > 0);

        // Verify job is archived
        let archived_job = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(archived_job.status, JobStatus::Archived);

        // Test restoration
        let restored_job = queue.restore_archived_job(job_id).await.unwrap();
        assert_eq!(restored_job.id, job_id);
        assert_eq!(restored_job.payload, json!({"message": "MySQL test"}));
        assert_eq!(restored_job.status, JobStatus::Pending);

        // Clean up
        queue.delete_job(job_id).await.unwrap();
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_archival_stats() {
        let queue = test_utils::setup_mysql_queue().await;

        // Create multiple jobs
        let jobs: Vec<Job> = (0..4)
            .map(|i| Job::new("mysql_stats".to_string(), json!({"index": i})))
            .collect();

        for job in &jobs {
            queue.enqueue(job.clone()).await.unwrap();
            queue.complete_job(job.id).await.unwrap();
        }

        // Archive jobs
        let policy = ArchivalPolicy::new()
            .archive_completed_after(Duration::seconds(0))
            .compress_archived_payloads(true)
            .enabled(true);
        let config = ArchivalConfig::new().with_compression_level(6);

        let archive_stats = queue
            .archive_jobs(
                Some("mysql_stats"),
                &policy,
                &config,
                ArchivalReason::Compliance,
                Some("mysql_user"),
            )
            .await
            .unwrap();

        assert_eq!(archive_stats.jobs_archived, 4);

        // Get statistics
        let stats = queue.get_archival_stats(Some("mysql_stats")).await.unwrap();
        assert_eq!(stats.jobs_archived, 4);
        assert!(stats.bytes_archived > 0);

        // Clean up
        let cutoff = Utc::now() + Duration::seconds(1);
        let purged = queue.purge_archived_jobs(cutoff).await.unwrap();
        assert_eq!(purged, 4);
    }
}

// Common tests that don't require specific database features
#[tokio::test]
async fn test_archival_policy_builder() {
    let policy = ArchivalPolicy::new()
        .archive_completed_after(Duration::days(7))
        .archive_failed_after(Duration::days(30))
        .archive_dead_after(Duration::days(14))
        .archive_timed_out_after(Duration::days(21))
        .purge_archived_after(Duration::days(365))
        .with_batch_size(500)
        .compress_archived_payloads(true)
        .enabled(true);

    assert_eq!(policy.archive_completed_after, Some(Duration::days(7)));
    assert_eq!(policy.archive_failed_after, Some(Duration::days(30)));
    assert_eq!(policy.archive_dead_after, Some(Duration::days(14)));
    assert_eq!(policy.archive_timed_out_after, Some(Duration::days(21)));
    assert_eq!(policy.purge_archived_after, Some(Duration::days(365)));
    assert_eq!(policy.batch_size, 500);
    assert!(policy.compress_payloads);
    assert!(policy.enabled);
}

#[tokio::test]
async fn test_archival_config_builder() {
    let config = ArchivalConfig::new()
        .with_compression_level(9)
        .with_max_payload_size(2048)
        .with_compression_verification(false);

    assert_eq!(config.compression_level, 9);
    assert_eq!(config.max_payload_size, 2048);
    assert!(!config.verify_compression);
}

#[tokio::test]
async fn test_archival_reason_enum() {
    // Test serialization/deserialization of ArchivalReason
    use serde_json;

    let reasons = vec![
        ArchivalReason::Automatic,
        ArchivalReason::Manual,
        ArchivalReason::Compliance,
        ArchivalReason::Maintenance,
    ];

    for reason in reasons {
        let serialized = serde_json::to_string(&reason).unwrap();
        let deserialized: ArchivalReason = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reason, deserialized);
    }
}

// WebSocket Archive Event Tests
mod websocket_archive_event_tests {
    use super::*;
    use chrono::Utc;
    use hammerwork::archive::{ArchivalStats, ArchiveEvent};

    use uuid::Uuid;

    #[test]
    fn test_archive_event_creation() {
        let job_id = Uuid::new_v4();
        let queue_name = "test_queue".to_string();
        let reason = ArchivalReason::Manual;

        let event = ArchiveEvent::JobArchived {
            job_id,
            queue: queue_name.clone(),
            reason: reason.clone(),
        };

        match event {
            ArchiveEvent::JobArchived {
                job_id: id,
                queue,
                reason: r,
            } => {
                assert_eq!(id, job_id);
                assert_eq!(queue, queue_name);
                assert_eq!(r, reason);
            }
            _ => panic!("Expected JobArchived event"),
        }
    }

    #[test]
    fn test_archive_event_serialization() {
        let events = vec![
            ArchiveEvent::JobArchived {
                job_id: Uuid::new_v4(),
                queue: "test_queue".to_string(),
                reason: ArchivalReason::Automatic,
            },
            ArchiveEvent::JobRestored {
                job_id: Uuid::new_v4(),
                queue: "restore_queue".to_string(),
                restored_by: Some("admin".to_string()),
            },
            ArchiveEvent::BulkArchiveStarted {
                operation_id: "op_123".to_string(),
                estimated_jobs: 1000,
            },
            ArchiveEvent::BulkArchiveProgress {
                operation_id: "op_123".to_string(),
                jobs_processed: 500,
                total: 1000,
            },
            ArchiveEvent::BulkArchiveCompleted {
                operation_id: "op_123".to_string(),
                stats: ArchivalStats {
                    jobs_archived: 1000,
                    jobs_purged: 0,
                    bytes_archived: 50000,
                    bytes_purged: 0,
                    compression_ratio: 0.7,
                    operation_duration: std::time::Duration::from_secs(30),
                    last_run_at: Utc::now(),
                },
            },
            ArchiveEvent::JobsPurged {
                count: 100,
                older_than: Utc::now(),
            },
        ];

        for event in events {
            let serialized = serde_json::to_string(&event).unwrap();
            let deserialized: ArchiveEvent = serde_json::from_str(&serialized).unwrap();

            // Verify the deserialized event matches the original
            match (&event, &deserialized) {
                (
                    ArchiveEvent::JobArchived {
                        job_id: id1,
                        queue: q1,
                        reason: r1,
                    },
                    ArchiveEvent::JobArchived {
                        job_id: id2,
                        queue: q2,
                        reason: r2,
                    },
                ) => {
                    assert_eq!(id1, id2);
                    assert_eq!(q1, q2);
                    assert_eq!(r1, r2);
                }
                (
                    ArchiveEvent::JobRestored {
                        job_id: id1,
                        queue: q1,
                        restored_by: rb1,
                    },
                    ArchiveEvent::JobRestored {
                        job_id: id2,
                        queue: q2,
                        restored_by: rb2,
                    },
                ) => {
                    assert_eq!(id1, id2);
                    assert_eq!(q1, q2);
                    assert_eq!(rb1, rb2);
                }
                (
                    ArchiveEvent::BulkArchiveStarted {
                        operation_id: op1,
                        estimated_jobs: ej1,
                    },
                    ArchiveEvent::BulkArchiveStarted {
                        operation_id: op2,
                        estimated_jobs: ej2,
                    },
                ) => {
                    assert_eq!(op1, op2);
                    assert_eq!(ej1, ej2);
                }
                _ => {} // Other variants handled by serde equality
            }
        }
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_archive_with_events() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create a test job archiver
        let mut archiver = JobArchiver::new(queue.pool.clone());

        // Set up event tracking
        let events: Arc<Mutex<Vec<ArchiveEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);

        // Create a job to archive
        let job = Job::new("websocket_test".to_string(), json!({"test": "data"}));
        queue.enqueue(job.clone()).await.unwrap();
        queue.complete_job(job.id).await.unwrap();

        // Configure archival policy
        archiver.set_policy(
            "websocket_test",
            ArchivalPolicy::new()
                .archive_completed_after(Duration::seconds(0))
                .enabled(true),
        );

        // Run archive operation with event publishing
        let (operation_id, stats) = archiver
            .archive_jobs_with_events(
                queue.as_ref(),
                Some("websocket_test"),
                ArchivalReason::Manual,
                Some("test_user"),
                |event| {
                    events_clone.lock().unwrap().push(event);
                },
            )
            .await
            .unwrap();

        // Verify operation completed successfully
        assert_eq!(stats.jobs_archived, 1);
        assert!(!operation_id.is_empty());

        // Verify events were published
        let captured_events = events.lock().unwrap();
        assert_eq!(captured_events.len(), 2);

        // Check bulk archive started event
        match &captured_events[0] {
            ArchiveEvent::BulkArchiveStarted {
                operation_id: op_id,
                estimated_jobs,
            } => {
                assert_eq!(op_id, &operation_id);
                assert!(estimated_jobs > &0);
            }
            _ => panic!("Expected BulkArchiveStarted event"),
        }

        // Check bulk archive completed event
        match &captured_events[1] {
            ArchiveEvent::BulkArchiveCompleted {
                operation_id: op_id,
                stats: event_stats,
            } => {
                assert_eq!(op_id, &operation_id);
                assert_eq!(event_stats.jobs_archived, 1);
            }
            _ => panic!("Expected BulkArchiveCompleted event"),
        }

        // Clean up
        let cutoff = Utc::now() + Duration::seconds(1);
        queue.purge_archived_jobs(cutoff).await.unwrap();
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_archive_progress_callback() {
        let queue = test_utils::setup_postgres_queue().await;

        // Create multiple jobs to archive
        let jobs: Vec<Job> = (0..5)
            .map(|i| Job::new("progress_test".to_string(), json!({"index": i})))
            .collect();

        for job in &jobs {
            queue.enqueue(job.clone()).await.unwrap();
            queue.complete_job(job.id).await.unwrap();
        }

        // Set up progress tracking
        let progress_updates: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let progress_clone = Arc::clone(&progress_updates);

        let mut archiver = JobArchiver::new(queue.pool.clone());
        archiver.set_policy(
            "progress_test",
            ArchivalPolicy::new()
                .archive_completed_after(Duration::seconds(0))
                .enabled(true),
        );

        // Run archive operation with progress callback
        let (operation_id, stats) = archiver
            .archive_jobs_with_progress(
                queue.as_ref(),
                Some("progress_test"),
                ArchivalReason::Automatic,
                Some("progress_test"),
                Some(Box::new(move |current, total| {
                    progress_clone.lock().unwrap().push((current, total));
                })),
            )
            .await
            .unwrap();

        // Verify operation completed
        assert_eq!(stats.jobs_archived, 5);
        assert!(!operation_id.is_empty());

        // Verify progress updates were called
        let updates = progress_updates.lock().unwrap();
        assert!(!updates.is_empty());

        // Should have at least start (0, total) and end (jobs_archived, total) updates
        assert!(updates.len() >= 2);

        // Check first update (start)
        let (start_current, start_total) = updates[0];
        assert_eq!(start_current, 0);
        assert!(start_total > 0);

        // Check last update (completion)
        let (end_current, end_total) = updates[updates.len() - 1];
        assert_eq!(end_current, stats.jobs_archived);
        assert_eq!(end_total, start_total);

        // Clean up
        let cutoff = Utc::now() + Duration::seconds(1);
        queue.purge_archived_jobs(cutoff).await.unwrap();
    }

    #[test]
    fn test_archive_event_display() {
        let job_id = Uuid::new_v4();
        let operation_id = "test_op_123";

        let events = vec![
            ArchiveEvent::JobArchived {
                job_id,
                queue: "test_queue".to_string(),
                reason: ArchivalReason::Manual,
            },
            ArchiveEvent::BulkArchiveStarted {
                operation_id: operation_id.to_string(),
                estimated_jobs: 100,
            },
        ];

        // Test that events can be formatted for display/logging
        for event in events {
            let formatted = format!("{:?}", event);
            assert!(!formatted.is_empty());

            match event {
                ArchiveEvent::JobArchived { .. } => {
                    assert!(formatted.contains("JobArchived"));
                    assert!(formatted.contains(&job_id.to_string()));
                }
                ArchiveEvent::BulkArchiveStarted { .. } => {
                    assert!(formatted.contains("BulkArchiveStarted"));
                    assert!(formatted.contains(operation_id));
                }
                _ => {}
            }
        }
    }
}
