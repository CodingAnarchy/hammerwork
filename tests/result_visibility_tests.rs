//! A job's stored result must be readable as soon as the job is visible as Completed.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    Job, JobQueue, JobStatus, ResultStorage, Worker, WorkerPool,
    queue::DatabaseQueue,
    worker::{JobHandlerWithResult, JobResult},
};
use serde_json::json;
use std::{sync::Arc, time::Duration};

async fn result_is_stored_before_completion_is_visible<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("result_visibility");
    let handler: JobHandlerWithResult = Arc::new(|job| {
        Box::pin(async move { Ok(JobResult::with_data(json!({ "echo": job.payload["n"] }))) })
    });

    let mut job_ids = Vec::new();
    for n in 0..20 {
        let job = Job::new(queue_name.clone(), json!({ "n": n }))
            .with_result_storage(ResultStorage::Database)
            .with_result_ttl(Duration::from_secs(3600));
        job_ids.push(queue.enqueue(job).await.unwrap());
    }

    let worker = Worker::new_with_result_handler(queue.clone(), queue_name, handler)
        .with_poll_interval(Duration::from_millis(10));
    let mut pool = WorkerPool::new();
    pool.add_worker(worker);
    let pool_task = tokio::spawn(async move { pool.start().await });

    // Poll as fast as possible: whenever a job reads as Completed, its result must
    // already be stored.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut pending = job_ids.clone();
    while !pending.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "jobs did not complete"
        );
        let mut still_pending = Vec::new();
        for id in pending {
            let job = queue.get_job(id).await.unwrap().unwrap();
            if job.status == JobStatus::Completed {
                let result = queue.get_job_result(id).await.unwrap();
                assert!(
                    result.is_some(),
                    "job {id} was visible as Completed before its result was stored"
                );
            } else {
                still_pending.push(id);
            }
        }
        pending = still_pending;
    }

    pool_task.abort();
    for id in job_ids {
        queue.delete_job(id).await.unwrap();
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_result_is_stored_before_completion_is_visible() {
        result_is_stored_before_completion_is_visible(test_utils::setup_postgres_queue().await)
            .await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_result_is_stored_before_completion_is_visible() {
        result_is_stored_before_completion_is_visible(test_utils::setup_mysql_queue().await).await;
    }
}
