# Job Types and Configuration

Hammerwork supports various types of jobs with flexible configuration options.

## Basic Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::job::Job;
use serde_json::json;

// Simple job
let job = Job::new("email_queue".to_string(), json!({
    "to": "user@example.com",
    "subject": "Welcome!",
    "body": "Thanks for signing up"
}));

queue.enqueue(job).await?;
# Ok(())
# }
```

## Job Priority

Jobs support five priority levels:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, JobPriority};

let high_priority_job = Job::new("urgent".to_string(), json!({"task": "urgent_task"}))
    .with_priority(JobPriority::High);

let background_job = Job::new("cleanup".to_string(), json!({"task": "cleanup"}))
    .with_priority(JobPriority::Background);
# Ok(())
# }
```

### Priority Levels
- `Critical` - Highest priority (4)
- `High` - High priority (3)  
- `Normal` - Default priority (2)
- `Low` - Low priority (1)
- `Background` - Lowest priority (0)

## Delayed Jobs

Schedule jobs to run at a specific time:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::{Utc, Duration};

let delayed_job = Job::with_delay("reminder".to_string(), json!({"user_id": 123}), Duration::hours(24)); // Run in 24 hours

let scheduled_job = Job::new("report".to_string(), json!({"type": "weekly"}))
    .with_scheduled_at(Utc::now() + Duration::days(7));
# Ok(())
# }
```

## Job Timeouts

Configure per-job or worker-level timeouts:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler) -> std::result::Result<(), Box<dyn std::error::Error>> {
use std::time::Duration;

// Per-job timeout
let job = Job::new("long_task".to_string(), json!({"data": "..."}))
    .with_timeout(Duration::from_secs(300)); // 5 minute timeout

// Worker-level default timeout
let worker = Worker::new(queue, "default".to_string(), handler)
    .with_default_timeout(Duration::from_secs(120)); // 2 minute default
# Ok(())
# }
```

## Retry Configuration

Jobs automatically retry on failure with configurable limits:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler) -> std::result::Result<(), Box<dyn std::error::Error>> {
let job = Job::new("api_call".to_string(), json!({"url": "https://api.example.com"}))
    .with_max_attempts(5); // At most 5 attempts in total (the first run plus 4 retries)

// Worker-level retry configuration. The job's max_attempts is the limit;
// with_max_retries only caps it for jobs processed by this worker.
let worker = Worker::new(queue, "default".to_string(), handler)
    .with_max_retries(3)
    .with_retry_delay(std::time::Duration::from_secs(30)); // Wait 30s between retries
# Ok(())
# }
```

## Cron Jobs

Schedule recurring jobs with cron expressions:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::cron::CronSchedule;

let schedule = CronSchedule::with_timezone("0 0 8 * * *", "America/New_York")?; // Every day at 8 AM
let cron_job = Job::new("daily_report".to_string(), json!({"type": "daily"}))
    .with_cron(schedule)?;

queue.enqueue_cron_job(cron_job).await?;
# Ok(())
# }
```

### Cron Examples
- `"0 */30 * * * *"` - Every 30 minutes
- `"0 0 9 * * MON-FRI"` - 9 AM on weekdays
- `"0 0 0 1 * *"` - First day of every month
- `"@daily"` - Once a day at midnight
- `"@hourly"` - Once an hour

## Job Status Lifecycle

Jobs progress through these states:

1. **Pending** - Waiting to be processed (also after a failed or timed-out run that will be retried)
2. **Running** - Currently being processed
3. **Completed** - Successfully finished
4. **Failed** - Failed without running out of attempts: set by `fail_job`, or when a
   job is cancelled because a dependency, its fail-fast workflow or its fail-fast batch
   failed. Terminal; `retry_job` re-runs it.
5. **Dead** - Failed permanently (its last attempt failed). `retry_dead_job` re-runs it.
6. **TimedOut** - Its last attempt exceeded the timeout (earlier timeouts are retried).
   `retry_dead_job` re-runs it.
7. **Retrying** - Scheduled for retry (used by `TestQueue`)

A job runs at most `max_attempts` times. Recurring (cron) jobs never end in a terminal
status from a worker: after a completed, dead or timed-out run they go back to
`Pending` for their next scheduled run.

Status changes are guarded. Manual transitions only apply from these statuses, and
return `HammerworkError::InvalidJobTransition` otherwise:

| Method | Allowed from |
|---|---|
| `complete_job`, `fail_job` | `Pending`, `Running`, `Retrying` |
| `retry_job` | `Running`, `Retrying`, `Failed`, `TimedOut` |
| `mark_job_dead` | `Pending`, `Running`, `Retrying`, `Failed`, `TimedOut` |
| `mark_job_timed_out` | `Running` |
| `reschedule_cron_job` | any status except `Completed` and `Archived` (recurring jobs only) |
| `retry_dead_job` | `Dead`, `TimedOut` |

Workers record their runs with `finish_job_run`, which only applies to the run the
worker dequeued (see [Worker Configuration](worker-configuration.md#recording-outcomes-and-zombie-workers)).

## Job Handlers

Define how jobs are processed:

```rust
use hammerwork::{HammerworkError, Job, worker::JobHandler};
use std::sync::Arc;
# async fn send_email(_to: &str) -> hammerwork::Result<()> { Ok(()) }
# async fn generate_report(_kind: &str) -> hammerwork::Result<()> { Ok(()) }

// Simple handler
let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        match job.payload.get("task").and_then(|v| v.as_str()) {
            Some("send_email") => {
                let email = job.payload["email"].as_str().unwrap_or_default();
                send_email(email).await?;
            },
            Some("generate_report") => {
                let report_type = job.payload["type"].as_str().unwrap_or_default();
                generate_report(report_type).await?;
            },
            _ => {
                return Err(HammerworkError::Processing("Unknown task type".to_string()));
            }
        }
        Ok(())
    })
});
```

## Job Results

A handler created with `Worker::new_with_result_handler` can return data with
`JobResult::with_data`. The job's result configuration decides what happens to it:

| `ResultStorage` | Where the result goes | Who can read it |
|---|---|---|
| `None` (default) | Discarded | Nobody |
| `Database` | The job's row in `hammerwork_jobs` | Any process with database access |
| `Memory` | The `JobQueue`'s in-memory store | Only this process, through the same `JobQueue` (or its clones) |

The worker stores the result before it marks the job Completed, so whoever sees the job as
Completed can read its result with `get_job_result`. `Job::with_result_ttl` sets how long
the result is kept; without a TTL it is kept until it is deleted (`delete_job_result`,
or deleting the job). `cleanup_expired_results` removes expired results from the
database and from the in-memory store.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, queue::*, worker::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::sync::Arc;
# #[allow(unused_variables)]
# async fn doc(pool: sqlx::PgPool) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, JobQueue, ResultStorage, Worker, WorkerPool};
use hammerwork::worker::{JobHandlerWithResult, JobResult};
use std::time::Duration;

// Keep up to 50,000 in-memory results (the default is 10,000).
let queue = Arc::new(JobQueue::new(pool).with_memory_result_capacity(50_000));

let handler: JobHandlerWithResult = Arc::new(|job: Job| {
    Box::pin(async move { Ok(JobResult::with_data(json!({"total": job.payload["n"]}))) })
});
let mut pool = WorkerPool::new();
pool.add_worker(Worker::new_with_result_handler(queue.clone(), "reports".to_string(), handler));

let job = Job::new("reports".to_string(), json!({"n": 42}))
    .with_result_storage(ResultStorage::Memory)
    .with_result_ttl(Duration::from_secs(600));
let job_id = queue.enqueue(job).await?;

// ...once the job has completed, in the same process:
if let Some(result) = queue.get_job_result(job_id).await? {
    println!("total = {}", result["total"]);
}
# Ok(())
# }
```

In-memory results are faster to store than database results, but:

- **They are per process.** A worker in one process stores them in its own `JobQueue`.
  Another process (another worker host, `cargo hammerwork`, the web dashboard) reads
  only the database, so for it the job has no stored result. A separately created
  `JobQueue` in the same process has its own store too: share one `Arc<JobQueue>`.
- **They are lost when the process exits.**
- **They are bounded.** Each `JobQueue` keeps at most `DEFAULT_MEMORY_RESULT_CAPACITY`
  (10,000) results, or the number set with `JobQueue::with_memory_result_capacity`. When
  it is full, storing a result evicts the one that expires soonest, or the oldest if
  none expire. Expired results are removed when read and pruned as new results are
  stored.
- They don't appear in `Job::result_data` (from `get_job`), which reflects the database
  columns; read them with `get_job_result`.

`ResultConfig::with_max_size` is recorded with the job but not enforced, for database
and in-memory results alike.

The `TestQueue` (feature `test`) keeps all results in memory regardless of
`ResultStorage`, expiring them by its mock clock.

## Error Handling

Jobs can fail and be retried automatically:

```rust
# use hammerwork::{Job, worker::JobHandler};
# use std::sync::Arc;
# async fn process_job(_job: &Job) -> hammerwork::Result<String> { Ok(String::new()) }
let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Your processing logic
        match process_job(&job).await {
            Ok(result) => {
                println!("Job completed: {:?}", result);
                Ok(())
            },
            Err(e) => {
                eprintln!("Job failed: {}", e);
                Err(e) // Will trigger retry if max_attempts not reached
            }
        }
    })
});
```