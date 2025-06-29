//! Tests to ensure all JobRow queries include the complete set of required fields.
//!
//! This test suite verifies that all database queries that use JobRow include
//! all the fields that the JobRow struct expects, preventing runtime errors
//! due to missing columns.

use hammerwork::queue::DatabaseQueue;
use hammerwork::{Job, JobPriority, ResultStorage};
use serde_json::json;
use tokio::time::Duration;

mod test_utils;

/// Test that all JobRow queries include the complete field set
#[cfg(test)]
mod jobrow_field_tests {
    use super::*;

    #[cfg(feature = "postgres")]
    mod postgres_tests {
        use super::*;

        #[tokio::test]
        async fn test_get_job_includes_all_fields() {
            let queue = test_utils::setup_postgres_queue().await;
            let unique_queue = format!("pg_get_test_{}", uuid::Uuid::new_v4());

            // Create a job with all possible fields populated
            let job = Job::new(unique_queue.clone(), json!({"test": "data"}))
                .with_priority(JobPriority::High)
                .with_timeout(Duration::from_secs(30))
                .with_max_attempts(5)
                .with_result_storage(ResultStorage::Database);

            let job_id = queue.enqueue(job).await.unwrap();

            // This should not panic with missing field errors
            let retrieved = queue.get_job(job_id).await.unwrap();
            assert!(retrieved.is_some());

            let job = retrieved.unwrap();
            // Verify all fields are accessible (this would fail if any were missing)
            assert_eq!(job.queue_name, unique_queue);
            assert_eq!(job.priority, JobPriority::High);
            assert_eq!(job.max_attempts, 5);
            // Workflow fields should have default values
            assert!(job.depends_on.is_empty());
            assert!(job.dependents.is_empty());
            assert_eq!(
                job.dependency_status,
                hammerwork::workflow::DependencyStatus::None
            );
            assert!(job.workflow_id.is_none());
            assert!(job.workflow_name.is_none());
        }

        #[tokio::test]
        async fn test_dequeue_includes_all_fields() {
            let queue = test_utils::setup_postgres_queue().await;
            let unique_queue = format!("pg_dequeue_test_{}", uuid::Uuid::new_v4());

            let job = Job::new(unique_queue.clone(), json!({"test": "dequeue"}))
                .with_priority(JobPriority::Critical);

            queue.enqueue(job).await.unwrap();

            // This should not panic with missing field errors
            let dequeued = queue.dequeue(&unique_queue).await.unwrap();
            assert!(dequeued.is_some());

            let job = dequeued.unwrap();
            assert_eq!(job.queue_name, unique_queue);
            assert_eq!(job.priority, JobPriority::Critical);
            // Verify dependency fields are accessible
            assert!(job.depends_on.is_empty());
            assert!(job.dependents.is_empty());
        }

        #[tokio::test]
        async fn test_dequeue_with_priority_weights_includes_all_fields() {
            let queue = test_utils::setup_postgres_queue().await;
            let unique_queue = format!("pg_priority_test_{}", uuid::Uuid::new_v4());

            let weights = hammerwork::priority::PriorityWeights::default();

            let job = Job::new(unique_queue.clone(), json!({"test": "priority"}))
                .with_priority(JobPriority::Low);

            queue.enqueue(job).await.unwrap();

            // This should not panic with missing field errors
            let dequeued = queue
                .dequeue_with_priority_weights(&unique_queue, &weights)
                .await
                .unwrap();
            assert!(dequeued.is_some());

            let job = dequeued.unwrap();
            assert_eq!(job.queue_name, unique_queue);
            // Verify all fields are accessible
            assert!(job.workflow_id.is_none());
            assert!(job.workflow_name.is_none());
        }
    }

    #[cfg(feature = "mysql")]
    mod mysql_tests {
        use super::*;

        #[tokio::test]
        async fn test_mysql_get_job_includes_all_fields() {
            let queue = test_utils::setup_mysql_queue().await;
            let unique_queue = format!("mysql_get_test_{}", uuid::Uuid::new_v4());

            let job = Job::new(unique_queue.clone(), json!({"test": "mysql"}))
                .with_priority(JobPriority::High)
                .with_timeout(Duration::from_secs(30));

            let job_id = queue.enqueue(job).await.unwrap();

            // This should not panic with missing field errors
            let retrieved = queue.get_job(job_id).await.unwrap();
            assert!(retrieved.is_some());

            let job = retrieved.unwrap();
            assert_eq!(job.queue_name, unique_queue);
            // Verify dependency fields are accessible
            assert!(job.depends_on.is_empty());
            assert!(job.dependents.is_empty());
            assert_eq!(
                job.dependency_status,
                hammerwork::workflow::DependencyStatus::None
            );
        }

        #[tokio::test]
        async fn test_mysql_dequeue_includes_all_fields() {
            let queue = test_utils::setup_mysql_queue().await;
            let unique_queue = format!("mysql_dequeue_test_{}", uuid::Uuid::new_v4());

            let job = Job::new(unique_queue.clone(), json!({"test": "dequeue"}));
            queue.enqueue(job).await.unwrap();

            let dequeued = queue.dequeue(&unique_queue).await.unwrap();
            assert!(dequeued.is_some());

            let job = dequeued.unwrap();
            assert_eq!(job.queue_name, unique_queue);
            // Verify workflow fields are accessible
            assert!(job.workflow_id.is_none());
            assert!(job.workflow_name.is_none());
        }

        #[tokio::test]
        async fn test_mysql_dequeue_with_priority_weights_includes_all_fields() {
            let queue = test_utils::setup_mysql_queue().await;
            let unique_queue = format!("mysql_priority_test_{}", uuid::Uuid::new_v4());

            let weights = hammerwork::priority::PriorityWeights::default();
            let job = Job::new(unique_queue.clone(), json!({"test": "priority"}));
            queue.enqueue(job).await.unwrap();

            let dequeued = queue
                .dequeue_with_priority_weights(&unique_queue, &weights)
                .await
                .unwrap();
            assert!(dequeued.is_some());

            let job = dequeued.unwrap();
            assert_eq!(job.queue_name, unique_queue);
            // Verify dependency fields are accessible
            assert!(job.depends_on.is_empty());
            assert_eq!(
                job.dependency_status,
                hammerwork::workflow::DependencyStatus::None
            );
        }
    }
}

/// Test that workflow-specific functionality works correctly
#[cfg(test)]
mod workflow_field_tests {
    use super::*;
    use hammerwork::workflow::DependencyStatus;
    use uuid::Uuid;

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_postgres_job_with_dependencies() {
        let queue = test_utils::setup_postgres_queue().await;
        let unique_queue = format!("postgres_deps_test_{}", Uuid::new_v4());

        // Create a job with dependencies
        let dependency_job = Job::new(unique_queue.clone(), json!({"step": 1}));
        let dep_id = queue.enqueue(dependency_job).await.unwrap();

        let dependent_job = Job::new(unique_queue, json!({"step": 2}))
            .depends_on_jobs(&[dep_id])
            .with_workflow(Uuid::new_v4(), "test_workflow");

        let job_id = queue.enqueue(dependent_job).await.unwrap();

        // Retrieve and verify all fields are correctly populated
        let retrieved = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(retrieved.depends_on, vec![dep_id]);
        assert_eq!(retrieved.dependency_status, DependencyStatus::Waiting);
        assert!(retrieved.workflow_id.is_some());
        assert_eq!(retrieved.workflow_name, Some("test_workflow".to_string()));
    }

    #[cfg(feature = "mysql")]
    #[tokio::test]
    async fn test_mysql_job_with_dependencies() {
        let queue = test_utils::setup_mysql_queue().await;
        let unique_queue = format!("mysql_deps_test_{}", Uuid::new_v4());

        // Create a job with dependencies
        let dependency_job = Job::new(unique_queue.clone(), json!({"step": 1}));
        let dep_id = queue.enqueue(dependency_job).await.unwrap();

        let dependent_job = Job::new(unique_queue, json!({"step": 2}))
            .depends_on_jobs(&[dep_id])
            .with_workflow(Uuid::new_v4(), "mysql_workflow");

        let job_id = queue.enqueue(dependent_job).await.unwrap();

        // Retrieve and verify all fields are correctly populated
        let retrieved = queue.get_job(job_id).await.unwrap().unwrap();
        assert_eq!(retrieved.depends_on, vec![dep_id]);
        assert_eq!(retrieved.dependency_status, DependencyStatus::Waiting);
        assert!(retrieved.workflow_id.is_some());
        assert_eq!(retrieved.workflow_name, Some("mysql_workflow".to_string()));
    }
}
