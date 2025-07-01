# TestQueue - In-Memory Testing Framework

The `TestQueue` provides a complete in-memory implementation of the `DatabaseQueue` trait, enabling fast, deterministic unit testing without requiring database connections. It includes time manipulation capabilities through `MockClock` for testing time-dependent features like delayed jobs, cron schedules, and timeouts.

## Table of Contents

- [Quick Start](#quick-start)
- [Features](#features)
- [Basic Usage](#basic-usage)
- [Time Control with MockClock](#time-control-with-mockclock)
- [Testing Delayed Jobs](#testing-delayed-jobs)
- [Testing Cron Jobs](#testing-cron-jobs)
- [Testing Job Priorities](#testing-job-priorities)
- [Testing Batch Operations](#testing-batch-operations)
- [Testing Workflows](#testing-workflows)
- [Testing Error Scenarios](#testing-error-scenarios)
- [Best Practices](#best-practices)
- [Complete Examples](#complete-examples)

## Quick Start

Add TestQueue to your test dependencies:

```toml
[dependencies]
hammerwork = { version = "1.2", features = ["test"] }

[dev-dependencies]
tokio-test = "0.4"
```

Basic test example:

```rust
use hammerwork::queue::test::TestQueue;
use hammerwork::{Job, queue::DatabaseQueue};
use serde_json::json;

#[tokio::test]
async fn test_job_processing() {
    let queue = TestQueue::new();
    
    // Enqueue a job
    let job = Job::new("test_queue".to_string(), json!({"task": "test"}));
    let job_id = queue.enqueue(job).await.unwrap();
    
    // Dequeue and process
    let dequeued_job = queue.dequeue("test_queue").await.unwrap().unwrap();
    assert_eq!(dequeued_job.id, job_id);
    
    // Complete the job
    queue.complete_job(job_id).await.unwrap();
    
    // Verify job is completed
    let completed_job = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(completed_job.status, JobStatus::Completed);
}
```

## Features

- **In-Memory Operations**: All operations performed in memory for speed
- **Thread-Safe**: Full async/await support with proper synchronization
- **Time Control**: MockClock for deterministic testing of time-dependent features
- **Full DatabaseQueue Compatibility**: Drop-in replacement for database implementations
- **Job Logic Testing**: Ideal for testing job processing logic, queue operations, and business workflows
- **Comprehensive Testing**: Support for all Hammerwork features including:
  - Job priorities and weighted scheduling
  - Delayed job execution
  - Cron job scheduling
  - Batch operations with failure modes
  - Workflow dependencies and cancellation
  - Retry logic and dead job handling
  - Result storage with TTL
  - Queue statistics and monitoring

## Basic Usage

### Creating a TestQueue

```rust
use hammerwork::queue::test::{TestQueue, MockClock};

// Basic TestQueue
let queue = TestQueue::new();

// TestQueue with custom MockClock
let clock = MockClock::new();
let queue = TestQueue::with_clock(clock.clone());
```

### Enqueuing and Dequeuing Jobs

```rust
use hammerwork::{Job, JobPriority, queue::DatabaseQueue};
use serde_json::json;

#[tokio::test]
async fn test_job_lifecycle() {
    let queue = TestQueue::new();
    
    // Create job with priority
    let job = Job::new("priority_queue".to_string(), json!({"priority": "high"}))
        .with_priority(JobPriority::High);
    
    let job_id = queue.enqueue(job).await.unwrap();
    
    // Dequeue job
    let dequeued = queue.dequeue("priority_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.priority, JobPriority::High);
    
    // Complete job
    queue.complete_job(job_id).await.unwrap();
}
```

### Testing Job Counts and Statistics

```rust
#[tokio::test]
async fn test_queue_statistics() {
    let queue = TestQueue::new();
    
    // Check initial state
    assert_eq!(queue.get_job_count("test_queue").await.unwrap(), 0);
    
    // Enqueue jobs
    let job1 = Job::new("test_queue".to_string(), json!({"id": 1}));
    let job2 = Job::new("test_queue".to_string(), json!({"id": 2}));
    
    queue.enqueue(job1).await.unwrap();
    queue.enqueue(job2).await.unwrap();
    
    assert_eq!(queue.get_job_count("test_queue").await.unwrap(), 2);
    
    // Get all jobs
    let all_jobs = queue.get_all_jobs().await.unwrap();
    assert_eq!(all_jobs.len(), 2);
}
```

## Time Control with MockClock

The `MockClock` enables deterministic testing of time-dependent features:

### Basic Time Manipulation

```rust
use hammerwork::queue::test::MockClock;
use chrono::Duration;

#[tokio::test]
async fn test_time_control() {
    let clock = MockClock::new();
    let initial_time = clock.now();
    
    // Advance time by 2 hours
    clock.advance(Duration::hours(2));
    
    let after_advance = clock.now();
    assert_eq!((after_advance - initial_time).num_hours(), 2);
    
    // Set specific time
    let specific_time = chrono::Utc::now() + Duration::days(1);
    clock.set(specific_time);
    assert_eq!(clock.now(), specific_time);
}
```

### Time Control in Job Processing

```rust
#[tokio::test]
async fn test_time_dependent_processing() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Create job that should process "later"
    let future_time = clock.now() + Duration::hours(1);
    let job = Job::new("delayed_queue".to_string(), json!({"delayed": true}))
        .with_scheduled_at(future_time);
    
    queue.enqueue(job).await.unwrap();
    
    // Job shouldn't be available yet
    assert!(queue.dequeue("delayed_queue").await.unwrap().is_none());
    
    // Advance time past scheduled time
    clock.advance(Duration::hours(2));
    
    // Now job should be available
    let dequeued = queue.dequeue("delayed_queue").await.unwrap();
    assert!(dequeued.is_some());
}
```

## Testing Delayed Jobs

```rust
use chrono::Duration;

#[tokio::test]
async fn test_delayed_job_execution() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Schedule job for 30 minutes from now
    let delay = Duration::minutes(30);
    let scheduled_time = clock.now() + delay;
    
    let job = Job::new("delayed_queue".to_string(), json!({"data": "delayed_payload"}))
        .with_scheduled_at(scheduled_time);
    
    queue.enqueue(job).await.unwrap();
    
    // Verify job is not available immediately
    assert!(queue.dequeue("delayed_queue").await.unwrap().is_none());
    
    // Advance time to just before scheduled time
    clock.advance(Duration::minutes(29));
    assert!(queue.dequeue("delayed_queue").await.unwrap().is_none());
    
    // Advance past scheduled time
    clock.advance(Duration::minutes(2));
    let dequeued = queue.dequeue("delayed_queue").await.unwrap().unwrap();
    assert_eq!(dequeued.payload["data"], "delayed_payload");
}
```

## Testing Cron Jobs

```rust
#[tokio::test]
async fn test_cron_job_scheduling() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Create hourly cron job (every hour at minute 0)
    let cron_job = Job::new("cron_queue".to_string(), json!({"task": "hourly_report"}))
        .with_cron_schedule("0 0 * * * *".to_string()) // 6-field format with seconds
        .with_timezone("UTC".to_string());
    
    queue.enqueue(cron_job).await.unwrap();
    
    // Process the first occurrence
    let first_run = queue.dequeue("cron_queue").await.unwrap().unwrap();
    queue.complete_job(first_run.id).await.unwrap();
    
    // Advance time by 59 minutes - next occurrence shouldn't be ready
    clock.advance(Duration::minutes(59));
    assert!(queue.dequeue("cron_queue").await.unwrap().is_none());
    
    // Advance to the next hour - next occurrence should be ready
    clock.advance(Duration::minutes(2));
    let second_run = queue.dequeue("cron_queue").await.unwrap();
    assert!(second_run.is_some());
}
```

## Testing Job Priorities

```rust
use hammerwork::{JobPriority, priority::PriorityWeights};

#[tokio::test]
async fn test_priority_ordering() {
    let queue = TestQueue::new();
    
    // Enqueue jobs with different priorities
    let low_job = Job::new("priority_queue".to_string(), json!({"priority": "low"}))
        .with_priority(JobPriority::Low);
    let high_job = Job::new("priority_queue".to_string(), json!({"priority": "high"}))
        .with_priority(JobPriority::High);
    let critical_job = Job::new("priority_queue".to_string(), json!({"priority": "critical"}))
        .with_priority(JobPriority::Critical);
    
    queue.enqueue(low_job).await.unwrap();
    queue.enqueue(high_job).await.unwrap();
    queue.enqueue(critical_job).await.unwrap();
    
    // Jobs should be dequeued in priority order (highest first)
    let first = queue.dequeue("priority_queue").await.unwrap().unwrap();
    assert_eq!(first.priority, JobPriority::Critical);
    
    let second = queue.dequeue("priority_queue").await.unwrap().unwrap();
    assert_eq!(second.priority, JobPriority::High);
    
    let third = queue.dequeue("priority_queue").await.unwrap().unwrap();
    assert_eq!(third.priority, JobPriority::Low);
}
```

### Testing Weighted Priority Selection

```rust
#[tokio::test]
async fn test_weighted_priority_selection() {
    let queue = TestQueue::new();
    
    // Create custom priority weights favoring background jobs for testing
    let weights = PriorityWeights::new()
        .with_background(50)
        .with_low(30)
        .with_normal(15)
        .with_high(4)
        .with_critical(1);
    
    // Enqueue jobs with different priorities
    for _ in 0..10 {
        queue.enqueue(
            Job::new("weighted_queue".to_string(), json!({"type": "background"}))
                .with_priority(JobPriority::Background)
        ).await.unwrap();
        
        queue.enqueue(
            Job::new("weighted_queue".to_string(), json!({"type": "critical"}))
                .with_priority(JobPriority::Critical)
        ).await.unwrap();
    }
    
    // Use weighted dequeue
    let job = queue.dequeue_with_priority_weights("weighted_queue", &weights).await.unwrap();
    assert!(job.is_some());
    
    // With these weights, background jobs should be selected more frequently
    let dequeued = job.unwrap();
    // Note: Weighted selection is probabilistic, so we can't assert specific priority
    // but we can verify the job was dequeued correctly
    assert!(matches!(dequeued.priority, JobPriority::Background | JobPriority::Critical));
}
```

## Testing Batch Operations

```rust
use hammerwork::batch::{PartialFailureMode, BatchId};

#[tokio::test]
async fn test_batch_operations() {
    let queue = TestQueue::new();
    
    // Create batch of jobs
    let jobs = vec![
        Job::new("batch_queue".to_string(), json!({"id": 1})),
        Job::new("batch_queue".to_string(), json!({"id": 2})),
        Job::new("batch_queue".to_string(), json!({"id": 3})),
    ];
    
    let batch_result = queue.enqueue_batch(
        jobs,
        Some(PartialFailureMode::ContinueOnError)
    ).await.unwrap();
    
    assert_eq!(batch_result.successful_jobs.len(), 3);
    assert!(batch_result.failed_jobs.is_empty());
    
    // Process all batch jobs
    for _ in 0..3 {
        if let Some(job) = queue.dequeue("batch_queue").await.unwrap() {
            queue.complete_job(job.id).await.unwrap();
        }
    }
    
    // Get batch status
    let batch_id = batch_result.successful_jobs[0].batch_id.unwrap();
    let batch_status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(batch_status.completed_count, 3);
}
```

### Testing Batch Failure Modes

```rust
#[tokio::test]
async fn test_batch_fail_fast() {
    let queue = TestQueue::new();
    
    let jobs = vec![
        Job::new("fail_fast_queue".to_string(), json!({"id": 1})),
        Job::new("fail_fast_queue".to_string(), json!({"id": 2})),
        Job::new("fail_fast_queue".to_string(), json!({"id": 3})),
    ];
    
    let batch_result = queue.enqueue_batch(
        jobs,
        Some(PartialFailureMode::FailFast)
    ).await.unwrap();
    
    let batch_id = batch_result.successful_jobs[0].batch_id.unwrap();
    
    // Process first job and fail it
    let first_job = queue.dequeue("fail_fast_queue").await.unwrap().unwrap();
    queue.fail_job(first_job.id, "Intentional failure".to_string()).await.unwrap();
    
    // With FailFast, remaining jobs should be automatically failed
    let batch_status = queue.get_batch_status(batch_id).await.unwrap();
    assert_eq!(batch_status.failed_count, 3); // All jobs failed due to fail-fast
}
```

## Testing Workflows

```rust
use hammerwork::workflow::{JobGroup, FailurePolicy};

#[tokio::test]
async fn test_workflow_dependencies() {
    let queue = TestQueue::new();
    
    // Create workflow with dependencies: job1 → job2 → job3
    let job1 = Job::new("workflow_queue".to_string(), json!({"step": 1}));
    let job2 = Job::new("workflow_queue".to_string(), json!({"step": 2}))
        .depends_on(&job1.id);
    let job3 = Job::new("workflow_queue".to_string(), json!({"step": 3}))
        .depends_on(&job2.id);
    
    let workflow = JobGroup::new("test_workflow")
        .add_job(job1.clone())
        .add_job(job2.clone())
        .add_job(job3.clone())
        .with_failure_policy(FailurePolicy::ContinueOnFailure);
    
    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();
    
    // Only job1 should be available initially (no dependencies)
    let first_available = queue.dequeue("workflow_queue").await.unwrap().unwrap();
    assert_eq!(first_available.payload["step"], 1);
    
    // Complete job1
    queue.complete_job(first_available.id).await.unwrap();
    
    // Now job2 should be available
    let second_available = queue.dequeue("workflow_queue").await.unwrap().unwrap();
    assert_eq!(second_available.payload["step"], 2);
    
    // Complete job2
    queue.complete_job(second_available.id).await.unwrap();
    
    // Now job3 should be available
    let third_available = queue.dequeue("workflow_queue").await.unwrap().unwrap();
    assert_eq!(third_available.payload["step"], 3);
    
    // Complete job3
    queue.complete_job(third_available.id).await.unwrap();
    
    // Workflow should be complete
    let status = queue.get_workflow_status(workflow_id).await.unwrap();
    assert_eq!(status, WorkflowStatus::Completed);
}
```

### Testing Workflow Failure Policies

```rust
#[tokio::test]
async fn test_workflow_fail_fast() {
    let queue = TestQueue::new();
    
    let job1 = Job::new("fail_fast_workflow".to_string(), json!({"step": 1}));
    let job2 = Job::new("fail_fast_workflow".to_string(), json!({"step": 2}));
    let job3 = Job::new("fail_fast_workflow".to_string(), json!({"step": 3}));
    
    let workflow = JobGroup::new("fail_fast_test")
        .add_job(job1.clone())
        .add_job(job2.clone()) 
        .add_job(job3.clone())
        .with_failure_policy(FailurePolicy::FailFast);
    
    let workflow_id = queue.enqueue_workflow(workflow).await.unwrap();
    
    // Dequeue any job and fail it
    let job = queue.dequeue("fail_fast_workflow").await.unwrap().unwrap();
    queue.fail_job(job.id, "Intentional failure for testing".to_string()).await.unwrap();
    
    // With FailFast policy, the workflow should be marked as failed
    let status = queue.get_workflow_status(workflow_id).await.unwrap();
    assert_eq!(status, WorkflowStatus::Failed);
}
```

## Testing Error Scenarios

### Testing Job Retries

```rust
#[tokio::test]
async fn test_job_retry_logic() {
    let queue = TestQueue::new();
    
    // Create job with 3 max attempts
    let job = Job::new("retry_queue".to_string(), json!({"retry_test": true}))
        .with_max_attempts(3);
    
    let job_id = queue.enqueue(job).await.unwrap();
    
    // First attempt
    let attempt1 = queue.dequeue("retry_queue").await.unwrap().unwrap();
    assert_eq!(attempt1.attempts, 0);
    queue.fail_job(job_id, "First failure".to_string()).await.unwrap();
    
    // Second attempt
    let attempt2 = queue.dequeue("retry_queue").await.unwrap().unwrap();
    assert_eq!(attempt2.attempts, 1);
    queue.fail_job(job_id, "Second failure".to_string()).await.unwrap();
    
    // Third attempt
    let attempt3 = queue.dequeue("retry_queue").await.unwrap().unwrap();
    assert_eq!(attempt3.attempts, 2);
    queue.fail_job(job_id, "Third failure".to_string()).await.unwrap();
    
    // Fourth failure should make job dead (no more retries)
    assert!(queue.dequeue("retry_queue").await.unwrap().is_none());
    
    // Verify job is dead
    let final_job = queue.get_job(job_id).await.unwrap().unwrap();
    assert_eq!(final_job.status, JobStatus::Dead);
}
```

### Testing Dead Job Management

```rust
use chrono::Duration;

#[tokio::test]
async fn test_dead_job_purging() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Create and fail jobs to make them dead
    for i in 0..5 {
        let job = Job::new("purge_queue".to_string(), json!({"id": i}))
            .with_max_attempts(1);
        let job_id = queue.enqueue(job).await.unwrap();
        
        let dequeued = queue.dequeue("purge_queue").await.unwrap().unwrap();
        queue.fail_job(job_id, "Failed for purge test".to_string()).await.unwrap();
    }
    
    // Advance time to make jobs eligible for purging
    clock.advance(Duration::days(8)); // Beyond default purge threshold
    
    // Purge dead jobs older than 7 days
    let purged_count = queue.purge_dead_jobs(Duration::days(7)).await.unwrap();
    assert_eq!(purged_count, 5);
    
    // Verify jobs are purged
    let remaining_jobs = queue.get_all_jobs().await.unwrap();
    assert!(remaining_jobs.is_empty());
}
```

## TestQueue Limitations

### Worker Compatibility

**Important**: TestQueue is designed for testing job logic and queue operations, **not** for testing Worker functionality. Workers (`Worker<DB>`) are tightly coupled to `JobQueue<DB>` and cannot be directly used with TestQueue.

#### What TestQueue is for:
- Testing job enqueue/dequeue logic
- Testing job priority and scheduling
- Testing workflow dependencies and failure policies
- Testing batch operations
- Testing cron job scheduling
- Testing business logic that interacts with the queue

#### What TestQueue is NOT for:
- Testing Worker behavior (polling, retries, error handling)
- Testing WorkerPool functionality
- Testing worker autoscaling
- Testing worker-specific configurations

#### Testing Job Processing Logic

Instead of using Workers with TestQueue, manually dequeue and process jobs to test your job handler logic:

```rust
#[tokio::test]
async fn test_job_processing_logic() {
    let queue = TestQueue::new();
    
    // Enqueue a job
    let job = Job::new("test_queue".to_string(), json!({"task": "process_data"}));
    queue.enqueue(job).await.unwrap();
    
    // Manually dequeue and test your processing logic
    if let Some(job) = queue.dequeue("test_queue").await.unwrap() {
        // This is where you test your actual job processing logic
        let result = your_job_handler(job.clone()).await;
        assert!(result.is_ok());
        
        // Complete the job
        queue.complete_job(job.id).await.unwrap();
    }
}
```

For testing Worker behavior, use a real database connection with `JobQueue<DB>` in integration tests.

## Best Practices

### 1. Use Descriptive Queue Names

```rust
// Good: Descriptive queue names prevent test interference
let queue = TestQueue::new();
queue.enqueue(Job::new("user_registration_queue".to_string(), data)).await?;
queue.enqueue(Job::new("email_notification_queue".to_string(), data)).await?;

// Avoid: Generic names that might clash between tests
queue.enqueue(Job::new("test".to_string(), data)).await?;
```

### 2. Test Time-Dependent Features with MockClock

```rust
#[tokio::test]
async fn test_delayed_processing() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Always use MockClock for time-dependent tests
    let future_time = clock.now() + Duration::hours(1);
    let job = Job::new("delayed".to_string(), data)
        .with_scheduled_at(future_time);
    
    queue.enqueue(job).await.unwrap();
    
    // Test before and after the scheduled time
    assert!(queue.dequeue("delayed").await.unwrap().is_none());
    clock.advance(Duration::hours(2));
    assert!(queue.dequeue("delayed").await.unwrap().is_some());
}
```

### 3. Test Edge Cases and Error Conditions

```rust
#[tokio::test]
async fn test_edge_cases() {
    let queue = TestQueue::new();
    
    // Test empty queue
    assert!(queue.dequeue("nonexistent").await.unwrap().is_none());
    
    // Test invalid job operations
    let fake_id = JobId::new();
    assert!(queue.get_job(fake_id).await.unwrap().is_none());
    
    // Test boundary conditions
    let job_with_zero_retries = Job::new("zero_retry".to_string(), json!({}))
        .with_max_attempts(0);
    // This should fail immediately without retries
}
```

### 4. Isolate Tests with Fresh TestQueue Instances

```rust
// Each test should create its own TestQueue to avoid interference
#[tokio::test]
async fn test_job_completion() {
    let queue = TestQueue::new(); // Fresh instance per test
    // ... test logic
}

#[tokio::test]  
async fn test_job_failure() {
    let queue = TestQueue::new(); // Another fresh instance
    // ... test logic
}
```

### 5. Test Realistic Scenarios

```rust
#[tokio::test]
async fn test_realistic_workflow() {
    let queue = TestQueue::new();
    
    // Test realistic business workflow
    let order_processing = Job::new("orders".to_string(), json!({
        "order_id": "12345",
        "customer_id": "user_789",
        "items": ["item1", "item2"]
    }));
    
    let payment_processing = Job::new("payments".to_string(), json!({
        "order_id": "12345",
        "amount": 99.99,
        "method": "credit_card"
    })).depends_on(&order_processing.id);
    
    let email_notification = Job::new("notifications".to_string(), json!({
        "order_id": "12345",
        "template": "order_confirmation",
        "recipient": "customer@example.com"
    })).depends_on(&payment_processing.id);
    
    // Test the complete workflow
    let workflow = JobGroup::new("order_fulfillment")
        .add_job(order_processing)
        .add_job(payment_processing)
        .add_job(email_notification);
    
    queue.enqueue_workflow(workflow).await.unwrap();
    // ... test workflow execution
}
```

## Complete Examples

### Testing a Job Processing Service

```rust
use hammerwork::queue::test::{TestQueue, MockClock};
use hammerwork::{Job, JobStatus, JobPriority, queue::DatabaseQueue};
use serde_json::json;
use chrono::Duration;

#[tokio::test]
async fn test_email_service() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Test immediate email
    let urgent_email = Job::new("email_queue".to_string(), json!({
        "to": "urgent@example.com",
        "subject": "Urgent: System Alert",
        "template": "system_alert"
    })).with_priority(JobPriority::Critical);
    
    let urgent_id = queue.enqueue(urgent_email).await.unwrap();
    
    // Test scheduled email (newsletter)
    let newsletter_time = clock.now() + Duration::hours(24);
    let newsletter = Job::new("email_queue".to_string(), json!({
        "to": "subscriber@example.com", 
        "subject": "Weekly Newsletter",
        "template": "newsletter"
    }))
    .with_scheduled_at(newsletter_time)
    .with_priority(JobPriority::Low);
    
    queue.enqueue(newsletter).await.unwrap();
    
    // Process urgent email immediately
    let urgent_job = queue.dequeue("email_queue").await.unwrap().unwrap();
    assert_eq!(urgent_job.id, urgent_id);
    assert_eq!(urgent_job.priority, JobPriority::Critical);
    queue.complete_job(urgent_id).await.unwrap();
    
    // Newsletter shouldn't be available yet
    assert!(queue.dequeue("email_queue").await.unwrap().is_none());
    
    // Advance time to newsletter time
    clock.advance(Duration::hours(25));
    
    // Newsletter should now be available
    let newsletter_job = queue.dequeue("email_queue").await.unwrap().unwrap();
    assert_eq!(newsletter_job.payload["template"], "newsletter");
    
    // Simulate email sending failure and retry
    queue.fail_job(newsletter_job.id, "SMTP server unavailable".to_string()).await.unwrap();
    
    // Job should be available for retry
    let retry_job = queue.dequeue("email_queue").await.unwrap().unwrap();
    assert_eq!(retry_job.attempts, 1);
    queue.complete_job(retry_job.id).await.unwrap();
}
```

### Testing Data Processing Pipeline

```rust
use hammerwork::workflow::{JobGroup, FailurePolicy};

#[tokio::test]
async fn test_data_pipeline() {
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());
    
    // Create data processing pipeline
    let extract_job = Job::new("data_pipeline".to_string(), json!({
        "stage": "extract",
        "source": "s3://raw-data/batch-001.csv"
    }));
    
    let transform_job = Job::new("data_pipeline".to_string(), json!({
        "stage": "transform",
        "operations": ["normalize", "validate", "enrich"]
    })).depends_on(&extract_job.id);
    
    let load_job = Job::new("data_pipeline".to_string(), json!({
        "stage": "load", 
        "destination": "warehouse.processed_data"
    })).depends_on(&transform_job.id);
    
    let notify_job = Job::new("notifications".to_string(), json!({
        "stage": "notify",
        "message": "Data pipeline completed successfully"
    })).depends_on(&load_job.id);
    
    let pipeline = JobGroup::new("daily_data_processing")
        .add_job(extract_job.clone())
        .add_job(transform_job.clone())
        .add_job(load_job.clone())
        .add_job(notify_job.clone())
        .with_failure_policy(FailurePolicy::FailFast);
    
    let workflow_id = queue.enqueue_workflow(pipeline).await.unwrap();
    
    // Process extract stage
    let extract = queue.dequeue("data_pipeline").await.unwrap().unwrap();
    assert_eq!(extract.payload["stage"], "extract");
    queue.complete_job(extract.id).await.unwrap();
    
    // Process transform stage
    let transform = queue.dequeue("data_pipeline").await.unwrap().unwrap();
    assert_eq!(transform.payload["stage"], "transform");
    queue.complete_job(transform.id).await.unwrap();
    
    // Process load stage
    let load = queue.dequeue("data_pipeline").await.unwrap().unwrap();
    assert_eq!(load.payload["stage"], "load");
    queue.complete_job(load.id).await.unwrap();
    
    // Process notification
    let notify = queue.dequeue("notifications").await.unwrap().unwrap();
    assert_eq!(notify.payload["stage"], "notify");
    queue.complete_job(notify.id).await.unwrap();
    
    // Verify workflow completion
    let status = queue.get_workflow_status(workflow_id).await.unwrap();
    assert_eq!(status, WorkflowStatus::Completed);
}
```

This comprehensive testing framework enables you to thoroughly test your Hammerwork-based applications without requiring database connections, while maintaining full feature compatibility and deterministic behavior through time control.