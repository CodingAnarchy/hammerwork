use hammerwork::queue::DatabaseQueue;
use hammerwork::{HammerworkError, JobQueue};
use sqlx::{Pool, Postgres};
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
                .add_directive("postgres_integration=info".parse()?)
                .add_directive("hammerwork=debug".parse()?),
        )
        .init();

    info!("🐘 Starting PostgreSQL integration tests for Hammerwork");

    // Get database URL from environment
    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        warn!("DATABASE_URL not set, using default");
        "postgres://postgres:password@localhost:5432/hammerwork_test".to_string()
    });

    info!("📡 Connecting to PostgreSQL: {}", database_url);

    // Connect to PostgreSQL
    let pool = match Pool::<Postgres>::connect(&database_url).await {
        Ok(pool) => {
            info!("✅ Successfully connected to PostgreSQL");
            pool
        }
        Err(e) => {
            error!("❌ Failed to connect to PostgreSQL: {}", e);
            return Err(e.into());
        }
    };

    // Create job queue
    let queue = Arc::new(JobQueue::new(pool));

    // Initialize database tables
    info!("🛠️ Initializing database tables");
    if let Err(e) = queue.create_tables().await {
        error!("❌ Failed to create tables: {}", e);
        return Err(e.into());
    }
    info!("✅ Database tables initialized");

    // Run database-specific tests
    if let Err(e) = run_postgres_specific_tests(&queue).await {
        error!("❌ PostgreSQL-specific tests failed: {}", e);
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

    info!("🎉 All PostgreSQL integration tests completed successfully!");
    Ok(())
}

/// PostgreSQL-specific tests
async fn run_postgres_specific_tests(
    queue: &Arc<JobQueue<Postgres>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Running PostgreSQL-specific tests");

    // Test JSONB operations
    test_jsonb_operations(queue).await?;

    // Test PostgreSQL-specific features
    test_postgres_features(queue).await?;

    info!("✅ PostgreSQL-specific tests completed");
    Ok(())
}

/// Test JSONB operations specific to PostgreSQL
async fn test_jsonb_operations(
    queue: &Arc<JobQueue<Postgres>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Testing PostgreSQL JSONB operations");

    use hammerwork::Job;
    use serde_json::json;

    // Create a job with complex JSON payload
    let complex_payload = json!({
        "user": {
            "id": 12345,
            "name": "Test User",
            "preferences": {
                "theme": "dark",
                "notifications": true,
                "languages": ["en", "es", "fr"]
            }
        },
        "metadata": {
            "source": "integration_test",
            "timestamp": chrono::Utc::now().timestamp(),
            "tags": ["test", "postgres", "jsonb"]
        },
        "nested_array": [
            {"id": 1, "value": "first"},
            {"id": 2, "value": "second"},
            {"id": 3, "value": "third"}
        ]
    });

    let job = Job::new("jsonb_test".to_string(), complex_payload.clone());
    let job_id = job.id;

    // Enqueue and process the job
    queue.enqueue(job).await?;
    info!("✅ Complex JSONB job enqueued");

    let dequeued_job = queue.dequeue("jsonb_test").await?;
    assert!(dequeued_job.is_some());
    let dequeued_job = dequeued_job.unwrap();

    // Verify the complex payload was preserved
    assert_eq!(dequeued_job.payload, complex_payload);
    info!("✅ Complex JSONB payload preserved correctly");

    // Complete and cleanup
    queue.complete_job(job_id).await?;
    queue.delete_job(job_id).await?;

    info!("✅ JSONB operations test completed");
    Ok(())
}

/// Test PostgreSQL-specific features
async fn test_postgres_features(
    queue: &Arc<JobQueue<Postgres>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🧪 Testing PostgreSQL-specific features");

    use hammerwork::Job;
    use serde_json::json;

    // Test FOR UPDATE SKIP LOCKED behavior with concurrent access
    let mut job_ids = Vec::new();
    for i in 0..5 {
        let job = Job::new(
            "postgres_concurrent".to_string(),
            json!({
                "index": i,
                "message": format!("Concurrent test job {}", i)
            }),
        );
        job_ids.push(job.id);
        queue.enqueue(job).await?;
    }
    info!("✅ Enqueued 5 jobs for concurrent dequeue test");

    // Try to dequeue concurrently to test SKIP LOCKED
    let mut handles = Vec::new();
    for worker_id in 0..3 {
        let queue_clone = queue.clone();
        let handle = tokio::spawn(async move {
            let result = queue_clone.dequeue("postgres_concurrent").await;
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
        "✅ Successfully dequeued {} jobs concurrently",
        successful_dequeues
    );
    assert!(
        successful_dequeues > 0,
        "Should have dequeued at least one job"
    );

    // Cleanup remaining jobs
    for job_id in job_ids {
        let _ = queue.delete_job(job_id).await;
    }

    info!("✅ PostgreSQL-specific features test completed");
    Ok(())
}

/// Run performance benchmarks
async fn run_performance_tests(
    queue: &Arc<JobQueue<Postgres>>,
) -> std::result::Result<(), HammerworkError> {
    info!("🏎️ Running performance benchmarks");

    use hammerwork::Job;
    use serde_json::json;
    use std::time::Instant;

    // Benchmark job enqueueing
    let start = Instant::now();
    let job_count = 100;

    let mut job_ids = Vec::new();
    for i in 0..job_count {
        let job = Job::new(
            "performance_test".to_string(),
            json!({
                "index": i,
                "timestamp": chrono::Utc::now().timestamp(),
                "data": format!("Performance test data {}", i)
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
        if let Ok(Some(job)) = queue.dequeue("performance_test").await {
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

    // Cleanup
    for job_id in job_ids {
        let _ = queue.delete_job(job_id).await;
    }

    // Performance assertions
    assert!(enqueue_rate > 10.0, "Enqueue rate should be > 10 jobs/sec");
    assert!(dequeue_rate > 10.0, "Dequeue rate should be > 10 jobs/sec");

    info!("✅ Performance benchmarks completed");
    Ok(())
}
