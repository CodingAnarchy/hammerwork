# Batch Operations

Hammerwork provides comprehensive batch operations for high-performance bulk job processing. This feature enables efficient enqueueing and processing of large numbers of jobs with minimal overhead.

## Table of Contents

- [Overview](#overview)
- [Creating Job Batches](#creating-job-batches)
- [Batch Configuration](#batch-configuration)
- [Failure Handling Modes](#failure-handling-modes)
- [Worker Batch Processing](#worker-batch-processing)
- [Monitoring Batch Progress](#monitoring-batch-progress)
- [Best Practices](#best-practices)

## Overview

Batch operations provide significant performance improvements when processing large numbers of jobs:

- **Bulk insertion**: Jobs are inserted in optimized bulk operations
- **Atomic operations**: Entire batches succeed or fail together
- **Progress tracking**: Monitor batch completion and success rates
- **Flexible failure handling**: Configure how batches handle individual job failures
- **Worker optimization**: Enhanced worker processing for batch jobs

## Creating Job Batches

### Basic Batch Creation

```rust
use hammerwork::{batch::{JobBatch, PartialFailureMode}, Job};
use serde_json::json;

// Create individual jobs
let jobs = vec![
    Job::new("email_queue".to_string(), json!({
        "to": "user1@example.com",
        "subject": "Welcome!"
    })),
    Job::new("email_queue".to_string(), json!({
        "to": "user2@example.com",
        "subject": "Newsletter"
    })),
    Job::new("email_queue".to_string(), json!({
        "to": "user3@example.com",
        "subject": "Updates"
    })),
];

// Create a batch
let batch = JobBatch::new("welcome_emails")
    .with_jobs(jobs)
    .with_batch_size(100)
    .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

// Enqueue the batch
let batch_id = queue.enqueue_batch(batch).await?;
```

### Building Batches Incrementally

```rust
let mut batch = JobBatch::new("data_processing");

// Add jobs one at a time
for file in files {
    let job = Job::new("process_queue".to_string(), json!({
        "file": file.path,
        "size": file.size
    }));
    batch = batch.add_job(job);
}

// Configure batch settings
batch = batch
    .with_batch_size(50)
    .with_partial_failure_handling(PartialFailureMode::CollectErrors)
    .with_metadata("department", "data_science")
    .with_metadata("priority", "high");
```

## Batch Configuration

### Batch Size

Control how jobs are chunked for processing:

```rust
let batch = JobBatch::new("large_batch")
    .with_jobs(jobs)
    .with_batch_size(100); // Process in chunks of 100
```

### Metadata

Attach metadata to batches for tracking and filtering:

```rust
let batch = JobBatch::new("campaign_emails")
    .with_jobs(jobs)
    .with_metadata("campaign_id", "summer_2024")
    .with_metadata("department", "marketing")
    .with_metadata("priority", "high");
```

### Job Priorities

Jobs within batches maintain their individual priorities:

```rust
let jobs = vec![
    Job::new("queue".to_string(), json!({"critical": true}))
        .as_critical(),
    Job::new("queue".to_string(), json!({"important": true}))
        .as_high_priority(),
    Job::new("queue".to_string(), json!({"regular": true}))
        .as_normal_priority(),
];

let batch = JobBatch::new("mixed_priority_batch")
    .with_jobs(jobs);
```

## Failure Handling Modes

Batches support three failure handling modes:

### ContinueOnError

Continue processing even if some jobs fail:

```rust
let batch = JobBatch::new("resilient_batch")
    .with_jobs(jobs)
    .with_partial_failure_handling(PartialFailureMode::ContinueOnError);
```

### FailFast

Stop processing immediately on first failure:

```rust
let batch = JobBatch::new("critical_batch")
    .with_jobs(jobs)
    .with_partial_failure_handling(PartialFailureMode::FailFast);
```

### CollectErrors

Collect all errors for analysis while continuing:

```rust
let batch = JobBatch::new("analytics_batch")
    .with_jobs(jobs)
    .with_partial_failure_handling(PartialFailureMode::CollectErrors);
```

## Worker Batch Processing

Workers can be configured to optimize batch job processing:

### Enable Batch Processing

```rust
use hammerwork::{Worker, worker::JobHandler};

let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Process job
        Ok(())
    })
});

let worker = Worker::new(queue, "batch_queue".to_string(), handler)
    .with_batch_processing_enabled(true)  // Enable batch optimizations
    .with_poll_interval(Duration::from_millis(100));
```

### Access Batch Statistics

```rust
// Get current batch processing statistics
let stats = worker.get_batch_stats();
println!("Batch jobs processed: {}", stats.jobs_processed);
println!("Batch success rate: {:.1}%", stats.success_rate() * 100.0);
println!("Average processing time: {:.1}ms", stats.average_processing_time_ms);
```

## Monitoring Batch Progress

### Check Batch Status

```rust
// Get current batch status
let batch_result = queue.get_batch_status(batch_id).await?;

println!("Total jobs: {}", batch_result.total_jobs);
println!("Pending: {}", batch_result.pending_jobs);
println!("Completed: {}", batch_result.completed_jobs);
println!("Failed: {}", batch_result.failed_jobs);
println!("Success rate: {:.1}%", batch_result.success_rate() * 100.0);
```

### List Batch Jobs

```rust
// Get all jobs in a batch
let batch_jobs = queue.get_batch_jobs(batch_id).await?;

for job in batch_jobs {
    println!("Job {}: Status={:?}, Priority={:?}", 
             job.id, job.status, job.priority);
}
```

### Monitor Completion

```rust
// Wait for batch completion
loop {
    let batch_result = queue.get_batch_status(batch_id).await?;
    
    if batch_result.pending_jobs == 0 {
        println!("Batch completed!");
        println!("Success rate: {:.1}%", batch_result.success_rate() * 100.0);
        break;
    }
    
    tokio::time::sleep(Duration::from_secs(1)).await;
}
```

## Best Practices

### 1. Choose Appropriate Batch Sizes

```rust
// Small batches for quick processing
let small_batch = JobBatch::new("quick_tasks")
    .with_batch_size(10);

// Large batches for bulk operations
let large_batch = JobBatch::new("bulk_import")
    .with_batch_size(1000);
```

### 2. Use Metadata for Tracking

```rust
let batch = JobBatch::new("user_notifications")
    .with_metadata("run_id", Uuid::new_v4().to_string())
    .with_metadata("triggered_by", "automated_system")
    .with_metadata("timestamp", Utc::now().to_rfc3339());
```

### 3. Handle Large Batches

For very large batches, consider chunking at the application level:

```rust
// Process 10,000 jobs in multiple batches
let all_jobs = generate_jobs(10_000);
let chunks = all_jobs.chunks(1000);

for (i, chunk) in chunks.enumerate() {
    let batch = JobBatch::new(&format!("large_batch_part_{}", i))
        .with_jobs(chunk.to_vec())
        .with_metadata("parent_batch", "large_batch")
        .with_metadata("part", i.to_string());
    
    queue.enqueue_batch(batch).await?;
}
```

### 4. Monitor Batch Performance

```rust
use hammerwork::StatisticsCollector;

// Track batch processing metrics
let stats = stats_collector.get_queue_statistics("batch_queue", Duration::from_hours(1)).await?;

println!("Batches processed: {}", stats.total_processed);
println!("Average batch size: {:.1}", stats.average_batch_size);
println!("Batch error rate: {:.1}%", stats.error_rate * 100.0);
```

### 5. Clean Up Completed Batches

```rust
// Delete batch data after processing
let batch_result = queue.get_batch_status(batch_id).await?;
if batch_result.pending_jobs == 0 {
    queue.delete_batch(batch_id).await?;
}
```

## Example: Complete Batch Processing Workflow

```rust
use hammerwork::{
    batch::{JobBatch, PartialFailureMode},
    Job, JobQueue, Worker, WorkerPool,
    queue::DatabaseQueue,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    let queue = Arc::new(JobQueue::new(pool));
    queue.create_tables().await?;

    // Create a batch of data processing jobs
    let mut jobs = Vec::new();
    for i in 0..100 {
        jobs.push(Job::new(
            "data_processing".to_string(),
            json!({
                "file_id": format!("file_{}", i),
                "operation": "transform"
            })
        ));
    }

    // Configure and enqueue batch
    let batch = JobBatch::new("daily_data_processing")
        .with_jobs(jobs)
        .with_batch_size(25)
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError)
        .with_metadata("date", "2024-01-15")
        .with_metadata("source", "automated_pipeline");

    let batch_id = queue.enqueue_batch(batch).await?;
    println!("Enqueued batch: {}", batch_id);

    // Create worker with batch processing enabled
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            // Simulate processing
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
    });

    let worker = Worker::new(queue.clone(), "data_processing".to_string(), handler)
        .with_batch_processing_enabled(true);

    // Start processing
    let mut pool = WorkerPool::new();
    pool.add_worker(worker);
    
    tokio::spawn(async move {
        pool.start().await.unwrap();
    });

    // Monitor progress
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        
        let status = queue.get_batch_status(batch_id).await?;
        println!("Progress: {}/{} jobs completed", 
                 status.completed_jobs, status.total_jobs);
        
        if status.pending_jobs == 0 {
            println!("Batch completed with {:.1}% success rate", 
                     status.success_rate() * 100.0);
            break;
        }
    }

    // Cleanup
    queue.delete_batch(batch_id).await?;
    
    Ok(())
}
```

## Performance Considerations

### Database-Specific Optimizations

**PostgreSQL**: Uses `UNNEST` for optimal bulk insertions
```sql
INSERT INTO hammerwork_jobs (...) 
SELECT * FROM UNNEST($1::uuid[], $2::text[], ...)
```

**MySQL**: Uses multi-row `VALUES` with automatic chunking
```sql
INSERT INTO hammerwork_jobs (...) 
VALUES (?, ?, ...), (?, ?, ...), ...
```

### Memory Usage

Batches are processed in configurable chunks to manage memory:

```rust
// Limit memory usage with smaller batch sizes
let batch = JobBatch::new("memory_conscious")
    .with_jobs(large_job_list)
    .with_batch_size(50); // Process 50 at a time
```

### Network Overhead

Batch operations significantly reduce network round trips:

- Individual enqueue: 1000 jobs = 1000 network calls
- Batch enqueue: 1000 jobs = 1-10 network calls (depending on chunk size)

## Troubleshooting

### Batch Validation Errors

```rust
// Error: Batch cannot be empty
let empty_batch = JobBatch::new("empty");
// This will error when enqueuing

// Error: Jobs must have same queue name
let mixed_jobs = vec![
    Job::new("queue1".to_string(), json!({})),
    Job::new("queue2".to_string(), json!({})),
];
// This will error during validation
```

### Large Batch Handling

```rust
// Error: Batch too large (>10,000 jobs)
// Solution: Split into multiple batches
let batches = large_jobs
    .chunks(5000)
    .map(|chunk| JobBatch::new("large").with_jobs(chunk.to_vec()));
```

### Performance Issues

If batch processing is slow:

1. Check batch size configuration
2. Monitor database performance
3. Ensure workers have batch processing enabled
4. Review job handler efficiency
5. Consider parallel worker pools