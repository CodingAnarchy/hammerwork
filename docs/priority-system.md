# Priority System

Hammerwork provides a comprehensive job prioritization system with five priority levels and multiple scheduling algorithms to ensure critical jobs are processed first while preventing starvation.

## Priority Levels

### Five Priority Tiers

```rust
use hammerwork::{Job, JobPriority};
use serde_json::json;

// Five priority levels available (highest to lowest)
let critical_job = Job::new("alerts".to_string(), payload).as_critical();        // Priority 4
let high_job = Job::new("notifications".to_string(), payload).as_high_priority(); // Priority 3
let normal_job = Job::new("email".to_string(), payload);                         // Priority 2 (default)
let low_job = Job::new("analytics".to_string(), payload).as_low_priority();      // Priority 1
let background_job = Job::new("cleanup".to_string(), payload).as_background();   // Priority 0

// Set priority explicitly
let custom_job = Job::new("custom".to_string(), payload)
    .with_priority(JobPriority::High);
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

```rust
use hammerwork::priority::PriorityWeights;

let priority_weights = PriorityWeights::builder()
    .critical(50)     // Critical jobs are 50x more likely to be selected
    .high(20)         // High jobs are 20x more likely than normal
    .normal(10)       // Normal jobs baseline weight
    .low(5)           // Low jobs are 2x less likely than normal
    .background(1)    // Background jobs are 10x less likely than normal
    .fairness_factor(0.1) // 10% chance to select from lower priorities
    .build();

let worker = Worker::new(queue, "priority_queue".to_string(), handler)
    .with_priority_weights(priority_weights);
```

### Strict Priority Scheduling

Highest priority jobs are always processed first, with lower priorities only processed when higher priorities are empty.

```rust
// Strict priority mode - highest priority jobs always first
let worker = Worker::new(queue, "urgent_queue".to_string(), handler)
    .with_strict_priority();
```

### Understanding Weighted Selection

```rust
// Example: With default weights (50, 20, 10, 5, 1)
// If queue has: 1 Critical, 2 High, 10 Normal, 5 Low, 3 Background jobs
// Selection probability:
// - Critical: ~50% chance
// - High: ~20% chance  
// - Normal: ~10% chance
// - Low: ~5% chance
// - Background: ~1% chance
// Plus 10% fairness factor ensuring lower priorities get some processing time
```

## Priority Configuration Patterns

### High-Throughput System

```rust
// Favor critical and high priority jobs heavily
let high_throughput_weights = PriorityWeights::builder()
    .critical(100)    // Extremely high weight for critical
    .high(50)         // High weight for important jobs
    .normal(10)       // Standard baseline
    .low(2)           // Very low weight for analytics
    .background(1)    // Minimal background processing
    .fairness_factor(0.05) // Only 5% fairness to maximize critical job processing
    .build();
```

### Balanced Processing

```rust
// More balanced approach ensuring all jobs get processed
let balanced_weights = PriorityWeights::builder()
    .critical(25)     // Moderate boost for critical
    .high(15)         // Moderate boost for high
    .normal(10)       // Baseline
    .low(7)           // Small reduction for low
    .background(3)    // Background jobs still get reasonable processing
    .fairness_factor(0.2) // 20% fairness ensures good coverage
    .build();
```

### Background-Heavy System

```rust
// System that primarily processes background jobs with occasional high-priority items
let background_heavy_weights = PriorityWeights::builder()
    .critical(20)     // Critical gets priority when present
    .high(15)         // High gets some priority
    .normal(10)       // Standard baseline
    .low(8)           // Low priority gets good processing
    .background(6)    // Background jobs get substantial processing time
    .fairness_factor(0.3) // High fairness ensures background jobs aren't starved
    .build();
```

## Priority Statistics and Monitoring

### Collecting Priority Statistics

```rust
use hammerwork::stats::InMemoryStatsCollector;

let stats_collector = Arc::new(InMemoryStatsCollector::new());
let worker = Worker::new(queue, "monitored_queue".to_string(), handler)
    .with_stats_collector(stats_collector.clone());

// Get priority-specific statistics
let priority_stats = stats_collector.get_priority_stats().await?;

println!("Priority Distribution:");
println!("  Critical: {} ({:.1}%)", priority_stats.critical, priority_stats.critical_percentage());
println!("  High: {} ({:.1}%)", priority_stats.high, priority_stats.high_percentage());
println!("  Normal: {} ({:.1}%)", priority_stats.normal, priority_stats.normal_percentage());
println!("  Low: {} ({:.1}%)", priority_stats.low, priority_stats.low_percentage());
println!("  Background: {} ({:.1}%)", priority_stats.background, priority_stats.background_percentage());

println!("Most active priority: {:?}", priority_stats.most_active_priority());
```

### Starvation Detection

```rust
// Detect when lower priority jobs aren't getting processed
if priority_stats.has_starvation_risk(0.02) { // 2% threshold
    eprintln!("WARNING: Priority starvation detected!");
    eprintln!("Lower priority jobs may not be getting processed adequately");
    
    // Consider adjusting weights or fairness factor
    let adjusted_weights = PriorityWeights::builder()
        .critical(30)  // Reduce critical weight
        .high(15)      // Reduce high weight
        .normal(10)
        .low(8)
        .background(5)
        .fairness_factor(0.25) // Increase fairness
        .build();
}
```

### Real-time Monitoring

```rust
use tokio::time::{interval, Duration};

async fn monitor_priority_distribution(stats_collector: Arc<InMemoryStatsCollector>) {
    let mut monitor_interval = interval(Duration::from_secs(60));
    
    loop {
        monitor_interval.tick().await;
        
        let stats = stats_collector.get_priority_stats().await.unwrap();
        
        // Log priority distribution
        println!("Priority Stats - C:{} H:{} N:{} L:{} B:{}", 
                 stats.critical, stats.high, stats.normal, stats.low, stats.background);
        
        // Alert on starvation
        if stats.has_starvation_risk(0.05) {
            eprintln!("ALERT: Priority starvation detected!");
        }
        
        // Alert on priority imbalance (too much critical)
        if stats.critical_percentage() > 80.0 {
            eprintln!("ALERT: Over 80% critical jobs - may indicate system issues");
        }
        
        // Alert on lack of activity
        if stats.total() == 0 {
            eprintln!("ALERT: No jobs processed in monitoring window");
        }
    }
}
```

## Advanced Priority Patterns

### Queue-Specific Priority Strategies

```rust
// Different strategies for different types of work
let api_worker = Worker::new(queue.clone(), "api_calls".to_string(), api_handler)
    .with_strict_priority(); // API calls always process highest priority first

let background_worker = Worker::new(queue.clone(), "maintenance".to_string(), maintenance_handler)
    .with_priority_weights(
        PriorityWeights::builder()
            .critical(5)      // Even critical maintenance is lower priority
            .high(3)
            .normal(2)
            .low(2)
            .background(1)
            .fairness_factor(0.5) // Very fair processing
            .build()
    );

let email_worker = Worker::new(queue.clone(), "email".to_string(), email_handler)
    .with_priority_weights(
        PriorityWeights::builder()
            .critical(30)     // Balanced approach for email processing
            .high(15)
            .normal(10)
            .low(5)
            .background(2)
            .fairness_factor(0.15)
            .build()
    );
```

### Dynamic Priority Adjustment

```rust
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
```

### Priority-Aware Rate Limiting

```rust
use hammerwork::rate_limit::RateLimit;

// Higher priority jobs get higher rate limits
let worker = Worker::new(queue, "priority_limited".to_string(), handler)
    .with_priority_weights(
        PriorityWeights::builder()
            .critical(50)
            .high(25)
            .normal(10)
            .low(5)
            .background(1)
            .fairness_factor(0.1)
            .build()
    )
    .with_rate_limit(RateLimit::per_second(10)); // Overall rate limit

// Consider different rate limits for different priority levels in your handler
let priority_aware_handler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Apply different processing delays based on priority
        match job.priority {
            Some(JobPriority::Critical) => {
                // No additional delay for critical jobs
            },
            Some(JobPriority::High) => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            },
            Some(JobPriority::Normal) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
            Some(JobPriority::Low) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            },
            Some(JobPriority::Background) | None => {
                tokio::time::sleep(Duration::from_millis(200)).await;
            },
        }
        
        // Process the job
        process_job(&job).await
    })
});
```

## Best Practices

### Priority Assignment Guidelines

```rust
// Use Critical sparingly - only for true emergencies
let system_alert = Job::new("system_down_alert".to_string(), alert_data)
    .as_critical(); // Appropriate use

let user_signup_email = Job::new("welcome_email".to_string(), user_data)
    .as_high_priority(); // NOT critical - use High instead

// Use appropriate priorities for job types
let user_facing = Job::new("send_notification".to_string(), notification)
    .as_high_priority(); // User-facing, but not critical

let reporting = Job::new("generate_report".to_string(), report_params)
    .as_normal_priority(); // Standard business logic

let analytics = Job::new("update_metrics".to_string(), metrics_data)
    .as_low_priority(); // Important but not urgent

let cleanup = Job::new("cleanup_temp_files".to_string(), json!({}))
    .as_background(); // Can wait indefinitely
```

### Preventing Priority Abuse

```rust
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
```

### Monitoring and Alerting

```rust
// Set up alerting for priority system health
let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.1)
    .webhook("https://alerts.example.com/priority-system")
    .with_cooldown(Duration::from_minutes(5));

// Custom alert for priority starvation
async fn check_priority_health(
    stats_collector: Arc<InMemoryStatsCollector>,
    alerting: Arc<AlertingConfig>
) {
    let stats = stats_collector.get_priority_stats().await.unwrap();
    
    if stats.has_starvation_risk(0.05) {
        // Send custom alert
        let alert_payload = json!({
            "alert_type": "priority_starvation",
            "critical_percentage": stats.critical_percentage(),
            "background_percentage": stats.background_percentage(),
            "total_jobs": stats.total(),
            "recommendation": "Consider adjusting priority weights or fairness factor"
        });
        
        // Send alert via webhook (implement based on your alerting setup)
        send_priority_alert(&alert_payload).await;
    }
}
```