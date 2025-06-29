//! Debug test for result storage

mod test_utils;

use hammerwork::{Job, job::ResultStorage, queue::DatabaseQueue};
use serde_json::json;

#[tokio::test]
#[cfg(feature = "mysql")]
async fn test_mysql_direct_result_storage() {
    let queue = test_utils::setup_mysql_queue().await;

    // Create a simple job with result storage
    let job = Job::new("test_queue".to_string(), json!({"test": "data"}))
        .with_result_storage(ResultStorage::Database);

    let job_id = queue.enqueue(job).await.unwrap();

    // Manually store a result
    let result_data = json!({"success": true, "message": "Test result"});
    queue
        .store_job_result(job_id, result_data.clone(), None)
        .await
        .unwrap();

    // Retrieve the result
    let stored_result = queue.get_job_result(job_id).await.unwrap();
    assert!(stored_result.is_some());
    assert_eq!(stored_result.unwrap(), result_data);

    println!("Direct result storage works!");
}

#[tokio::test]
#[cfg(feature = "postgres")]
async fn test_postgres_direct_result_storage() {
    let queue = test_utils::setup_postgres_queue().await;

    // Create a simple job with result storage
    let job = Job::new("test_queue".to_string(), json!({"test": "data"}))
        .with_result_storage(ResultStorage::Database);

    let job_id = queue.enqueue(job).await.unwrap();

    // Manually store a result
    let result_data = json!({"success": true, "message": "Test result"});
    queue
        .store_job_result(job_id, result_data.clone(), None)
        .await
        .unwrap();

    // Retrieve the result
    let stored_result = queue.get_job_result(job_id).await.unwrap();
    assert!(stored_result.is_some());
    assert_eq!(stored_result.unwrap(), result_data);

    println!("Direct result storage works!");
}

#[tokio::test]
#[cfg(feature = "mysql")]
async fn test_mysql_job_completion() {
    let queue = test_utils::setup_mysql_queue().await;

    // Create and enqueue job
    let job = Job::new("completion_test".to_string(), json!({"test": "completion"}))
        .with_result_storage(ResultStorage::Database);

    let job_id = queue.enqueue(job).await.unwrap();

    // Dequeue the job
    let mut job = queue.dequeue("completion_test").await.unwrap().unwrap();
    println!("Job result config: {:?}", job.result_config);

    // Mark job as completed with result
    let result_data = json!({"completed": true});
    queue
        .store_job_result(job_id, result_data.clone(), None)
        .await
        .unwrap();
    queue.complete_job(job_id).await.unwrap();

    // Check result
    let result = queue.get_job_result(job_id).await.unwrap();
    assert!(result.is_some());
}
