# Batch Operations

Hammerwork supports batch operations for bulk job processing: a `JobBatch` is inserted in a single transaction, tracked as a unit, and configured with a failure handling mode.

## Table of Contents

- [Overview](#overview)
- [Creating Job Batches](#creating-job-batches)
- [Batch Configuration](#batch-configuration)
- [Failure Handling Modes](#failure-handling-modes)
- [Worker Batch Processing](#worker-batch-processing)
- [Monitoring Batch Progress](#monitoring-batch-progress)
- [CLI Batch Commands](#cli-batch-commands)
- [Best Practices](#best-practices)

## Overview

Batch operations provide:

- **Bulk insertion**: all jobs of a batch are inserted in one transaction instead of one round trip each
- **Atomic enqueue**: a batch is stored completely or not at all
- **Progress tracking**: `get_batch_status` reports pending, completed and failed counts
- **Flexible failure handling**: choose what happens when a job of the batch fails
- **Batch claims**: workers can claim many jobs per round trip and keep per-batch processing statistics

A batch holds up to 10,000 jobs, all on the **same queue**.

## Creating Job Batches

### Basic Batch Creation

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::{JobBatch, PartialFailureMode}, Job, queue::DatabaseQueue};
use serde_json::json;

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

let batch = JobBatch::new("welcome_emails")
    .with_jobs(jobs)
    .with_batch_size(100)
    .with_partial_failure_handling(PartialFailureMode::ContinueOnError);

assert_eq!(batch.job_count(), 3);

let batch_id = queue.enqueue_batch(batch).await?;
# Ok(())
# }
```

### Building Batches Incrementally

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::{JobBatch, PartialFailureMode}, Job};
use serde_json::json;

let files = vec![("a.csv", 10u64), ("b.csv", 20)];

let mut batch = JobBatch::new("data_processing");

for (path, size) in files {
    let job = Job::new("process_queue".to_string(), json!({
        "file": path,
        "size": size
    }));
    batch = batch.add_job(job);
}

batch = batch
    .with_partial_failure_handling(PartialFailureMode::CollectErrors)
    .with_metadata("department", "data_science")
    .with_metadata("priority", "high");
# Ok(())
# }
```

## Batch Configuration

### Batch Size

`with_batch_size` records a chunk size on the batch. `enqueue_batch` itself inserts every job of the batch in one transaction; the chunk size is used by `JobBatch::into_chunks`, which splits a batch into several smaller batches (named `<name>_chunk_<n>`, default chunk size 1000) that you can then enqueue one by one:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::JobBatch, queue::DatabaseQueue};

let batch = JobBatch::new("large_batch")
    .with_jobs(jobs)
    .with_batch_size(100);

for chunk in batch.into_chunks() {
    queue.enqueue_batch(chunk).await?;
}
# Ok(())
# }
```

### Metadata

Attach string metadata to batches for tracking:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::batch::JobBatch;

let batch = JobBatch::new("campaign_emails")
    .with_jobs(jobs)
    .with_metadata("campaign_id", "summer_2024")
    .with_metadata("department", "marketing")
    .with_metadata("priority", "high");

assert_eq!(batch.metadata.get("campaign_id").map(String::as_str), Some("summer_2024"));
# Ok(())
# }
```

### Job Priorities

Jobs within batches keep their individual priorities:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::JobBatch, Job, JobPriority};
use serde_json::json;

let jobs = vec![
    Job::new("queue".to_string(), json!({"critical": true})).as_critical(),
    Job::new("queue".to_string(), json!({"important": true})).as_high_priority(),
    Job::new("queue".to_string(), json!({"regular": true})).with_priority(JobPriority::Normal),
];

let batch = JobBatch::new("mixed_priority_batch").with_jobs(jobs);
# Ok(())
# }
```

### Validation

`JobBatch::validate` (called by `enqueue_batch`) rejects empty batches, batches over 10,000 jobs and batches whose jobs are not all on the same queue:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::JobBatch, Job};
use serde_json::json;

assert!(JobBatch::new("empty").validate().is_err());

let mixed = JobBatch::new("mixed").with_jobs(vec![
    Job::new("queue1".to_string(), json!({})),
    Job::new("queue2".to_string(), json!({})),
]);
assert!(mixed.validate().is_err());
# Ok(())
# }
```

## Failure Handling Modes

Inserting a batch is all-or-nothing: every job is stored with the same fields as
`enqueue` (result storage, dependencies, workflow, tracing and retry strategy) in one
transaction, whatever the failure mode.

`get_batch_status` tallies progress from the batch's jobs. The `hammerwork_batches` row
moves to `Processing` when its first job finishes, and its counters, final status and
`completed_at` are written when the last job finishes (or when it fails fast).

Batches support three failure handling modes:

### ContinueOnError

Continue processing even if some jobs fail:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::batch::{JobBatch, PartialFailureMode};

let batch = JobBatch::new("resilient_batch")
    .with_jobs(jobs)
    .with_partial_failure_handling(PartialFailureMode::ContinueOnError);
# Ok(())
# }
```

### FailFast

Stop processing on the first failure. When a job of the batch fails terminally (its
last attempt failed or timed out, or it was failed manually), the batch's jobs that have
not started yet are marked `Failed` ("Batch failed: job ... failed") in the same
transaction, so no worker picks them up. Jobs already running finish normally. Retried
attempts do not trigger it.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::batch::{JobBatch, PartialFailureMode};

let batch = JobBatch::new("critical_batch")
    .with_jobs(jobs)
    .with_partial_failure_handling(PartialFailureMode::FailFast);
# Ok(())
# }
```

### CollectErrors

Keep processing and collect the error of every failed job. The errors are available from
`BatchResult::job_errors` (keyed by job ID) when you query the batch status:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::{JobBatch, PartialFailureMode}, queue::DatabaseQueue};

let batch = JobBatch::new("analytics_batch")
    .with_jobs(jobs)
    .with_partial_failure_handling(PartialFailureMode::CollectErrors);

let batch_id = queue.enqueue_batch(batch).await?;

// Later, once jobs have run
let result = queue.get_batch_status(batch_id).await?;
for (job_id, error) in &result.job_errors {
    eprintln!("Job {} failed: {}", job_id, error);
}
# Ok(())
# }
```

## Worker Batch Processing

### Batch Claims

A worker in batch mode claims up to `batch_size` jobs per poll in one round trip instead
of one job per round trip, then runs them `batch_concurrency` at a time (one at a time by
default). `with_batch_size(n)` enables batch mode; `with_batch_processing_enabled(true)`
alone enables it with the default batch size of 10:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, Worker, worker::JobHandler};
use std::{sync::Arc, time::Duration};

let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Process job
        Ok(())
    })
});

// Claim up to 50 jobs per poll and run up to 8 of them at once.
let worker = Worker::new(queue, "batch_queue".to_string(), handler)
    .with_batch_size(50)
    .with_batch_concurrency(8)
    .with_poll_interval(Duration::from_millis(100));
assert!(worker.is_batch_processing_enabled());
# Ok(())
# }
```

Every claimed job gets the same guarantees as a job claimed on its own:

- **One statement per claim.** PostgreSQL claims the batch with a single
  `UPDATE ... WHERE id IN (SELECT ... ORDER BY priority DESC, scheduled_at ASC LIMIT n FOR UPDATE SKIP LOCKED) RETURNING ...`;
  MySQL locks the rows with `SELECT ... FOR UPDATE SKIP LOCKED LIMIT n` and claims them
  with one `UPDATE` in the same READ COMMITTED transaction (retried on deadlock).
  Concurrent workers never claim the same job.
- **Same eligibility.** Paused queues, unfinished dependencies and disabled recurring jobs
  are honoured, and each job's `attempts` is incremented.
- **A lease per job, from the claim.** Each job's lease is written by the claiming
  statement. While a job waits for its turn in the batch, the worker renews its lease
  with heartbeats every third of the lease duration, as it does for running jobs, so the
  stale job reaper never reclaims a held job while its worker is alive, however long the
  jobs ahead of it take. (Sizing the lease to cover the whole batch would not work: the
  worker cannot know how long the jobs ahead will run.)
- **Per-job processing.** Timeouts, retries, hooks, events, spawning, result storage,
  encrypted payloads and statistics work exactly as for single jobs.
- **Throttle and rate limit.** With `ThrottleConfig::max_concurrent`, every claimed job
  holds a permit until it finishes, so a worker claims no more jobs than there are free
  permits. With a rate limit, a batch claims no more jobs than there are tokens, and
  tokens for jobs it did not get are handed back.
- **Shutdown.** Claimed jobs that have not started are released back to `Pending` at once
  (`DatabaseQueue::release_job_run`, which also takes back the claim's attempt), so other
  workers can run them right away. Running jobs get the shutdown grace period.

#### Priorities

Without priority weights, or with `PriorityWeights::strict()`, a batch is filled in strict
priority order: highest priority first, then oldest. With weighted priorities, a priority
level is picked by weight (as a single weighted claim picks one) and the batch is filled
with that level's oldest jobs; if the level runs out first, another level is picked by
weight among the remaining ones to fill the rest. Over many batches each level gets
batches in proportion to its weight, and no batch comes back short while other levels
have runnable jobs.

#### Choosing a batch size

Batch claims save database round trips, so they help most when jobs are short. A worker
holds the jobs it claimed until it gets to them, so idle workers cannot take them: keep
`batch_size` small relative to the queue depth when jobs are slow, or raise
`batch_concurrency` so held jobs start sooner. In a local benchmark (PostgreSQL 16 and
MySQL 8 on the same machine, no-op handler, 2,000 jobs) claiming in batches of 50 was
about 5-6x faster than claiming one job at a time, and a single batch-mode worker with
batch size 50 and concurrency 10 processed 3-4x more jobs per second than a single-job
worker. Run `HAMMERWORK_BENCH=1 cargo test --all-features --test batch_dequeue_tests -- --include-ignored --nocapture throughput`
to measure your own setup.

### Claiming Batches Directly

The queue API claims batches too, for custom consumers:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{JobOutcome, queue::DatabaseQueue};
use std::time::Duration;

// Up to 20 jobs, strict priority order, each leased for 2 minutes.
let jobs = queue
    .dequeue_batch_leased("emails", None, Duration::from_secs(120), 20)
    .await?;
for job in jobs {
    let run = job.clone();
    let job = queue.decrypt_job(job).await?; // a no-op for unencrypted jobs
    // ... run it, renewing its lease with `heartbeat_job` while it runs or waits ...
    queue.finish_job_run(&run, JobOutcome::Completed).await?;
}
# Ok(())
# }
```

Jobs you claimed but will not run go back with `queue.release_job_run(&job)`.

### Access Batch Statistics

Workers with batch processing enabled also keep statistics for jobs that belong to a
`JobBatch`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
let stats = worker.get_batch_stats();
println!("Batch jobs processed: {}", stats.jobs_processed);
println!("Batch success rate: {:.1}%", stats.success_rate() * 100.0);
println!("Average processing time: {:.1}ms", stats.average_processing_time_ms);
# Ok(())
# }
```

## Monitoring Batch Progress

### Check Batch Status

`get_batch_status` returns a `BatchResult`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let batch_result = queue.get_batch_status(batch_id).await?;

println!("Status: {:?}", batch_result.status);
println!("Total jobs: {}", batch_result.total_jobs);
println!("Pending: {}", batch_result.pending_jobs);
println!("Completed: {}", batch_result.completed_jobs);
println!("Failed: {}", batch_result.failed_jobs);
println!("Success rate: {:.1}%", batch_result.success_rate() * 100.0);
# Ok(())
# }
```

### List Batch Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let batch_jobs = queue.get_batch_jobs(batch_id).await?;

for job in batch_jobs {
    println!("Job {}: Status={:?}, Priority={:?}", job.id, job.status, job.priority);
}
# Ok(())
# }
```

### Monitor Completion

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;
use std::time::Duration;

loop {
    let batch_result = queue.get_batch_status(batch_id).await?;

    if batch_result.pending_jobs == 0 {
        println!("Batch finished with {:.1}% success", batch_result.success_rate() * 100.0);
        break;
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
}
# Ok(())
# }
```

## CLI Batch Commands

`cargo hammerwork batch` works on jobs in bulk (it does not create `JobBatch` records):

```bash
# Enqueue jobs from a JSON-lines file
cargo hammerwork batch enqueue --file jobs.jsonl --queue emails --progress-every 500 --continue-on-error

# Retry failed or dead jobs matching criteria (preview first with --dry-run)
cargo hammerwork batch retry --queue emails --status failed --failed-since-hours 24 --dry-run
cargo hammerwork batch retry --queue emails --status failed --failed-since-hours 24 --confirm

# Cancel pending jobs older than a day
cargo hammerwork batch cancel --queue emails --status pending --older-than-hours 24 --confirm

# Export jobs to a file (csv, json or jsonl)
cargo hammerwork batch export --output jobs.csv --queue emails --format csv
```

## Best Practices

### 1. Choose Appropriate Batch Sizes

Smaller chunks give faster feedback and shorter transactions; larger ones mean fewer round trips. Chunk with `into_chunks`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::batch::JobBatch;

let small = JobBatch::new("quick_tasks").with_jobs(jobs.clone()).with_batch_size(10);
let large = JobBatch::new("bulk_import").with_jobs(jobs).with_batch_size(1000);
# Ok(())
# }
```

### 2. Use Metadata for Tracking

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::batch::JobBatch;

let batch = JobBatch::new("user_notifications")
    .with_metadata("run_id", uuid::Uuid::new_v4().to_string())
    .with_metadata("triggered_by", "automated_system")
    .with_metadata("timestamp", chrono::Utc::now().to_rfc3339());
# Ok(())
# }
```

### 3. Handle Large Batches

A batch is limited to 10,000 jobs. For more, split at the application level:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{batch::JobBatch, queue::DatabaseQueue, Job};
use serde_json::json;

let all_jobs: Vec<Job> = (0..25_000)
    .map(|i| Job::new("bulk".to_string(), json!({"n": i})))
    .collect();

for (i, chunk) in all_jobs.chunks(5000).enumerate() {
    let batch = JobBatch::new(format!("large_batch_part_{}", i))
        .with_jobs(chunk.to_vec())
        .with_metadata("parent_batch", "large_batch")
        .with_metadata("part", i.to_string());

    queue.enqueue_batch(batch).await?;
}
# Ok(())
# }
```

### 4. Monitor Batch Performance

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::stats::{InMemoryStatsCollector, StatisticsCollector};
use std::{sync::Arc, time::Duration};

let stats_collector = Arc::new(InMemoryStatsCollector::new_default());
// ... pass it to workers with `with_stats_collector` ...

let stats = stats_collector
    .get_queue_statistics("batch_queue", Duration::from_secs(3600))
    .await?;

println!("Jobs processed: {}", stats.total_processed);
println!("Error rate: {:.1}%", stats.error_rate * 100.0);
# Ok(())
# }
```

### 5. Clean Up Finished Batches

`delete_batch` removes the batch row and **all of its jobs**:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, jobs: Vec<hammerwork::Job>, worker: hammerwork::Worker<sqlx::Postgres>, batch_id: hammerwork::BatchId) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let batch_result = queue.get_batch_status(batch_id).await?;
if batch_result.pending_jobs == 0 {
    queue.delete_batch(batch_id).await?;
}
# Ok(())
# }
```

## Example: Complete Batch Processing Workflow

```rust,no_run
use hammerwork::{
    batch::{JobBatch, PartialFailureMode},
    queue::DatabaseQueue,
    worker::JobHandler,
    Job, JobQueue, Worker, WorkerPool,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup (run the migrations first, e.g. with `cargo hammerwork migration run`)
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Create a batch of data processing jobs
    let jobs: Vec<Job> = (0..100)
        .map(|i| {
            Job::new(
                "data_processing".to_string(),
                json!({
                    "file_id": format!("file_{}", i),
                    "operation": "transform"
                }),
            )
        })
        .collect();

    let batch = JobBatch::new("daily_data_processing")
        .with_jobs(jobs)
        .with_partial_failure_handling(PartialFailureMode::ContinueOnError)
        .with_metadata("date", "2024-01-15")
        .with_metadata("source", "automated_pipeline");

    let batch_id = queue.enqueue_batch(batch).await?;
    println!("Enqueued batch: {}", batch_id);

    // Worker with batch processing enabled
    let handler: JobHandler = Arc::new(|_job: Job| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        })
    });

    let worker = Worker::new(queue.clone(), "data_processing".to_string(), handler)
        .with_batch_processing_enabled(true);

    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(worker);

    tokio::spawn(async move {
        if let Err(e) = worker_pool.start().await {
            eprintln!("Worker pool stopped: {}", e);
        }
    });

    // Monitor progress
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;

        let status = queue.get_batch_status(batch_id).await?;
        println!(
            "Progress: {}/{} jobs completed",
            status.completed_jobs, status.total_jobs
        );

        if status.pending_jobs == 0 {
            println!(
                "Batch finished with {:.1}% success rate",
                status.success_rate() * 100.0
            );
            break;
        }
    }

    // Cleanup (removes the batch and its jobs)
    queue.delete_batch(batch_id).await?;

    Ok(())
}
```

## Performance Considerations

### Database-Specific Optimizations

**PostgreSQL** inserts a batch's jobs inside one transaction using bulk statements, and **MySQL** uses multi-row `INSERT ... VALUES` statements. Either way a batch costs a handful of statements instead of one per job.

### Network Overhead

- Individual enqueue: 1000 jobs = 1000 round trips
- Batch enqueue: 1000 jobs = a few statements in one transaction

### Memory Usage

The whole batch is held in memory while it is built and inserted. Keep batches moderate (a few thousand jobs) and use `into_chunks` or application-level chunking for more.

## Troubleshooting

### Batch Validation Errors

`enqueue_batch` returns `HammerworkError::Queue` when validation fails: an empty batch, more than 10,000 jobs, or jobs with different queue names. Call `batch.validate()` yourself to check ahead of time.

### Performance Issues

If batch processing is slow:

1. Check the size of the batches you enqueue
2. Monitor database performance
3. Make sure workers poll the right queue and have enough capacity
4. Review job handler efficiency
5. Consider more workers in a `WorkerPool`
