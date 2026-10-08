# Worker Configuration

Hammerwork workers can be extensively configured for optimal performance and reliability.

## Basic Worker Setup

```rust
use hammerwork::{Worker, WorkerPool};
use std::time::Duration;

let worker = Worker::new(queue, "email_queue".to_string(), handler)
    .with_poll_interval(Duration::from_millis(500))  // Check for jobs every 500ms
    .with_max_retries(3)                             // Retry failed jobs up to 3 times
    .with_retry_delay(Duration::from_secs(30));      // Wait 30s between retries
```

## Timeout Configuration

### Worker-Level Default Timeouts

```rust
let worker = Worker::new(queue, "processing_queue".to_string(), handler)
    .with_default_timeout(Duration::from_secs(600)); // 10 minute default timeout

// Jobs without specific timeouts will use this default
// Jobs with specific timeouts override this setting
```

### Timeout Precedence

1. **Job-specific timeout** (highest priority)
2. **Worker default timeout**
3. **No timeout** (job runs until completion)

## Priority Configuration

### Weighted Priority Scheduling

```rust
use hammerwork::priority::PriorityWeights;

let priority_weights = PriorityWeights::builder()
    .critical(50)     // Critical jobs 50x more likely
    .high(20)         // High jobs 20x more likely
    .normal(10)       // Normal baseline
    .low(5)           // Low jobs 2x less likely
    .background(1)    // Background jobs 10x less likely
    .fairness_factor(0.1) // 10% chance for lower priorities
    .build();

let worker = Worker::new(queue, "priority_queue".to_string(), handler)
    .with_priority_weights(priority_weights);
```

### Strict Priority Scheduling

```rust
// Always process highest priority jobs first
let worker = Worker::new(queue, "urgent_queue".to_string(), handler)
    .with_strict_priority();
```

## Rate Limiting

### Worker-Level Rate Limiting

```rust
use hammerwork::rate_limit::RateLimit;

// Limit to 10 jobs per second with burst of 20
let rate_limit = RateLimit::per_second(10).with_burst_limit(20);

let worker = Worker::new(queue, "api_queue".to_string(), handler)
    .with_rate_limit(rate_limit);
```

### Advanced Throttling

```rust
use hammerwork::rate_limit::ThrottleConfig;

let throttle_config = ThrottleConfig::new()
    .with_max_concurrent_jobs(5)
    .with_rate_limit(RateLimit::per_minute(100))
    .with_error_backoff(Duration::from_secs(60));

let worker = Worker::new(queue, "throttled_queue".to_string(), handler)
    .with_throttle_config(throttle_config);
```

## Statistics Collection

```rust
use hammerwork::stats::InMemoryStatsCollector;

let stats_collector = Arc::new(InMemoryStatsCollector::new());

let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_stats_collector(stats_collector.clone());

// Access statistics
let stats = stats_collector.get_queue_stats("monitored_queue").await?;
println!("Jobs processed: {}", stats.completed_count);
```

## Metrics and Alerting

### Prometheus Metrics

```rust
use hammerwork::{MetricsConfig, PrometheusMetricsCollector};

let metrics_config = MetricsConfig::new()
    .with_prometheus_exporter("127.0.0.1:9090".parse().unwrap())
    .with_update_interval(Duration::from_secs(30));

let metrics_collector = Arc::new(PrometheusMetricsCollector::new(metrics_config)?);

let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_metrics_collector(metrics_collector);
```

### Alerting Configuration

```rust
use hammerwork::AlertingConfig;

let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.05)  // Alert at 5% error rate
    .alert_on_queue_depth(100)       // Alert when queue > 100 jobs
    .alert_on_worker_starvation(Duration::from_minutes(2))
    .webhook("https://alerts.example.com/webhook")
    .slack("https://hooks.slack.com/webhook", "#alerts")
    .with_cooldown(Duration::from_minutes(5));

let worker = Worker::new(queue, "production_queue".to_string(), handler)
    .with_alerting_config(alerting_config);
```

## Worker Pools

### Basic Pool Management

```rust
let mut pool = WorkerPool::new();

// Add multiple workers
pool.add_worker(email_worker);
pool.add_worker(processing_worker);
pool.add_worker(background_worker);

// Start all workers
pool.start().await?;

// Graceful shutdown
pool.shutdown().await?;
```

`start` spawns every worker and a supervisor, then waits until the pool shuts down.
The workers run in their own tasks, so you can drop the `start` future (for example
in a `tokio::select!` on a shutdown signal) and then call `shutdown`:

```rust
tokio::select! {
    result = pool.start() => result?,
    _ = tokio::signal::ctrl_c() => pool.shutdown().await?,
}
```

Dropping the pool also shuts its workers down gracefully.

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

```rust
let worker = Worker::new(queue.clone(), "reports".to_string(), handler)
    .with_shutdown_grace_period(Duration::from_secs(120));
```

### Job Leases and Stale Job Recovery

A worker that crashes, is OOM-killed or loses its pod leaves its job in `Running`.
Hammerwork recovers these jobs with leases (requires migration `015_add_job_leases`):

- While a handler runs, the worker records a heartbeat and extends the job's lease
  every third of its lease duration (5 minutes by default). Jobs that finish sooner
  never write a heartbeat.
- `DatabaseQueue::requeue_stale_jobs(older_than)` reclaims `Running` jobs whose
  lease has expired. Jobs that never recorded a lease (they had not reached their
  first heartbeat, or were started by an older Hammerwork version) count as stale
  once they started more than `older_than` ago.
- A reclaimed job goes back to `Pending` and runs again immediately. The interrupted
  run already counted as an attempt; a job with no attempts left is moved to `Dead`.
- Every `WorkerPool` runs this reaper every 60 seconds, with `older_than` set to the
  longest lease of its workers. Several pools or processes can reap at once safely.

```rust
let worker = Worker::new(queue.clone(), "video".to_string(), handler)
    .with_lease_duration(Duration::from_secs(60)); // detect crashed workers within ~1 min

let mut pool = WorkerPool::new()
    // Reap every 30s; jobs without a lease are stale after 10 minutes
    .with_stale_job_reaper(Duration::from_secs(30), Duration::from_secs(600));
// Or turn it off and run `cargo hammerwork job requeue-stale` from a scheduler:
// let mut pool = WorkerPool::new().without_stale_job_reaper();
```

If a lease expires while the handler is still running (for example after a long
stop-the-world pause), the job can be reclaimed and run again elsewhere, so handlers
should be idempotent. The worker logs a warning when it notices it lost a lease.

### Mixed Configuration Pools

```rust
// High-priority worker for urgent tasks
let urgent_worker = Worker::new(queue.clone(), "urgent".to_string(), urgent_handler)
    .with_strict_priority()
    .with_default_timeout(Duration::from_secs(60));

// Background worker for cleanup tasks
let background_worker = Worker::new(queue.clone(), "cleanup".to_string(), cleanup_handler)
    .with_poll_interval(Duration::from_secs(30))
    .with_rate_limit(RateLimit::per_minute(10));

// Email worker with retry configuration
let email_worker = Worker::new(queue.clone(), "email".to_string(), email_handler)
    .with_max_retries(5)
    .with_retry_delay(Duration::from_secs(60));

let mut pool = WorkerPool::new();
pool.add_worker(urgent_worker);
pool.add_worker(background_worker);
pool.add_worker(email_worker);
```

## Error Handling

### Custom Error Handling

```rust
let handler = Arc::new(|job: Job| {
    Box::pin(async move {
        match process_job(&job).await {
            Ok(result) => {
                println!("Job {} completed successfully", job.id);
                Ok(())
            },
            Err(e) if is_retryable_error(&e) => {
                eprintln!("Retryable error for job {}: {}", job.id, e);
                Err(e) // Will trigger retry
            },
            Err(e) => {
                eprintln!("Permanent failure for job {}: {}", job.id, e);
                // Mark job as dead to prevent retries
                Err(e)
            }
        }
    })
});
```

### Retry Configuration

```rust
let worker = Worker::new(queue, "retry_queue".to_string(), handler)
    .with_max_retries(5)                        // Try up to 5 times
    .with_retry_delay(Duration::from_secs(30))  // Wait 30s between attempts
    .with_default_timeout(Duration::from_secs(300)); // 5 minute timeout per attempt
```

## Performance Tuning

### High-Throughput Configuration

```rust
let worker = Worker::new(queue, "high_volume".to_string(), handler)
    .with_poll_interval(Duration::from_millis(100))  // Check very frequently
    .with_rate_limit(RateLimit::per_second(100))     // Allow high throughput
    .with_default_timeout(Duration::from_secs(30));  // Short timeouts
```

### Resource-Intensive Jobs

```rust
let worker = Worker::new(queue, "heavy_processing".to_string(), handler)
    .with_poll_interval(Duration::from_secs(5))      // Check less frequently
    .with_rate_limit(RateLimit::per_minute(10))      // Limit concurrent load
    .with_default_timeout(Duration::from_secs(1800)) // 30 minute timeout
    .with_max_retries(1);                            // Minimal retries
```

## Monitoring Worker Health

```rust
use tokio::time::{interval, Duration};

// Monitor worker statistics
let mut monitor_interval = interval(Duration::from_secs(60));
loop {
    monitor_interval.tick().await;
    
    let stats = stats_collector.get_queue_stats("my_queue").await?;
    
    println!("Queue Stats:");
    println!("  Completed: {}", stats.completed_count);
    println!("  Failed: {}", stats.failed_count);
    println!("  Error Rate: {:.2}%", stats.error_rate * 100.0);
    println!("  Avg Processing Time: {:?}", stats.avg_processing_time);
    
    // Alert on high error rates
    if stats.error_rate > 0.1 {
        eprintln!("HIGH ERROR RATE: {:.2}%", stats.error_rate * 100.0);
    }
}
```

## Configuration Examples by Use Case

### API Rate-Limited Jobs

```rust
let api_worker = Worker::new(queue, "api_calls".to_string(), api_handler)
    .with_rate_limit(RateLimit::per_second(5).with_burst_limit(10))
    .with_max_retries(3)
    .with_retry_delay(Duration::from_secs(60))
    .with_default_timeout(Duration::from_secs(30));
```

### Background Maintenance

```rust
let cleanup_worker = Worker::new(queue, "cleanup".to_string(), cleanup_handler)
    .with_poll_interval(Duration::from_secs(30))
    .with_priority_weights(
        PriorityWeights::builder()
            .background(10)
            .low(5)
            .normal(1)
            .high(1)
            .critical(1)
            .build()
    )
    .with_default_timeout(Duration::from_secs(3600)); // 1 hour
```

### Critical System Jobs

```rust
let critical_worker = Worker::new(queue, "system".to_string(), system_handler)
    .with_strict_priority()
    .with_poll_interval(Duration::from_millis(100))
    .with_max_retries(5)
    .with_retry_delay(Duration::from_secs(5))
    .with_default_timeout(Duration::from_secs(120));
```