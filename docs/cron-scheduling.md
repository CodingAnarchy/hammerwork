# Cron Scheduling

Hammerwork supports cron-based recurring jobs with timezone awareness. A recurring job is a
single row in the jobs table that is moved back to `Pending` at its next scheduled time after
each run.

## Basic Cron Jobs

### Creating Cron Jobs

Build a `CronSchedule`, then attach it to a job with `Job::with_cron`. `with_cron` computes
the first `next_run_at`, sets `scheduled_at` to it and marks the job as recurring.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::CronSchedule};
use serde_json::json;

// Daily backup at 2 AM UTC (seconds, minutes, hours, day, month, weekday)
let schedule = CronSchedule::new("0 0 2 * * *")?;
let backup_job = Job::new("backup".to_string(), json!({"type": "daily_backup"}))
    .with_cron(schedule)?;

assert!(backup_job.is_recurring());
queue.enqueue_cron_job(backup_job).await?;
# Ok(())
# }
```

`Job::with_cron_schedule(queue_name, payload, schedule)` is the equivalent constructor: it
builds a recurring job directly from a schedule in one step.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::CronSchedule};
use serde_json::json;

let job = Job::with_cron_schedule(
    "backup".to_string(),
    json!({"type": "daily_backup"}),
    CronSchedule::new("0 0 2 * * *")?,
)?;
assert!(job.has_cron_schedule());
# Ok(())
# }
```

### Cron Expression Format

Hammerwork uses 6-field cron expressions (the syntax of the `cron` crate). The seconds
field comes first:

```text
sec  min  hour  day-of-month  month  day-of-week
 0    0    2         *          *         *
```

Seconds are 0-59, minutes 0-59, hours 0-23, day of month 1-31 and month 1-12. Day of
week accepts names (`Mon`, `Sat`) and ranges (`Mon-Fri`); the `cron` crate numbers days
1-7 starting at Sunday, so prefer names to avoid mistakes.

Use `CronSchedule::validate` to check an expression without building a schedule:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::cron::CronSchedule;

assert!(CronSchedule::validate("0 0 2 * * *").is_ok());
assert!(CronSchedule::validate("invalid cron").is_err());
# Ok(())
# }
```

## Common Cron Patterns

### Built-in Presets

`CronSchedule` has constructors for common schedules; they return a `Result`. The
`cron::presets` module has infallible versions of the same schedules.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::{CronSchedule, presets}};
use serde_json::json;

let hourly = Job::new("cleanup".to_string(), json!({"type": "temp_files"}))
    .with_cron(CronSchedule::every_hour()?)?;

let daily = Job::new("reports".to_string(), json!({"type": "daily"}))
    .with_cron(CronSchedule::every_day_at_midnight()?)?;

let weekdays = Job::new("business".to_string(), json!({"type": "weekday"}))
    .with_cron(CronSchedule::every_weekday_at_9am()?)?;

let weekly = Job::new("weekly_report".to_string(), json!({"type": "weekly"}))
    .with_cron(CronSchedule::every_monday_at_noon()?)?;

// The presets module returns a CronSchedule directly
let heartbeat = Job::new("heartbeat".to_string(), json!({}))
    .with_cron(presets::every_minute())?;
# Ok(())
# }
```

### Custom Expressions

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::CronSchedule};
use serde_json::json;

// Every 30 minutes
let frequent = Job::new("sync".to_string(), json!({"action": "sync"}))
    .with_cron(CronSchedule::new("0 */30 * * * *")?)?;

// Every hour from 9 to 17, Monday to Friday
let business_hours = Job::new("business_sync".to_string(), json!({"type": "sync"}))
    .with_cron(CronSchedule::new("0 0 9-17 * * Mon-Fri")?)?;

// First day of every month at 9 AM
let monthly = Job::new("billing".to_string(), json!({"cycle": "monthly"}))
    .with_cron(CronSchedule::new("0 0 9 1 * *")?)?;

// Every 15 minutes during business hours
let health = Job::new("monitoring".to_string(), json!({"type": "health_check"}))
    .with_cron(CronSchedule::new("0 */15 9-17 * * Mon-Fri")?)?;
# Ok(())
# }
```

## Timezone Support

### Setting Timezones

Pass an IANA timezone name to `CronSchedule::with_timezone`. The job records the schedule's
timezone, and `next_run_at` is computed in that timezone. Use this rather than
`Job::with_timezone`, which only sets the stored timezone string and does not recompute
`next_run_at`.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::CronSchedule};

let eastern_job = Job::new("east_coast".to_string(), payload.clone())
    .with_cron(CronSchedule::with_timezone("0 0 9 * * *", "America/New_York")?)?;
assert_eq!(eastern_job.timezone.as_deref(), Some("America/New_York"));

let london_job = Job::new("london_office".to_string(), payload.clone())
    .with_cron(CronSchedule::with_timezone("0 0 9 * * Mon-Fri", "Europe/London")?)?;

let tokyo_job = Job::new("tokyo_office".to_string(), payload.clone())
    .with_cron(CronSchedule::with_timezone("0 0 9 * * Mon-Fri", "Asia/Tokyo")?)?;
# Ok(())
# }
```

An unknown timezone name is rejected with `CronError::InvalidTimezone`.

### Daylight Saving Time

Occurrences are computed in the schedule's timezone, so "9 AM" stays 9 AM local time across
daylight saving transitions while the UTC instant shifts.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::{TimeZone, Utc};
use hammerwork::cron::CronSchedule;

let schedule = CronSchedule::with_timezone("0 0 9 * * *", "America/New_York")?;

// Winter (EST, UTC-5): 9 AM local is 14:00 UTC
let winter = schedule.next_execution(Utc.with_ymd_and_hms(2026, 1, 15, 0, 0, 0).unwrap());
assert_eq!(winter, Some(Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap()));

// Summer (EDT, UTC-4): 9 AM local is 13:00 UTC
let summer = schedule.next_execution(Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap());
assert_eq!(summer, Some(Utc.with_ymd_and_hms(2026, 7, 15, 13, 0, 0).unwrap()));
# Ok(())
# }
```

## Advanced Scheduling

### Inspecting a Schedule

`CronSchedule::next_execution` returns the next occurrence after a given instant,
`next_execution_from_now` does the same from the current time, and `matches` tests whether
an instant is an occurrence.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use chrono::{TimeZone, Utc};
use hammerwork::cron::CronSchedule;

let schedule = CronSchedule::new("0 0 8,12,16,20 * * *")?; // 8 AM, 12 PM, 4 PM, 8 PM
let after = Utc.with_ymd_and_hms(2026, 3, 10, 9, 0, 0).unwrap();
assert_eq!(
    schedule.next_execution(after),
    Some(Utc.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap())
);
assert!(schedule.matches(Utc.with_ymd_and_hms(2026, 3, 10, 16, 0, 0).unwrap()));
let _next = schedule.next_execution_from_now();
# Ok(())
# }
```

### Complex Patterns

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::CronSchedule};

// The 15th and 30th of each month at 9 AM
let payroll = Job::new("payroll".to_string(), payload.clone())
    .with_cron(CronSchedule::new("0 0 9 15,30 * *")?)?;

// First day of each quarter at 9 AM
let quarterly = Job::new("quarterly_report".to_string(), payload.clone())
    .with_cron(CronSchedule::new("0 0 9 1 1,4,7,10 *")?)?;

// 2 AM on Saturday and Sunday
let weekend_only = Job::new("maintenance".to_string(), payload.clone())
    .with_cron(CronSchedule::new("0 0 2 * * Sat,Sun")?)?;

// Every 2 hours from 9 to 17 on weekdays
let business_checks = Job::new("business_hours".to_string(), payload.clone())
    .with_cron(CronSchedule::new("0 0 9-17/2 * * Mon-Fri")?)?;

// Every 5 minutes at 8-10 AM and 2-4 PM on weekdays
let peak = Job::new("peak_monitoring".to_string(), payload.clone())
    .with_cron(CronSchedule::new("0 */5 8-10,14-16 * * Mon-Fri")?)?;
# Ok(())
# }
```

## Cron Job Management

### Retrieving Due Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

// Pending recurring jobs whose next_run_at has passed (or is unset)
let due_jobs = queue.get_due_cron_jobs(None).await?;

// Only for one queue
let backup_due = queue.get_due_cron_jobs(Some("backup")).await?;

for job in due_jobs {
    println!("Job {} is due (next run {:?})", job.id, job.next_run_at);
}
# Ok(())
# }
```

Workers pick up due recurring jobs through their normal polling, so there is no need to
re-enqueue what `get_due_cron_jobs` returns; it is useful for inspection and monitoring.

### Managing Recurring Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let recurring_jobs = queue.get_recurring_jobs("backup").await?;
for job in &recurring_jobs {
    println!("Recurring job: {} - Next run: {:?}", job.id, job.next_run_at);
}

// Stop future executions: a pending run is held, a running one is not rescheduled
queue.disable_recurring_job(job_id).await?;

// Resume at the next occurrence (also revives a job whose last run finished meanwhile)
queue.enable_recurring_job(job_id).await?;

// Move a job back to Pending at an explicit time
let next = chrono::Utc::now() + chrono::Duration::hours(1);
queue.reschedule_cron_job(job_id, next).await?;
# Ok(())
# }
```

Disabling and enabling:

- `disable_recurring_job` clears `recurring` and `next_run_at`. If the job's next run is
  pending, it stays `Pending` but is held: a job with a cron schedule is only dequeued
  while it is recurring. A run already in progress finishes and is then not
  rescheduled (it ends `Completed`, `Dead` or `TimedOut`). Disabling a disabled job does
  nothing.
- `enable_recurring_job` sets `recurring` again and, unless the job is `Running` (that
  run's outcome reschedules it), schedules it for the next occurrence of its schedule
  after now: `Pending`, attempts reset. This also revives a job whose last run finished
  while it was disabled. Occurrences missed while it was disabled are not run.
- Both fail with `JobNotFound` for a missing job and with an error for a job without a
  cron schedule. `cargo hammerwork cron disable|enable <JOB_ID>` call them.

### Job Lifecycle

To run a cron-timed job only once, do not give it a schedule: compute the time from the
schedule and use `Job::with_scheduled_at`.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::CronSchedule};
use serde_json::json;

let schedule = CronSchedule::with_timezone("0 0 9 1 * *", "America/New_York")?;
if let Some(first_run) = schedule.next_execution_from_now() {
    let one_time = Job::new("special_report".to_string(), json!({}))
        .with_scheduled_at(first_run);
    assert!(!one_time.is_recurring());
    queue.enqueue(one_time).await?;
}
# Ok(())
# }
```

A recurring job is rescheduled after every run that ends, not only after a success.
Failed attempts are retried as usual (up to the job's `max_attempts`); when a run
completes, dies or times out on its last attempt, the job goes back to `Pending` at
its next scheduled time with its attempts reset. After a failed run the job keeps its
`error_message` and `failed_at` (or `timed_out_at`) until the next run, and the
worker's fail/timeout hooks and events fire as for any other job.

The next run is computed from the run's scheduled slot (its `next_run_at`), not from
the time the run ended, using the database clock:

- If the next occurrence after the slot is still ahead, the job runs then. A run that
  takes a while does not push the schedule back.
- If it has already passed (the run overran into the next slot, or no worker ran the
  job for a while), the missed occurrences are coalesced into **one** catch-up run,
  scheduled at the latest missed occurrence and therefore due immediately. After it,
  the job is back on its regular schedule. Slots are never skipped silently and never
  pile up into a burst of runs.

For example, a daily job (`0 0 0 * * *`) whose 2026-03-10 run ends at 00:05 runs
next at 2026-03-11 00:00. If that run starts days late, on 2026-03-15 at 12:00, it is
followed by one catch-up run due immediately (slot 2026-03-15 00:00) and then by the
2026-03-16 00:00 run.

## Monitoring Cron Jobs

### Queue Statistics and Overdue Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::queue::DatabaseQueue;

let stats = queue.get_queue_stats("backup").await?;
println!("Pending: {}, running: {}", stats.pending_count, stats.running_count);
println!("Completed: {}", stats.completed_count);

// A recurring job whose next_run_at is in the past has not been picked up yet
for job in queue.get_recurring_jobs("backup").await? {
    if let Some(next_run) = job.next_run_at {
        if next_run < chrono::Utc::now() {
            eprintln!("Job {} is overdue (was due {})", job.id, next_run);
        }
    }
}
# Ok(())
# }
```

### Alerting on Cron Issues

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Worker, alerting::AlertingConfig};
use std::time::Duration;

let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.1)
    .alert_on_worker_starvation(Duration::from_secs(2 * 60 * 60))
    .webhook("https://alerts.example.com/cron-failure");

let cron_worker = Worker::new(queue, "backup".to_string(), handler)
    .with_alerting_config(alerting_config);
# Ok(())
# }
```

## Best Practices

### Error Handling

An invalid expression or timezone surfaces as a `CronError` when the schedule is built:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, cron::{CronError, CronSchedule}};

match CronSchedule::new("invalid cron") {
    Ok(schedule) => {
        let job = Job::new("backup".to_string(), payload.clone()).with_cron(schedule)?;
        queue.enqueue_cron_job(job).await?;
    }
    Err(CronError::InvalidExpression(e)) => eprintln!("Invalid cron expression: {}", e),
    Err(e) => eprintln!("Bad schedule: {}", e),
}

assert!(matches!(
    CronSchedule::with_timezone("0 0 9 * * *", "Not/AZone"),
    Err(CronError::InvalidTimezone(_))
));
# Ok(())
# }
```

### Performance Considerations

Be explicit about the seconds field (`0 * * * * *` is every minute, `* * * * * *` is every
second). For high-frequency schedules, cap the worker's execution rate:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, Worker, cron::CronSchedule, rate_limit::RateLimit};

let frequent_job = Job::new("frequent_task".to_string(), payload.clone())
    .with_cron(CronSchedule::new("0 * * * * *")?)?; // every minute

let worker = Worker::new(queue, "frequent_queue".to_string(), handler)
    .with_rate_limit(RateLimit::per_minute(10));
# Ok(())
# }
```

## Example: Complete Backup System

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use std::result::Result;
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, payload: serde_json::Value, stats_collector: Arc<InMemoryStatsCollector>, job_id: JobId, job: Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, JobQueue, Worker, WorkerPool, cron::CronSchedule, queue::DatabaseQueue};
use hammerwork::worker::JobHandler;
use serde_json::json;
use std::{sync::Arc, time::Duration};

async fn setup_backup_system(
    queue: Arc<JobQueue<sqlx::Postgres>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let eastern = |expr: &str| CronSchedule::with_timezone(expr, "America/New_York");

    // Daily database backup at 2 AM Eastern
    let db_backup = Job::new(
        "backup".to_string(),
        json!({"type": "database", "retention_days": 30}),
    )
    .with_cron(eastern("0 0 2 * * *")?)?;

    // Weekly file backup on Sundays at 3 AM Eastern
    let file_backup = Job::new(
        "backup".to_string(),
        json!({"type": "files", "retention_weeks": 12}),
    )
    .with_cron(eastern("0 0 3 * * Sun")?)?;

    // Monthly archive on the first day of the month at 1 AM Eastern
    let monthly_archive = Job::new(
        "backup".to_string(),
        json!({"type": "archive", "retention_months": 12}),
    )
    .with_cron(eastern("0 0 1 1 * *")?)?;

    queue.enqueue_cron_job(db_backup).await?;
    queue.enqueue_cron_job(file_backup).await?;
    queue.enqueue_cron_job(monthly_archive).await?;

    let backup_handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            match job.payload.get("type").and_then(|v| v.as_str()) {
                Some("database") => Ok(()), // dump the database
                Some("files") => Ok(()),    // sync files
                Some("archive") => Ok(()),  // build the monthly archive
                _ => Err(hammerwork::HammerworkError::Worker {
                    message: "Unknown backup type".to_string(),
                }),
            }
        })
    });

    let backup_worker = Worker::new(queue, "backup".to_string(), backup_handler)
        .with_default_timeout(Duration::from_secs(4 * 60 * 60))
        .with_max_retries(2)
        .with_retry_delay(Duration::from_secs(30 * 60));

    let mut pool = WorkerPool::new();
    pool.add_worker(backup_worker);
    pool.start().await?;

    Ok(())
}
# Ok(())
# }
```
