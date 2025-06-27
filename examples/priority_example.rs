use hammerwork::{
    Job, JobQueue, Worker, WorkerPool,
    priority::{JobPriority, PriorityWeights},
    queue::DatabaseQueue,
    stats::{InMemoryStatsCollector, StatisticsCollector},
};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::{error, info};

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    info!("🔧 Hammerwork Priority Example");
    info!("This example demonstrates job prioritization and weighted scheduling");

    // Create database connection
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@localhost/hammerwork".to_string());

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await?;

    let queue = Arc::new(JobQueue::new(pool));

    // Create tables
    queue.create_tables().await?;
    info!("✅ Database tables created");

    // Setup statistics collection
    let stats_collector = Arc::new(InMemoryStatsCollector::new_default());

    // Demonstrate different priority levels
    demonstrate_priority_levels(&queue).await?;

    // Demonstrate weighted priority scheduling
    demonstrate_weighted_scheduling(&queue, stats_collector.clone()).await?;

    // Demonstrate strict priority scheduling
    demonstrate_strict_priority(&queue, stats_collector.clone()).await?;

    // Show priority statistics
    show_priority_statistics(stats_collector).await?;

    info!("🎉 Priority example completed successfully!");
    Ok(())
}

/// Demonstrate creating jobs with different priority levels
async fn demonstrate_priority_levels(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    info!("\n📋 === PRIORITY LEVELS DEMONSTRATION ===");

    // Create jobs with different priorities
    let critical_job = Job::new(
        "notifications".to_string(),
        json!({
            "type": "system_alert",
            "message": "Critical system failure detected!"
        }),
    )
    .as_critical();

    let high_job = Job::new(
        "notifications".to_string(),
        json!({
            "type": "user_alert",
            "message": "Payment failed for premium account"
        }),
    )
    .as_high_priority();

    let normal_job = Job::new(
        "processing".to_string(),
        json!({
            "type": "data_processing",
            "batch_id": 12345
        }),
    ); // Default normal priority

    let low_job = Job::new(
        "cleanup".to_string(),
        json!({
            "type": "log_cleanup",
            "older_than_days": 30
        }),
    )
    .as_low_priority();

    let background_job = Job::new(
        "analytics".to_string(),
        json!({
            "type": "analytics_aggregation",
            "time_range": "last_month"
        }),
    )
    .as_background();

    // Enqueue all jobs
    let critical_id = queue.enqueue(critical_job).await?;
    let high_id = queue.enqueue(high_job).await?;
    let normal_id = queue.enqueue(normal_job).await?;
    let low_id = queue.enqueue(low_job).await?;
    let background_id = queue.enqueue(background_job).await?;

    info!("📤 Enqueued jobs with different priorities:");
    info!("  🔴 Critical:    {} (system alert)", critical_id);
    info!("  🟡 High:        {} (payment alert)", high_id);
    info!("  🟢 Normal:      {} (data processing)", normal_id);
    info!("  🔵 Low:         {} (log cleanup)", low_id);
    info!("  ⚫ Background:  {} (analytics)", background_id);

    Ok(())
}

/// Demonstrate weighted priority scheduling (default behavior)
async fn demonstrate_weighted_scheduling(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
    stats_collector: Arc<InMemoryStatsCollector>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    info!("\n⚖️  === WEIGHTED PRIORITY SCHEDULING ===");

    // Create custom priority weights
    let weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 100) // Very high weight
        .with_weight(JobPriority::High, 20) // High weight
        .with_weight(JobPriority::Normal, 5) // Normal weight
        .with_weight(JobPriority::Low, 2) // Low weight
        .with_weight(JobPriority::Background, 1) // Minimal weight
        .with_fairness_factor(0.1); // 10% fairness to prevent starvation

    info!("🎛️  Priority weights configured:");
    info!(
        "   Critical: {}x weight",
        weights.get_weight(JobPriority::Critical)
    );
    info!("   High: {}x weight", weights.get_weight(JobPriority::High));
    info!(
        "   Normal: {}x weight",
        weights.get_weight(JobPriority::Normal)
    );
    info!("   Low: {}x weight", weights.get_weight(JobPriority::Low));
    info!(
        "   Background: {}x weight",
        weights.get_weight(JobPriority::Background)
    );
    info!(
        "   Fairness factor: {:.1}%",
        weights.fairness_factor() * 100.0
    );

    // Create worker with weighted priority
    let worker = Worker::new(
        queue.clone(),
        "notifications".to_string(),
        Arc::new(|job| {
            Box::pin(async move {
                info!(
                    "🔄 Processing {} job: {} (Priority: {:?})",
                    job.queue_name, job.id, job.priority
                );

                // Simulate different processing times based on priority
                let delay = match job.priority {
                    JobPriority::Critical => Duration::from_millis(100), // Fast
                    JobPriority::High => Duration::from_millis(300),     // Medium-fast
                    JobPriority::Normal => Duration::from_millis(500),   // Medium
                    JobPriority::Low => Duration::from_millis(800),      // Slow
                    JobPriority::Background => Duration::from_millis(1000), // Slowest
                };

                sleep(delay).await;
                info!("✅ Completed job: {} (took {:?})", job.id, delay);
                Ok(())
            })
        }),
    )
    .with_priority_weights(weights)
    .with_stats_collector(stats_collector.clone())
    .with_poll_interval(Duration::from_millis(100));

    info!("👷 Starting weighted priority worker...");

    // Start worker in background
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
    let worker_handle = {
        let worker = worker;
        tokio::spawn(async move {
            if let Err(e) = worker.run(shutdown_rx).await {
                error!("Worker error: {}", e);
            }
        })
    };

    // Let worker process for a bit
    sleep(Duration::from_secs(3)).await;

    // Shutdown worker
    let _ = shutdown_tx.send(()).await;
    let _ = worker_handle.await;

    info!("⏹️  Weighted priority worker stopped");
    Ok(())
}

/// Demonstrate strict priority scheduling
async fn demonstrate_strict_priority(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
    stats_collector: Arc<InMemoryStatsCollector>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    info!("\n🎯 === STRICT PRIORITY SCHEDULING ===");
    info!("In strict mode, higher priority jobs are ALWAYS processed first");

    // Add more jobs to demonstrate strict ordering
    let jobs = vec![
        ("Background task", JobPriority::Background),
        ("Regular task", JobPriority::Normal),
        ("Urgent task", JobPriority::High),
        ("Emergency task", JobPriority::Critical),
        ("Another normal task", JobPriority::Normal),
        ("Low priority task", JobPriority::Low),
    ];

    for (name, priority) in &jobs {
        let job = Job::new(
            "processing".to_string(),
            json!({
                "task_name": name,
                "timestamp": chrono::Utc::now().to_rfc3339()
            }),
        )
        .with_priority(*priority);

        let job_id = queue.enqueue(job).await?;
        info!(
            "📤 Enqueued: {} (Priority: {:?}) - ID: {}",
            name, priority, job_id
        );
    }

    // Create worker with strict priority
    let worker = Worker::new(
        queue.clone(),
        "processing".to_string(),
        Arc::new(|job| {
            Box::pin(async move {
                let task_name = job.payload["task_name"].as_str().unwrap_or("unknown");
                info!(
                    "🎯 STRICT: Processing '{}' (Priority: {:?})",
                    task_name, job.priority
                );

                // Quick processing for demo
                sleep(Duration::from_millis(200)).await;
                info!("✅ STRICT: Completed '{}'", task_name);
                Ok(())
            })
        }),
    )
    .with_strict_priority() // This enables strict priority mode
    .with_stats_collector(stats_collector.clone())
    .with_poll_interval(Duration::from_millis(100));

    info!("👷 Starting strict priority worker...");
    info!("📋 Expected order: Critical → High → Normal → Normal → Low → Background");

    // Start worker in background
    let (shutdown_tx, shutdown_rx) = tokio::sync::mpsc::channel(1);
    let worker_handle = {
        let worker = worker;
        tokio::spawn(async move {
            if let Err(e) = worker.run(shutdown_rx).await {
                error!("Worker error: {}", e);
            }
        })
    };

    // Let worker process all jobs
    sleep(Duration::from_secs(3)).await;

    // Shutdown worker
    let _ = shutdown_tx.send(()).await;
    let _ = worker_handle.await;

    info!("⏹️  Strict priority worker stopped");
    Ok(())
}

/// Show comprehensive priority statistics
async fn show_priority_statistics(
    stats_collector: Arc<InMemoryStatsCollector>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    info!("\n📊 === PRIORITY STATISTICS ===");

    // Get statistics for different queues
    let queues = vec!["notifications", "processing", "cleanup", "analytics"];

    for queue_name in queues {
        match stats_collector
            .get_queue_statistics(queue_name, Duration::from_secs(300))
            .await
        {
            Ok(stats) => {
                if stats.total_processed > 0 {
                    info!("\n📈 Queue: {}", queue_name);
                    info!("   Total processed: {}", stats.total_processed);
                    info!("   Completed: {}", stats.completed);
                    info!(
                        "   Average processing time: {:.1}ms",
                        stats.avg_processing_time_ms
                    );
                    info!("   Throughput: {:.1} jobs/min", stats.throughput_per_minute);
                    info!("   Error rate: {:.1}%", stats.error_rate * 100.0);

                    // Show priority breakdown if available
                    if let Some(priority_stats) = &stats.priority_stats {
                        info!("   Priority breakdown:");
                        for priority in JobPriority::all_priorities() {
                            if let Some(count) = priority_stats.job_counts.get(&priority) {
                                if *count > 0 {
                                    let percentage = priority_stats
                                        .priority_distribution
                                        .get(&priority)
                                        .copied()
                                        .unwrap_or(0.0);
                                    let avg_time = priority_stats
                                        .avg_processing_times
                                        .get(&priority)
                                        .copied()
                                        .unwrap_or(0.0);

                                    info!(
                                        "     {:?}: {} jobs ({:.1}%, avg: {:.1}ms)",
                                        priority, count, percentage, avg_time
                                    );
                                }
                            }
                        }

                        // Check for priority starvation
                        let starved = priority_stats.check_starvation(5.0); // 5% threshold
                        if !starved.is_empty() {
                            info!("   ⚠️  Potential starvation detected for: {:?}", starved);
                            info!("      Consider adjusting priority weights or fairness factor");
                        }
                    }
                }
            }
            Err(e) => {
                error!("Failed to get stats for queue {}: {}", queue_name, e);
            }
        }
    }

    // Show overall system statistics
    match stats_collector
        .get_all_statistics(Duration::from_secs(300))
        .await
    {
        Ok(all_stats) => {
            if !all_stats.is_empty() {
                info!("\n🌐 Overall system statistics:");
                let total_processed: u64 =
                    all_stats.iter().map(|s| s.statistics.total_processed).sum();
                let total_completed: u64 = all_stats.iter().map(|s| s.statistics.completed).sum();
                let avg_throughput: f64 = all_stats
                    .iter()
                    .map(|s| s.statistics.throughput_per_minute)
                    .sum::<f64>()
                    / all_stats.len() as f64;

                info!("   Total jobs processed: {}", total_processed);
                info!("   Total completed: {}", total_completed);
                info!(
                    "   Average system throughput: {:.1} jobs/min",
                    avg_throughput
                );
                info!("   Active queues: {}", all_stats.len());
            }
        }
        Err(e) => {
            error!("Failed to get overall stats: {}", e);
        }
    }

    Ok(())
}

/// Demonstrate WorkerPool with mixed priority configurations
#[allow(dead_code)]
async fn demonstrate_worker_pool_priorities(
    queue: Arc<JobQueue<sqlx::Postgres>>,
    stats_collector: Arc<InMemoryStatsCollector>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    info!("\n👥 === WORKER POOL WITH PRIORITY CONFIGURATIONS ===");

    let mut pool = WorkerPool::new().with_stats_collector(stats_collector.clone());

    // Add workers with different priority configurations
    let handler: Arc<
        dyn Fn(
                Job,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = hammerwork::Result<()>> + Send>,
            > + Send
            + Sync,
    > = Arc::new(|job: Job| {
        Box::pin(async move {
            info!(
                "🔄 Pool worker processing: {} (Priority: {:?})",
                job.id, job.priority
            );
            sleep(Duration::from_millis(300)).await;
            Ok(())
        })
    });

    // Worker 1: Strict priority for critical notifications
    let critical_worker = Worker::new(queue.clone(), "notifications".to_string(), handler.clone())
        .with_strict_priority()
        .with_poll_interval(Duration::from_millis(50));

    // Worker 2: Weighted priority for general processing
    let weighted_worker = Worker::new(queue.clone(), "processing".to_string(), handler.clone())
        .with_weighted_priority()
        .with_poll_interval(Duration::from_millis(100));

    // Worker 3: Custom weights for specialized tasks
    let custom_weights = PriorityWeights::new()
        .with_weight(JobPriority::Critical, 50)
        .with_weight(JobPriority::High, 15)
        .with_fairness_factor(0.2);

    let custom_worker = Worker::new(queue.clone(), "cleanup".to_string(), handler)
        .with_priority_weights(custom_weights)
        .with_poll_interval(Duration::from_millis(200));

    pool.add_worker(critical_worker);
    pool.add_worker(weighted_worker);
    pool.add_worker(custom_worker);

    info!("🚀 Starting worker pool with 3 workers (different priority configs)...");

    // Note: In a real application, you would call:
    // pool.start().await?;
    // For this example, we just demonstrate the setup

    info!("✅ Worker pool configured successfully");
    Ok(())
}
