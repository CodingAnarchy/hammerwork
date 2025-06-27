use hammerwork::{
    Result,
    job::Job,
    queue::JobQueue,
    rate_limit::{RateLimit, ThrottleConfig},
    stats::{InMemoryStatsCollector, StatisticsCollector},
    worker::{Worker, WorkerPool},
};
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // Connect to PostgreSQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost/hammerwork_test".to_string());

    let pool = Pool::<Postgres>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    // Run `cargo hammerwork migrate` before running this example to set up the database tables
    #[cfg(feature = "postgres")]
    {
        // Tables should be created using: cargo hammerwork migrate
    }

    // Create statistics collector for monitoring job processing
    let stats_collector =
        Arc::new(InMemoryStatsCollector::new_default()) as Arc<dyn StatisticsCollector>;

    // Create a job handler
    let handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Processing job: {} with payload: {}", job.id, job.payload);

            // Simulate some work
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

            // For demo purposes, fail jobs with "fail" in the payload
            if job.payload.get("action").and_then(|v| v.as_str()) == Some("fail") {
                return Err(hammerwork::HammerworkError::Worker {
                    message: "Simulated failure".to_string(),
                });
            }

            info!("Job {} completed successfully", job.id);
            Ok(()) as Result<()>
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create and start workers with statistics collection, timeout, and rate limiting configuration
    let worker1 = Worker::new(queue.clone(), "email".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(3)
        .with_default_timeout(tokio::time::Duration::from_secs(30)) // 30 second default timeout
        .with_rate_limit(RateLimit::per_second(2)) // Limit to 2 emails per second
        .with_stats_collector(Arc::clone(&stats_collector));

    let worker2 = Worker::new(queue.clone(), "notifications".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_secs(2))
        .with_max_retries(2)
        .with_default_timeout(tokio::time::Duration::from_secs(60)) // 60 second default timeout
        .with_throttle_config(
            ThrottleConfig::new()
                .rate_per_minute(30) // Max 30 notifications per minute
                .backoff_on_error(tokio::time::Duration::from_secs(10)), // 10 second backoff on errors
        )
        .with_stats_collector(Arc::clone(&stats_collector));

    // Create a high-throughput worker for API calls with burst support
    let api_worker = Worker::new(queue.clone(), "api_calls".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_millis(100))
        .with_rate_limit(
            RateLimit::per_second(10) // 10 calls per second
                .with_burst_limit(50), // Allow bursts up to 50 calls
        )
        .with_stats_collector(Arc::clone(&stats_collector));

    let mut worker_pool = WorkerPool::new().with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(worker1);
    worker_pool.add_worker(worker2);
    worker_pool.add_worker(api_worker);

    // Enqueue some test jobs
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;

        // Create jobs with various timeout configurations
        let job1 = Job::new(
            "email".to_string(),
            json!({"to": "user@example.com", "subject": "Hello"}),
        ); // Uses worker default timeout (30s)

        let job2 = Job::new(
            "notifications".to_string(),
            json!({"message": "Welcome!", "user_id": 123}),
        )
        .with_timeout(std::time::Duration::from_secs(10)); // Custom 10-second timeout

        let job3 = Job::new("email".to_string(), json!({"action": "fail"})); // This will fail

        let job4 = Job::new("email".to_string(), json!({"action": "fail"})) // This will also fail and become dead
            .with_timeout(std::time::Duration::from_secs(5)); // Short timeout for demo

        // Job with a very long timeout for heavy processing
        let job5 = Job::new(
            "email".to_string(),
            json!({"to": "admin@example.com", "subject": "Heavy Processing"}),
        )
        .with_timeout(std::time::Duration::from_secs(300)) // 5-minute timeout
        .with_max_attempts(5); // More retry attempts for important jobs

        // Demonstrate queue-level throttling configuration
        queue
            .set_throttle(
                "api_calls",
                ThrottleConfig::new()
                    .max_concurrent(5)
                    .rate_per_minute(300) // Global rate limit for API calls
                    .backoff_on_error(tokio::time::Duration::from_secs(30)),
            )
            .await?;

        info!("Configured throttling for api_calls queue: max 5 concurrent, 300/min global rate");

        let job1_id = queue.enqueue(job1).await?;
        let job2_id = queue.enqueue(job2).await?;
        let job3_id = queue.enqueue(job3).await?;
        let job4_id = queue.enqueue(job4).await?;
        let job5_id = queue.enqueue(job5).await?;

        // Enqueue some API call jobs to demonstrate rate limiting
        for i in 1..=10 {
            let api_job = Job::new(
                "api_calls".to_string(),
                json!({
                    "endpoint": format!("/api/users/{}", i),
                    "method": "GET",
                    "user_id": i
                }),
            );
            queue.enqueue(api_job).await?;
        }

        info!("Enqueued 10 API call jobs to demonstrate rate limiting");

        info!("Enqueued test jobs with timeouts:");
        info!("  {} - uses worker default timeout (30s)", job1_id);
        info!("  {} - custom 10s timeout", job2_id);
        info!("  {} - will fail (uses default timeout)", job3_id);
        info!("  {} - will fail with 5s timeout", job4_id);
        info!("  {} - heavy processing with 5min timeout", job5_id);

        // Let jobs process for a bit to see rate limiting in action
        info!("Processing jobs for 10 seconds to demonstrate rate limiting...");
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

        // Demonstrate statistics collection with timeout tracking
        info!("=== Job Processing Statistics (Including Timeouts) ===");
        let system_stats = stats_collector
            .get_system_statistics(std::time::Duration::from_secs(300))
            .await?;
        info!(
            "System Stats - Total: {}, Completed: {}, Failed: {}, Dead: {}, Timed Out: {}, Error Rate: {:.2}%",
            system_stats.total_processed,
            system_stats.completed,
            system_stats.failed,
            system_stats.dead,
            system_stats.timed_out,
            system_stats.error_rate * 100.0
        );

        // Get queue-specific statistics
        let email_stats = stats_collector
            .get_queue_statistics("email", std::time::Duration::from_secs(300))
            .await?;
        info!(
            "Email Queue Stats - Total: {}, Completed: {}, Failed: {}, Timed Out: {}, Avg Processing Time: {:.2}ms",
            email_stats.total_processed,
            email_stats.completed,
            email_stats.failed,
            email_stats.timed_out,
            email_stats.avg_processing_time_ms
        );

        let notifications_stats = stats_collector
            .get_queue_statistics("notifications", std::time::Duration::from_secs(300))
            .await?;
        info!(
            "Notifications Queue Stats - Total: {}, Completed: {}, Timed Out: {}, Avg Processing Time: {:.2}ms",
            notifications_stats.total_processed,
            notifications_stats.completed,
            notifications_stats.timed_out,
            notifications_stats.avg_processing_time_ms
        );

        // Get API calls statistics to show rate limiting effects
        let api_stats = stats_collector
            .get_queue_statistics("api_calls", std::time::Duration::from_secs(300))
            .await?;
        info!(
            "API Calls Queue Stats - Total: {}, Completed: {}, Rate Limiting Effects Visible: {}",
            api_stats.total_processed,
            api_stats.completed,
            if api_stats.total_processed < 10 {
                "Yes (some jobs still processing due to rate limits)"
            } else {
                "No"
            }
        );

        // Show current throttling configurations
        info!("=== Current Throttling Configurations ===");
        let throttle_configs = queue.get_all_throttles().await;
        for (queue_name, config) in throttle_configs {
            info!(
                "Queue '{}': max_concurrent={:?}, rate_per_minute={:?}, backoff_on_error={:?}",
                queue_name, config.max_concurrent, config.rate_per_minute, config.backoff_on_error
            );
        }

        // Demonstrate dead job management
        info!("=== Dead Job Management ===");
        let dead_jobs = queue.get_dead_jobs(Some(10), None).await?;
        info!("Found {} dead jobs", dead_jobs.len());

        for dead_job in &dead_jobs {
            info!(
                "Dead Job {} in queue '{}': {:?}",
                dead_job.id, dead_job.queue_name, dead_job.error_message
            );
        }

        // Get dead job summary
        let dead_summary = queue.get_dead_job_summary().await?;
        info!(
            "Dead Job Summary - Total: {}, By Queue: {:?}, Error Patterns: {:?}",
            dead_summary.total_dead_jobs,
            dead_summary.dead_jobs_by_queue,
            dead_summary.error_patterns
        );

        // Demonstrate queue statistics from database (including timeout counts)
        let queue_stats = queue.get_queue_stats("email").await?;
        info!(
            "Email Queue DB Stats - Pending: {}, Running: {}, Dead: {}, Timed Out: {}, Completed: {}",
            queue_stats.pending_count,
            queue_stats.running_count,
            queue_stats.dead_count,
            queue_stats.timed_out_count,
            queue_stats.completed_count
        );

        // Show all queue statistics
        let all_queue_stats = queue.get_all_queue_stats().await?;
        info!("=== All Queue Statistics ===");
        for stats in &all_queue_stats {
            info!(
                "Queue '{}' - Pending: {}, Running: {}, Dead: {}, Timed Out: {}, Completed: {}",
                stats.queue_name,
                stats.pending_count,
                stats.running_count,
                stats.dead_count,
                stats.timed_out_count,
                stats.completed_count
            );
        }

        // If there are dead jobs, demonstrate retry functionality
        if !dead_jobs.is_empty() {
            let dead_job_id = dead_jobs[0].id;
            info!("Attempting to retry dead job: {}", dead_job_id);
            queue.retry_dead_job(dead_job_id).await?;
            info!("Dead job {} has been reset for retry", dead_job_id);
        }
    }

    info!("Example completed successfully!");
    info!("Demonstrated features:");
    info!("  ✓ Job timeout configuration (per-job and worker defaults)");
    info!("  ✓ Timeout statistics tracking and monitoring");
    info!("  ✓ Dead job management and retry functionality");
    info!("  ✓ Comprehensive job processing statistics");
    info!("");
    info!("Timeout Features Shown:");
    info!("  • Worker default timeouts (30s for email, 60s for notifications)");
    info!("  • Job-specific timeouts (10s, 5s, 300s examples)");
    info!("  • Timeout event tracking in statistics");
    info!("  • Timeout counts in database queue statistics");
    info!("");
    info!("In a real application, you would start the worker pool to run indefinitely:");
    info!("worker_pool.start().await?;");

    Ok(())
}
