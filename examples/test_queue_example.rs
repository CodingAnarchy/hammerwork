//! Example demonstrating the TestQueue for testing job processing logic
//!
//! This example shows how to use the TestQueue to test job handlers and
//! complex workflows without requiring a database connection. This is
//! particularly useful for:
//!
//! - Unit testing job processing logic
//! - Testing complex workflows and dependencies
//! - Development and prototyping
//! - CI/CD pipelines that don't have database access
//! - Testing error scenarios and edge cases

use chrono::Duration;
use hammerwork::{
    Job, JobPriority, JobStatus,
    batch::JobBatch,
    priority::PriorityWeights,
    queue::{
        DatabaseQueue,
        test::{MockClock, TestQueue},
    },
    workflow::{FailurePolicy, JobGroup},
};
use serde_json::json;
use std::sync::Arc;
use tracing::info;

// Sample job handler for email processing
async fn email_handler(job: Job) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing email job: {}", job.id);

    let payload = &job.payload;
    let to = payload["to"].as_str().ok_or("Missing 'to' field")?;
    let subject = payload["subject"]
        .as_str()
        .ok_or("Missing 'subject' field")?;
    let _body = payload["body"].as_str().unwrap_or("(no body)");

    // Simulate email sending
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    info!("Email sent to {}: {}", to, subject);

    // Simulate some failures
    if to.contains("invalid") {
        return Err("Invalid email address".into());
    }

    Ok(())
}

// Sample job handler for image processing
async fn image_handler(job: Job) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing image job: {}", job.id);

    let payload = &job.payload;
    let image_url = payload["image_url"]
        .as_str()
        .ok_or("Missing 'image_url' field")?;
    let operation = payload["operation"].as_str().unwrap_or("resize");

    // Simulate image processing
    let processing_time = match operation {
        "resize" => 200,
        "compress" => 300,
        "thumbnail" => 150,
        _ => 250,
    };

    tokio::time::sleep(std::time::Duration::from_millis(processing_time)).await;

    info!("Image processed: {} ({})", image_url, operation);
    Ok(())
}

// Sample job handler for data analysis
async fn analysis_handler(job: Job) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing analysis job: {}", job.id);

    let payload = &job.payload;
    let dataset = payload["dataset"]
        .as_str()
        .ok_or("Missing 'dataset' field")?;
    let analysis_type = payload["analysis_type"].as_str().unwrap_or("basic");

    // Simulate data analysis
    let processing_time = match analysis_type {
        "basic" => 500,
        "advanced" => 1000,
        "ml" => 2000,
        _ => 500,
    };

    tokio::time::sleep(std::time::Duration::from_millis(processing_time)).await;

    info!(
        "Analysis completed for dataset: {} ({})",
        dataset, analysis_type
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt().with_env_filter("info").init();

    println!("🧪 Hammerwork TestQueue Example");
    println!("==============================");

    // Basic TestQueue Usage
    basic_usage().await?;

    // Time Control for Testing Delayed Jobs
    time_control_example().await?;

    // Priority and Weighted Selection
    priority_example().await?;

    // Batch Processing
    batch_example().await?;

    // Workflow with Dependencies
    workflow_example().await?;

    // Error Handling and Retry Logic
    error_handling_example().await?;

    // Cron Job Testing
    cron_job_example().await?;

    // Job Result Storage
    result_storage_example().await?;

    // Worker Integration Testing
    worker_integration_example().await?;

    // Performance Testing
    performance_testing_example().await?;

    println!("\n✅ All examples completed successfully!");

    Ok(())
}

async fn basic_usage() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📝 Basic TestQueue Usage");
    println!("------------------------");

    // Create a test queue
    let queue = TestQueue::new();

    // Enqueue some jobs
    let email_job = Job::new(
        "email_queue".to_string(),
        json!({
            "to": "user@example.com",
            "subject": "Welcome!",
            "body": "Thanks for signing up"
        }),
    );

    let job_id = queue.enqueue(email_job).await?;
    println!("📧 Enqueued email job: {}", job_id);

    // Check queue state
    let pending_count = queue
        .get_job_count("email_queue", &JobStatus::Pending)
        .await;
    println!("📊 Pending jobs: {}", pending_count);

    // Dequeue and process
    if let Some(job) = queue.dequeue("email_queue").await? {
        println!("🔄 Processing job: {}", job.id);

        // Simulate processing
        match email_handler(job.clone()).await {
            Ok(_) => {
                queue.complete_job(job.id).await?;
                println!("✅ Job completed successfully");
            }
            Err(e) => {
                queue.fail_job(job.id, &e.to_string()).await?;
                println!("❌ Job failed: {}", e);
            }
        }
    }

    // Check final state
    let completed_count = queue
        .get_job_count("email_queue", &JobStatus::Completed)
        .await;
    println!("🎉 Completed jobs: {}", completed_count);

    Ok(())
}

async fn time_control_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n⏰ Time Control Example");
    println!("----------------------");

    // Create a test queue with mock clock
    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create delayed jobs
    let immediate_job = Job::new("delayed_queue".to_string(), json!({"delay": "none"}));
    let delayed_job = Job::with_delay(
        "delayed_queue".to_string(),
        json!({"delay": "1 hour"}),
        Duration::hours(1),
    );

    let immediate_id = queue.enqueue(immediate_job).await?;
    let delayed_id = queue.enqueue(delayed_job).await?;

    println!("📨 Enqueued immediate job: {}", immediate_id);
    println!("⏳ Enqueued delayed job: {} (1 hour delay)", delayed_id);

    // Only immediate job should be available
    assert!(queue.dequeue("delayed_queue").await?.is_some());
    assert!(queue.dequeue("delayed_queue").await?.is_none());
    println!("✅ Only immediate job was available");

    // Advance time by 1 hour
    clock.advance(Duration::hours(1));
    println!("🕐 Advanced time by 1 hour");

    // Now delayed job should be available
    if let Some(job) = queue.dequeue("delayed_queue").await? {
        assert_eq!(job.id, delayed_id);
        queue.complete_job(job.id).await?;
        println!("✅ Delayed job is now available and processed");
    }

    Ok(())
}

async fn priority_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🏆 Priority and Weighted Selection Example");
    println!("------------------------------------------");

    let queue = TestQueue::new();

    // Enqueue jobs with different priorities
    let low_job = Job::new("priority_queue".to_string(), json!({"task": "cleanup"}))
        .with_priority(JobPriority::Low);
    let normal_job = Job::new("priority_queue".to_string(), json!({"task": "processing"}));
    let high_job = Job::new("priority_queue".to_string(), json!({"task": "urgent_fix"}))
        .with_priority(JobPriority::High);
    let critical_job = Job::new(
        "priority_queue".to_string(),
        json!({"task": "security_patch"}),
    )
    .with_priority(JobPriority::Critical);

    let low_id = queue.enqueue(low_job).await?;
    let normal_id = queue.enqueue(normal_job).await?;
    let high_id = queue.enqueue(high_job).await?;
    let critical_id = queue.enqueue(critical_job).await?;

    println!("📋 Enqueued jobs with priorities:");
    println!("   🔴 Critical: {}", critical_id);
    println!("   🟠 High: {}", high_id);
    println!("   🟡 Normal: {}", normal_id);
    println!("   🟢 Low: {}", low_id);

    // Dequeue in priority order
    let order = [critical_id, high_id, normal_id, low_id];
    for (i, expected_id) in order.iter().enumerate() {
        if let Some(job) = queue.dequeue("priority_queue").await? {
            assert_eq!(job.id, *expected_id);
            queue.complete_job(job.id).await?;
            println!("   {}. Processed: {} ({:?})", i + 1, job.id, job.priority);
        }
    }

    // Test weighted selection
    println!("\n🎲 Testing weighted priority selection:");

    // Enqueue multiple jobs of each priority
    for i in 0..3 {
        for priority in [JobPriority::High, JobPriority::Normal, JobPriority::Low] {
            let mut job = Job::new("weighted_queue".to_string(), json!({"index": i}));
            job.priority = priority;
            queue.enqueue(job).await?;
        }
    }

    // Create weights that heavily favor normal priority
    let weights = PriorityWeights::new()
        .with_weight(JobPriority::High, 20)
        .with_weight(JobPriority::Normal, 60) // Heavily weighted
        .with_weight(JobPriority::Low, 20);

    // Process with weighted selection
    while let Some(job) = queue
        .dequeue_with_priority_weights("weighted_queue", &weights)
        .await?
    {
        println!("   Selected: {:?} priority job", job.priority);
        queue.complete_job(job.id).await?;
    }

    Ok(())
}

async fn batch_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n📦 Batch Processing Example");
    println!("--------------------------");

    let queue = TestQueue::new();

    // Create a batch of image processing jobs
    let jobs: Vec<Job> = (1..=5)
        .map(|i| {
            Job::new(
                "image_queue".to_string(),
                json!({
                    "image_url": format!("https://example.com/image_{}.jpg", i),
                    "operation": if i % 2 == 0 { "resize" } else { "compress" }
                }),
            )
        })
        .collect();

    let batch = JobBatch::new("image_batch".to_string()).with_jobs(jobs);
    let batch_id = queue.enqueue_batch(batch).await?;

    println!("📦 Created batch: {}", batch_id);

    // Check initial batch status
    let status = queue.get_batch_status(batch_id).await?;
    println!(
        "📊 Initial status: {} total, {} pending",
        status.total_jobs, status.pending_jobs
    );

    // Process batch jobs
    let mut processed = 0;
    while let Some(job) = queue.dequeue("image_queue").await? {
        println!("🖼️  Processing image job: {}", job.id);

        match image_handler(job.clone()).await {
            Ok(_) => {
                queue.complete_job(job.id).await?;
                processed += 1;
                println!("   ✅ Completed ({}/{})", processed, status.total_jobs);
            }
            Err(e) => {
                queue.fail_job(job.id, &e.to_string()).await?;
                println!("   ❌ Failed: {}", e);
            }
        }

        // Check batch progress
        let current_status = queue.get_batch_status(batch_id).await?;
        if current_status.status == hammerwork::batch::BatchStatus::Completed {
            println!("🎉 Batch completed!");
            break;
        }
    }

    // Final batch status
    let final_status = queue.get_batch_status(batch_id).await?;
    println!("📈 Final batch status:");
    println!("   Total: {}", final_status.total_jobs);
    println!("   Completed: {}", final_status.completed_jobs);
    println!("   Failed: {}", final_status.failed_jobs);
    println!("   Status: {:?}", final_status.status);

    Ok(())
}

async fn workflow_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🔄 Workflow with Dependencies Example");
    println!("------------------------------------");

    let queue = TestQueue::new();

    // Create a data processing workflow:
    // 1. Extract data (no dependencies)
    // 2. Clean data (depends on extract)
    // 3. Analyze data (depends on clean)
    // 4. Generate report (depends on analyze)

    let extract_job = Job::new(
        "data_pipeline".to_string(),
        json!({
            "step": "extract",
            "dataset": "user_events.csv"
        }),
    );

    let clean_job = Job::new(
        "data_pipeline".to_string(),
        json!({
            "step": "clean",
            "operations": ["remove_duplicates", "fill_nulls"]
        }),
    )
    .depends_on(&extract_job.id);

    let analyze_job = Job::new(
        "data_pipeline".to_string(),
        json!({
            "step": "analyze",
            "analysis_type": "ml"
        }),
    )
    .depends_on(&clean_job.id);

    let report_job = Job::new(
        "data_pipeline".to_string(),
        json!({
            "step": "report",
            "format": "pdf"
        }),
    )
    .depends_on(&analyze_job.id);

    let workflow = JobGroup::new("data_processing_workflow".to_string())
        .add_job(extract_job.clone())
        .add_job(clean_job.clone())
        .add_job(analyze_job.clone())
        .add_job(report_job.clone())
        .with_failure_policy(FailurePolicy::FailFast);

    let workflow_id = queue.enqueue_workflow(workflow).await?;
    println!("🔄 Created workflow: {}", workflow_id);

    // Process jobs in dependency order
    let job_steps = ["extract", "clean", "analyze", "report"];

    for (i, step) in job_steps.iter().enumerate() {
        println!("\n📝 Step {}: {}", i + 1, step);

        if let Some(job) = queue.dequeue("data_pipeline").await? {
            let job_step = job.payload["step"].as_str().unwrap();
            assert_eq!(job_step, *step);

            println!("   Processing: {}", job_step);

            // Simulate processing
            match *step {
                "extract" => {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    println!("   📥 Data extracted successfully");
                }
                "clean" => {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    println!("   🧹 Data cleaned successfully");
                }
                "analyze" => {
                    if let Err(e) = analysis_handler(job.clone()).await {
                        eprintln!("Analysis error: {}", e);
                    }
                }
                "report" => {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    println!("   📄 Report generated successfully");
                }
                _ => {}
            }

            queue.complete_job(job.id).await?;
            println!("   ✅ Step {} completed", step);
        } else {
            panic!("Expected job for step {} but none available", step);
        }
    }

    // Check workflow completion
    let workflow_status = queue.get_workflow_status(workflow_id).await?.unwrap();
    println!("\n🎉 Workflow completed!");
    println!("   Status: {:?}", workflow_status.status);
    println!("   Completed jobs: {}", workflow_status.completed_jobs);
    println!("   Total jobs: {}", workflow_status.total_jobs);

    Ok(())
}

async fn error_handling_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n⚠️  Error Handling and Retry Logic Example");
    println!("------------------------------------------");

    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create a job that will fail
    let failing_job = Job::new(
        "email_queue".to_string(),
        json!({
            "to": "invalid@invalid.invalid",
            "subject": "Test",
            "body": "This will fail"
        }),
    )
    .with_max_attempts(3);

    let job_id = queue.enqueue(failing_job).await?;
    println!("📧 Enqueued failing job: {}", job_id);

    // Attempt 1
    if let Some(job) = queue.dequeue("email_queue").await? {
        println!("\n🔄 Attempt 1:");
        match email_handler(job.clone()).await {
            Err(e) => {
                queue.fail_job(job.id, &e.to_string()).await?;
                println!("   ❌ Failed: {}", e);

                let job_state = queue.get_job(job.id).await?.unwrap();
                println!(
                    "   📊 Status: {:?}, Attempts: {}",
                    job_state.status, job_state.attempts
                );
            }
            _ => panic!("Job should have failed"),
        }
    }

    // Schedule retry
    let retry_at = clock.now() + Duration::minutes(5);
    queue.retry_job(job_id, retry_at).await?;
    println!("⏳ Scheduled retry in 5 minutes");

    // Advance time and retry
    clock.advance(Duration::minutes(5));

    // Attempt 2
    if let Some(job) = queue.dequeue("email_queue").await? {
        println!("\n🔄 Attempt 2:");
        match email_handler(job.clone()).await {
            Err(e) => {
                queue.fail_job(job.id, &e.to_string()).await?;
                println!("   ❌ Failed again: {}", e);

                let job_state = queue.get_job(job.id).await?.unwrap();
                println!(
                    "   📊 Status: {:?}, Attempts: {}",
                    job_state.status, job_state.attempts
                );
            }
            _ => panic!("Job should have failed"),
        }
    }

    // Attempt 3 (final)
    queue.retry_job(job_id, clock.now()).await?;

    if let Some(job) = queue.dequeue("email_queue").await? {
        println!("\n🔄 Attempt 3 (final):");
        match email_handler(job.clone()).await {
            Err(e) => {
                queue.fail_job(job.id, &e.to_string()).await?;
                println!("   ❌ Failed permanently: {}", e);

                let job_state = queue.get_job(job.id).await?.unwrap();
                println!(
                    "   📊 Status: {:?}, Attempts: {}",
                    job_state.status, job_state.attempts
                );
                println!("   💀 Job marked as dead");
            }
            _ => panic!("Job should have failed"),
        }
    }

    // Check dead job summary
    let dead_summary = queue.get_dead_job_summary().await?;
    println!("\n💀 Dead jobs summary:");
    println!("   Total dead jobs: {}", dead_summary.total_dead_jobs);
    println!("   By queue: {:?}", dead_summary.dead_jobs_by_queue);

    Ok(())
}

async fn cron_job_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n⏰ Cron Job Testing Example");
    println!("--------------------------");

    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create a cron job that runs every hour
    let cron_job = Job::new(
        "maintenance_queue".to_string(),
        json!({
            "task": "cleanup_temp_files",
            "max_age_hours": 24
        }),
    )
    .with_cron("0 0 * * * *".parse()?)? // Every hour at minute 0
    .with_timezone("UTC".to_string());

    let job_id = queue.enqueue_cron_job(cron_job).await?;
    println!("⏰ Created cron job: {}", job_id);

    let job = queue.get_job(job_id).await?.unwrap();
    println!("📅 Next run: {:?}", job.next_run_at);

    // Simulate multiple cron executions
    for run in 1..=3 {
        println!("\n🔄 Cron run #{}", run);

        // Advance to next hour
        clock.advance(Duration::hours(1));

        // Process the job
        if let Some(job) = queue.dequeue("maintenance_queue").await? {
            println!("   🧹 Running maintenance task");
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            queue.complete_job(job.id).await?;
            println!("   ✅ Maintenance completed");
        }

        // Check for due cron jobs and reschedule
        let due_jobs = queue.get_due_cron_jobs(Some("maintenance_queue")).await?;
        if !due_jobs.is_empty() {
            let next_run = clock.now() + Duration::hours(1);
            queue.reschedule_cron_job(job_id, next_run).await?;
            println!("   📅 Rescheduled for next hour");
        }
    }

    // Show recurring jobs
    let recurring = queue.get_recurring_jobs("maintenance_queue").await?;
    println!("\n📋 Recurring jobs: {}", recurring.len());

    Ok(())
}

async fn result_storage_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n💾 Job Result Storage Example");
    println!("----------------------------");

    let clock = MockClock::new();
    let queue = TestQueue::with_clock(clock.clone());

    // Create and process a computation job
    let compute_job = Job::new(
        "compute_queue".to_string(),
        json!({
            "operation": "fibonacci",
            "n": 20
        }),
    );

    let job_id = queue.enqueue(compute_job).await?;
    println!("🧮 Enqueued computation job: {}", job_id);

    // Process the job
    if let Some(job) = queue.dequeue("compute_queue").await? {
        println!("🔄 Computing fibonacci(20)...");

        // Simulate computation
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let result = 6765; // fibonacci(20)

        queue.complete_job(job.id).await?;
        println!("✅ Computation completed");

        // Store the result with expiration
        let result_data = json!({
            "result": result,
            "computation_time_ms": 200,
            "algorithm": "iterative"
        });

        let expires_at = clock.now() + Duration::hours(24);
        queue
            .store_job_result(job.id, result_data.clone(), Some(expires_at))
            .await?;
        println!("💾 Result stored (expires in 24 hours)");

        // Retrieve the result
        if let Some(stored_result) = queue.get_job_result(job.id).await? {
            println!("📊 Retrieved result: {}", stored_result);
        }

        // Test expiration
        clock.advance(Duration::hours(25));
        println!("⏰ Advanced time by 25 hours");

        if queue.get_job_result(job.id).await?.is_none() {
            println!("🗑️  Result expired and no longer available");
        }

        // Clean up expired results
        let cleaned = queue.cleanup_expired_results().await?;
        println!("🧹 Cleaned up {} expired results", cleaned);
    }

    Ok(())
}

async fn worker_integration_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n👷 Worker Integration Example");
    println!("----------------------------");

    let queue = Arc::new(TestQueue::new());

    // Enqueue some jobs
    for i in 1..=5 {
        let job = Job::new(
            "worker_queue".to_string(),
            json!({
                "task": "process_data",
                "batch_id": i,
                "data": format!("dataset_{}.csv", i)
            }),
        );

        queue.enqueue(job).await?;
    }

    println!("📋 Enqueued 5 jobs for processing");

    // NOTE: TestQueue is designed for testing job logic and queue operations,
    // not for testing Worker functionality. Workers are tightly coupled to JobQueue<DB>
    // and cannot be directly used with TestQueue. For testing job processing logic,
    // manually dequeue and process jobs as shown below.

    println!("🔄 Processing jobs manually (TestQueue doesn't support Worker):");

    let mut processed_count = 0;
    while let Some(job) = queue.dequeue("worker_queue").await? {
        let batch_id = job.payload["batch_id"].as_u64().unwrap();
        let data = job.payload["data"].as_str().unwrap();

        info!("Processing batch {} with data: {}", batch_id, data);

        // Simulate processing (this is where your job handler logic would go)
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        queue.complete_job(job.id).await?;
        processed_count += 1;

        info!("Batch {} processing completed", batch_id);

        // Process only a few jobs for demonstration
        if processed_count >= 3 {
            break;
        }
    }

    println!("✅ Processed {} jobs manually", processed_count);

    // Check results
    let completed = queue
        .get_job_count("worker_queue", &JobStatus::Completed)
        .await;
    println!("✅ Worker processed {} jobs", completed);

    Ok(())
}

async fn performance_testing_example() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n🚀 Performance Testing Example");
    println!("------------------------------");

    let queue = TestQueue::new();
    let start_time = std::time::Instant::now();

    // Enqueue a large number of jobs
    const JOB_COUNT: usize = 1000;
    println!("📦 Enqueuing {} jobs...", JOB_COUNT);

    for i in 0..JOB_COUNT {
        let job = Job::new(
            "perf_queue".to_string(),
            json!({
                "index": i,
                "data": format!("item_{}", i)
            }),
        );
        queue.enqueue(job).await?;
    }

    let enqueue_time = start_time.elapsed();
    println!("⏱️  Enqueued {} jobs in {:?}", JOB_COUNT, enqueue_time);
    println!(
        "📊 Enqueue rate: {:.2} jobs/sec",
        JOB_COUNT as f64 / enqueue_time.as_secs_f64()
    );

    // Process all jobs
    let process_start = std::time::Instant::now();
    let mut processed = 0;

    while let Some(job) = queue.dequeue("perf_queue").await? {
        // Minimal processing
        queue.complete_job(job.id).await?;
        processed += 1;

        if processed % 100 == 0 {
            println!("   Processed {}/{} jobs", processed, JOB_COUNT);
        }
    }

    let process_time = process_start.elapsed();
    println!("⏱️  Processed {} jobs in {:?}", processed, process_time);
    println!(
        "📊 Process rate: {:.2} jobs/sec",
        processed as f64 / process_time.as_secs_f64()
    );

    // Get final statistics
    let stats = queue.get_queue_stats("perf_queue").await?;
    println!("\n📈 Final statistics:");
    println!("   Pending: {}", stats.pending_count);
    println!("   Running: {}", stats.running_count);
    println!("   Completed: {}", stats.statistics.completed);
    println!(
        "   Average processing time: {:?}ms",
        stats.statistics.avg_processing_time_ms
    );

    Ok(())
}
