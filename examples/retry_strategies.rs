//! Advanced Retry Strategies Example
//!
//! This example demonstrates all the retry strategies available in Hammerwork,
//! showing how to configure jobs and workers with different retry patterns
//! for various failure scenarios.
//!
//! Run with:
//! ```bash
//! # PostgreSQL
//! cargo run --example retry_strategies --features postgres
//!
//! # MySQL  
//! cargo run --example retry_strategies --features mysql
//! ```

use hammerwork::{
    HammerworkError, Job, JobQueue, Worker, WorkerPool,
    queue::DatabaseQueue,
    retry::{JitterType, RetryStrategy},
    worker::JobHandler,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tracing::{info, warn};

// Conditional type aliases for database-specific JobQueue types
#[cfg(all(feature = "postgres", not(feature = "mysql")))]
type ExampleJobQueue = JobQueue<sqlx::Postgres>;

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
type ExampleJobQueue = JobQueue<sqlx::MySql>;

#[cfg(all(feature = "postgres", feature = "mysql"))]
type ExampleJobQueue = JobQueue<sqlx::Postgres>; // Default to postgres when both enabled

/// Helper function to get database URL based on feature flags
fn get_database_url() -> String {
    #[cfg(all(feature = "postgres", not(feature = "mysql")))]
    return "postgresql://localhost/hammerwork".to_string();
    #[cfg(all(feature = "mysql", not(feature = "postgres")))]
    return "mysql://localhost/hammerwork".to_string();
    #[cfg(all(feature = "postgres", feature = "mysql"))]
    return "postgresql://localhost/hammerwork".to_string(); // Default to postgres when both enabled
    #[cfg(not(any(feature = "postgres", feature = "mysql")))]
    panic!("No database feature enabled. Use --features postgres or --features mysql");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    info!("🔄 Hammerwork Advanced Retry Strategies Example");
    info!("This example demonstrates all retry strategies available in Hammerwork");

    // Connect to database
    let database_url = get_database_url();
    info!("🔗 Connecting to database: {}", database_url);

    let pool;
    #[cfg(all(feature = "postgres", not(feature = "mysql")))]
    {
        pool = sqlx::PgPool::connect(&database_url).await?;
    }

    #[cfg(all(feature = "mysql", not(feature = "postgres")))]
    {
        pool = sqlx::MySqlPool::connect(&database_url).await?;
    }

    // Default to PostgreSQL when both features are enabled
    #[cfg(all(feature = "postgres", feature = "mysql"))]
    {
        pool = sqlx::PgPool::connect(&database_url).await?;
    }

    let queue = Arc::new(JobQueue::new(pool));

    // Note: Database tables should be created using migrations
    // Run `cargo run --bin hammerwork migrate` before running this example
    info!("📋 Database ready (tables should be created via migrations)");

    info!("✅ Database setup complete!");
    info!("");

    // Demonstrate all retry strategies
    demonstrate_fixed_retry_strategy(&queue).await?;
    demonstrate_linear_backoff_strategy(&queue).await?;
    demonstrate_exponential_backoff_strategy(&queue).await?;
    demonstrate_exponential_with_jitter(&queue).await?;
    demonstrate_fibonacci_backoff_strategy(&queue).await?;
    demonstrate_custom_retry_strategy(&queue).await?;
    demonstrate_worker_default_strategies(&queue).await?;
    demonstrate_mixed_strategies_in_action(&queue).await?;

    info!("🎉 All retry strategy demonstrations completed!");
    info!("Check the logs above to see how different strategies calculate retry delays.");

    Ok(())
}

/// Demonstrate fixed delay retry strategy (original Hammerwork behavior)
async fn demonstrate_fixed_retry_strategy(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🔧 FIXED RETRY STRATEGY");
    info!("Uses the same delay for every retry attempt (classic behavior)");

    // Create job with fixed retry strategy
    let job = Job::new(
        "fixed_retry_demo".to_string(),
        json!({
            "task": "api_call",
            "url": "https://api.example.com/data",
            "description": "Simple API call with fixed 10 second retries"
        }),
    )
    .with_retry_strategy(RetryStrategy::fixed(Duration::from_secs(10)))
    .with_max_attempts(5);

    // Show calculated delays
    let strategy = RetryStrategy::fixed(Duration::from_secs(10));
    info!(
        "Retry delays: attempt 1 → {}s, attempt 2 → {}s, attempt 3 → {}s",
        strategy.calculate_delay(1).as_secs(),
        strategy.calculate_delay(2).as_secs(),
        strategy.calculate_delay(3).as_secs()
    );

    queue.enqueue(job).await?;
    info!("✅ Fixed retry job enqueued");
    info!("");

    Ok(())
}

/// Demonstrate linear backoff retry strategy
async fn demonstrate_linear_backoff_strategy(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("📈 LINEAR BACKOFF STRATEGY");
    info!("Increases delay by a fixed amount for each retry attempt");

    // Create job with linear backoff strategy
    let job = Job::new(
        "linear_backoff_demo".to_string(),
        json!({
            "task": "database_operation",
            "query": "SELECT * FROM large_table WHERE condition = ?",
            "description": "Database query that might fail due to locks or load"
        }),
    )
    .with_linear_backoff(
        Duration::from_secs(5),        // base delay
        Duration::from_secs(10),       // increment per attempt
        Some(Duration::from_secs(60)), // max delay cap
    )
    .with_max_attempts(7);

    // Show calculated delays
    let strategy = RetryStrategy::linear(
        Duration::from_secs(5),
        Duration::from_secs(10),
        Some(Duration::from_secs(60)),
    );
    info!("Retry delays:");
    for attempt in 1..=6 {
        info!(
            "  attempt {} → {}s",
            attempt,
            strategy.calculate_delay(attempt).as_secs()
        );
    }

    queue.enqueue(job).await?;
    info!("✅ Linear backoff job enqueued");
    info!("");

    Ok(())
}

/// Demonstrate exponential backoff retry strategy
async fn demonstrate_exponential_backoff_strategy(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🚀 EXPONENTIAL BACKOFF STRATEGY");
    info!("Doubles the delay for each retry attempt (most common for network operations)");

    // Create job with exponential backoff strategy
    let job = Job::new(
        "exponential_backoff_demo".to_string(),
        json!({
            "task": "external_api_call",
            "service": "payment_processor",
            "description": "Payment API call that should back off exponentially on failure"
        }),
    )
    .with_exponential_backoff(
        Duration::from_secs(1),   // start with 1 second
        2.0,                      // double each time
        Duration::from_secs(600), // cap at 10 minutes
    )
    .with_max_attempts(8);

    // Show calculated delays
    let strategy =
        RetryStrategy::exponential(Duration::from_secs(1), 2.0, Some(Duration::from_secs(600)));
    info!("Retry delays:");
    for attempt in 1..=8 {
        let delay = strategy.calculate_delay(attempt);
        if delay.as_secs() >= 60 {
            info!(
                "  attempt {} → {}m {}s",
                attempt,
                delay.as_secs() / 60,
                delay.as_secs() % 60
            );
        } else {
            info!("  attempt {} → {}s", attempt, delay.as_secs());
        }
    }

    queue.enqueue(job).await?;
    info!("✅ Exponential backoff job enqueued");
    info!("");

    Ok(())
}

/// Demonstrate exponential backoff with jitter to prevent thundering herd
async fn demonstrate_exponential_with_jitter(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🎲 EXPONENTIAL BACKOFF WITH JITTER");
    info!("Adds randomness to prevent all jobs from retrying at the same time");

    // Create multiple jobs with jittered exponential backoff
    for i in 1..=3 {
        let job = Job::new(
            "jittered_exponential_demo".to_string(),
            json!({
                "task": "high_volume_api_call",
                "batch_id": i,
                "description": format!("API call #{} with jitter to prevent thundering herd", i)
            }),
        )
        .with_retry_strategy(RetryStrategy::exponential_with_jitter(
            Duration::from_secs(2),          // base delay
            2.0,                             // multiplier
            Some(Duration::from_secs(300)),  // max delay
            JitterType::Multiplicative(0.2), // ±20% randomness
        ))
        .with_max_attempts(6);

        queue.enqueue(job).await?;
    }

    // Show example of jittered delays
    let strategy = RetryStrategy::exponential_with_jitter(
        Duration::from_secs(2),
        2.0,
        Some(Duration::from_secs(300)),
        JitterType::Multiplicative(0.2),
    );

    info!("Example jittered delays (will vary each time):");
    for attempt in 1..=5 {
        let delay1 = strategy.calculate_delay(attempt);
        let delay2 = strategy.calculate_delay(attempt);
        let delay3 = strategy.calculate_delay(attempt);
        info!(
            "  attempt {}: {}s, {}s, {}s (all different due to jitter)",
            attempt,
            delay1.as_secs(),
            delay2.as_secs(),
            delay3.as_secs()
        );
    }

    info!("✅ {} jittered exponential backoff jobs enqueued", 3);
    info!("");

    Ok(())
}

/// Demonstrate Fibonacci sequence backoff strategy
async fn demonstrate_fibonacci_backoff_strategy(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🌀 FIBONACCI BACKOFF STRATEGY");
    info!("Uses Fibonacci sequence for retry delays (gentle exponential growth)");

    // Create job with Fibonacci backoff strategy
    let job = Job::new(
        "fibonacci_backoff_demo".to_string(),
        json!({
            "task": "file_processing",
            "file_type": "large_csv",
            "description": "File processing that may fail due to memory or I/O issues"
        }),
    )
    .with_fibonacci_backoff(
        Duration::from_secs(3),         // base multiplier
        Some(Duration::from_secs(480)), // max delay cap
    )
    .with_max_attempts(10);

    // Show calculated delays
    let strategy = RetryStrategy::fibonacci(Duration::from_secs(3), Some(Duration::from_secs(480)));
    info!("Retry delays (3 seconds × Fibonacci sequence):");
    for attempt in 1..=10 {
        let delay = strategy.calculate_delay(attempt);
        if delay.as_secs() >= 60 {
            info!(
                "  attempt {} → {}m {}s",
                attempt,
                delay.as_secs() / 60,
                delay.as_secs() % 60
            );
        } else {
            info!("  attempt {} → {}s", attempt, delay.as_secs());
        }
    }

    queue.enqueue(job).await?;
    info!("✅ Fibonacci backoff job enqueued");
    info!("");

    Ok(())
}

/// Demonstrate custom retry strategy with business logic
async fn demonstrate_custom_retry_strategy(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("⚙️  CUSTOM RETRY STRATEGY");
    info!("Uses business-specific logic to determine retry delays");

    // Create job with custom retry strategy
    let job = Job::new(
        "custom_retry_demo".to_string(),
        json!({
            "task": "business_critical_operation",
            "priority": "high",
            "description": "Critical operation with custom retry logic based on business rules"
        }),
    )
    .with_retry_strategy(RetryStrategy::custom(|attempt| {
        match attempt {
            // Quick retries for transient issues (likely network blips)
            1..=3 => Duration::from_secs(2),
            // Medium delays for potential system load issues
            4..=6 => Duration::from_secs(30),
            // Long delays for potential maintenance windows
            7..=9 => Duration::from_secs(300), // 5 minutes
            // Very long delays for major outages
            _ => Duration::from_secs(1800), // 30 minutes
        }
    }))
    .with_max_attempts(12);

    // Show custom delay logic
    info!("Custom retry logic:");
    info!("  attempts 1-3: 2s (quick retries for transient issues)");
    info!("  attempts 4-6: 30s (medium delays for system load)");
    info!("  attempts 7-9: 5m (long delays for maintenance)");
    info!("  attempts 10+: 30m (very long delays for major outages)");

    let custom_strategy = RetryStrategy::custom(|attempt| match attempt {
        1..=3 => Duration::from_secs(2),
        4..=6 => Duration::from_secs(30),
        7..=9 => Duration::from_secs(300),
        _ => Duration::from_secs(1800),
    });

    info!("Example delays:");
    for attempt in [1, 3, 4, 6, 7, 9, 10, 12] {
        let delay = custom_strategy.calculate_delay(attempt);
        if delay.as_secs() >= 60 {
            info!("  attempt {} → {}m", attempt, delay.as_secs() / 60);
        } else {
            info!("  attempt {} → {}s", attempt, delay.as_secs());
        }
    }

    queue.enqueue(job).await?;
    info!("✅ Custom retry strategy job enqueued");
    info!("");

    Ok(())
}

/// Demonstrate worker-level default retry strategies
async fn demonstrate_worker_default_strategies(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🏭 WORKER DEFAULT RETRY STRATEGIES");
    info!(
        "Workers can have default retry strategies that apply to all jobs without specific strategies"
    );

    // Create jobs without retry strategies (will use worker defaults)
    let job1 = Job::new(
        "worker_default_demo_1".to_string(),
        json!({
            "task": "email_sending",
            "description": "Email job that will use worker's exponential backoff default"
        }),
    )
    .with_max_attempts(4);

    let job2 = Job::new(
        "worker_default_demo_2".to_string(),
        json!({
            "task": "data_sync",
            "description": "Data sync job that will use worker's exponential backoff default"
        }),
    )
    .with_max_attempts(5);

    // Jobs with specific strategies override worker defaults
    let job3 = Job::new("worker_default_demo_3".to_string(), json!({
        "task": "priority_task",
        "description": "Priority task with its own fixed retry strategy (overrides worker default)"
    }))
    .with_retry_strategy(RetryStrategy::fixed(Duration::from_secs(5)))
    .with_max_attempts(3);

    queue.enqueue(job1).await?;
    queue.enqueue(job2).await?;
    queue.enqueue(job3).await?;

    info!("Enqueued 3 jobs:");
    info!("  📧 Email job (will use worker default: exponential backoff)");
    info!("  🔄 Data sync job (will use worker default: exponential backoff)");
    info!("  ⚡ Priority job (uses own fixed 5s strategy, overrides worker default)");

    // Note: Worker creation with default strategy is shown in demonstrate_mixed_strategies_in_action
    info!("✅ Worker default strategy demo jobs enqueued");
    info!("");

    Ok(())
}

/// Demonstrate multiple strategies working together in a realistic scenario
async fn demonstrate_mixed_strategies_in_action(
    queue: &Arc<ExampleJobQueue>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("🎪 MIXED STRATEGIES IN ACTION");
    info!("Realistic example showing different retry strategies for different job types");

    // Create a worker pool with different strategies for different queue types
    let mut pool = WorkerPool::new();

    // API worker with exponential backoff default
    let api_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            info!("🌐 Processing API job: {}", job.id);
            // Simulate API call that might fail
            if job.attempts < 2 {
                warn!("API call failed (simulated), will retry with exponential backoff");
                Err(HammerworkError::Worker {
                    message: "API temporarily unavailable".to_string(),
                })
            } else {
                info!("✅ API call succeeded on attempt {}", job.attempts + 1);
                Ok(())
            }
        })
    });

    let api_worker = Worker::new(queue.clone(), "api_tasks".to_string(), api_handler)
        .with_default_retry_strategy(RetryStrategy::exponential(
            Duration::from_secs(1),
            2.0,
            Some(Duration::from_secs(300)),
        ));

    // Database worker with linear backoff default
    let db_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            info!("💾 Processing database job: {}", job.id);
            // Simulate database operation that might fail due to locks
            if job.attempts < 3 {
                warn!("Database lock detected (simulated), will retry with linear backoff");
                Err(HammerworkError::Worker {
                    message: "Database locked".to_string(),
                })
            } else {
                info!(
                    "✅ Database operation succeeded on attempt {}",
                    job.attempts + 1
                );
                Ok(())
            }
        })
    });

    let db_worker = Worker::new(queue.clone(), "database_tasks".to_string(), db_handler)
        .with_default_retry_strategy(RetryStrategy::linear(
            Duration::from_secs(5),
            Duration::from_secs(5),
            Some(Duration::from_secs(30)),
        ));

    // File processing worker with Fibonacci backoff default
    let file_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            info!("📁 Processing file job: {}", job.id);
            // Simulate file processing that might fail due to I/O
            if job.attempts < 2 {
                warn!("File I/O error (simulated), will retry with Fibonacci backoff");
                Err(HammerworkError::Worker {
                    message: "File temporarily unavailable".to_string(),
                })
            } else {
                info!(
                    "✅ File processing succeeded on attempt {}",
                    job.attempts + 1
                );
                Ok(())
            }
        })
    });

    let file_worker = Worker::new(queue.clone(), "file_tasks".to_string(), file_handler)
        .with_default_retry_strategy(RetryStrategy::fibonacci(
            Duration::from_secs(2),
            Some(Duration::from_secs(180)),
        ));

    pool.add_worker(api_worker);
    pool.add_worker(db_worker);
    pool.add_worker(file_worker);

    // Enqueue jobs for different workers
    let api_job = Job::new(
        "api_tasks".to_string(),
        json!({
            "task": "fetch_user_data",
            "user_id": 12345
        }),
    )
    .with_max_attempts(4);

    let db_job = Job::new(
        "database_tasks".to_string(),
        json!({
            "task": "update_user_preferences",
            "user_id": 12345
        }),
    )
    .with_max_attempts(5);

    let file_job = Job::new(
        "file_tasks".to_string(),
        json!({
            "task": "process_uploaded_image",
            "file_path": "/uploads/user_avatar.jpg"
        }),
    )
    .with_max_attempts(4);

    // High priority job with custom strategy that overrides worker default
    let priority_job = Job::new(
        "api_tasks".to_string(),
        json!({
            "task": "critical_notification",
            "urgency": "high"
        }),
    )
    .with_retry_strategy(RetryStrategy::fixed(Duration::from_secs(1))) // Override worker's exponential default
    .with_max_attempts(3);

    queue.enqueue(api_job).await?;
    queue.enqueue(db_job).await?;
    queue.enqueue(file_job).await?;
    queue.enqueue(priority_job).await?;

    info!("Enqueued jobs for mixed strategy demonstration:");
    info!("  🌐 API job → exponential backoff (worker default)");
    info!("  💾 Database job → linear backoff (worker default)");
    info!("  📁 File job → Fibonacci backoff (worker default)");
    info!("  ⚡ Priority job → fixed 1s delay (job-specific, overrides worker default)");
    info!("");

    info!("🚀 Starting worker pool to process jobs...");

    // Process jobs for a short time to demonstrate the retry strategies
    tokio::select! {
        _ = pool.start() => {},
        _ = tokio::time::sleep(Duration::from_secs(30)) => {
            info!("⏰ Demo timeout reached, shutting down workers");
        }
    }

    info!("✅ Mixed strategies demonstration completed");
    info!("");

    Ok(())
}
