//! Autoscaling worker pool example
//!
//! This example demonstrates how to configure and use the autoscaling functionality
//! in hammerwork. It shows different autoscaling configurations and how they respond
//! to changing job queue loads.

use hammerwork::{
    AutoscaleConfig, Job, JobQueue, Result, Worker, WorkerPool, queue::DatabaseQueue,
    worker::JobHandler,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing for better logging
    tracing_subscriber::fmt::init();

    info!("Starting autoscaling worker pool example");

    // Note: This example requires either PostgreSQL or MySQL to be running
    // Uncomment the appropriate line below based on your database setup

    // For PostgreSQL:
    #[cfg(all(feature = "postgres", not(feature = "mysql")))]
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork")
        .await
        .map_err(hammerwork::HammerworkError::Database)?;

    // For MySQL:
    #[cfg(all(feature = "mysql", not(feature = "postgres")))]
    let pool = sqlx::MySqlPool::connect("mysql://localhost/hammerwork")
        .await
        .map_err(hammerwork::HammerworkError::Database)?;

    // Default to PostgreSQL when both features are enabled
    #[cfg(all(feature = "postgres", feature = "mysql"))]
    let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork")
        .await
        .map_err(hammerwork::HammerworkError::Database)?;

    // Error if no database features are enabled
    #[cfg(not(any(feature = "postgres", feature = "mysql")))]
    compile_error!("This example requires either 'postgres' or 'mysql' feature to be enabled");

    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    // Run `cargo hammerwork migrate` before running this example to set up the database tables

    // Create job handler
    let handler: JobHandler = Arc::new(|job: Job| {
        Box::pin(async move {
            info!("Processing job {} with payload: {:?}", job.id, job.payload);

            // Simulate variable job processing time
            let processing_time = job
                .payload
                .get("processing_time")
                .and_then(|v| v.as_u64())
                .unwrap_or(1000);

            sleep(Duration::from_millis(processing_time)).await;

            info!("Completed job {}", job.id);
            Ok(())
        })
    });

    // Example 1: Default autoscaling configuration
    info!("=== Example 1: Default Autoscaling ===");
    run_default_autoscaling_example(&queue, handler.clone()).await?;

    // Example 2: Conservative autoscaling
    info!("=== Example 2: Conservative Autoscaling ===");
    run_conservative_autoscaling_example(&queue, handler.clone()).await?;

    // Example 3: Aggressive autoscaling
    info!("=== Example 3: Aggressive Autoscaling ===");
    run_aggressive_autoscaling_example(&queue, handler.clone()).await?;

    // Example 4: Custom autoscaling configuration
    info!("=== Example 4: Custom Autoscaling ===");
    run_custom_autoscaling_example(&queue, handler.clone()).await?;

    // Example 5: Disabled autoscaling (static worker pool)
    info!("=== Example 5: Disabled Autoscaling ===");
    run_static_worker_pool_example(&queue, handler).await?;

    info!("Autoscaling examples completed!");
    Ok(())
}

/// Example with default autoscaling settings
async fn run_default_autoscaling_example<DB>(
    queue: &Arc<JobQueue<DB>>,
    handler: JobHandler,
) -> Result<()>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    info!("Creating worker pool with default autoscaling settings");

    // Create worker template
    let worker_template = Worker::new(
        Arc::clone(queue),
        "autoscaling_default".to_string(),
        handler,
    )
    .with_poll_interval(Duration::from_millis(100));

    // Create worker pool with default autoscaling
    let mut pool = WorkerPool::new()
        .with_worker_template(worker_template.clone())
        .with_autoscaling(AutoscaleConfig::default());

    // Add initial worker
    pool.add_worker(worker_template);

    // Enqueue some jobs to trigger scaling
    enqueue_test_jobs(queue, "autoscaling_default", 20).await?;

    // Run pool for a short time to demonstrate autoscaling
    tokio::select! {
        _ = pool.start() => {},
        _ = sleep(Duration::from_secs(30)) => {
            info!("Stopping default autoscaling example");
            pool.shutdown().await?;
        }
    }

    // Show final metrics
    let metrics = pool.get_autoscale_metrics();
    info!(
        "Final metrics - Active workers: {}, Avg queue depth: {:.1}",
        metrics.active_workers, metrics.avg_queue_depth
    );

    Ok(())
}

/// Example with conservative autoscaling settings
async fn run_conservative_autoscaling_example<DB>(
    queue: &Arc<JobQueue<DB>>,
    handler: JobHandler,
) -> Result<()>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    info!("Creating worker pool with conservative autoscaling settings");

    let worker_template = Worker::new(
        Arc::clone(queue),
        "autoscaling_conservative".to_string(),
        handler,
    )
    .with_poll_interval(Duration::from_millis(100));

    let mut pool = WorkerPool::new()
        .with_worker_template(worker_template.clone())
        .with_autoscaling(AutoscaleConfig::conservative());

    pool.add_worker(worker_template);

    // Conservative scaling needs more jobs to trigger scale-up
    enqueue_test_jobs(queue, "autoscaling_conservative", 50).await?;

    tokio::select! {
        _ = pool.start() => {},
        _ = sleep(Duration::from_secs(45)) => {
            info!("Stopping conservative autoscaling example");
            pool.shutdown().await?;
        }
    }

    let metrics = pool.get_autoscale_metrics();
    info!(
        "Conservative final metrics - Active workers: {}, Avg queue depth: {:.1}",
        metrics.active_workers, metrics.avg_queue_depth
    );

    Ok(())
}

/// Example with aggressive autoscaling settings
async fn run_aggressive_autoscaling_example<DB>(
    queue: &Arc<JobQueue<DB>>,
    handler: JobHandler,
) -> Result<()>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    info!("Creating worker pool with aggressive autoscaling settings");

    let worker_template = Worker::new(
        Arc::clone(queue),
        "autoscaling_aggressive".to_string(),
        handler,
    )
    .with_poll_interval(Duration::from_millis(100));

    let mut pool = WorkerPool::new()
        .with_worker_template(worker_template.clone())
        .with_autoscaling(AutoscaleConfig::aggressive());

    pool.add_worker(worker_template);

    // Aggressive scaling responds quickly to smaller loads
    enqueue_test_jobs(queue, "autoscaling_aggressive", 15).await?;

    tokio::select! {
        _ = pool.start() => {},
        _ = sleep(Duration::from_secs(25)) => {
            info!("Stopping aggressive autoscaling example");
            pool.shutdown().await?;
        }
    }

    let metrics = pool.get_autoscale_metrics();
    info!(
        "Aggressive final metrics - Active workers: {}, Avg queue depth: {:.1}",
        metrics.active_workers, metrics.avg_queue_depth
    );

    Ok(())
}

/// Example with custom autoscaling configuration
async fn run_custom_autoscaling_example<DB>(
    queue: &Arc<JobQueue<DB>>,
    handler: JobHandler,
) -> Result<()>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    info!("Creating worker pool with custom autoscaling settings");

    let worker_template = Worker::new(Arc::clone(queue), "autoscaling_custom".to_string(), handler)
        .with_poll_interval(Duration::from_millis(100));

    // Custom configuration optimized for medium-scale workloads
    let custom_config = AutoscaleConfig::new()
        .with_min_workers(2)
        .with_max_workers(8)
        .with_scale_up_threshold(6)
        .with_scale_down_threshold(1)
        .with_cooldown_period(Duration::from_secs(45))
        .with_scale_step(2)
        .with_evaluation_window(Duration::from_secs(20))
        .with_idle_timeout(Duration::from_secs(180));

    let mut pool = WorkerPool::new()
        .with_worker_template(worker_template.clone())
        .with_autoscaling(custom_config);

    pool.add_worker(worker_template);

    // Enqueue jobs in waves to demonstrate scaling behavior
    enqueue_test_jobs(queue, "autoscaling_custom", 25).await?;

    sleep(Duration::from_secs(10)).await;

    // Add more jobs to trigger further scaling
    enqueue_test_jobs(queue, "autoscaling_custom", 30).await?;

    tokio::select! {
        _ = pool.start() => {},
        _ = sleep(Duration::from_secs(60)) => {
            info!("Stopping custom autoscaling example");
            pool.shutdown().await?;
        }
    }

    let metrics = pool.get_autoscale_metrics();
    info!(
        "Custom final metrics - Active workers: {}, Avg queue depth: {:.1}",
        metrics.active_workers, metrics.avg_queue_depth
    );

    Ok(())
}

/// Example with autoscaling disabled (static worker pool)
async fn run_static_worker_pool_example<DB>(
    queue: &Arc<JobQueue<DB>>,
    handler: JobHandler,
) -> Result<()>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    info!("Creating static worker pool (autoscaling disabled)");

    let worker1 = Worker::new(
        Arc::clone(queue),
        "static_pool".to_string(),
        handler.clone(),
    )
    .with_poll_interval(Duration::from_millis(100));

    let worker2 = Worker::new(Arc::clone(queue), "static_pool".to_string(), handler)
        .with_poll_interval(Duration::from_millis(100));

    let mut pool = WorkerPool::new().without_autoscaling(); // Disable autoscaling

    pool.add_worker(worker1);
    pool.add_worker(worker2);

    enqueue_test_jobs(queue, "static_pool", 30).await?;

    tokio::select! {
        _ = pool.start() => {},
        _ = sleep(Duration::from_secs(20)) => {
            info!("Stopping static worker pool example");
            pool.shutdown().await?;
        }
    }

    let metrics = pool.get_autoscale_metrics();
    info!(
        "Static pool final metrics - Active workers: {}, Avg queue depth: {:.1}",
        metrics.active_workers, metrics.avg_queue_depth
    );

    Ok(())
}

/// Helper function to enqueue test jobs
async fn enqueue_test_jobs<DB>(
    queue: &Arc<JobQueue<DB>>,
    queue_name: &str,
    count: u32,
) -> Result<()>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    info!("Enqueuing {} test jobs to queue '{}'", count, queue_name);

    for i in 0..count {
        let job = Job::new(
            queue_name.to_string(),
            json!({
                "task_id": i,
                "message": format!("Test job {}", i),
                "processing_time": 500 + (i % 3) * 500 // Variable processing time
            }),
        );

        queue.enqueue(job).await?;
    }

    Ok(())
}

/// Example showing how to monitor autoscaling metrics
#[allow(dead_code)]
async fn monitor_autoscaling_metrics<DB>(pool: &WorkerPool<DB>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync,
{
    let mut interval = tokio::time::interval(Duration::from_secs(5));

    for _ in 0..12 {
        // Monitor for 1 minute
        interval.tick().await;

        let metrics = pool.get_autoscale_metrics();
        info!(
            "Autoscaling metrics - Workers: {}, Queue depth: {}/{:.1} (current/avg), Jobs/sec: {:.2}, Utilization: {:.1}%",
            metrics.active_workers,
            metrics.current_queue_depth,
            metrics.avg_queue_depth,
            metrics.jobs_per_second,
            metrics.worker_utilization * 100.0
        );

        if let Some(last_scale) = metrics.last_scale_time {
            let time_since = chrono::Utc::now() - last_scale;
            info!(
                "Time since last scaling action: {}s",
                time_since.num_seconds()
            );
        }
    }
}
