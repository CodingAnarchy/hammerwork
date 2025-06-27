//! # Job Result Storage Example
//!
//! This example demonstrates how to use Hammerwork's job result storage functionality
//! to store and retrieve data from completed jobs.
//!
//! ## Features Demonstrated
//!
//! - Creating jobs with result storage enabled
//! - Using enhanced job handlers that return result data
//! - Configuring TTL (time-to-live) for results
//! - Automatic result storage by workers
//! - Manual result retrieval and cleanup
//! - Different result storage configurations
//!
//! ## Usage
//!
//! ```bash
//! # With PostgreSQL
//! DATABASE_URL=postgresql://localhost/hammerwork cargo run --example result_storage_example --features postgres
//!
//! # With MySQL  
//! DATABASE_URL=mysql://localhost/hammerwork cargo run --example result_storage_example --features mysql
//! ```

use hammerwork::{
    Job, JobQueue, Worker, WorkerPool,
    job::ResultStorage,
    queue::DatabaseQueue,
    worker::{JobHandler, JobHandlerWithResult, JobResult},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Connect to database
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        #[cfg(feature = "postgres")]
        return "postgresql://localhost/hammerwork".to_string();
        #[cfg(feature = "mysql")]
        return "mysql://localhost/hammerwork".to_string();
        #[cfg(not(any(feature = "postgres", feature = "mysql")))]
        panic!("No database feature enabled. Use --features postgres or --features mysql");
    });

    println!("🔗 Connecting to database: {}", database_url);

    #[cfg(feature = "postgres")]
    let pool = sqlx::PgPool::connect(&database_url).await?;
    #[cfg(feature = "mysql")]
    let pool = sqlx::MySqlPool::connect(&database_url).await?;

    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    // Note: Run `cargo hammerwork migrate` to create the necessary database tables
    println!("📋 Database tables should be initialized using 'cargo hammerwork migrate'");

    // Demonstrate different aspects of result storage
    #[cfg(feature = "postgres")]
    {
        demonstrate_basic_result_storage_postgres(&queue).await?;
        demonstrate_enhanced_workers_postgres(&queue).await?;
        demonstrate_result_expiration_postgres(&queue).await?;
        demonstrate_legacy_compatibility_postgres(&queue).await?;
    }

    #[cfg(feature = "mysql")]
    {
        demonstrate_basic_result_storage_mysql(&queue).await?;
        demonstrate_enhanced_workers_mysql(&queue).await?;
        demonstrate_result_expiration_mysql(&queue).await?;
        demonstrate_legacy_compatibility_mysql(&queue).await?;
    }

    println!("✅ Example completed successfully!");
    Ok(())
}

#[cfg(feature = "postgres")]
async fn demonstrate_basic_result_storage_postgres(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🎯 === Basic Result Storage ===");

    // Create a job with result storage enabled
    let job = Job::new(
        "data_processing".to_string(),
        json!({
            "dataset": "customer_data_2024",
            "operation": "analytics"
        }),
    )
    .with_result_storage(ResultStorage::Database)
    .with_result_ttl(Duration::from_secs(3600)); // 1 hour TTL

    println!("📝 Created job with result storage enabled");
    let job_id = queue.enqueue(job).await?;
    println!("   Job ID: {}", job_id);

    // Simulate processing and store result manually
    let processing_result = json!({
        "total_records": 150_000,
        "processed_records": 149_890,
        "errors": 110,
        "processing_time_ms": 45_230,
        "output_files": [
            "/data/output/summary.json",
            "/data/output/detailed_report.csv"
        ],
        "statistics": {
            "avg_processing_time_per_record_ms": 0.301,
            "memory_usage_mb": 2_048,
            "cpu_usage_percent": 85.2
        }
    });

    println!("💾 Storing job result...");
    queue
        .store_job_result(job_id, processing_result.clone(), None)
        .await?;

    // Retrieve the result
    println!("🔍 Retrieving stored result...");
    if let Some(stored_result) = queue.get_job_result(job_id).await? {
        println!("   ✅ Result retrieved successfully");
        println!("   📊 Processed {} records", stored_result["total_records"]);
        println!(
            "   ⏱️  Processing time: {}ms",
            stored_result["processing_time_ms"]
        );
        println!("   📁 Output files: {:?}", stored_result["output_files"]);
    } else {
        println!("   ❌ No result found");
    }

    // Clean up
    queue.delete_job_result(job_id).await?;
    println!("🗑️  Result deleted");

    Ok(())
}

#[cfg(feature = "postgres")]
async fn demonstrate_enhanced_workers_postgres(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🤖 === Enhanced Workers with Result Storage ===");

    // Create an enhanced job handler that returns result data
    let handler: JobHandlerWithResult = Arc::new(|job| {
        Box::pin(async move {
            let start_time = std::time::Instant::now();

            // Simulate different types of processing based on job payload
            let task_type = job.payload["task_type"].as_str().unwrap_or("default");
            let processing_duration = match task_type {
                "quick" => Duration::from_millis(100),
                "medium" => Duration::from_millis(500),
                "heavy" => Duration::from_millis(1000),
                _ => Duration::from_millis(300),
            };

            println!(
                "   🔄 Processing {} task (estimated {}ms)...",
                task_type,
                processing_duration.as_millis()
            );

            // Simulate processing
            tokio::time::sleep(processing_duration).await;

            let actual_duration = start_time.elapsed();

            // Generate realistic result data
            let result_data = match task_type {
                "quick" => json!({
                    "task_type": task_type,
                    "processing_time_ms": actual_duration.as_millis(),
                    "cache_hits": 95,
                    "cache_misses": 5,
                    "status": "completed"
                }),
                "medium" => json!({
                    "task_type": task_type,
                    "processing_time_ms": actual_duration.as_millis(),
                    "records_processed": 1_000,
                    "transformations_applied": 15,
                    "validation_passed": true,
                    "output_size_bytes": 256_000,
                    "status": "completed"
                }),
                "heavy" => json!({
                    "task_type": task_type,
                    "processing_time_ms": actual_duration.as_millis(),
                    "dataset_size_gb": 2.5,
                    "models_trained": 3,
                    "accuracy_score": 0.94,
                    "feature_importance": {
                        "price": 0.45,
                        "location": 0.32,
                        "size": 0.23
                    },
                    "status": "completed"
                }),
                _ => json!({
                    "task_type": task_type,
                    "processing_time_ms": actual_duration.as_millis(),
                    "status": "completed"
                }),
            };

            println!("   ✅ Task completed in {}ms", actual_duration.as_millis());

            Ok(JobResult::with_data(result_data))
        })
    });

    // Create worker with enhanced handler
    let worker =
        Worker::new_with_result_handler(queue.clone(), "enhanced_processing".to_string(), handler)
            .with_poll_interval(Duration::from_millis(100));

    // Create different types of jobs
    let job_types = ["quick", "medium", "heavy"];
    let mut job_ids = Vec::new();

    for task_type in &job_types {
        let job = Job::new(
            "enhanced_processing".to_string(),
            json!({
                "task_type": task_type,
                "priority": "high"
            }),
        )
        .with_result_storage(ResultStorage::Database)
        .with_result_ttl(Duration::from_secs(7200)); // 2 hours TTL

        let job_id = queue.enqueue(job).await?;
        job_ids.push(job_id);
        println!("📝 Enqueued {} task with ID: {}", task_type, job_id);
    }

    // Start worker pool to process jobs
    println!("🚀 Starting worker to process jobs...");
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(worker);

    // Run worker pool for a limited time
    let worker_handle = tokio::spawn(async move { worker_pool.start().await });

    // Wait for jobs to be processed
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check results for each job
    println!("🔍 Checking stored results...");
    for (i, &job_id) in job_ids.iter().enumerate() {
        if let Some(result) = queue.get_job_result(job_id).await? {
            println!("   📊 {} task result:", job_types[i]);
            println!(
                "      - Processing time: {}ms",
                result["processing_time_ms"]
            );
            println!("      - Status: {}", result["status"]);

            // Show specific metrics based on task type
            match job_types[i] {
                "quick" => {
                    println!("      - Cache hits: {}", result["cache_hits"]);
                }
                "medium" => {
                    println!("      - Records processed: {}", result["records_processed"]);
                    println!("      - Output size: {} bytes", result["output_size_bytes"]);
                }
                "heavy" => {
                    println!("      - Dataset size: {} GB", result["dataset_size_gb"]);
                    println!("      - Models trained: {}", result["models_trained"]);
                    println!("      - Accuracy: {}", result["accuracy_score"]);
                }
                _ => {}
            }
        } else {
            println!("   ❌ No result found for {} task", job_types[i]);
        }
    }

    // Stop worker
    worker_handle.abort();
    println!("🛑 Worker stopped");

    Ok(())
}

#[cfg(feature = "postgres")]
async fn demonstrate_result_expiration_postgres(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n⏰ === Result Expiration and Cleanup ===");

    // Create jobs with different expiration times
    let job1 = Job::new(
        "temp_processing".to_string(),
        json!({"task": "short_lived"}),
    );
    let job2 = Job::new("temp_processing".to_string(), json!({"task": "long_lived"}));

    let job_id1 = queue.enqueue(job1).await?;
    let job_id2 = queue.enqueue(job2).await?;

    // Store results with different expiration times
    let short_lived_result = json!({"data": "expires_soon", "created_at": chrono::Utc::now()});
    let long_lived_result = json!({"data": "expires_later", "created_at": chrono::Utc::now()});

    // First result expires in 2 seconds
    let expires_soon = chrono::Utc::now() + chrono::Duration::seconds(2);
    // Second result expires in 1 hour
    let expires_later = chrono::Utc::now() + chrono::Duration::hours(1);

    println!("💾 Storing results with different expiration times...");
    queue
        .store_job_result(job_id1, short_lived_result, Some(expires_soon))
        .await?;
    queue
        .store_job_result(job_id2, long_lived_result, Some(expires_later))
        .await?;

    // Check both results are initially available
    println!("🔍 Checking initial availability...");
    assert!(queue.get_job_result(job_id1).await?.is_some());
    assert!(queue.get_job_result(job_id2).await?.is_some());
    println!("   ✅ Both results available");

    // Wait for the first result to expire
    println!("⏳ Waiting for first result to expire...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Check results after expiration
    println!("🔍 Checking after expiration...");
    let result1 = queue.get_job_result(job_id1).await?;
    let result2 = queue.get_job_result(job_id2).await?;

    if result1.is_none() {
        println!("   ✅ Short-lived result correctly expired");
    } else {
        println!("   ❌ Short-lived result should have expired");
    }

    if result2.is_some() {
        println!("   ✅ Long-lived result still available");
    } else {
        println!("   ❌ Long-lived result should still be available");
    }

    // Demonstrate cleanup of expired results
    println!("🧹 Running cleanup of expired results...");
    let cleaned_count = queue.cleanup_expired_results().await?;
    println!("   🗑️  Cleaned up {} expired results", cleaned_count);

    Ok(())
}

#[cfg(feature = "postgres")]
async fn demonstrate_legacy_compatibility_postgres(
    queue: &Arc<JobQueue<sqlx::Postgres>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🔄 === Legacy Handler Compatibility ===");

    // Create a traditional job handler (returns ())
    let legacy_handler: JobHandler = Arc::new(|job| {
        Box::pin(async move {
            println!("   🔧 Processing job with legacy handler...");

            // Simulate work
            let work_duration = Duration::from_millis(200);
            tokio::time::sleep(work_duration).await;

            // Legacy handlers just return Ok(()) - no result data
            println!("   ✅ Legacy job completed successfully");
            Ok(())
        })
    });

    // Create worker with legacy handler
    let legacy_worker = Worker::new(queue.clone(), "legacy_queue".to_string(), legacy_handler)
        .with_poll_interval(Duration::from_millis(100));

    // Create jobs - some with result storage enabled, some without
    let job1 = Job::new("legacy_queue".to_string(), json!({"task": "no_storage"}))
        .with_result_storage(ResultStorage::None);

    let job2 = Job::new(
        "legacy_queue".to_string(),
        json!({"task": "storage_enabled"}),
    )
    .with_result_storage(ResultStorage::Database);

    let job_id1 = queue.enqueue(job1).await?;
    let job_id2 = queue.enqueue(job2).await?;

    println!("📝 Created jobs with legacy worker:");
    println!("   - Job 1: Result storage disabled");
    println!("   - Job 2: Result storage enabled (but handler returns no data)");

    // Process jobs
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(legacy_worker);

    let worker_handle = tokio::spawn(async move { worker_pool.start().await });

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Check results
    println!("🔍 Checking results from legacy handler:");

    let result1 = queue.get_job_result(job_id1).await?;
    let result2 = queue.get_job_result(job_id2).await?;

    if result1.is_none() && result2.is_none() {
        println!("   ✅ No results stored (expected for legacy handlers)");
        println!("   💡 Legacy handlers work normally, just without result data");
    } else {
        println!("   ⚠️  Unexpected results found");
    }

    worker_handle.abort();

    Ok(())
}

// MySQL versions of the demonstration functions
#[cfg(feature = "mysql")]
async fn demonstrate_basic_result_storage_mysql(
    queue: &Arc<JobQueue<sqlx::MySql>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🎯 === Basic Result Storage (MySQL) ===");

    // Create a job with result storage enabled
    let job = Job::new(
        "data_processing".to_string(),
        json!({
            "dataset": "customer_data_2024",
            "operation": "analytics"
        }),
    )
    .with_result_storage(ResultStorage::Database)
    .with_result_ttl(Duration::from_secs(3600)); // 1 hour TTL

    println!("📝 Created job with result storage enabled");
    let job_id = queue.enqueue(job).await?;
    println!("   Job ID: {}", job_id);

    // Simulate processing and store result manually
    let processing_result = json!({
        "total_records": 150_000,
        "processed_records": 149_890,
        "errors": 110,
        "processing_time_ms": 45_230,
        "output_files": [
            "/data/output/summary.json",
            "/data/output/detailed_report.csv"
        ],
        "database": "mysql"
    });

    println!("💾 Storing job result...");
    queue
        .store_job_result(job_id, processing_result.clone(), None)
        .await?;

    // Retrieve the result
    println!("🔍 Retrieving stored result...");
    if let Some(stored_result) = queue.get_job_result(job_id).await? {
        println!("   ✅ Result retrieved successfully from MySQL");
        println!("   📊 Processed {} records", stored_result["total_records"]);
        println!(
            "   ⏱️  Processing time: {}ms",
            stored_result["processing_time_ms"]
        );
    } else {
        println!("   ❌ No result found");
    }

    // Clean up
    queue.delete_job_result(job_id).await?;
    println!("🗑️  Result deleted");

    Ok(())
}

#[cfg(feature = "mysql")]
async fn demonstrate_enhanced_workers_mysql(
    queue: &Arc<JobQueue<sqlx::MySql>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🤖 === Enhanced Workers with Result Storage (MySQL) ===");

    // Create an enhanced job handler
    let handler: JobHandlerWithResult = Arc::new(|job| {
        Box::pin(async move {
            let task_type = job.payload["task_type"].as_str().unwrap_or("default");

            println!("   🔄 Processing {} task with MySQL backend...", task_type);

            // Simulate processing
            tokio::time::sleep(Duration::from_millis(300)).await;

            let result_data = json!({
                "task_type": task_type,
                "database": "mysql",
                "processing_time_ms": 300,
                "status": "completed"
            });

            println!("   ✅ Task completed");
            Ok(JobResult::with_data(result_data))
        })
    });

    // Create worker
    let worker =
        Worker::new_with_result_handler(queue.clone(), "mysql_processing".to_string(), handler);

    // Create a job
    let job = Job::new(
        "mysql_processing".to_string(),
        json!({"task_type": "mysql_test"}),
    )
    .with_result_storage(ResultStorage::Database);

    let job_id = queue.enqueue(job).await?;
    println!("📝 Enqueued MySQL job with ID: {}", job_id);

    // Process the job
    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(worker);

    let worker_handle = tokio::spawn(async move { worker_pool.start().await });

    tokio::time::sleep(Duration::from_secs(1)).await;

    // Check result
    if let Some(result) = queue.get_job_result(job_id).await? {
        println!("   📊 MySQL result:");
        println!("      - Database: {}", result["database"]);
        println!("      - Status: {}", result["status"]);
    }

    worker_handle.abort();
    Ok(())
}

#[cfg(feature = "mysql")]
async fn demonstrate_result_expiration_mysql(
    queue: &Arc<JobQueue<sqlx::MySql>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n⏰ === Result Expiration and Cleanup (MySQL) ===");

    let job = Job::new(
        "temp_processing".to_string(),
        json!({"task": "mysql_expiration"}),
    );
    let job_id = queue.enqueue(job).await?;

    let result_data = json!({"data": "expires_soon", "database": "mysql"});
    let expires_soon = chrono::Utc::now() + chrono::Duration::seconds(2);

    println!("💾 Storing MySQL result with 2-second TTL...");
    queue
        .store_job_result(job_id, result_data, Some(expires_soon))
        .await?;

    println!("🔍 Result available initially");
    assert!(queue.get_job_result(job_id).await?.is_some());

    println!("⏳ Waiting for expiration...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let result_after_expiration = queue.get_job_result(job_id).await?;
    if result_after_expiration.is_none() {
        println!("   ✅ MySQL result correctly expired");
    }

    let cleaned_count = queue.cleanup_expired_results().await?;
    println!("🧹 Cleaned up {} expired MySQL results", cleaned_count);

    Ok(())
}

#[cfg(feature = "mysql")]
async fn demonstrate_legacy_compatibility_mysql(
    queue: &Arc<JobQueue<sqlx::MySql>>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🔄 === Legacy Handler Compatibility (MySQL) ===");

    let legacy_handler: JobHandler = Arc::new(|_job| {
        Box::pin(async move {
            println!("   🔧 Processing job with legacy handler on MySQL...");
            tokio::time::sleep(Duration::from_millis(200)).await;
            println!("   ✅ Legacy MySQL job completed");
            Ok(())
        })
    });

    let legacy_worker = Worker::new(queue.clone(), "mysql_legacy".to_string(), legacy_handler);

    let job = Job::new("mysql_legacy".to_string(), json!({"task": "legacy_mysql"}))
        .with_result_storage(ResultStorage::Database);

    let job_id = queue.enqueue(job).await?;

    let mut worker_pool = WorkerPool::new();
    worker_pool.add_worker(legacy_worker);

    let worker_handle = tokio::spawn(async move { worker_pool.start().await });

    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = queue.get_job_result(job_id).await?;
    if result.is_none() {
        println!("   ✅ No result stored for MySQL legacy handler (expected)");
    }

    worker_handle.abort();
    Ok(())
}
