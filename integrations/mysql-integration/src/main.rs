use hammerwork::queue::DatabaseQueue;
use hammerwork::{HammerworkError, JobQueue};
use sqlx::{MySql, Pool};
use std::sync::Arc;
use tracing::{error, info, warn};

// Include shared test scenarios
#[path = "../../shared/test_scenarios.rs"]
mod test_scenarios;
use test_scenarios::TestScenarios;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mysql_integration=info".parse()?)
                .add_directive("hammerwork=debug".parse()?),
        )
        .init();

    info!("🐬 Starting MySQL integration tests for Hammerwork");

    // Get database URL from environment
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        warn!("DATABASE_URL not set, using default");
        "mysql://hammerwork:password@localhost:3306/hammerwork_test".to_string()
    });

    info!("📡 Connecting to MySQL: {}", database_url);

    // Connect to MySQL
    let pool = match Pool::<MySql>::connect(&database_url).await {
        Ok(pool) => {
            info!("✅ Successfully connected to MySQL");
            pool
        }
        Err(e) => {
            error!("❌ Failed to connect to MySQL: {}", e);
            return Err(e.into());
        }
    };

    // Create job queue
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables (use the migration system in production)
    info!("🛠️ Initializing database tables");
    // Note: In production, use the cargo-hammerwork migrate command instead
    info!("✅ Database tables should be initialized via migrations");

    // Run database-specific tests
    if let Err(e) = run_mysql_specific_tests(&queue).await {
        error!("❌ MySQL-specific tests failed: {}", e);
        return Err(e.into());
    }

    // Run shared test scenarios
    let test_scenarios = TestScenarios::new(queue.clone());

    info!("🧪 Running comprehensive integration tests");
    if let Err(e) = test_scenarios.run_all_tests().await {
        error!("❌ Integration tests failed: {}", e);
        return Err(e.into());
    }

    // Run performance benchmarks
    if let Err(e) = run_performance_tests(&queue).await {
        error!("❌ Performance tests failed: {}", e);
        return Err(e.into());
    }

    info!("🎉 All MySQL integration tests completed successfully!");
    Ok(())
}

/// MySQL-specific tests
async fn run_mysql_specific_tests(
    queue: &Arc<JobQueue<MySql>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Running MySQL-specific tests");

    // Test JSON operations
    test_json_operations(queue).await?;

    // Test MySQL transaction behavior
    test_mysql_transactions(queue).await?;

    // Test MySQL-specific character handling
    test_mysql_character_handling(queue).await?;

    info!("✅ MySQL-specific tests completed");
    Ok(())
}

/// Test JSON operations specific to MySQL
async fn test_json_operations(
    queue: &Arc<JobQueue<MySql>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Testing MySQL JSON operations");

    use hammerwork::Job;
    use serde_json::json;

    // Create a job with complex JSON payload
    let complex_payload = json!({
        "user": {
            "id": 67890,
            "name": "MySQL Test User",
            "settings": {
                "theme": "light",
                "locale": "en_US",
                "features": ["feature1", "feature2", "feature3"]
            }
        },
        "metadata": {
            "source": "mysql_integration_test",
            "timestamp": chrono::Utc::now().timestamp(),
            "tags": ["test", "mysql", "json"],
            "config": {
                "retry_count": 3,
                "timeout": 30,
                "priority": "high"
            }
        }
    });

    let job = Job::new("json_test".to_string(), complex_payload.clone());
    let job_id = job.id;

    // Enqueue and process the job
    queue.enqueue(job).await?;
    info!("✅ Complex JSON job enqueued");

    let dequeued_job = queue.dequeue("json_test").await?;
    assert!(dequeued_job.is_some());
    let dequeued_job = dequeued_job.unwrap();

    // Verify the complex payload was preserved
    assert_eq!(dequeued_job.payload, complex_payload);
    info!("✅ Complex JSON payload preserved correctly");

    // Complete and cleanup
    queue.complete_job(job_id).await?;
    queue.delete_job(job_id).await?;

    info!("✅ JSON operations test completed");
    Ok(())
}

/// Test MySQL transaction behavior
async fn test_mysql_transactions(
    queue: &Arc<JobQueue<MySql>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Testing MySQL transaction behavior");

    use hammerwork::Job;
    use serde_json::json;

    // Test that MySQL transactions work correctly for job dequeuing
    let mut job_ids = Vec::new();
    for i in 0..3 {
        let job = Job::new(
            "transaction_test".to_string(),
            json!({
                "index": i,
                "message": format!("Transaction test job {}", i)
            }),
        );
        job_ids.push(job.id);
        queue.enqueue(job).await?;
    }
    info!("✅ Enqueued 3 jobs for transaction test");

    // Try concurrent dequeuing to test MySQL's transaction isolation
    let mut handles = Vec::new();
    for worker_id in 0..2 {
        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            let result = queue_clone.dequeue("transaction_test").await;
            (worker_id, result)
        });
        handles.push(handle);
    }

    let mut successful_dequeues = 0;
    for handle in handles {
        let (worker_id, result) = handle.await.unwrap();
        match result {
            Ok(Some(job)) => {
                info!(
                    "✅ Worker {} successfully dequeued job {}",
                    worker_id, job.id
                );
                successful_dequeues += 1;
                let _ = queue.complete_job(job.id).await;
            }
            Ok(None) => {
                info!("ℹ️ Worker {} found no available jobs", worker_id);
            }
            Err(e) => {
                error!("❌ Worker {} failed to dequeue: {}", worker_id, e);
            }
        }
    }

    info!(
        "✅ MySQL transaction test completed with {} successful dequeues",
        successful_dequeues
    );

    // Cleanup remaining jobs
    for job_id in job_ids {
        let _ = queue.delete_job(job_id).await;
    }

    info!("✅ MySQL transaction behavior test completed");
    Ok(())
}

/// Test MySQL character encoding and special characters
async fn test_mysql_character_handling(
    queue: &Arc<JobQueue<MySql>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Testing MySQL character encoding");

    use hammerwork::Job;
    use serde_json::json;

    // Test with various character encodings and special characters
    let special_payload = json!({
        "unicode_text": "Hello 世界! 🌟 Ñoñó café naïve résumé",
        "emoji": "🚀🎉🔥💪🌈",
        "special_chars": "!@#$%^&*()_+-=[]{}|;':\",./<>?",
        "multilingual": {
            "english": "Hello World",
            "spanish": "Hola Mundo",
            "chinese": "你好世界",
            "japanese": "こんにちは世界",
            "arabic": "مرحبا بالعالم",
            "russian": "Привет мир"
        }
    });

    let job = Job::new("encoding_test".to_string(), special_payload.clone());
    let job_id = job.id;

    // Enqueue and process the job
    queue.enqueue(job).await?;
    info!("✅ Special character job enqueued");

    let dequeued_job = queue.dequeue("encoding_test").await?;
    assert!(dequeued_job.is_some());
    let dequeued_job = dequeued_job.unwrap();

    // Verify the special characters were preserved
    assert_eq!(dequeued_job.payload, special_payload);
    info!("✅ Special characters preserved correctly");

    // Complete and cleanup
    queue.complete_job(job_id).await?;
    queue.delete_job(job_id).await?;

    info!("✅ Character encoding test completed");
    Ok(())
}

/// Run performance benchmarks for MySQL
async fn run_performance_tests(
    queue: &Arc<JobQueue<MySql>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🏎️ Running MySQL performance benchmarks");

    use hammerwork::Job;
    use serde_json::json;
    use std::time::Instant;

    // Benchmark job enqueueing
    let start = Instant::now();
    let job_count = 100;

    let mut job_ids = Vec::new();
    for i in 0..job_count {
        let job = Job::new(
            "mysql_performance_test".to_string(),
            json!({
                "index": i,
                "timestamp": chrono::Utc::now().timestamp(),
                "data": format!("MySQL performance test data {}", i),
                "metadata": {
                    "worker": "mysql-integration",
                    "iteration": i
                }
            }),
        );
        job_ids.push(job.id);
        queue.enqueue(job).await?;
    }

    let enqueue_duration = start.elapsed();
    let enqueue_rate = job_count as f64 / enqueue_duration.as_secs_f64();
    info!(
        "✅ Enqueued {} jobs in {:?} ({:.2} jobs/sec)",
        job_count, enqueue_duration, enqueue_rate
    );

    // Benchmark job dequeuing
    let start = Instant::now();
    let mut dequeued_count = 0;

    for _ in 0..job_count {
        if let Ok(Some(job)) = queue.dequeue("mysql_performance_test").await {
            dequeued_count += 1;
            let _ = queue.complete_job(job.id).await;
        }
    }

    let dequeue_duration = start.elapsed();
    let dequeue_rate = dequeued_count as f64 / dequeue_duration.as_secs_f64();
    info!(
        "✅ Dequeued {} jobs in {:?} ({:.2} jobs/sec)",
        dequeued_count, dequeue_duration, dequeue_rate
    );

    // Test batch operations
    let start = Instant::now();
    let batch_size = 50;
    let mut batch_job_ids = Vec::new();

    for i in 0..batch_size {
        let job = Job::new(
            "mysql_batch_test".to_string(),
            json!({
                "batch_index": i,
                "batch_id": uuid::Uuid::new_v4()
            }),
        );
        batch_job_ids.push(job.id);
        queue.enqueue(job).await?;
    }

    let batch_duration = start.elapsed();
    let batch_rate = batch_size as f64 / batch_duration.as_secs_f64();
    info!(
        "✅ Batch processed {} jobs in {:?} ({:.2} jobs/sec)",
        batch_size, batch_duration, batch_rate
    );

    // Cleanup
    for job_id in job_ids.into_iter().chain(batch_job_ids.into_iter()) {
        let _ = queue.delete_job(job_id).await;
    }

    // Performance assertions (MySQL might be slightly slower than PostgreSQL)
    assert!(
        enqueue_rate > 5.0,
        "MySQL enqueue rate should be > 5 jobs/sec"
    );
    assert!(
        dequeue_rate > 5.0,
        "MySQL dequeue rate should be > 5 jobs/sec"
    );

    info!("✅ MySQL performance benchmarks completed");
    Ok(())
}
