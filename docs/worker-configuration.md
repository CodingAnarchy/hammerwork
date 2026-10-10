# Worker Configuration

Hammerwork workers can be extensively configured for optimal performance and reliability.

## Basic Worker Setup

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use std::time::Duration;

let worker = Worker::new(queue, "email_queue".to_string(), handler)
    .with_poll_interval(Duration::from_millis(500))  // Check for jobs every 500ms
    .with_max_retries(3)                             // At most 3 attempts per job (job max_attempts still applies)
    .with_retry_delay(Duration::from_secs(30));      // Wait 30s between retries
# Ok(())
# }
```

`handler` is a `JobHandler`: an `Arc` of a function from `Job` to a boxed future returning `hammerwork::Result<()>`.

An idle worker waits the poll interval between polls, and never less than 10ms
(`worker::MIN_POLL_INTERVAL`), so a zero interval cannot make it poll in a tight loop.
`HammerworkConfig` rejects `worker.polling_interval = 0` (and `worker.job_timeout = 0`)
when it is loaded.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, worker::JobHandler};
use std::sync::Arc;

let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        println!("Processing job {}", job.id);
        Ok(())
    })
});
# Ok(())
# }
```

## Timeout Configuration

### Worker-Level Default Timeouts

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use std::time::Duration;

let worker = Worker::new(queue, "processing_queue".to_string(), handler)
    .with_default_timeout(Duration::from_secs(600)); // 10 minute default timeout

// Jobs without specific timeouts use this default;
// jobs with their own timeout override it
# Ok(())
# }
```

### Timeout Precedence

1. **Job-specific timeout** (highest priority)
2. **Worker default timeout**
3. **No timeout** (job runs until completion)

`with_default_timeout(Duration::ZERO)` means no default timeout. A worker built with
`Worker::with_config` or `with_hammerwork_config` uses `worker.job_timeout` (5 minutes
by default) as its default timeout, so every job without its own timeout is limited to
it.

## Priority Configuration

### Weighted Priority Scheduling

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use hammerwork::priority::{JobPriority, PriorityWeights};

let priority_weights = PriorityWeights::new()
    .with_weight(JobPriority::Critical, 50)
    .with_weight(JobPriority::High, 20)
    .with_weight(JobPriority::Normal, 10)
    .with_weight(JobPriority::Low, 5)
    .with_weight(JobPriority::Background, 1);

let worker = Worker::new(queue, "priority_queue".to_string(), handler)
    .with_priority_weights(priority_weights);
# Ok(())
# }
```

### Strict Priority Scheduling

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;

// Always process the highest priority jobs first
let worker = Worker::new(queue, "urgent_queue".to_string(), handler)
    .with_strict_priority();
# Ok(())
# }
```

See the [priority system guide](priority-system.md) for details.

## Rate Limiting

### Worker-Level Rate Limiting

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{RateLimit, Worker};

// Limit to 10 jobs per second with a burst of 20
let rate_limit = RateLimit::per_second(10).with_burst_limit(20);

let worker = Worker::new(queue, "api_queue".to_string(), handler)
    .with_rate_limit(rate_limit);
# Ok(())
# }
```

A worker takes a token before it polls and hands it back when the poll claims no job
(the queue is empty or paused), so only claimed jobs spend the rate limit and the
full burst is available when jobs arrive after an idle period.

### Advanced Throttling

`ThrottleConfig` combines a concurrency cap, a rate limit and a pause after errors:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{ThrottleConfig, Worker};
use std::time::Duration;

let throttle_config = ThrottleConfig::new()
    .max_concurrent(5)
    .rate_per_minute(100)
    .backoff_on_error(Duration::from_secs(60));

let worker = Worker::new(queue, "throttled_queue".to_string(), handler)
    .with_throttle_config(throttle_config);
# Ok(())
# }
```

## Statistics Collection

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use hammerwork::stats::{InMemoryStatsCollector, StatisticsCollector};
use std::{sync::Arc, time::Duration};

let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_stats_collector(stats_collector.clone());

// Statistics for one queue over a time window
let stats = stats_collector
    .get_queue_statistics("monitored_queue", Duration::from_secs(3600))
    .await?;
println!("Jobs completed in the last hour: {}", stats.completed);
# Ok(())
# }
```

## Metrics and Alerting

### Prometheus Metrics

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{MetricsConfig, PrometheusMetricsCollector, Worker};
use std::{sync::Arc, time::Duration};

let metrics_config = MetricsConfig::new()
    .with_prometheus_exporter("127.0.0.1:9090".parse()?)
    .with_update_interval(Duration::from_secs(30));

let metrics_collector = Arc::new(PrometheusMetricsCollector::new(metrics_config)?);

let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_metrics_collector(metrics_collector);
# Ok(())
# }
```

### Alerting Configuration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{AlertingConfig, Worker};
use std::time::Duration;

let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.05)  // Alert at 5% error rate
    .alert_on_queue_depth(100)       // Alert when the queue holds more than 100 jobs
    .alert_on_worker_starvation(Duration::from_secs(120))
    .webhook("https://alerts.example.com/webhook")
    .slack("https://hooks.slack.com/webhook", "#alerts")
    .with_cooldown(Duration::from_secs(300));

let worker = Worker::new(queue, "production_queue".to_string(), handler)
    .with_alerting_config(alerting_config)
    // How often thresholds are checked (default 30 seconds)
    .with_monitoring_interval(Duration::from_secs(30));
# Ok(())
# }
```

Alert thresholds (and the queue depth metric) are checked by a background monitoring
task every `with_monitoring_interval` (30 seconds by default), never in the loop that
dequeues jobs, so a slow or unreachable alert target cannot hold up job processing.
Webhook and Slack alerts time out after 10 seconds. An alert that reached no target is
retried after 60 seconds (or the cooldown, if shorter) instead of on every check; one
that was delivered starts the cooldown. The monitoring task stops with the worker,
however the worker stops.

## Worker Pools

### Basic Pool Management

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Worker, WorkerPool};

let email_worker = Worker::new(queue.clone(), "email".to_string(), handler.clone());
let processing_worker = Worker::new(queue.clone(), "processing".to_string(), handler.clone());
let background_worker = Worker::new(queue.clone(), "background".to_string(), handler.clone());

let mut pool = WorkerPool::new();
pool.add_worker(email_worker);
pool.add_worker(processing_worker);
pool.add_worker(background_worker);

// Start all workers (returns once the pool has shut down)
pool.start().await?;

// Graceful shutdown, normally triggered from another task
pool.shutdown().await?;
# Ok(())
# }
```

`start` spawns every worker and a supervisor, then waits until the pool shuts down.
The workers run in their own tasks, so you can drop the `start` future (for example
in a `tokio::select!` on a shutdown signal) and then call `shutdown`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Worker, WorkerPool};

let mut pool = WorkerPool::new();
pool.add_worker(Worker::new(queue.clone(), "email".to_string(), handler.clone()));

tokio::select! {
    result = pool.start() => result?,
    _ = tokio::signal::ctrl_c() => pool.shutdown().await?,
}
# Ok(())
# }
```

Dropping the pool also shuts its workers down gracefully.

### Autoscaling

A `WorkerPool` does not autoscale unless you enable it with `with_autoscaling` (or
`WorkerPool::from_config` with `worker.autoscaling_enabled = true`). Autoscaling scales
a single queue: that of the worker template (`with_worker_template`), or of the first
worker added. It measures that queue's depth, starts clones of the template while the
queue is deep (up to `max_workers`), and retires workers of that queue (down to
`min_workers`) once it drains. Workers of other queues in the same pool are never
started or retired. A retired worker stops like on shutdown: it finishes its in-flight
job within its shutdown grace period.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Worker, WorkerPool, worker::AutoscaleConfig};

let mut pool = WorkerPool::new().with_autoscaling(
    AutoscaleConfig::new()
        .with_min_workers(1)
        .with_max_workers(8)
        .with_scale_up_threshold(10), // queued jobs per worker before scaling up
);
// The first worker is the template: only "email" is scaled.
pool.add_worker(Worker::new(queue.clone(), "email".to_string(), handler.clone()));
// Always exactly one "reports" worker.
pool.add_worker(Worker::new(queue.clone(), "reports".to_string(), handler.clone()));
# Ok(())
# }
```

To scale several queues, run one pool per queue.

### Supervision

If a worker task stops unexpectedly (a bug that panics outside a job handler, for
example), the pool logs it and restarts the worker after one second. A panic inside
a job handler never reaches this point: the worker catches it and fails the job
with the error `Job handler panicked: <message>`, which then goes through the
normal retry / dead-letter path.

Errors in the worker loop itself (the database failing while a job is dequeued or
its outcome recorded) are logged, and the worker backs off exponentially, starting
at the throttle's `backoff_on_error` (or the poll interval, at least 100ms) and
capped at 60 seconds.

### Graceful Shutdown

On `shutdown`, idle workers stop immediately. A worker in the middle of a job stops
polling but lets the job finish and record its outcome, for up to its shutdown grace
period (30 seconds by default). Only then is the handler cancelled. `shutdown`
returns once every worker has stopped.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use std::time::Duration;

let worker = Worker::new(queue.clone(), "reports".to_string(), handler)
    .with_shutdown_grace_period(Duration::from_secs(120));
# Ok(())
# }
```

### Job Leases and Stale Job Recovery

A worker that crashes, is OOM-killed or loses its pod leaves its job in `Running`.
Hammerwork recovers these jobs with leases (requires migration `015_add_job_leases`):

- A worker takes the lease in the same statement that claims the job
  (`DatabaseQueue::dequeue_leased`; plain `dequeue` takes the default 5 minute lease).
  While the handler runs, the worker records a heartbeat and extends the lease every
  third of its lease duration (5 minutes by default). Jobs that finish sooner never
  write a heartbeat.
- A heartbeat only extends the run it belongs to (same attempt and start time): a
  worker whose job was reclaimed and picked up by another worker cannot keep the new
  run's lease alive.
- `DatabaseQueue::requeue_stale_jobs(older_than)` reclaims `Running` jobs whose lease
  has expired. A job whose lease is valid is never reclaimed, whatever `older_than` is,
  so pools with different lease durations (or `cargo hammerwork job requeue-stale
  --older-than-secs 1`) can share a database safely. `older_than` only applies to jobs
  claimed by an older Hammerwork version that did not take a lease at claim time:
  they count as stale once they started more than `older_than` ago.
- A reclaimed job goes back to `Pending` and runs again immediately. The interrupted
  run already counted as an attempt; a job with no attempts left is moved to `Dead`.
- Every `WorkerPool` runs this reaper every 60 seconds, with `older_than` set to the
  longest lease of its workers. Several pools or processes can reap at once safely.
- `with_lease_duration(Duration::ZERO)` takes a lease that never expires and sends no
  heartbeats: the reaper never reclaims that worker's jobs, so if it dies an operator
  has to re-run them (`cargo hammerwork job retry`).

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Worker, WorkerPool};
use std::time::Duration;

let worker = Worker::new(queue.clone(), "video".to_string(), handler)
    .with_lease_duration(Duration::from_secs(60)); // detect crashed workers within ~1 min

let pool: WorkerPool<sqlx::Postgres> = WorkerPool::new()
    // Reap every 30s; jobs without a lease are stale after 10 minutes
    .with_stale_job_reaper(Duration::from_secs(30), Duration::from_secs(600));

// Or turn it off and run `cargo hammerwork job requeue-stale` from a scheduler:
let pool: WorkerPool<sqlx::Postgres> = WorkerPool::new().without_stale_job_reaper();
# Ok(())
# }
```

If a lease expires while the handler is still running (for example after a long
stop-the-world pause), the job can be reclaimed and run again elsewhere, so handlers
should be idempotent. The worker logs a warning when it notices it lost a lease.

### Maintenance Tasks

Besides its workers, a pool runs periodic maintenance tasks. Each pass runs in a task
of its own, so workers never wait for it; the first pass runs when the pool starts, and
`shutdown` stops the schedule without interrupting a pass part-way through its
transaction. Every task is safe to run in several pools or processes at once.

| Task | Default | Builder | Configuration (`from_hammerwork_config`) |
|------|---------|---------|------------------------------------------|
| Stale job reaper | every 60s | `with_stale_job_reaper` / `without_stale_job_reaper` | always on |
| Expired result cleanup | every 5 minutes | `with_result_cleanup` / `without_result_cleanup` | `[worker] result_cleanup_enabled`, `result_cleanup_interval` |
| Encrypted job retention purge | off | `with_encrypted_job_purge` | `[encryption] purge_interval_secs` |
| Archival | off | `with_archival` | `[archive] enabled`, `check_interval` |
| Key rotation (`encryption` feature) | off | `with_key_rotation` | `[encryption.key_rotation]` |

**Expired result cleanup.** Results stored with a TTL (`Job::with_result_ttl`) stop being
returned by `get_job_result` once they expire, but stay in `hammerwork_jobs` (and in
`Job::result_data` from `get_job`) until something removes them. The pool calls
`DatabaseQueue::cleanup_expired_results` every 5 minutes to clear them, and logs how many
it cleared at `info` level. It is on by default because expired results are already
unreadable through the result API: clearing them only frees space. Turn it off, or change
the interval, in code or in `[worker]`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::WorkerPool;
use std::time::Duration;

let pool: WorkerPool<sqlx::Postgres> = WorkerPool::new()
    .with_result_cleanup(Duration::from_secs(60)); // every minute

// Or leave it to another process:
let pool: WorkerPool<sqlx::Postgres> = WorkerPool::new().without_result_cleanup();
# Ok(())
# }
```

```toml
[worker]
# ...
result_cleanup_enabled = true      # default
result_cleanup_interval = "5m"     # default; must be greater than zero
```

Configuration files without these keys keep loading and get the defaults.

**Key rotation.** With the `encryption` feature, `with_key_rotation(key_manager,
interval)` rotates the keys of a `KeyManager` as they fall due, and
`[encryption.key_rotation] enabled = true` does the same for a pool built from
configuration. See [Automatic Rotation in a Worker Pool](encryption.md#automatic-rotation-in-a-worker-pool).

### Mixed Configuration Pools

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{RateLimit, Worker, WorkerPool};
use std::time::Duration;

// High-priority worker for urgent tasks
let urgent_worker = Worker::new(queue.clone(), "urgent".to_string(), handler.clone())
    .with_strict_priority()
    .with_default_timeout(Duration::from_secs(60));

// Background worker for cleanup tasks
let background_worker = Worker::new(queue.clone(), "cleanup".to_string(), handler.clone())
    .with_poll_interval(Duration::from_secs(30))
    .with_rate_limit(RateLimit::per_minute(10));

// Email worker with retry configuration
let email_worker = Worker::new(queue.clone(), "email".to_string(), handler.clone())
    .with_max_retries(5)
    .with_retry_delay(Duration::from_secs(60));

let mut pool = WorkerPool::new();
pool.add_worker(urgent_worker);
pool.add_worker(background_worker);
pool.add_worker(email_worker);
# Ok(())
# }
```

## Error Handling

### Custom Error Handling

A handler reports success with `Ok(())` and failure with `Err(HammerworkError)`. Any error
counts as a failed attempt and is retried while the job has attempts left (see below);
the handler cannot mark a job as permanently failed. To make a job non-retryable, give it
`max_attempts` of 1 when you create it.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{HammerworkError, Job, worker::JobHandler};
use std::sync::Arc;

async fn process_job(job: &Job) -> hammerwork::Result<()> {
    // Your business logic
    Ok(())
}

fn is_retryable_error(error: &HammerworkError) -> bool {
    matches!(error, HammerworkError::Database(_) | HammerworkError::Io(_))
}

let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        match process_job(&job).await {
            Ok(()) => {
                println!("Job {} completed successfully", job.id);
                Ok(())
            }
            Err(e) if is_retryable_error(&e) => {
                eprintln!("Retryable error for job {}: {}", job.id, e);
                Err(e) // retried while attempts remain
            }
            Err(e) => {
                eprintln!("Permanent failure for job {}: {}", job.id, e);
                Err(e) // also counts as a failed attempt
            }
        }
    })
});

// A job that must never be retried
let one_shot = Job::new("payments".to_string(), serde_json::json!({})).with_max_attempts(1);
# Ok(())
# }
```

### Retry Configuration

A job's own `max_attempts` (`Job::with_max_attempts`, default 3) decides how often it
runs. A failing run is retried while attempts remain; the last failure makes the job
`Dead`. A run that exceeds its timeout counts as a failed attempt too: it is retried
while attempts remain and the job only ends `TimedOut` on its last attempt.

`Worker::with_max_retries(n)` can only lower that limit (`min(job.max_attempts, n)`);
by default workers set no cap.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use std::time::Duration;

let worker = Worker::new(queue, "retry_queue".to_string(), handler)
    .with_max_retries(5)                        // Never more than 5 attempts per job
    .with_retry_delay(Duration::from_secs(30))  // Wait 30s between attempts
    .with_default_timeout(Duration::from_secs(300)); // 5 minute timeout per attempt
# Ok(())
# }
```

The delay before a retry comes from the job's retry strategy
(`Job::with_retry_strategy`, stored with the job since migration 018), then the
worker's `with_default_retry_strategy`, then `with_retry_delay`. A
`RetryStrategy::Custom` closure cannot be stored, so enqueueing a job that carries one
is rejected; set it with `with_default_retry_strategy` instead.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, RetryStrategy, Worker};
use std::time::Duration;

// Exponential backoff for every job on this worker that has no strategy of its own
let worker = Worker::new(queue.clone(), "retry_queue".to_string(), handler)
    .with_default_retry_strategy(RetryStrategy::exponential(
        Duration::from_secs(2),
        2.0,
        Some(Duration::from_secs(300)),
    ));

// A per-job strategy
let job = Job::new("flaky".to_string(), payload)
    .with_retry_strategy(RetryStrategy::fixed(Duration::from_secs(10)));
# Ok(())
# }
```

### Recording Outcomes and Zombie Workers

A worker records each run with `DatabaseQueue::finish_job_run`. The outcome only
applies while the job is still `Running` the same run (same `attempts` and
`started_at`). If the stale job reaper reclaimed the job, an operator changed it, or
another worker is running a newer attempt, the outcome is discarded with a warning, so
a late worker never overwrites newer state. The same transaction also:

- reschedules a recurring job for its next run after the run completed, died or timed
  out (a failed run's error stays on the job until the next run);
- makes dependents whose dependencies have all completed runnable;
- applies the workflow's `FailurePolicy` and the batch's `PartialFailureMode` on a
  terminal failure, and updates workflow and batch progress.

## Performance Tuning

### High-Throughput Configuration

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{RateLimit, Worker};
use std::time::Duration;

let worker = Worker::new(queue, "high_volume".to_string(), handler)
    .with_poll_interval(Duration::from_millis(100))  // Check very frequently
    .with_rate_limit(RateLimit::per_second(100))     // Allow high throughput
    .with_default_timeout(Duration::from_secs(30));  // Short timeouts
# Ok(())
# }
```

### Batch Claims

For many short jobs, a worker can claim several jobs per database round trip. With
`with_batch_size(n)` the worker claims up to `n` jobs per poll in one statement and runs
them `with_batch_concurrency(m)` at a time (default 1, so the worker still runs one handler
at a time unless you raise it):

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{ThrottleConfig, Worker};
use std::time::Duration;

let worker = Worker::new(queue, "notifications".to_string(), handler)
    .with_batch_size(20)                    // Claim up to 20 jobs per poll
    .with_batch_concurrency(5)              // Run up to 5 of them at once
    .with_lease_duration(Duration::from_secs(60))
    .with_throttle_config(ThrottleConfig::new().max_concurrent(50));
assert_eq!(worker.batch_size(), 20);
# Ok(())
# }
```

- Each claimed job holds its own lease from the moment it is claimed. Jobs waiting for
  their turn have their leases renewed every third of the lease duration, like running
  jobs, so a short lease is safe even for large batches of slow jobs.
- With a throttle, each claimed job holds one `max_concurrent` permit until it finishes:
  a worker never claims more jobs than there are free permits. With a rate limit, a batch
  claims no more jobs than there are tokens, and unused tokens are handed back.
- On shutdown, jobs that were claimed but not started go straight back to `Pending` (their
  attempt is not counted); running jobs get the shutdown grace period.
- Weighted priorities pick one level by weight per batch and fill the batch from it
  (topping up from other levels); strict priority fills in strict order. See
  [Batch Operations](batch-operations.md#worker-batch-processing).

A worker holds the jobs it claimed until it gets to them, so other workers cannot take
them meanwhile. Prefer small batches for slow jobs, or raise the batch concurrency.

### Resource-Intensive Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{RateLimit, Worker};
use std::time::Duration;

let worker = Worker::new(queue, "heavy_processing".to_string(), handler)
    .with_poll_interval(Duration::from_secs(5))      // Check less frequently
    .with_rate_limit(RateLimit::per_minute(10))      // Limit load
    .with_default_timeout(Duration::from_secs(1800)) // 30 minute timeout
    .with_max_retries(1);                            // Minimal retries
# Ok(())
# }
```

## Monitoring Worker Health

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::stats::{InMemoryStatsCollector, StatisticsCollector};
use std::{sync::Arc, time::Duration};
use tokio::time::interval;

let stats_collector = Arc::new(InMemoryStatsCollector::new_default());
// ... pass stats_collector.clone() to your workers with `with_stats_collector` ...

let mut monitor_interval = interval(Duration::from_secs(60));
loop {
    monitor_interval.tick().await;

    let stats = stats_collector
        .get_queue_statistics("my_queue", Duration::from_secs(300))
        .await?;

    println!("Queue stats (last 5 minutes):");
    println!("  Completed: {}", stats.completed);
    println!("  Failed: {}", stats.failed);
    println!("  Error rate: {:.2}%", stats.error_rate * 100.0);
    println!("  Avg processing time: {:.0} ms", stats.avg_processing_time_ms);

    if stats.error_rate > 0.1 {
        eprintln!("HIGH ERROR RATE: {:.2}%", stats.error_rate * 100.0);
    }
}
# Ok(())
# }
```

## Configuration Examples by Use Case

### API Rate-Limited Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{RateLimit, Worker};
use std::time::Duration;

let api_worker = Worker::new(queue, "api_calls".to_string(), handler)
    .with_rate_limit(RateLimit::per_second(5).with_burst_limit(10))
    .with_max_retries(3)
    .with_retry_delay(Duration::from_secs(60))
    .with_default_timeout(Duration::from_secs(30));
# Ok(())
# }
```

### Background Maintenance

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use hammerwork::priority::{JobPriority, PriorityWeights};
use std::time::Duration;

let cleanup_worker = Worker::new(queue, "cleanup".to_string(), handler)
    .with_poll_interval(Duration::from_secs(30))
    .with_priority_weights(
        PriorityWeights::new()
            .with_weight(JobPriority::Background, 10)
            .with_weight(JobPriority::Low, 5)
            .with_weight(JobPriority::Normal, 1)
            .with_weight(JobPriority::High, 1)
            .with_weight(JobPriority::Critical, 1),
    )
    .with_default_timeout(Duration::from_secs(3600)); // 1 hour
# Ok(())
# }
```

### Critical System Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Worker;
use std::time::Duration;

let critical_worker = Worker::new(queue, "system".to_string(), handler)
    .with_strict_priority()
    .with_poll_interval(Duration::from_millis(100))
    .with_max_retries(5)
    .with_retry_delay(Duration::from_secs(5))
    .with_default_timeout(Duration::from_secs(120));
# Ok(())
# }
```
