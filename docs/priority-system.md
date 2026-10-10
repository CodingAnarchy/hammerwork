# Priority System

Hammerwork provides a comprehensive job prioritization system with five priority levels and multiple scheduling algorithms to ensure critical jobs are processed first while preventing starvation.

## Priority Levels

### Five Priority Tiers

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, JobPriority};
use serde_json::json;

// Five priority levels available (highest to lowest)
let critical_job = Job::new("alerts".to_string(), payload.clone()).as_critical();        // Priority 4
let high_job = Job::new("notifications".to_string(), payload.clone()).as_high_priority(); // Priority 3
let normal_job = Job::new("email".to_string(), payload.clone());                         // Priority 2 (default)
let low_job = Job::new("analytics".to_string(), payload.clone()).as_low_priority();      // Priority 1
let background_job = Job::new("cleanup".to_string(), payload.clone()).as_background();   // Priority 0

// Set priority explicitly
let custom_job = Job::new("custom".to_string(), payload.clone())
    .with_priority(JobPriority::High);
# Ok(())
# }
```

### Priority Characteristics

- **Critical (4)**: System alerts, emergency responses, critical failures
- **High (3)**: User-facing notifications, important API calls, urgent processing
- **Normal (2)**: Standard application jobs, regular processing (default)
- **Low (1)**: Analytics, non-urgent background tasks, optimization jobs
- **Background (0)**: Cleanup tasks, maintenance, lowest priority work

## Priority Scheduling Algorithms

### Weighted Priority Scheduling (Default)

Ensures high-priority jobs are processed more frequently while preventing starvation of low-priority jobs.

Each time a worker polls, it picks one of the priorities that currently have a runnable job, with probability `weight / (sum of the weights of those priorities)`, and takes the oldest job of that priority. A priority with a non-zero weight is therefore picked regularly however many higher-priority jobs are queued: with the weights below and Critical, Normal and Background jobs all waiting, Background is picked 1 time in 61. To give a priority a bigger share, raise its weight. A weight of `0` means "only when nothing with a weight is runnable".

There is no separate fairness setting (`PriorityWeights::with_fairness_factor` was removed in 2.0 because it was never applied): the weighted pick above already prevents starvation. Configuration files that still set `fairness_factor` load; the key is ignored.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::priority::PriorityWeights;

let priority_weights = PriorityWeights::new()
    .with_weight(JobPriority::Critical, 50)     // Critical jobs are 50x more likely to be selected
    .with_weight(JobPriority::High, 20)         // High jobs are 20x more likely than normal
    .with_weight(JobPriority::Normal, 10)       // Normal jobs baseline weight
    .with_weight(JobPriority::Low, 5)           // Low jobs are 2x less likely than normal
    .with_weight(JobPriority::Background, 1);   // Background jobs are 10x less likely than normal

let worker = Worker::new(queue, "priority_queue".to_string(), handler)
    .with_priority_weights(priority_weights);
# Ok(())
# }
```

### Strict Priority Scheduling

Highest priority jobs are always processed first, with lower priorities only processed when higher priorities are empty.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Strict priority mode - highest priority jobs always first
let worker = Worker::new(queue, "urgent_queue".to_string(), handler)
    .with_strict_priority();
# Ok(())
# }
```

### Understanding Weighted Selection

Each weighted dequeue first finds which priority levels have a runnable job (one
index probe per level), picks one of those levels with probability proportional to
its weight, and claims the oldest runnable job of that level. If other workers hold
all of that level's jobs, it tries the remaining levels the same way.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Example: with weights Critical 50, High 20, Normal 10, Low 5, Background 1
// and runnable jobs at the High, Normal and Background levels (any number of each):
// - High:       20 / 31 ≈ 65%
// - Normal:     10 / 31 ≈ 32%
// - Background:  1 / 31 ≈  3%
# Ok(())
# }
```

The probability of a level depends only on the weights of the levels that have
runnable jobs, not on how many jobs each level holds, so a lower priority is never
starved by a long backlog of higher-priority jobs. A level with weight `0` is only
picked when no level with runnable jobs has a weight.

## Priority Configuration Patterns

### High-Throughput System

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Favor critical and high priority jobs heavily
let high_throughput_weights = PriorityWeights::new()
    .with_weight(JobPriority::Critical, 100)    // Extremely high weight for critical
    .with_weight(JobPriority::High, 50)         // High weight for important jobs
    .with_weight(JobPriority::Normal, 10)       // Standard baseline
    .with_weight(JobPriority::Low, 2)           // Very low weight for analytics
    .with_weight(JobPriority::Background, 1);   // Minimal background processing
# Ok(())
# }
```

### Balanced Processing

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// More balanced approach ensuring all jobs get processed
let balanced_weights = PriorityWeights::new()
    .with_weight(JobPriority::Critical, 25)     // Moderate boost for critical
    .with_weight(JobPriority::High, 15)         // Moderate boost for high
    .with_weight(JobPriority::Normal, 10)       // Baseline
    .with_weight(JobPriority::Low, 7)           // Small reduction for low
    .with_weight(JobPriority::Background, 3);   // Background jobs still get reasonable processing
# Ok(())
# }
```

### Background-Heavy System

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// System that primarily processes background jobs with occasional high-priority items
let background_heavy_weights = PriorityWeights::new()
    .with_weight(JobPriority::Critical, 20)     // Critical gets priority when present
    .with_weight(JobPriority::High, 15)         // High gets some priority
    .with_weight(JobPriority::Normal, 10)       // Standard baseline
    .with_weight(JobPriority::Low, 8)           // Low priority gets good processing
    .with_weight(JobPriority::Background, 6);   // Background jobs get substantial processing time
# Ok(())
# }
```

## Priority Statistics and Monitoring

### Collecting Priority Statistics

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::stats::InMemoryStatsCollector;

let stats_collector = Arc::new(InMemoryStatsCollector::new_default());
let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_stats_collector(stats_collector.clone());

// Get priority-specific statistics
let priority_stats = stats_collector.get_system_statistics(Duration::from_secs(300)).await?.priority_stats.unwrap_or_default();

println!("Priority Distribution:");
for priority in JobPriority::all_priorities() {
    let count = priority_stats.job_counts.get(&priority).copied().unwrap_or(0);
    let share = priority_stats.priority_distribution.get(&priority).copied().unwrap_or(0.0);
    println!("  {priority:?}: {count} ({share:.1}%)");
}

println!("Most active priority: {:?}", priority_stats.most_active_priority());
# Ok(())
# }
```

### Starvation Detection

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Detect when lower priority jobs aren't getting processed
if !priority_stats.check_starvation(2.0).is_empty() { // 2% threshold
    eprintln!("WARNING: Priority starvation detected!");
    eprintln!("Lower priority jobs may not be getting processed adequately");

    // Consider raising the weights of the lower priorities
    let adjusted_weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 30)  // Reduce critical weight
        .with_weight(JobPriority::High, 15)      // Reduce high weight
        .with_weight(JobPriority::Normal, 10)
        .with_weight(JobPriority::Low, 8)
        .with_weight(JobPriority::Background, 5);
}
# Ok(())
# }
```

### Real-time Monitoring

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
use tokio::time::{interval, Duration};

async fn monitor_priority_distribution(stats_collector: Arc<InMemoryStatsCollector>) {
    let mut monitor_interval = interval(Duration::from_secs(60));

    loop {
        monitor_interval.tick().await;

        let stats = stats_collector.get_system_statistics(Duration::from_secs(300)).await.unwrap().priority_stats.unwrap_or_default();

        // Log priority distribution
        let count = |p| stats.job_counts.get(&p).copied().unwrap_or(0);
        println!("Priority Stats - C:{} H:{} N:{} L:{} B:{}",
                 count(JobPriority::Critical), count(JobPriority::High), count(JobPriority::Normal),
                 count(JobPriority::Low), count(JobPriority::Background));

        // Alert on starvation
        if !stats.check_starvation(5.0).is_empty() {
            eprintln!("ALERT: Priority starvation detected!");
        }

        // Alert on priority imbalance (too much critical)
        let critical_share = stats.priority_distribution.get(&JobPriority::Critical).copied().unwrap_or(0.0);
        if critical_share > 80.0 {
            eprintln!("ALERT: Over 80% critical jobs - may indicate system issues");
        }

        // Alert on lack of activity
        if stats.job_counts.values().sum::<u64>() == 0 {
            eprintln!("ALERT: No jobs processed in monitoring window");
        }
    }
}
# Ok(())
# }
```

## Advanced Priority Patterns

### Queue-Specific Priority Strategies

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Different strategies for different types of work
let api_worker = Worker::new(queue.clone(), "api_calls".to_string(), api_handler)
    .with_strict_priority(); // API calls always process highest priority first

let background_worker = Worker::new(queue.clone(), "maintenance".to_string(), maintenance_handler)
    .with_priority_weights(
        PriorityWeights::new()
            .with_weight(JobPriority::Critical, 5)      // Even critical maintenance is lower priority
            .with_weight(JobPriority::High, 3)
            .with_weight(JobPriority::Normal, 2)
            .with_weight(JobPriority::Low, 2)
            .with_weight(JobPriority::Background, 1)
    );

let email_worker = Worker::new(queue.clone(), "email".to_string(), email_handler)
    .with_priority_weights(
        PriorityWeights::new()
            .with_weight(JobPriority::Critical, 30)     // Balanced approach for email processing
            .with_weight(JobPriority::High, 15)
            .with_weight(JobPriority::Normal, 10)
            .with_weight(JobPriority::Low, 5)
            .with_weight(JobPriority::Background, 2)
    );
# Ok(())
# }
```

### Dynamic Priority Adjustment

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Adjust job priority based on age or other factors
async fn enqueue_with_dynamic_priority(
    queue: &JobQueue<sqlx::Postgres>,
    job_type: &str,
    payload: serde_json::Value,
    urgency_factor: f64
) -> Result<(), Box<dyn std::error::Error>> {
    let priority = match urgency_factor {
        f if f >= 0.9 => JobPriority::Critical,
        f if f >= 0.7 => JobPriority::High,
        f if f >= 0.5 => JobPriority::Normal,
        f if f >= 0.3 => JobPriority::Low,
        _ => JobPriority::Background,
    };

    let job = Job::new(job_type.to_string(), payload)
        .with_priority(priority);

    queue.enqueue(job).await?;
    Ok(())
}
# Ok(())
# }
```

### Priority-Aware Rate Limiting

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::rate_limit::RateLimit;

// Higher priority jobs get higher rate limits
let worker = Worker::new(queue, "priority_limited".to_string(), handler)
    .with_priority_weights(
        PriorityWeights::new()
            .with_weight(JobPriority::Critical, 50)
            .with_weight(JobPriority::High, 25)
            .with_weight(JobPriority::Normal, 10)
            .with_weight(JobPriority::Low, 5)
            .with_weight(JobPriority::Background, 1)
    )
    .with_rate_limit(RateLimit::per_second(10)); // Overall rate limit

// Consider different rate limits for different priority levels in your handler
let priority_aware_handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Apply different processing delays based on priority
        match job.priority {
            JobPriority::Critical => {
                // No additional delay for critical jobs
            },
            JobPriority::High => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            },
            JobPriority::Normal => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
            JobPriority::Low => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            },
            JobPriority::Background => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            },
        }

        // Process the job
        process_job(&job).await
    })
});
# Ok(())
# }
```

## Best Practices

### Priority Assignment Guidelines

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Use Critical sparingly - only for true emergencies
let system_alert = Job::new("system_down_alert".to_string(), alert_data)
    .as_critical(); // Appropriate use

let user_signup_email = Job::new("welcome_email".to_string(), user_data)
    .as_high_priority(); // NOT critical - use High instead

// Use appropriate priorities for job types
let user_facing = Job::new("send_notification".to_string(), notification)
    .as_high_priority(); // User-facing, but not critical

let reporting = Job::new("generate_report".to_string(), report_params)
    .with_priority(JobPriority::Normal); // Standard business logic

let analytics = Job::new("update_metrics".to_string(), metrics_data)
    .as_low_priority(); // Important but not urgent

let cleanup = Job::new("cleanup_temp_files".to_string(), json!({}))
    .as_background(); // Can wait indefinitely
# Ok(())
# }
```

### Preventing Priority Abuse

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Validate priority assignments in your application logic
fn validate_job_priority(job_type: &str, requested_priority: JobPriority) -> JobPriority {
    match job_type {
        "system_alert" | "security_incident" => JobPriority::Critical,
        "user_notification" | "api_response" => {
            // Cap user-requested jobs at High priority
            if requested_priority as u8 > JobPriority::High as u8 {
                JobPriority::High
            } else {
                requested_priority
            }
        },
        "analytics" | "reporting" => {
            // Analytics jobs should never be higher than Normal
            if requested_priority as u8 > JobPriority::Normal as u8 {
                JobPriority::Normal
            } else {
                requested_priority
            }
        },
        "cleanup" | "maintenance" => JobPriority::Background,
        _ => JobPriority::Normal, // Default for unknown job types
    }
}
# Ok(())
# }
```

### Monitoring and Alerting

```rust,no_run
# #[allow(unused_imports)] use hammerwork::{*, priority::*, queue::*, worker::*, stats::*};
# #[allow(unused_imports)] use serde_json::json;
# #[allow(unused_imports)] use std::{sync::Arc, time::Duration};
# #[allow(unused_variables, unused_mut, dead_code)]
# #[allow(unused_imports)] use std::result::Result;
# async fn send_priority_alert(_alert: &serde_json::Value) {}
# async fn process_job(_job: &Job) -> hammerwork::Result<()> { Ok(()) }
# async fn doc(queue: Arc<JobQueue<sqlx::Postgres>>, handler: JobHandler, api_handler: JobHandler, email_handler: JobHandler, maintenance_handler: JobHandler, payload: serde_json::Value, alert_data: serde_json::Value, user_data: serde_json::Value, notification: serde_json::Value, report_params: serde_json::Value, metrics_data: serde_json::Value, priority_stats: PriorityStats) -> std::result::Result<(), Box<dyn std::error::Error>> {
// Set up alerting for priority system health
let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.1)
    .webhook("https://alerts.example.com/priority-system")
    .with_cooldown(Duration::from_secs(5 * 60));

// Custom alert for priority starvation
async fn check_priority_health(
    stats_collector: Arc<InMemoryStatsCollector>,
    alerting: Arc<AlertingConfig>
) {
    let stats = stats_collector.get_system_statistics(Duration::from_secs(300)).await.unwrap().priority_stats.unwrap_or_default();

    if !stats.check_starvation(5.0).is_empty() {
        // Send custom alert
        let alert_payload = json!({
            "alert_type": "priority_starvation",
            "critical_percentage": stats.priority_distribution.get(&JobPriority::Critical),
            "background_percentage": stats.priority_distribution.get(&JobPriority::Background),
            "total_jobs": stats.job_counts.values().sum::<u64>(),
            "recommendation": "Consider raising the weights of the lower priorities"
        });

        // Send alert via webhook (implement based on your alerting setup)
        send_priority_alert(&alert_payload).await;
    }
}
# Ok(())
# }
```