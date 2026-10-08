//! Concurrency and correctness tests for the dequeue paths.
//!
//! Each scenario is written once against the `DatabaseQueue` trait and run against
//! both backends.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    Job, JobQueue, JobStatus, PriorityWeights, ResultStorage, priority::JobPriority,
    queue::DatabaseQueue,
};
use serde_json::json;
use std::{collections::HashSet, sync::Arc};

const PRIORITIES: [JobPriority; 5] = [
    JobPriority::Background,
    JobPriority::Low,
    JobPriority::Normal,
    JobPriority::High,
    JobPriority::Critical,
];

/// Many workers draining a queue with weighted dequeue must claim every job exactly once.
async fn weighted_dequeue_never_double_claims<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("weighted_concurrency");
    let mut enqueued = HashSet::new();
    for i in 0..120 {
        let job = Job::new(queue_name.clone(), json!({ "index": i }))
            .with_priority(PRIORITIES[i % PRIORITIES.len()]);
        enqueued.insert(queue.enqueue(job).await.unwrap());
    }

    let mut workers = Vec::new();
    for _ in 0..16 {
        let queue = Arc::clone(&queue);
        let queue_name = queue_name.clone();
        workers.push(tokio::spawn(async move {
            let weights = PriorityWeights::new();
            let mut claimed = Vec::new();
            while let Some(job) = queue
                .dequeue_with_priority_weights(&queue_name, &weights)
                .await
                .unwrap()
            {
                assert_eq!(job.status, JobStatus::Running);
                claimed.push(job.id);
            }
            claimed
        }));
    }

    let mut claimed = HashSet::new();
    for worker in workers {
        for id in worker.await.unwrap() {
            assert!(claimed.insert(id), "job {id} was claimed by two workers");
        }
    }
    assert_eq!(claimed, enqueued, "every job must be claimed exactly once");

    for id in claimed {
        let job = queue.get_job(id).await.unwrap().unwrap();
        assert_eq!(job.attempts, 1, "job {id} was started more than once");
        queue.delete_job(id).await.unwrap();
    }
}

/// The job returned by weighted dequeue must carry the fields stored on the row.
async fn weighted_dequeue_preserves_job_fields<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("weighted_fields");
    let job = Job::new(queue_name.clone(), json!({ "keep": "fields" }))
        .with_result_storage(ResultStorage::Database)
        .with_trace_id("trace-weighted-dequeue");
    let job_id = queue.enqueue(job).await.unwrap();

    let dequeued = queue
        .dequeue_with_priority_weights(&queue_name, &PriorityWeights::new())
        .await
        .unwrap()
        .expect("job should be dequeued");

    assert_eq!(dequeued.id, job_id);
    assert_eq!(dequeued.result_config.storage, ResultStorage::Database);
    assert_eq!(dequeued.trace_id.as_deref(), Some("trace-weighted-dequeue"));

    queue.delete_job(job_id).await.unwrap();
}

/// Jobs still waiting on dependencies must not be handed out by any dequeue path.
async fn dequeue_skips_jobs_waiting_on_dependencies<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let parent_queue = test_utils::unique_queue("dependency_parent");
    let child_queue = test_utils::unique_queue("dependency_child");

    let parent = Job::new(parent_queue.clone(), json!({ "role": "parent" }));
    let parent_id = queue.enqueue(parent).await.unwrap();
    let child = Job::new(child_queue.clone(), json!({ "role": "child" })).depends_on(&parent_id);
    let child_id = queue.enqueue(child).await.unwrap();

    assert!(
        queue.dequeue(&child_queue).await.unwrap().is_none(),
        "dequeue returned a job whose dependency has not completed"
    );
    assert!(
        queue
            .dequeue_with_priority_weights(&child_queue, &PriorityWeights::new())
            .await
            .unwrap()
            .is_none(),
        "weighted dequeue returned a job whose dependency has not completed"
    );

    queue.delete_job(child_id).await.unwrap();
    queue.delete_job(parent_id).await.unwrap();
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_weighted_dequeue_never_double_claims() {
        weighted_dequeue_never_double_claims(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_weighted_dequeue_preserves_job_fields() {
        weighted_dequeue_preserves_job_fields(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_dequeue_skips_jobs_waiting_on_dependencies() {
        dequeue_skips_jobs_waiting_on_dependencies(test_utils::setup_postgres_queue().await).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_weighted_dequeue_never_double_claims() {
        weighted_dequeue_never_double_claims(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_weighted_dequeue_preserves_job_fields() {
        weighted_dequeue_preserves_job_fields(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_dequeue_skips_jobs_waiting_on_dependencies() {
        dequeue_skips_jobs_waiting_on_dependencies(test_utils::setup_mysql_queue().await).await;
    }

    /// A pooled connection returned while still inside a transaction (as happens when a
    /// future is dropped mid-transaction) must not break later claims on it.
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_dequeue_recovers_connection_left_in_transaction() {
        use sqlx::mysql::MySqlPoolOptions;

        // A single connection, so the dequeue below gets the abandoned one.
        let pool = MySqlPoolOptions::new()
            .max_connections(1)
            .connect(&test_utils::mysql_url())
            .await
            .unwrap();
        let queue = Arc::new(JobQueue::new(pool.clone()));
        let queue_name = test_utils::unique_queue("abandoned_transaction");
        let job_id = queue
            .enqueue(Job::new(queue_name.clone(), json!({ "n": 1 })))
            .await
            .unwrap();

        {
            // Open a transaction behind sqlx's back and return the connection to the pool.
            let mut conn = pool.acquire().await.unwrap();
            // Text protocol: BEGIN can't be sent as a prepared statement.
            sqlx::Executor::execute(&mut *conn, "BEGIN").await.unwrap();
            sqlx::query("SELECT id FROM hammerwork_jobs WHERE id = ? FOR UPDATE")
                .bind(job_id.to_string())
                .execute(&mut *conn)
                .await
                .unwrap();
        }

        let job = queue
            .dequeue(&queue_name)
            .await
            .expect("dequeue must recover the abandoned transaction")
            .expect("the job must be claimable once the abandoned locks are released");
        assert_eq!(job.id, job_id);
        assert_eq!(job.status, JobStatus::Running);

        queue.delete_job(job_id).await.unwrap();
    }
}
