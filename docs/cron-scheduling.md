# Cron Scheduling

Hammerwork provides comprehensive cron-based job scheduling with timezone awareness for recurring jobs.

## Basic Cron Jobs

### Creating Cron Jobs

```rust
use hammerwork::{Job, cron::CronSchedule};
use chrono_tz::US::Eastern;
use serde_json::json;

// Daily backup at 2 AM Eastern
let backup_job = Job::new("backup".to_string(), json!({"type": "daily_backup"}))
    .with_cron("0 0 2 * * *")?  // seconds, minutes, hours, day, month, weekday
    .with_timezone(Eastern)
    .as_recurring();

queue.enqueue_cron_job(backup_job).await?;
```

### Cron Expression Format

Hammerwork uses 6-field cron expressions:

```
┌─────────────── seconds (0-59)
│ ┌───────────── minutes (0-59)
│ │ ┌─────────── hours (0-23)
│ │ │ ┌───────── day of month (1-31)
│ │ │ │ ┌─────── month (1-12)
│ │ │ │ │ ┌───── day of week (0-6, Sunday=0)
│ │ │ │ │ │
* * * * * *
```

## Common Cron Patterns

### Built-in Presets

```rust
use hammerwork::cron::CronSchedule;

// Predefined common schedules
let hourly = Job::new("cleanup".to_string(), json!({"type": "temp_files"}))
    .with_cron_schedule(CronSchedule::every_hour())
    .as_recurring();

let daily = Job::new("reports".to_string(), json!({"type": "daily"}))
    .with_cron_schedule(CronSchedule::daily_at_midnight())
    .as_recurring();

let weekdays = Job::new("business".to_string(), json!({"type": "weekday"}))
    .with_cron_schedule(CronSchedule::weekdays_at_9am())
    .as_recurring();

let weekly = Job::new("weekly_report".to_string(), json!({"type": "weekly"}))
    .with_cron_schedule(CronSchedule::mondays_at_noon())
    .as_recurring();
```

### Custom Expressions

```rust
// Every 30 minutes
let frequent = Job::new("sync".to_string(), json!({"action": "sync"}))
    .with_cron("0 */30 * * * *")?
    .as_recurring();

// Business hours only (9 AM to 5 PM, weekdays)
let business_hours = Job::new("business_sync".to_string(), json!({"type": "sync"}))
    .with_cron("0 0 9-17 * * 1-5")?  // Every hour from 9-17, Mon-Fri
    .as_recurring();

// First day of every month at 9 AM
let monthly = Job::new("billing".to_string(), json!({"cycle": "monthly"}))
    .with_cron("0 0 9 1 * *")?
    .as_recurring();

// Every 15 minutes during business hours
let frequent_business = Job::new("monitoring".to_string(), json!({"type": "health_check"}))
    .with_cron("0 */15 9-17 * * 1-5")?
    .as_recurring();
```

## Timezone Support

### Setting Timezones

```rust
use chrono_tz::{US, Europe, Asia};

// US timezones
let eastern_job = Job::new("east_coast".to_string(), payload)
    .with_cron("0 0 9 * * *")?  // 9 AM Eastern
    .with_timezone(US::Eastern)
    .as_recurring();

let pacific_job = Job::new("west_coast".to_string(), payload)
    .with_cron("0 0 9 * * *")?  // 9 AM Pacific
    .with_timezone(US::Pacific)
    .as_recurring();

// International timezones
let london_job = Job::new("london_office".to_string(), payload)
    .with_cron("0 0 9 * * 1-5")?  // 9 AM London, weekdays
    .with_timezone(Europe::London)
    .as_recurring();

let tokyo_job = Job::new("tokyo_office".to_string(), payload)
    .with_cron("0 0 9 * * 1-5")?  // 9 AM Tokyo, weekdays
    .with_timezone(Asia::Tokyo)
    .as_recurring();
```

### Timezone Considerations

```rust
// Jobs are scheduled in the specified timezone
// Automatically handles daylight saving time transitions
let dst_aware = Job::new("dst_test".to_string(), payload)
    .with_cron("0 0 2 * * *")?  // 2 AM - handles DST transitions
    .with_timezone(US::Eastern)
    .as_recurring();
```

## Advanced Scheduling

### Complex Patterns

```rust
// Multiple specific times
let multi_daily = Job::new("multi_sync".to_string(), payload)
    .with_cron("0 0 8,12,16,20 * * *")?  // 8 AM, 12 PM, 4 PM, 8 PM
    .as_recurring();

// Specific days of month
let payroll = Job::new("payroll".to_string(), payload)
    .with_cron("0 0 9 15,30 * *")?  // 15th and 30th of each month at 9 AM
    .as_recurring();

// Quarterly reports (first day of quarter)
let quarterly = Job::new("quarterly_report".to_string(), payload)
    .with_cron("0 0 9 1 1,4,7,10 *")?  // Jan 1, Apr 1, Jul 1, Oct 1 at 9 AM
    .as_recurring();

// Weekend maintenance
let weekend_only = Job::new("maintenance".to_string(), payload)
    .with_cron("0 0 2 * * 0,6")?  // 2 AM on Saturday and Sunday
    .as_recurring();
```

### Range Expressions

```rust
// Every weekday at various times
let business_checks = Job::new("business_hours".to_string(), payload)
    .with_cron("0 0 9-17/2 * * 1-5")?  // Every 2 hours from 9-17, weekdays
    .as_recurring();

// Every 5 minutes during peak hours
let peak_monitoring = Job::new("peak_monitoring".to_string(), payload)
    .with_cron("0 */5 8-10,14-16 * * 1-5")?  // Every 5 min, 8-10 AM & 2-4 PM
    .as_recurring();
```

## Cron Job Management

### Retrieving Due Jobs

```rust
use hammerwork::queue::DatabaseQueue;

// Get all jobs ready to run
let due_jobs = queue.get_due_cron_jobs(None).await?;

// Get jobs for specific queue
let queue_jobs = queue.get_due_cron_jobs(Some("backup")).await?;

// Process due jobs
for job in due_jobs {
    println!("Job {} is ready to run", job.id);
    queue.enqueue(job).await?;
}
```

### Managing Recurring Jobs

```rust
// List all recurring jobs for a queue
let recurring_jobs = queue.get_recurring_jobs("backup").await?;

for job in recurring_jobs {
    println!("Recurring job: {} - Next run: {:?}", job.id, job.next_run_at);
}

// Disable a recurring job temporarily
queue.disable_recurring_job(&job_id).await?;

// Re-enable a recurring job
queue.enable_recurring_job(&job_id).await?;

// Manually reschedule a job
queue.reschedule_cron_job(&job_id).await?;
```

### Job Lifecycle

```rust
// When a recurring job completes, it's automatically rescheduled
// The next_run_at is calculated based on the cron expression

// For one-time execution of a recurring pattern
let one_time = Job::new("special_report".to_string(), payload)
    .with_cron("0 0 9 1 * *")?  // First of month at 9 AM
    .with_timezone(US::Eastern)
    // Note: NOT marked as recurring - runs once
    ;

queue.enqueue_cron_job(one_time).await?;
```

## Monitoring Cron Jobs

### Cron Job Statistics

```rust
// Monitor cron job execution
let stats = stats_collector.get_queue_stats("backup").await?;
println!("Backup jobs completed: {}", stats.completed_count);
println!("Average backup time: {:?}", stats.avg_processing_time);

// Check for missed executions
let recurring_jobs = queue.get_recurring_jobs("backup").await?;
for job in recurring_jobs {
    if let Some(next_run) = job.next_run_at {
        if next_run < chrono::Utc::now() {
            eprintln!("Job {} appears to have missed its scheduled time", job.id);
        }
    }
}
```

### Alerting on Cron Issues

```rust
// Alert when cron jobs fail
let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.1)  // Alert if 10% of cron jobs fail
    .alert_on_worker_starvation(Duration::from_hours(2))  // Alert if no jobs run for 2 hours
    .webhook("https://alerts.example.com/cron-failure");

let cron_worker = Worker::new(queue, "backup".to_string(), backup_handler)
    .with_alerting_config(alerting_config);
```

## Best Practices

### Cron Expression Guidelines

```rust
// Be explicit about seconds to avoid confusion
let explicit = Job::new("task".to_string(), payload)
    .with_cron("0 30 14 * * *")?  // 2:30 PM daily (explicit 0 seconds)
    .as_recurring();

// Use meaningful names for recurring jobs
let descriptive = Job::new("daily_user_analytics".to_string(), payload)
    .with_cron("0 0 1 * * *")?  // 1 AM daily
    .as_recurring();

// Consider timezone for your users
let user_timezone = Job::new("user_notification".to_string(), payload)
    .with_cron("0 0 9 * * *")?  // 9 AM in user's timezone
    .with_timezone(chrono_tz::America::New_York)
    .as_recurring();
```

### Error Handling

```rust
// Handle cron parsing errors gracefully
match Job::new("backup".to_string(), payload).with_cron("invalid cron") {
    Ok(job) => queue.enqueue_cron_job(job).await?,
    Err(e) => {
        eprintln!("Invalid cron expression: {}", e);
        // Use fallback schedule or notify admin
    }
}

// Validate cron expressions before storing
fn validate_cron_expression(expr: &str) -> Result<(), cron::error::Error> {
    expr.parse::<cron::Schedule>()?;
    Ok(())
}
```

### Performance Considerations

```rust
// For high-frequency jobs, consider rate limiting
let frequent_job = Job::new("frequent_task".to_string(), payload)
    .with_cron("0 * * * * *")?  // Every minute
    .as_recurring();

let worker = Worker::new(queue, "frequent_queue".to_string(), handler)
    .with_rate_limit(RateLimit::per_minute(10));  // Limit execution rate
```

## Example: Complete Backup System

```rust
use hammerwork::{Job, Worker, queue::DatabaseQueue};
use chrono_tz::US::Eastern;
use serde_json::json;

async fn setup_backup_system(queue: Arc<JobQueue<sqlx::Postgres>>) -> Result<(), Box<dyn std::error::Error>> {
    // Daily database backup at 2 AM Eastern
    let db_backup = Job::new("backup".to_string(), json!({"type": "database", "retention_days": 30}))
        .with_cron("0 0 2 * * *")?
        .with_timezone(Eastern)
        .as_recurring();

    // Weekly file backup on Sundays at 3 AM Eastern
    let file_backup = Job::new("backup".to_string(), json!({"type": "files", "retention_weeks": 12}))
        .with_cron("0 0 3 * * 0")?
        .with_timezone(Eastern)
        .as_recurring();

    // Monthly archive on first day of month at 1 AM Eastern
    let monthly_archive = Job::new("backup".to_string(), json!({"type": "archive", "retention_months": 12}))
        .with_cron("0 0 1 1 * *")?
        .with_timezone(Eastern)
        .as_recurring();

    // Enqueue all backup jobs
    queue.enqueue_cron_job(db_backup).await?;
    queue.enqueue_cron_job(file_backup).await?;
    queue.enqueue_cron_job(monthly_archive).await?;

    // Set up worker to process backups
    let backup_handler = Arc::new(|job: Job| {
        Box::pin(async move {
            match job.payload.get("type").and_then(|v| v.as_str()) {
                Some("database") => perform_database_backup(&job).await,
                Some("files") => perform_file_backup(&job).await,
                Some("archive") => perform_monthly_archive(&job).await,
                _ => Err("Unknown backup type".into()),
            }
        })
    });

    let backup_worker = Worker::new(queue, "backup".to_string(), backup_handler)
        .with_default_timeout(Duration::from_hours(4))  // 4 hour timeout for backups
        .with_max_retries(2)
        .with_retry_delay(Duration::from_minutes(30));

    // Start worker (in practice, you'd add this to a WorkerPool)
    backup_worker.start().await?;

    Ok(())
}
```