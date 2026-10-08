//! Database-backed tests for CLI commands that mutate job state. They run the real
//! command code (`JobCommand::execute`) and check the effect through the library, so
//! they fail if a command writes statuses the library does not recognise.
//!
//! Run with `DATABASE_URL` (Postgres) / `MYSQL_DATABASE_URL` (MySQL) pointing at a
//! migrated database: `cargo test -p cargo-hammerwork --test job_state_tests -- --ignored`.

use cargo_hammerwork::commands::batch::BatchCommand;
use cargo_hammerwork::commands::job::JobCommand;
use cargo_hammerwork::config::Config;
use cargo_hammerwork::utils::database::{DatabasePool, JobQueueWrapper};
use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobId, JobStatus};
use serde_json::json;

fn unique_queue(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

fn retry_all_in(url: &str, queue: &str) -> JobCommand {
    JobCommand::Retry {
        database_url: Some(url.to_string()),
        job_id: None,
        queue: Some(queue.to_string()),
        all: false,
    }
}

fn retry_one_cmd(url: &str, id: JobId) -> JobCommand {
    JobCommand::Retry {
        database_url: Some(url.to_string()),
        job_id: Some(id.to_string()),
        queue: None,
        all: false,
    }
}

fn cancel_queue(url: &str, queue: &str) -> JobCommand {
    JobCommand::Cancel {
        database_url: Some(url.to_string()),
        job_id: None,
        queue: Some(queue.to_string()),
        all_pending: false,
    }
}

async fn status_of<Q: DatabaseQueue>(queue: &Q, id: JobId) -> Option<JobStatus> {
    queue.get_job(id).await.unwrap().map(|j| j.status)
}

/// The scenario shared by both databases, parameterised over the queue type.
macro_rules! job_state_scenarios {
    ($url:expr, $queue:expr) => {{
        let url: &str = $url;
        let queue = $queue;
        let config = Config::default();

        // --- retry: Failed and Dead jobs return to (capitalized) Pending ---
        let qname = unique_queue("cli_retry");
        let other = unique_queue("cli_retry_other");

        let failed = queue
            .enqueue(Job::new(qname.clone(), json!({"k": "failed"})))
            .await
            .unwrap();
        queue.fail_job(failed, "boom").await.unwrap();

        let dead = queue
            .enqueue(Job::new(qname.clone(), json!({"k": "dead"})))
            .await
            .unwrap();
        queue.mark_job_dead(dead, "gave up").await.unwrap();

        let untouched = queue
            .enqueue(Job::new(other.clone(), json!({"k": "other"})))
            .await
            .unwrap();
        queue.fail_job(untouched, "boom").await.unwrap();

        retry_all_in(url, &qname).execute(&config).await.unwrap();

        assert_eq!(status_of(&queue, failed).await, Some(JobStatus::Pending));
        assert_eq!(status_of(&queue, dead).await, Some(JobStatus::Pending));
        assert_eq!(
            status_of(&queue, untouched).await,
            Some(JobStatus::Failed),
            "retry --queue must not touch other queues"
        );
        // The retried job is actually runnable by the library's dequeue.
        let dequeued = queue.dequeue(&qname).await.unwrap();
        assert!(dequeued.is_some(), "retried job should be dequeueable");

        // --- retry of a single job in a state that cannot be retried is rejected ---
        let done = queue
            .enqueue(Job::new(other.clone(), json!({"k": "done"})))
            .await
            .unwrap();
        queue.complete_job(done).await.unwrap();
        assert!(retry_one_cmd(url, done).execute(&config).await.is_err());
        assert_eq!(status_of(&queue, done).await, Some(JobStatus::Completed));

        // --- retry a single failed job by id ---
        retry_one_cmd(url, untouched).execute(&config).await.unwrap();
        assert_eq!(status_of(&queue, untouched).await, Some(JobStatus::Pending));

        // --- cancel only deletes Pending jobs ---
        let cancel_q = unique_queue("cli_cancel");
        let pending = queue
            .enqueue(Job::new(cancel_q.clone(), json!({"k": "pending"})))
            .await
            .unwrap();
        let failed_keep = queue
            .enqueue(Job::new(cancel_q.clone(), json!({"k": "failed"})))
            .await
            .unwrap();
        queue.fail_job(failed_keep, "boom").await.unwrap();

        cancel_queue(url, &cancel_q).execute(&config).await.unwrap();
        assert_eq!(status_of(&queue, pending).await, None);
        assert_eq!(status_of(&queue, failed_keep).await, Some(JobStatus::Failed));

        // --- batch retry (dead only) goes through the same library path ---
        let batch_q = unique_queue("cli_batch");
        let batch_dead = queue
            .enqueue(Job::new(batch_q.clone(), json!({"k": "dead"})))
            .await
            .unwrap();
        queue.mark_job_dead(batch_dead, "gave up").await.unwrap();
        let batch_failed = queue
            .enqueue(Job::new(batch_q.clone(), json!({"k": "failed"})))
            .await
            .unwrap();
        queue.fail_job(batch_failed, "boom").await.unwrap();

        BatchCommand::Retry {
            database_url: Some(url.to_string()),
            queue: Some(batch_q.clone()),
            status: Some("dead".to_string()),
            failed_since_hours: None,
            max_attempts_reached: false,
            confirm: true,
            dry_run: false,
        }
        .execute(&config)
        .await
        .unwrap();
        assert_eq!(status_of(&queue, batch_dead).await, Some(JobStatus::Pending));
        assert_eq!(status_of(&queue, batch_failed).await, Some(JobStatus::Failed));

        // --- dry run changes nothing ---
        BatchCommand::Retry {
            database_url: Some(url.to_string()),
            queue: Some(batch_q.clone()),
            status: Some("failed".to_string()),
            failed_since_hours: None,
            max_attempts_reached: false,
            confirm: false,
            dry_run: true,
        }
        .execute(&config)
        .await
        .unwrap();
        assert_eq!(status_of(&queue, batch_failed).await, Some(JobStatus::Failed));
    }};
}

#[tokio::test]
#[ignore] // Requires a Postgres database
async fn test_cli_job_state_commands_postgres() {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string());
    let pool = DatabasePool::connect(&url, 5).await.unwrap();
    let JobQueueWrapper::Postgres(queue) = pool.create_job_queue() else {
        panic!("expected a Postgres queue");
    };
    job_state_scenarios!(&url, queue);
}

#[tokio::test]
#[ignore] // Requires a MySQL database
async fn test_cli_job_state_commands_mysql() {
    let url = std::env::var("MYSQL_DATABASE_URL")
        .unwrap_or_else(|_| "mysql://root:hammerwork@localhost:3307/hammerwork".to_string());
    let pool = DatabasePool::connect(&url, 5).await.unwrap();
    let JobQueueWrapper::MySQL(queue) = pool.create_job_queue() else {
        panic!("expected a MySQL queue");
    };
    job_state_scenarios!(&url, queue);
}
