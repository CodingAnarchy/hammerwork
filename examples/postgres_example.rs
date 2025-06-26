use hammerwork::{
    job::Job,
    queue::JobQueue,
    stats::{InMemoryStatsCollector, StatisticsCollector, JobEvent, JobEventType},
    worker::{Worker, WorkerPool},
    Result,
};
use serde_json::json;
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tracing::info;
use chrono::Utc;

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();

    // Connect to PostgreSQL
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://localhost/hammerwork_test".to_string());

    let pool = Pool::<Postgres>::connect(&database_url).await?;
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;
        queue.create_tables().await?;
    }

    // Create statistics collector for monitoring job processing
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default()) as Arc<dyn StatisticsCollector>;

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

    // Create and start workers with statistics collection
    let worker1 = Worker::new(queue.clone(), "email".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_secs(1))
        .with_max_retries(3)
        .with_stats_collector(Arc::clone(&stats_collector));

    let worker2 = Worker::new(queue.clone(), "notifications".to_string(), handler.clone())
        .with_poll_interval(tokio::time::Duration::from_secs(2))
        .with_max_retries(2)
        .with_stats_collector(Arc::clone(&stats_collector));

    let mut worker_pool = WorkerPool::new()
        .with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(worker1);
    worker_pool.add_worker(worker2);

    // Enqueue some test jobs
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;

        let job1 = Job::new(
            "email".to_string(),
            json!({"to": "user@example.com", "subject": "Hello"}),
        );
        let job2 = Job::new(
            "notifications".to_string(),
            json!({"message": "Welcome!", "user_id": 123}),
        );
        let job3 = Job::new("email".to_string(), json!({"action": "fail"})); // This will fail
        let job4 = Job::new("email".to_string(), json!({"action": "fail"})); // This will also fail and become dead

        let job1_id = queue.enqueue(job1).await?;
        let job2_id = queue.enqueue(job2).await?;
        let job3_id = queue.enqueue(job3).await?;
        let job4_id = queue.enqueue(job4).await?;

        info!("Enqueued test jobs: {}, {}, {}, {}", job1_id, job2_id, job3_id, job4_id);

        // Let jobs process for a bit
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Demonstrate statistics collection
        info!("=== Job Processing Statistics ===");
        let system_stats = stats_collector.get_system_statistics(std::time::Duration::from_secs(300)).await?;
        info!("System Stats - Total: {}, Completed: {}, Failed: {}, Dead: {}, Error Rate: {:.2}%",
            system_stats.total_processed,
            system_stats.completed,
            system_stats.failed,
            system_stats.dead,
            system_stats.error_rate * 100.0
        );

        // Get queue-specific statistics
        let email_stats = stats_collector.get_queue_statistics("email", std::time::Duration::from_secs(300)).await?;
        info!("Email Queue Stats - Total: {}, Completed: {}, Failed: {}, Avg Processing Time: {:.2}ms",
            email_stats.total_processed,
            email_stats.completed,
            email_stats.failed,
            email_stats.avg_processing_time_ms
        );

        // Demonstrate dead job management
        info!("=== Dead Job Management ===");
        let dead_jobs = queue.get_dead_jobs(Some(10), None).await?;
        info!("Found {} dead jobs", dead_jobs.len());
        
        for dead_job in &dead_jobs {
            info!("Dead Job {} in queue '{}': {:?}", dead_job.id, dead_job.queue_name, dead_job.error_message);
        }

        // Get dead job summary
        let dead_summary = queue.get_dead_job_summary().await?;
        info!("Dead Job Summary - Total: {}, By Queue: {:?}, Error Patterns: {:?}",
            dead_summary.total_dead_jobs,
            dead_summary.dead_jobs_by_queue,
            dead_summary.error_patterns
        );

        // Demonstrate queue statistics from database
        let queue_stats = queue.get_queue_stats("email").await?;
        info!("Email Queue DB Stats - Pending: {}, Running: {}, Dead: {}, Completed: {}",
            queue_stats.pending_count,
            queue_stats.running_count,
            queue_stats.dead_count,
            queue_stats.completed_count
        );

        // If there are dead jobs, demonstrate retry functionality
        if !dead_jobs.is_empty() {
            let dead_job_id = dead_jobs[0].id;
            info!("Attempting to retry dead job: {}", dead_job_id);
            queue.retry_dead_job(dead_job_id).await?;
            info!("Dead job {} has been reset for retry", dead_job_id);
        }
    }

    info!("Example completed successfully. Check the statistics and dead job management features.");
    info!("In a real application, you would start the worker pool to run indefinitely:");
    info!("worker_pool.start().await?;");

    Ok(())
}
