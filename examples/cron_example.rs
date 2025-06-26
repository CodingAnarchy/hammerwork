use hammerwork::{
    cron::{CronSchedule, presets},
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

    // Create a job handler for report generation
    let report_handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Generating report: {} with payload: {}", job.id, job.payload);

            // Simulate report generation work
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            if let Some(report_type) = job.payload.get("report_type").and_then(|v| v.as_str()) {
                match report_type {
                    "daily" => info!("Generated daily sales report"),
                    "weekly" => info!("Generated weekly analytics report"),
                    "monthly" => info!("Generated monthly summary report"),
                    _ => info!("Generated generic report"),
                }
            }

            info!("Report {} completed successfully", job.id);
            Ok(()) as Result<()>
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create a job handler for cleanup tasks
    let cleanup_handler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Running cleanup task: {} with payload: {}", job.id, job.payload);

            // Simulate cleanup work
            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

            if let Some(cleanup_type) = job.payload.get("cleanup_type").and_then(|v| v.as_str()) {
                match cleanup_type {
                    "temp_files" => info!("Cleaned up temporary files"),
                    "logs" => info!("Rotated and archived log files"),
                    "cache" => info!("Cleared expired cache entries"),
                    _ => info!("Performed generic cleanup"),
                }
            }

            info!("Cleanup task {} completed successfully", job.id);
            Ok(()) as Result<()>
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
    });

    // Create workers for different types of jobs
    let report_worker = Worker::new(queue.clone(), "reports".to_string(), report_handler)
        .with_poll_interval(tokio::time::Duration::from_secs(2))
        .with_max_retries(3)
        .with_default_timeout(tokio::time::Duration::from_secs(60))
        .with_stats_collector(Arc::clone(&stats_collector));

    let cleanup_worker = Worker::new(queue.clone(), "cleanup".to_string(), cleanup_handler)
        .with_poll_interval(tokio::time::Duration::from_secs(3))
        .with_max_retries(2)
        .with_default_timeout(tokio::time::Duration::from_secs(30))
        .with_stats_collector(Arc::clone(&stats_collector));

    let mut worker_pool = WorkerPool::new()
        .with_stats_collector(Arc::clone(&stats_collector));
    worker_pool.add_worker(report_worker);
    worker_pool.add_worker(cleanup_worker);

    // Enqueue some cron jobs
    #[cfg(feature = "postgres")]
    {
        use hammerwork::queue::DatabaseQueue;

        info!("=== Creating Cron Jobs ===");

        // 1. Daily report at 9 AM
        let daily_report_schedule = CronSchedule::new("0 9 * * *")?; // Every day at 9:00 AM
        let daily_report_job = Job::with_cron_schedule(
            "reports".to_string(),
            json!({
                "report_type": "daily",
                "description": "Daily sales report"
            }),
            daily_report_schedule
        )?;
        let daily_job_id = queue.enqueue_cron_job(daily_report_job).await?;
        info!("Enqueued daily report job: {}", daily_job_id);

        // 2. Weekly report every Monday at 8 AM
        let weekly_report_schedule = CronSchedule::new("0 8 * * 1")?; // Every Monday at 8:00 AM
        let weekly_report_job = Job::with_cron_schedule(
            "reports".to_string(),
            json!({
                "report_type": "weekly",
                "description": "Weekly analytics report"
            }),
            weekly_report_schedule
        )?;
        let weekly_job_id = queue.enqueue_cron_job(weekly_report_job).await?;
        info!("Enqueued weekly report job: {}", weekly_job_id);

        // 3. Monthly report on the 1st of each month at 7 AM
        let monthly_report_schedule = CronSchedule::new("0 7 1 * *")?; // 1st day of every month at 7:00 AM
        let monthly_report_job = Job::with_cron_schedule(
            "reports".to_string(),
            json!({
                "report_type": "monthly",
                "description": "Monthly summary report"
            }),
            monthly_report_schedule
        )?;
        let monthly_job_id = queue.enqueue_cron_job(monthly_report_job).await?;
        info!("Enqueued monthly report job: {}", monthly_job_id);

        // 4. Cleanup job every 30 minutes using preset
        let cleanup_schedule = CronSchedule::new("*/30 * * * *")?; // Every 30 minutes
        let cleanup_job = Job::with_cron_schedule(
            "cleanup".to_string(),
            json!({
                "cleanup_type": "temp_files",
                "description": "Clean up temporary files"
            }),
            cleanup_schedule
        )?;
        let cleanup_job_id = queue.enqueue_cron_job(cleanup_job).await?;
        info!("Enqueued cleanup job: {}", cleanup_job_id);

        // 5. Log rotation job every hour using presets
        let log_rotation_job = Job::new(
            "cleanup".to_string(),
            json!({
                "cleanup_type": "logs",
                "description": "Rotate log files"
            })
        ).with_cron(presets::every_hour())?;
        let log_job_id = queue.enqueue_cron_job(log_rotation_job).await?;
        info!("Enqueued log rotation job: {}", log_job_id);

        // 6. Cache cleanup with timezone (New York time)
        let ny_cleanup_schedule = CronSchedule::with_timezone("0 2 * * *", "America/New_York")?; // 2 AM EST/EDT
        let cache_cleanup_job = Job::with_cron_schedule(
            "cleanup".to_string(),
            json!({
                "cleanup_type": "cache",
                "description": "Clear expired cache entries",
                "timezone": "America/New_York"
            }),
            ny_cleanup_schedule
        )?;
        let cache_job_id = queue.enqueue_cron_job(cache_cleanup_job).await?;
        info!("Enqueued cache cleanup job (NY timezone): {}", cache_job_id);

        // 7. Demonstration: Create a job that runs every minute for immediate testing
        let demo_schedule = CronSchedule::new("* * * * *")?; // Every minute
        let demo_job = Job::with_cron_schedule(
            "reports".to_string(),
            json!({
                "report_type": "demo",
                "description": "Demo job that runs every minute"
            }),
            demo_schedule
        )?;
        let demo_job_id = queue.enqueue_cron_job(demo_job).await?;
        info!("Enqueued demo job (every minute): {}", demo_job_id);

        // Show all recurring jobs
        info!("=== Recurring Jobs ===");
        let recurring_reports = queue.get_recurring_jobs("reports").await?;
        let recurring_cleanup = queue.get_recurring_jobs("cleanup").await?;
        
        info!("Report queue has {} recurring jobs:", recurring_reports.len());
        for job in &recurring_reports {
            info!("  Job {}: {} (next run: {:?})", 
                job.id, 
                job.payload.get("description").unwrap_or(&json!("Unknown")),
                job.next_run_at
            );
        }

        info!("Cleanup queue has {} recurring jobs:", recurring_cleanup.len());
        for job in &recurring_cleanup {
            info!("  Job {}: {} (next run: {:?})", 
                job.id, 
                job.payload.get("description").unwrap_or(&json!("Unknown")),
                job.next_run_at
            );
        }

        // Get due cron jobs
        info!("=== Due Cron Jobs ===");
        let due_jobs = queue.get_due_cron_jobs(None).await?;
        info!("Found {} jobs due for execution", due_jobs.len());
        for job in &due_jobs {
            info!("  Due job {}: {} (scheduled: {})", 
                job.id, 
                job.payload.get("description").unwrap_or(&json!("Unknown")),
                job.scheduled_at
            );
        }

        // Let the workers process jobs for a bit
        info!("=== Processing Jobs ===");
        info!("Starting workers to process jobs...");
        
        // Start workers in background
        let worker_handle = tokio::spawn(async move {
            // Run for 2 minutes to see some jobs execute
            tokio::time::sleep(tokio::time::Duration::from_secs(120)).await;
            info!("Stopping workers after demonstration period");
        });

        // Let some jobs process
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        // Show statistics
        info!("=== Job Processing Statistics ===");
        let system_stats = stats_collector.get_system_statistics(std::time::Duration::from_secs(300)).await?;
        info!("System Stats - Total: {}, Completed: {}, Failed: {}, Error Rate: {:.2}%",
            system_stats.total_processed,
            system_stats.completed,
            system_stats.failed,
            system_stats.error_rate * 100.0
        );

        let report_stats = stats_collector.get_queue_statistics("reports", std::time::Duration::from_secs(300)).await?;
        info!("Reports Queue - Total: {}, Completed: {}, Avg Processing Time: {:.2}ms",
            report_stats.total_processed,
            report_stats.completed,
            report_stats.avg_processing_time_ms
        );

        let cleanup_stats = stats_collector.get_queue_statistics("cleanup", std::time::Duration::from_secs(300)).await?;
        info!("Cleanup Queue - Total: {}, Completed: {}, Avg Processing Time: {:.2}ms",
            cleanup_stats.total_processed,
            cleanup_stats.completed,
            cleanup_stats.avg_processing_time_ms
        );

        // Demonstrate management operations
        info!("=== Cron Job Management ===");
        
        // Disable the demo job (stop it from running)
        queue.disable_recurring_job(demo_job_id).await?;
        info!("Disabled demo job {} from further executions", demo_job_id);

        // Show updated recurring jobs
        let updated_reports = queue.get_recurring_jobs("reports").await?;
        let active_count = updated_reports.iter().filter(|j| j.recurring).count();
        info!("Reports queue now has {} active recurring jobs", active_count);

        // Wait for worker to finish
        worker_handle.await?;
    }

    info!("=== Cron Jobs Example Completed ===");
    info!("Demonstrated features:");
    info!("  ✓ Creating cron jobs with various schedules");
    info!("  ✓ Using preset cron schedules");
    info!("  ✓ Timezone-aware cron scheduling");
    info!("  ✓ Automatic job rescheduling after completion");
    info!("  ✓ Listing and managing recurring jobs");
    info!("  ✓ Enabling/disabling recurring jobs");
    info!("  ✓ Getting due jobs for execution");
    info!("");
    info!("Cron Expression Examples Used:");
    info!("  • '0 9 * * *'     - Daily at 9:00 AM");
    info!("  • '0 8 * * 1'     - Weekly on Monday at 8:00 AM");
    info!("  • '0 7 1 * *'     - Monthly on 1st day at 7:00 AM");
    info!("  • '*/30 * * * *'  - Every 30 minutes");
    info!("  • '0 * * * *'     - Every hour (preset)");
    info!("  • '* * * * *'     - Every minute (demo)");
    info!("");
    info!("In a production environment, workers would run continuously:");
    info!("worker_pool.start().await?;");

    Ok(())
}