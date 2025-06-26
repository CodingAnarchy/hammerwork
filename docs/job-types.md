# Job Types and Configuration

Hammerwork supports various types of jobs with flexible configuration options.

## Basic Jobs

```rust
use hammerwork::job::Job;
use serde_json::json;

// Simple job
let job = Job::new("email_queue".to_string(), json!({
    "to": "user@example.com",
    "subject": "Welcome!",
    "body": "Thanks for signing up"
}));

queue.enqueue(job).await?;
```

## Job Priority

Jobs support five priority levels:

```rust
use hammerwork::{Job, JobPriority};

let high_priority_job = Job::new("urgent".to_string(), json!({"task": "urgent_task"}))
    .with_priority(JobPriority::High);

let background_job = Job::new("cleanup".to_string(), json!({"task": "cleanup"}))
    .with_priority(JobPriority::Background);
```

### Priority Levels
- `Critical` - Highest priority (4)
- `High` - High priority (3)  
- `Normal` - Default priority (2)
- `Low` - Low priority (1)
- `Background` - Lowest priority (0)

## Delayed Jobs

Schedule jobs to run at a specific time:

```rust
use chrono::{Utc, Duration};

let delayed_job = Job::new("reminder".to_string(), json!({"user_id": 123}))
    .with_delay(Duration::hours(24)); // Run in 24 hours

let scheduled_job = Job::new("report".to_string(), json!({"type": "weekly"}))
    .with_scheduled_at(Utc::now() + Duration::days(7));
```

## Job Timeouts

Configure per-job or worker-level timeouts:

```rust
use std::time::Duration;

// Per-job timeout
let job = Job::new("long_task".to_string(), json!({"data": "..."}))
    .with_timeout(Duration::from_secs(300)); // 5 minute timeout

// Worker-level default timeout
let worker = Worker::new(queue, "default".to_string(), handler)
    .with_default_timeout(Duration::from_secs(120)); // 2 minute default
```

## Retry Configuration

Jobs automatically retry on failure with configurable limits:

```rust
let job = Job::new("api_call".to_string(), json!({"url": "https://api.example.com"}))
    .with_max_attempts(5); // Retry up to 5 times

// Worker-level retry configuration
let worker = Worker::new(queue, "default".to_string(), handler)
    .with_max_retries(3)
    .with_retry_delay(Duration::from_secs(30)); // Wait 30s between retries
```

## Cron Jobs

Schedule recurring jobs with cron expressions:

```rust
use hammerwork::cron::CronSchedule;

let cron_job = Job::new("daily_report".to_string(), json!({"type": "daily"}))
    .with_cron_schedule("0 8 * * *".parse::<CronSchedule>()?)  // Every day at 8 AM
    .with_timezone("America/New_York".to_string());

queue.enqueue_cron_job(cron_job).await?;
```

### Cron Examples
- `"0 */30 * * * *"` - Every 30 minutes
- `"0 0 9 * * MON-FRI"` - 9 AM on weekdays
- `"0 0 0 1 * *"` - First day of every month
- `"@daily"` - Once a day at midnight
- `"@hourly"` - Once an hour

## Job Status Lifecycle

Jobs progress through these states:

1. **Pending** - Waiting to be processed
2. **Running** - Currently being processed
3. **Completed** - Successfully finished
4. **Failed** - Failed but may retry
5. **Dead** - Failed permanently (exhausted retries)
6. **TimedOut** - Exceeded timeout duration
7. **Retrying** - Scheduled for retry

## Job Handlers

Define how jobs are processed:

```rust
use hammerwork::{Job, Result};

// Simple handler
let handler = Arc::new(|job: Job| {
    Box::pin(async move {
        match job.payload.get("task").and_then(|v| v.as_str()) {
            Some("send_email") => {
                let email = job.payload.get("email").unwrap().as_str().unwrap();
                send_email(email).await?;
            },
            Some("generate_report") => {
                let report_type = job.payload.get("type").unwrap().as_str().unwrap();
                generate_report(report_type).await?;
            },
            _ => {
                return Err("Unknown task type".into());
            }
        }
        Ok(())
    })
});
```

## Error Handling

Jobs can fail and be retried automatically:

```rust
let handler = Arc::new(|job: Job| {
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