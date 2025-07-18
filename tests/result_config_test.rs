//! Simple test to verify result_config persistence

mod test_utils;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires PostgreSQL database"]
async fn test_postgres_result_config_persistence() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create a job with result storage configuration
    let config = ResultConfig {
        storage: ResultStorage::Database,
        ttl: Some(Duration::from_secs(3600)),
        max_size_bytes: Some(1024),
    };
    let job = Job::new("test_result_config".to_string(), json!({"task": "test"}))
        .with_result_config(config);

    // Enqueue the job
    let job_id = queue.enqueue(job).await.unwrap();

    // Dequeue the job to verify result_config is preserved
    let dequeued_job = queue.dequeue("test_result_config").await.unwrap().unwrap();

    assert_eq!(dequeued_job.id, job_id);
    assert_eq!(dequeued_job.result_config.storage, ResultStorage::Database);
    assert_eq!(
        dequeued_job.result_config.ttl,
        Some(Duration::from_secs(3600))
    );
    assert_eq!(dequeued_job.result_config.max_size_bytes, Some(1024));

    println!("✅ PostgreSQL result_config persistence test passed!");
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires MySQL database"]
async fn test_mysql_result_config_persistence() {
    let queue = test_utils::setup_mysql_queue().await;

    // Create a job with result storage configuration
    let config = ResultConfig {
        storage: ResultStorage::Database,
        ttl: Some(Duration::from_secs(7200)),
        max_size_bytes: Some(2048),
    };
    let job = Job::new(
        "test_result_config_mysql".to_string(),
        json!({"task": "test"}),
    )
    .with_result_config(config);

    // Enqueue the job
    let job_id = queue.enqueue(job).await.unwrap();

    // Dequeue the job to verify result_config is preserved
    let dequeued_job = queue
        .dequeue("test_result_config_mysql")
        .await
        .unwrap()
        .unwrap();

    assert_eq!(dequeued_job.id, job_id);
    assert_eq!(dequeued_job.result_config.storage, ResultStorage::Database);
    assert_eq!(
        dequeued_job.result_config.ttl,
        Some(Duration::from_secs(7200))
    );
    assert_eq!(dequeued_job.result_config.max_size_bytes, Some(2048));

    println!("✅ MySQL result_config persistence test passed!");
}
