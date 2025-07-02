//! Integration tests for the TestQueue implementation

#![cfg(feature = "test")]

use chrono::Duration;
use hammerwork::{
    Job, JobPriority, JobStatus,
    batch::{BatchStatus, JobBatch, PartialFailureMode},
    priority::PriorityWeights,
    queue::{
        DatabaseQueue,
        test::{MockClock, TestQueue},
    },
    rate_limit::ThrottleConfig,
    workflow::{FailurePolicy, JobGroup, WorkflowStatus},
};
use serde_json::json;
use std::collections::HashMap;

#[tokio::test]
async fn test_queue_basic_operations() {
    let queue = TestQueue::new();

    // Test empty queue
    assert!(queue.dequeue("empty_queue").await.unwrap().is_none());

    // Enqueue and dequeue
    let job = Job::new("test_queue".to_string(), json!({"test": "data"}));
    let job_id = queue.enqueue(job.clone()).await.unwrap();

    let dequeued = queue.dequeue("test_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.id, job_id);
    assert_eq!(dequeued.status, JobStatus::Running);
    assert!(dequeued.started_at.is_some());

    // Complete the job
    queue.complete_job(job_id).await.unwrap();
    let completed = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(completed.status, JobStatus::Completed);
    assert!(completed.completed_at.is_some());
}

#[tokio::test]
async fn test_priority_ordering() {
    let queue = TestQueue::new();

    // Enqueue jobs with different priorities
    let critical =
        Job::new("prio_queue".to_string(), json!({"priority": "critical"})).as_critical();
    let high = Job::new("prio_queue".to_string(), json!({"priority": "high"})).as_high_priority();
    let normal = Job::new("prio_queue".to_string(), json!({"priority": "normal"}));
    let low = Job::new("prio_queue".to_string(), json!({"priority": "low"})).as_low_priority();
    let background =
        Job::new("prio_queue".to_string(), json!({"priority": "background"})).as_background();

    // Enqueue in random order
    let low_id = queue.enqueue(low).await.unwrap();
    let critical_id = queue.enqueue(critical).await.unwrap();
    let normal_id = queue.enqueue(normal).await.unwrap();
    let background_id = queue.enqueue(background).await.unwrap();
    let high_id = queue.enqueue(high).await.unwrap();

    // Dequeue should follow priority order
    assert_eq!(
        queue.dequeue("prio_queue").await.unwrap().unwrap().id,
        critical_id
    );
    assert_eq!(
        queue.dequeue("prio_queue").await.unwrap().unwrap().id,
        high_id
    );
    assert_eq!(
        queue.dequeue("prio_queue").await.unwrap().unwrap().id,
        normal_id
    );
    assert_eq!(
        queue.dequeue("prio_queue").await.unwrap().unwrap().id,
        low_id
    );
    assert_eq!(
        queue.dequeue("prio_queue").await.unwrap().unwrap().id,
        background_id
    );
}

#[tokio::test]
async fn test_weighted_priority_selection() {
    let queue = TestQueue::new();

    // Create weights that favor normal jobs
    let weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 10)
        .with_weight(JobPriority::High, 20)
        .with_weight(JobPriority::Normal, 50) // Heavily weighted
        .with_weight(JobPriority::Low, 15)
        .with_weight(JobPriority::Background, 5);

    // Enqueue multiple jobs of each priority
    for i in 0..5 {
        for priority in [
            JobPriority::Critical,
            JobPriority::High,
            JobPriority::Normal,
            JobPriority::Low,
            JobPriority::Background,
        ] {
            let mut job = Job::new(
                "weighted_queue".to_string(),
                json!({"index": i, "priority": format!("{:?}", priority)}),
            );
            job.priority = priority;
            queue.enqueue(job).await.unwrap();
        }
    }

    // Dequeue with weights and count priorities
    let mut priority_counts = HashMap::new();
    while let Some(job) = queue
        .dequeue_with_priority_weights("weighted_queue", &weights)
        .await
        .unwrap()
    {
        *priority_counts.entry(job.priority).or_insert(0) += 1;
        queue.complete_job(job.id).await.unwrap();
    }

    // Normal priority should have been selected more often due to higher weight
    assert_eq!(priority_counts[&JobPriority::Normal], 5);
}

#[tokio::test]
async fn test_delayed_jobs_with_time_control() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create jobs with different delays
    let immediate = Job::new("delayed_queue".to_string(), json!({"delay": "none"}));
    let delayed_1h = Job::with_delay(
        "delayed_queue".to_string(),
        json!({"delay": "1h"}),
        Duration::hours(1),
    );
    let delayed_2h = Job::with_delay(
        "delayed_queue".to_string(),
        json!({"delay": "2h"}),
        Duration::hours(2),
    );

    let immediate_id = queue.enqueue(immediate).await.unwrap();
    let delayed_1h_id = queue.enqueue(delayed_1h).await.unwrap();
    let delayed_2h_id = queue.enqueue(delayed_2h).await.unwrap();

    // Only immediate job should be available
    assert_eq!(
        queue.dequeue("delayed_queue").await.unwrap().unwrap().id,
        immediate_id
    );
    assert!(queue.dequeue("delayed_queue").await.unwrap().is_none());

    // Advance time by 1 hour
    clock.advance(Duration::hours(1));

    // Now 1h delayed job should be available
    assert_eq!(
        queue.dequeue("delayed_queue").await.unwrap().unwrap().id,
        delayed_1h_id
    );
    assert!(queue.dequeue("delayed_queue").await.unwrap().is_none());

    // Advance time by another hour
    clock.advance(Duration::hours(1));

    // Now 2h delayed job should be available
    assert_eq!(
        queue.dequeue("delayed_queue").await.unwrap().unwrap().id,
        delayed_2h_id
    );
}

#[tokio::test]
async fn test_job_retry_mechanism() {
    let queue = TestQueue::new();
    let clock = queue.clock();

    // Create a job with 3 max attempts
    let job = Job::new("retry_queue".to_string(), json!({"retry": "test"})).with_max_attempts(3);
    let job_id = queue.enqueue(job).await.unwrap();

    // First attempt - dequeue and fail
    let _dequeued = queue.dequeue("retry_queue").await.unwrap().unwrap();
    queue.fail_job(job_id, "First failure").await.unwrap();

    let job = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Retrying);
    assert_eq!(job.attempts, 1);

    // Schedule retry
    let retry_at = clock.now() + Duration::minutes(5);
    queue.retry_job(job_id, retry_at).await.unwrap();

    // Job should not be available immediately
    assert!(queue.dequeue("retry_queue").await.unwrap().is_none());

    // Advance time
    clock.advance(Duration::minutes(10));

    // Second attempt - fail again
    let dequeued = queue.dequeue("retry_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.id, job_id);
    queue.fail_job(job_id, "Second failure").await.unwrap();

    let job = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.attempts, 2);

    // Third attempt - fail again (should mark as dead)
    queue.retry_job(job_id, clock.now()).await.unwrap();
    queue.dequeue("retry_queue").await.unwrap();
    queue.fail_job(job_id, "Third failure").await.unwrap();

    let job = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Dead);
    assert_eq!(job.attempts, 3);
    assert!(job.failed_at.is_some());
}

#[tokio::test]
async fn test_batch_operations() {
    let queue = TestQueue::new();

    // Create a batch with mixed results
    let jobs = vec![
        Job::new(
            "batch_queue".to_string(),
            json!({"batch": 1, "will": "succeed"}),
        ),
        Job::new(
            "batch_queue".to_string(),
            json!({"batch": 2, "will": "succeed"}),
        ),
        Job::new(
            "batch_queue".to_string(),
            json!({"batch": 3, "will": "fail"}),
        ),
        Job::new(
            "batch_queue".to_string(),
            json!({"batch": 4, "will": "succeed"}),
        ),
    ];

    let batch = JobBatch::new("test_batch".to_string())
        .with_jobs(jobs)
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError);
    let batch_id = queue.enqueue_batch(batch).await.unwrap();

    // Process jobs
    for i in 0..4 {
        if let Some(job) = queue.dequeue("batch_queue").await.unwrap() {
            if i == 2 {
                // Fail the third job
                queue.fail_job(job.id, "Planned failure").await.unwrap();
                // Exhaust retries
                queue.fail_job(job.id, "Planned failure").await.unwrap();
                queue.fail_job(job.id, "Planned failure").await.unwrap();
            } else {
                queue.complete_job(job.id).await.unwrap();
            }
        }
    }

    // Check batch status
    let status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(status.total_jobs, 4);
    assert_eq!(status.completed_jobs, 3);
    assert_eq!(status.failed_jobs, 1);
    assert_eq!(status.status, BatchStatus::PartiallyFailed);
    assert!(status.error_summary.is_some());
    assert_eq!(status.job_errors.len(), 1);
}

#[tokio::test]
async fn test_batch_fail_fast() {
    let queue = TestQueue::new();

    // Create a batch with fail fast mode
    let jobs = vec![
        Job::new("failfast_queue".to_string(), json!({"index": 1})),
        Job::new("failfast_queue".to_string(), json!({"index": 2})),
        Job::new("failfast_queue".to_string(), json!({"index": 3})),
    ];

    let batch = JobBatch::new("failfast_batch".to_string())
        .with_jobs(jobs)
        .with_partial_failure_handling(PartialFailureMode::FailFast);
    let batch_id = queue.enqueue_batch(batch).await.unwrap();

    // Process first job successfully
    if let Some(job) = queue.dequeue("failfast_queue").await.unwrap() {
        queue.complete_job(job.id).await.unwrap();
    }

    // Fail second job
    if let Some(job) = queue.dequeue("failfast_queue").await.unwrap() {
        queue.fail_job(job.id, "Failure").await.unwrap();
        queue.fail_job(job.id, "Failure").await.unwrap();
        queue.fail_job(job.id, "Failure").await.unwrap();
    }

    // Check batch status - should be failed even though third job is pending
    let status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(status.status, BatchStatus::Failed);
}

#[tokio::test]
async fn test_workflow_with_dependencies() {
    let queue = TestQueue::new();

    // Create a diamond-shaped dependency graph:
    //     A
    //    / \
    //   B   C
    //    \ /
    //     D
    let job_a = Job::new("workflow_queue".to_string(), json!({"node": "A"}));
    let job_b = Job::new("workflow_queue".to_string(), json!({"node": "B"}));
    let job_c = Job::new("workflow_queue".to_string(), json!({"node": "C"}));
    let job_d = Job::new("workflow_queue".to_string(), json!({"node": "D"}));

    let id_a = job_a.id;
    let id_b = job_b.id;
    let id_c = job_c.id;
    let id_d = job_d.id;

    // Create workflow manually since complex dependencies aren't in the public API yet
    let mut workflow = JobGroup::new("diamond_workflow".to_string());
    workflow.jobs = vec![job_a, job_b, job_c, job_d];
    workflow.dependencies.insert(id_b, vec![id_a]);
    workflow.dependencies.insert(id_c, vec![id_a]);
    workflow.dependencies.insert(id_d, vec![id_b, id_c]);
    workflow.total_jobs = 4;

    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();

    // Only A should be available
    let dequeued = queue.dequeue("workflow_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.id, id_a);
    assert!(queue.dequeue("workflow_queue").await.unwrap().is_none());

    // Complete A
    queue.complete_job(id_a).await.unwrap();

    // B and C should now be available
    let mut dequeued_ids = vec![];
    dequeued_ids.push(queue.dequeue("workflow_queue").await.unwrap().unwrap().id);
    dequeued_ids.push(queue.dequeue("workflow_queue").await.unwrap().unwrap().id);
    assert!(dequeued_ids.contains(&id_b));
    assert!(dequeued_ids.contains(&id_c));
    assert!(queue.dequeue("workflow_queue").await.unwrap().is_none());

    // Complete B and C
    queue.complete_job(id_b).await.unwrap();
    queue.complete_job(id_c).await.unwrap();

    // D should now be available
    let dequeued = queue.dequeue("workflow_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.id, id_d);

    // Complete D
    queue.complete_job(id_d).await.unwrap();

    // Check workflow status
    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Completed);
    assert_eq!(status.completed_jobs, 4);
}

#[tokio::test]
async fn test_workflow_failure_policies() {
    let queue = TestQueue::new();

    // Test FailFast policy
    let job1 = Job::new("failfast_wf".to_string(), json!({"index": 1}));
    let job2 = Job::new("failfast_wf".to_string(), json!({"index": 2}));
    let job3 = Job::new("failfast_wf".to_string(), json!({"index": 3}));

    let mut workflow =
        JobGroup::new("failfast_workflow".to_string()).with_failure_policy(FailurePolicy::FailFast);
    workflow.jobs = vec![job1.clone(), job2.clone(), job3.clone()];
    workflow.total_jobs = 3;

    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();

    // Fail the first job
    let dequeued = queue.dequeue("failfast_wf").await.unwrap().unwrap();
    queue.fail_job(dequeued.id, "Test failure").await.unwrap();
    queue.fail_job(dequeued.id, "Test failure").await.unwrap();
    queue.fail_job(dequeued.id, "Test failure").await.unwrap();

    // Other jobs should have been failed due to fail-fast policy
    let failed_job_id = dequeued.id;

    // Check that the failed job is dead
    let failed_job_status = queue.get_job(failed_job_id).await.unwrap().unwrap();
    assert_eq!(failed_job_status.status, JobStatus::Dead);

    // Check that all other jobs are failed
    for job in [&job1, &job2, &job3] {
        if job.id != failed_job_id {
            let job_status = queue.get_job(job.id).await.unwrap().unwrap();
            assert_eq!(
                job_status.status,
                JobStatus::Failed,
                "Job {} should be Failed due to fail-fast policy",
                job.id
            );
        }
    }

    // Workflow should be failed
    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Failed);
}

#[tokio::test]
async fn test_cron_job_scheduling() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create a cron job that runs every hour at minute 0
    let mut cron_job = Job::new("cron_queue".to_string(), json!({"type": "hourly"}));
    cron_job.cron_schedule = Some("0 0 * * * *".to_string());
    cron_job.timezone = Some("UTC".to_string());

    let job_id = queue.enqueue_cron_job(cron_job).await.unwrap();

    // Job should be scheduled for the next hour
    let job = queue.get_job(job_id).await.unwrap().unwrap();
    assert!(job.recurring);
    assert!(job.next_run_at.is_some());

    // Process the first run
    clock.advance(Duration::hours(1));
    let dequeued = queue.dequeue("cron_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.id, job_id);
    queue.complete_job(job_id).await.unwrap();

    // Check for due cron jobs
    clock.advance(Duration::hours(1));
    let due_jobs = queue.get_due_cron_jobs(Some("cron_queue")).await.unwrap();
    assert_eq!(due_jobs.len(), 1);
    assert_eq!(due_jobs[0].id, job_id);

    // Reschedule for next run
    let next_run = clock.now() + Duration::hours(1);
    queue.reschedule_cron_job(job_id, next_run).await.unwrap();

    // Should have created a new job instance
    assert_eq!(
        queue.get_job_count("cron_queue", &JobStatus::Pending).await,
        1
    );

    // Disable the recurring job
    queue.disable_recurring_job(job_id).await.unwrap();
    let job = queue.get_job(job_id).await.unwrap().unwrap();
    assert!(!job.recurring);

    // Re-enable the recurring job
    queue.enable_recurring_job(job_id).await.unwrap();
    let job = queue.get_job(job_id).await.unwrap().unwrap();
    assert!(job.recurring);
    assert!(job.next_run_at.is_some());
}

#[tokio::test]
async fn test_dead_job_management() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create and fail multiple jobs
    let mut dead_job_ids = vec![];
    for i in 0..5 {
        let job = Job::new(format!("dead_queue_{}", i % 2), json!({"index": i}));
        let job_id = queue.enqueue(job).await.unwrap();

        // Dequeue and mark as dead
        queue
            .dequeue(&format!("dead_queue_{}", i % 2))
            .await
            .unwrap();
        queue
            .mark_job_dead(job_id, &format!("Dead job {}", i))
            .await
            .unwrap();
        dead_job_ids.push(job_id);

        // Advance time between jobs
        clock.advance(Duration::hours(1));
    }

    // Get dead jobs
    let dead_jobs = queue.get_dead_jobs(Some(3), Some(0)).await.unwrap();
    assert_eq!(dead_jobs.len(), 3);

    // Get dead jobs by queue
    let queue0_dead = queue
        .get_dead_jobs_by_queue("dead_queue_0", None, None)
        .await
        .unwrap();
    assert_eq!(queue0_dead.len(), 3); // Jobs 0, 2, 4

    let queue1_dead = queue
        .get_dead_jobs_by_queue("dead_queue_1", None, None)
        .await
        .unwrap();
    assert_eq!(queue1_dead.len(), 2); // Jobs 1, 3

    // Get dead job summary
    let summary = queue.get_dead_job_summary().await.unwrap();
    assert_eq!(summary.total_dead_jobs, 5);
    assert_eq!(summary.dead_jobs_by_queue.len(), 2);
    assert!(summary.oldest_dead_job.is_some());
    assert!(summary.newest_dead_job.is_some());

    // Retry a dead job
    let retry_id = dead_job_ids[0];
    queue.retry_dead_job(retry_id).await.unwrap();
    let retried = queue.get_job(retry_id).await.unwrap().unwrap();
    assert_eq!(retried.status, JobStatus::Pending);
    assert_eq!(retried.attempts, 0);

    // Purge old dead jobs
    let purge_before = clock.now() - Duration::hours(2);
    let purged = queue.purge_dead_jobs(purge_before).await.unwrap();
    assert_eq!(purged, 3); // Jobs older than 2 hours
}

#[tokio::test]
async fn test_timeout_handling() {
    let queue = TestQueue::new();

    // Create a job with timeout
    let job = Job::new("timeout_queue".to_string(), json!({"timeout": "test"}))
        .with_timeout(std::time::Duration::from_secs(60));
    let job_id = queue.enqueue(job).await.unwrap();

    // Dequeue and mark as timed out
    queue.dequeue("timeout_queue").await.unwrap();
    queue
        .mark_job_timed_out(job_id, "Operation timed out")
        .await
        .unwrap();

    let timed_out = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(timed_out.status, JobStatus::TimedOut);
    assert!(timed_out.timed_out_at.is_some());
    assert_eq!(
        timed_out.error_message,
        Some("Operation timed out".to_string())
    );
}

#[tokio::test]
async fn test_job_result_storage() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create and process a job
    let job = Job::new("result_queue".to_string(), json!({"compute": "fibonacci"}));
    let job_id = queue.enqueue(job).await.unwrap();

    queue.dequeue("result_queue").await.unwrap();
    queue.complete_job(job_id).await.unwrap();

    // Store job result with expiration
    let result = json!({
        "result": 233,
        "computation_time_ms": 42,
        "algorithm": "recursive"
    });
    let expires_at = clock.now() + Duration::hours(24);
    queue
        .store_job_result(job_id, result.clone(), Some(expires_at))
        .await
        .unwrap();

    // Retrieve result
    let retrieved = queue.get_job_result(job_id).await.unwrap().unwrap();
    assert_eq!(retrieved, result);

    // Advance time past expiration
    clock.advance(Duration::hours(25));

    // Result should be expired
    let expired = queue.get_job_result(job_id).await.unwrap();
    assert!(expired.is_none());

    // Clean up expired results
    let cleaned = queue.cleanup_expired_results().await.unwrap();
    assert_eq!(cleaned, 1);

    // Store result without expiration
    queue
        .store_job_result(job_id, result.clone(), None)
        .await
        .unwrap();

    // Should still be available after time advance
    clock.advance(Duration::days(365));
    let permanent = queue.get_job_result(job_id).await.unwrap().unwrap();
    assert_eq!(permanent, result);

    // Delete result
    queue.delete_job_result(job_id).await.unwrap();
    assert!(queue.get_job_result(job_id).await.unwrap().is_none());
}

#[tokio::test]
async fn test_throttle_configuration() {
    let queue = TestQueue::new();

    // Set throttle config
    let config = ThrottleConfig::new().max_concurrent(5).rate_per_minute(100);

    queue
        .set_throttle_config("throttled_queue", config.clone())
        .await
        .unwrap();

    // Get throttle config
    let retrieved = queue
        .get_throttle_config("throttled_queue")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.max_concurrent, Some(5));
    assert_eq!(retrieved.rate_per_minute, Some(100));

    // Get all configs
    let all_configs = queue.get_all_throttle_configs().await.unwrap();
    assert_eq!(all_configs.len(), 1);
    assert!(all_configs.contains_key("throttled_queue"));

    // Remove config
    queue
        .remove_throttle_config("throttled_queue")
        .await
        .unwrap();
    assert!(
        queue
            .get_throttle_config("throttled_queue")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn test_queue_statistics() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create a mix of jobs
    for i in 0..10 {
        let job = Job::new("stats_queue".to_string(), json!({"index": i}));
        queue.enqueue(job).await.unwrap();
    }

    // Process some jobs
    for i in 0..5 {
        if let Some(job) = queue.dequeue("stats_queue").await.unwrap() {
            // Simulate processing time
            clock.advance(Duration::milliseconds(100 + i * 50));

            if i < 3 {
                queue.complete_job(job.id).await.unwrap();
            } else {
                // Fail the last 2
                queue.fail_job(job.id, "Test failure").await.unwrap();
                queue.fail_job(job.id, "Test failure").await.unwrap();
                queue.fail_job(job.id, "Test failure").await.unwrap();
            }
        }
    }

    // Get queue stats
    let stats = queue.get_queue_stats("stats_queue").await.unwrap();
    assert_eq!(stats.pending_count, 5);
    assert_eq!(stats.statistics.completed, 3);
    assert_eq!(stats.statistics.failed, 2);
    assert_eq!(stats.statistics.total_processed, 5); // 3 completed + 2 failed
    assert!(stats.statistics.avg_processing_time_ms > 0.0);
    assert!(stats.statistics.min_processing_time_ms > 0);
    assert!(stats.statistics.max_processing_time_ms > 0);

    // Get job counts by status
    let counts = queue.get_job_counts_by_status("stats_queue").await.unwrap();
    assert_eq!(counts["pending"], 5);
    assert_eq!(counts["completed"], 3);
    assert_eq!(counts["dead"], 2);

    // Get processing times
    let since = clock.now() - Duration::hours(1);
    let times = queue
        .get_processing_times("stats_queue", since)
        .await
        .unwrap();
    assert_eq!(times.len(), 3);

    // Get error frequencies
    let errors = queue
        .get_error_frequencies(Some("stats_queue"), since)
        .await
        .unwrap();
    assert_eq!(errors["Test failure"], 2);

    // Get all queue stats
    let all_stats = queue.get_all_queue_stats().await.unwrap();
    assert!(all_stats.iter().any(|s| s.queue_name == "stats_queue"));
}

#[tokio::test]
async fn test_queue_depth() {
    let queue = TestQueue::new();

    // Initially empty
    assert_eq!(queue.get_queue_depth("depth_queue").await.unwrap(), 0);

    // Add jobs
    for i in 0..5 {
        let job = Job::new("depth_queue".to_string(), json!({"index": i}));
        queue.enqueue(job).await.unwrap();
    }

    // Check depth
    assert_eq!(queue.get_queue_depth("depth_queue").await.unwrap(), 5);

    // Process some jobs
    for _ in 0..2 {
        if let Some(job) = queue.dequeue("depth_queue").await.unwrap() {
            queue.complete_job(job.id).await.unwrap();
        }
    }

    // Depth should be reduced
    assert_eq!(queue.get_queue_depth("depth_queue").await.unwrap(), 3);
}

#[tokio::test]
async fn test_concurrent_operations() {
    let queue = TestQueue::new();

    // Spawn multiple tasks that enqueue jobs
    let mut handles = vec![];
    for i in 0..10 {
        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            let job = Job::new("concurrent_queue".to_string(), json!({"task": i}));
            queue_clone.enqueue(job).await
        });
        handles.push(handle);
    }

    // Wait for all enqueue operations
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Should have 10 jobs
    assert_eq!(
        queue
            .get_job_count("concurrent_queue", &JobStatus::Pending)
            .await,
        10
    );

    // Spawn multiple tasks that dequeue and process jobs
    let mut handles = vec![];
    for _ in 0..5 {
        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            if let Some(job) = queue_clone.dequeue("concurrent_queue").await.unwrap() {
                // Simulate processing
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                queue_clone.complete_job(job.id).await
            } else {
                Ok(())
            }
        });
        handles.push(handle);
    }

    // Wait for all processing
    for handle in handles {
        handle.await.unwrap().unwrap();
    }

    // Should have 5 completed and 5 pending
    assert_eq!(
        queue
            .get_job_count("concurrent_queue", &JobStatus::Completed)
            .await,
        5
    );
    assert_eq!(
        queue
            .get_job_count("concurrent_queue", &JobStatus::Pending)
            .await,
        5
    );
}

#[tokio::test]
async fn test_workflow_cancellation() {
    let queue = TestQueue::new();

    // Create a workflow with multiple jobs
    let jobs: Vec<Job> = (0..5)
        .map(|i| Job::new("cancel_queue".to_string(), json!({"index": i})))
        .collect();

    let mut workflow = JobGroup::new("cancel_workflow".to_string());
    workflow.jobs = jobs;
    workflow.total_jobs = 5;

    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();

    // Process one job
    if let Some(job) = queue.dequeue("cancel_queue").await.unwrap() {
        queue.complete_job(job.id).await.unwrap();
    }

    // Cancel the workflow
    queue.cancel_workflow(workflow_id).await.unwrap();

    // Check workflow status
    let status = queue
        .get_workflow_status(workflow_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(status.status, WorkflowStatus::Cancelled);

    // All pending jobs should be failed
    let workflow_jobs = queue.get_workflow_jobs(workflow_id).await.unwrap();
    let failed_count = workflow_jobs
        .iter()
        .filter(|j| {
            j.status == JobStatus::Failed
                && j.error_message == Some("Workflow cancelled".to_string())
        })
        .count();
    assert_eq!(failed_count, 4); // 5 total - 1 completed = 4 cancelled
}

#[tokio::test]
async fn test_get_all_jobs() {
    let queue = TestQueue::new();

    // Enqueue various jobs
    for i in 0..5 {
        let job = Job::new(format!("queue_{}", i % 2), json!({"index": i}));
        queue.enqueue(job).await.unwrap();
    }

    let all_jobs = queue.get_all_jobs().await;
    assert_eq!(all_jobs.len(), 5);

    // Process some jobs
    for _ in 0..2 {
        if let Some(job) = queue.dequeue("queue_0").await.unwrap() {
            queue.complete_job(job.id).await.unwrap();
        }
    }

    let all_jobs = queue.get_all_jobs().await;
    assert_eq!(all_jobs.len(), 5); // Still 5 jobs, just different statuses

    let completed_count = all_jobs
        .iter()
        .filter(|j| j.status == JobStatus::Completed)
        .count();
    assert_eq!(completed_count, 2);
}
