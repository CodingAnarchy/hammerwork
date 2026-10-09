//! `JobQueue::from_config` on both backends: pool settings, `auto_migrate`, throttle
//! registration and connection errors.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use hammerwork::{
    HammerworkConfig, Job, JobQueue, JobStatus, queue::DatabaseQueue, rate_limit::ThrottleConfig,
};
use serde_json::json;
use std::{future::Future, time::Duration};

fn config_for(url: &str) -> HammerworkConfig {
    let mut config = HammerworkConfig::new()
        .with_database_url(url)
        .with_database_pool_size(3);
    config.database.auto_migrate = true;
    config.rate_limiting.enabled = true;
    config.rate_limiting.default_throttle = ThrottleConfig::new().rate_per_minute(10);
    config.rate_limiting.queue_throttles.insert(
        "emails".to_string(),
        ThrottleConfig::new().max_concurrent(4).rate_per_minute(120),
    );
    config
}

/// A queue from `from_config` (with `auto_migrate`) is connected, migrated, uses the
/// configured pool size and has the per-queue throttles registered.
async fn from_config_builds_a_working_queue<DB, F, Fut>(url: String, connect: F)
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    F: Fn(HammerworkConfig) -> Fut,
    Fut: Future<Output = hammerwork::Result<JobQueue<DB>>>,
{
    let config = config_for(&url);
    let queue = connect(config.clone()).await.expect("from_config");
    assert_eq!(queue.get_pool().options().get_max_connections(), 3);

    // Migrations ran (they are idempotent, so this also works on a migrated database).
    let queue_name = test_utils::unique_queue("from_config");
    let id = queue
        .enqueue(Job::new(queue_name.clone(), json!({ "n": 1 })))
        .await
        .unwrap();
    let job = queue.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Pending);
    assert_eq!(job.payload, json!({ "n": 1 }));

    // Only per-queue throttles are registered; the default applies through workers.
    let throttle = queue.get_throttle("emails").await.expect("emails throttle");
    assert_eq!(throttle.max_concurrent, Some(4));
    assert_eq!(throttle.rate_per_minute, Some(120));
    assert!(queue.get_throttle("other").await.is_none());
    assert_eq!(queue.get_all_throttles().await.len(), 1);
    queue.remove_throttle("emails").await.unwrap();
    assert!(queue.get_throttle("emails").await.is_none());

    // Clones share the throttle registry.
    queue
        .set_throttle("reports", ThrottleConfig::new().rate_per_minute(1))
        .await
        .unwrap();
    let clone = queue.clone();
    assert!(clone.get_throttle("reports").await.is_some());

    // With rate limiting disabled nothing is registered.
    let mut disabled = config;
    disabled.rate_limiting.enabled = false;
    disabled.database.auto_migrate = false;
    let queue2 = connect(disabled)
        .await
        .expect("from_config without auto_migrate");
    assert!(queue2.get_all_throttles().await.is_empty());
    assert!(queue2.get_job(id).await.unwrap().is_some());

    queue.delete_job(id).await.unwrap();
}

/// An unreachable database is an error, reported within the connection timeout.
async fn from_config_fails_for_unreachable_database<DB, F, Fut>(url: &str, connect: F)
where
    DB: sqlx::Database,
    F: Fn(HammerworkConfig) -> Fut,
    Fut: Future<Output = hammerwork::Result<JobQueue<DB>>>,
{
    let mut config = config_for(url);
    config.database.connection_timeout_secs = 2;
    let started = std::time::Instant::now();
    let result = connect(config).await;
    assert!(
        matches!(result, Err(hammerwork::HammerworkError::Database(_))),
        "expected a database error"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    async fn connect(config: HammerworkConfig) -> hammerwork::Result<JobQueue<sqlx::Postgres>> {
        JobQueue::<sqlx::Postgres>::from_config(&config).await
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_from_config_builds_a_working_queue() {
        from_config_builds_a_working_queue(test_utils::postgres_url(), connect).await;
    }

    #[tokio::test]
    async fn test_postgres_from_config_fails_for_unreachable_database() {
        from_config_fails_for_unreachable_database(
            "postgres://postgres:x@127.0.0.1:1/none",
            connect,
        )
        .await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    async fn connect(config: HammerworkConfig) -> hammerwork::Result<JobQueue<sqlx::MySql>> {
        JobQueue::<sqlx::MySql>::from_config(&config).await
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_from_config_builds_a_working_queue() {
        from_config_builds_a_working_queue(test_utils::mysql_url(), connect).await;
    }

    #[tokio::test]
    async fn test_mysql_from_config_fails_for_unreachable_database() {
        from_config_fails_for_unreachable_database("mysql://root:x@127.0.0.1:1/none", connect)
            .await;
    }
}
