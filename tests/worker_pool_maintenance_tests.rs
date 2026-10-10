//! Maintenance tasks a `WorkerPool` runs besides its workers: the expired result
//! cleanup (`WorkerPool::with_result_cleanup`, on by default and configured by
//! `[worker] result_cleanup_*`) and key rotation (`WorkerPool::with_key_rotation`,
//! configured by `[encryption.key_rotation]`). Each scenario runs against both backends.
//!
//! The result cleanup clears every queue, so each test holds `test_utils::serial()`.

#![cfg(any(feature = "postgres", feature = "mysql"))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    HammerworkConfig, Job, JobId, JobQueue, Worker, WorkerPool, queue::DatabaseQueue,
    worker::JobHandler,
};
use serde_json::json;
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant},
};

/// Polls `condition` every 50ms until it holds or 20 seconds pass.
async fn eventually<F, Fut>(condition: F) -> bool
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if condition().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A worker on a queue of its own, so it never touches the jobs under test.
fn idle_worker<DB>(queue: &Arc<JobQueue<DB>>) -> Worker<DB>
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let handler: JobHandler = Arc::new(|_job: Job| Box::pin(async { Ok(()) }));
    Worker::new(
        Arc::clone(queue),
        test_utils::unique_queue("maintenance_idle"),
        handler,
    )
    .with_poll_interval(Duration::from_millis(50))
}

/// A completed job with a stored result expiring at `expires_at`.
async fn job_with_result<DB>(
    queue: &JobQueue<DB>,
    queue_name: &str,
    expires_at: chrono::DateTime<Utc>,
) -> JobId
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB>,
{
    let id = queue
        .enqueue(Job::new(queue_name.to_string(), json!({"n": 1})))
        .await
        .unwrap();
    queue.complete_job(id).await.unwrap();
    queue
        .store_job_result(id, json!({"answer": 42}), Some(expires_at))
        .await
        .unwrap();
    id
}

/// Whether the job still has its stored result (expired or not).
async fn has_result<DB>(queue: &JobQueue<DB>, id: JobId) -> bool
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB>,
{
    queue
        .get_job(id)
        .await
        .unwrap()
        .expect("job exists")
        .result_data
        .is_some()
}

/// The pool clears expired results on its interval (keeping unexpired ones and the
/// jobs themselves) and stops once it is shut down.
async fn pool_cleans_expired_results<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("result_cleanup");
    let past = Utc::now() - chrono::Duration::minutes(1);
    let future = Utc::now() + chrono::Duration::hours(1);

    let expired = job_with_result(&queue, &queue_name, past).await;
    let fresh = job_with_result(&queue, &queue_name, future).await;
    assert!(has_result(&queue, expired).await, "expired results linger");

    let mut pool = WorkerPool::new()
        .without_autoscaling()
        .without_stale_job_reaper()
        .with_result_cleanup(Duration::from_millis(100));
    pool.add_worker(idle_worker(&queue));

    // The first cleanup runs at start; a result that expires later is cleared by a
    // later one.
    let later = job_with_result(
        &queue,
        &queue_name,
        Utc::now() + chrono::Duration::milliseconds(300),
    )
    .await;
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        done = eventually(|| async {
            !has_result(&queue, expired).await && !has_result(&queue, later).await
        }) => assert!(done, "the pool cleared the expired results"),
    }
    pool.shutdown().await.unwrap();

    assert!(
        has_result(&queue, fresh).await,
        "unexpired results are kept"
    );
    assert!(
        queue.get_job(expired).await.unwrap().is_some(),
        "only the result is cleared, not the job"
    );
    assert_eq!(
        queue.get_job_result(fresh).await.unwrap(),
        Some(json!({"answer": 42}))
    );

    // After shutdown nothing is cleared any more.
    let after = job_with_result(&queue, &queue_name, past).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        has_result(&queue, after).await,
        "the cleanup stopped on shutdown"
    );

    for id in [expired, fresh, later, after] {
        queue.delete_job(id).await.unwrap();
    }
}

/// `from_hammerwork_config` clears expired results by default, on
/// `worker.result_cleanup_interval`, and not at all with `result_cleanup_enabled =
/// false`.
async fn configured_pool_cleans_results<DB>(queue: Arc<JobQueue<DB>>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let _serial = test_utils::serial().await;
    let queue_name = test_utils::unique_queue("cfg_result_cleanup");
    let past = Utc::now() - chrono::Duration::minutes(1);

    let mut config = HammerworkConfig::new();
    config.worker.pool_size = 1;
    config.worker.result_cleanup_enabled = false;
    config.worker.result_cleanup_interval = Duration::from_millis(100);

    let id = job_with_result(&queue, &queue_name, past).await;
    let mut pool = WorkerPool::from_hammerwork_config(idle_worker(&queue), &config)
        .unwrap()
        .without_stale_job_reaper();
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        () = tokio::time::sleep(Duration::from_millis(500)) => {}
    }
    pool.shutdown().await.unwrap();
    assert!(has_result(&queue, id).await, "cleanup is off when disabled");

    config.worker.result_cleanup_enabled = true;
    let mut pool = WorkerPool::from_hammerwork_config(idle_worker(&queue), &config)
        .unwrap()
        .without_stale_job_reaper();
    tokio::select! {
        result = pool.start() => panic!("pool stopped early: {result:?}"),
        done = eventually(|| async { !has_result(&queue, id).await }) => {
            assert!(done, "the configured pool cleared the expired result");
        }
    }
    pool.shutdown().await.unwrap();
    queue.delete_job(id).await.unwrap();
}

#[cfg(feature = "encryption")]
mod key_rotation {
    use super::*;
    use hammerwork::encryption::{
        EncryptionAlgorithm, KeyManager, KeyManagerBackend, KeyManagerConfig, KeySource,
    };

    /// Environment variable holding the master key of these tests.
    const MASTER_KEY_ENV: &str = "HAMMERWORK_TEST_POOL_ROTATION_MASTER_KEY";

    /// The master key of these tests: the one the key manager's unit tests use, so a
    /// key-encryption key they leave behind in the shared database stays readable.
    fn master_key() -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode([0x42u8; 32])
    }

    fn set_master_key_env() {
        // SAFETY: set before any key manager of this binary reads the environment;
        // every test sets the same value.
        unsafe { std::env::set_var(MASTER_KEY_ENV, master_key()) };
    }

    /// A key manager on `pool` with automatic rotation and the test master key.
    async fn key_manager<DB: KeyManagerBackend>(pool: sqlx::Pool<DB>) -> KeyManager<DB> {
        KeyManager::new(
            KeyManagerConfig::new()
                .with_master_key_source(KeySource::Static(master_key()))
                .with_auto_rotation_enabled(true),
            pool,
        )
        .await
        .unwrap()
    }

    /// A configuration with key rotation every second, using the test master key.
    fn rotation_config() -> HammerworkConfig {
        let mut config = HammerworkConfig::new();
        config.worker.pool_size = 1;
        config.worker.result_cleanup_enabled = false;
        config.encryption.key_rotation.enabled = true;
        config.encryption.key_rotation.master_key_source =
            hammerwork::KeySourceRef::parse(&format!("env://{MASTER_KEY_ENV}")).unwrap();
        config.encryption.key_rotation.check_interval_secs = 1;
        config
    }

    /// A new key, due for rotation now, with a 30 day rotation interval (so each
    /// rotation makes it due again only in 30 days).
    async fn due_key<DB: KeyManagerBackend>(manager: &mut KeyManager<DB>) -> String {
        let key_id = test_utils::unique_queue("rotation_key");
        manager
            .generate_key(&key_id, EncryptionAlgorithm::AES256GCM)
            .await
            .unwrap();
        manager
            .update_key_rotation_schedule(&key_id, Some(chrono::Duration::days(30)))
            .await
            .unwrap();
        manager
            .schedule_key_rotation(&key_id, Utc::now() - chrono::Duration::minutes(1))
            .await
            .unwrap();
        key_id
    }

    /// `[encryption.key_rotation] enabled = true` makes `from_hammerwork_config` start
    /// key rotation: a due key is rotated, and nothing is rotated after shutdown. Two
    /// pools rotating at once (as two processes would) create exactly one new active
    /// version.
    pub(super) async fn configured_pools_rotate_due_keys<DB, V, VFut>(
        queue: Arc<JobQueue<DB>>,
        versions: V,
    ) where
        DB: KeyManagerBackend + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
        V: Fn(String) -> VFut,
        VFut: Future<Output = (i64, i64)>,
    {
        let _serial = test_utils::serial().await;
        set_master_key_env();
        let mut manager = key_manager(queue.pool.clone()).await;

        // Rotation is off unless configured.
        let key_id = due_key(&mut manager).await;
        let mut off = rotation_config();
        off.encryption.key_rotation.enabled = false;
        let mut pool = WorkerPool::from_hammerwork_config(idle_worker(&queue), &off)
            .unwrap()
            .without_stale_job_reaper();
        tokio::select! {
            result = pool.start() => panic!("pool stopped early: {result:?}"),
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
        }
        pool.shutdown().await.unwrap();
        assert_eq!(versions(key_id.clone()).await, (1, 1), "not rotated");

        // Two configured pools at once: the key gets exactly one new version.
        let config = rotation_config();
        let mut first = WorkerPool::from_hammerwork_config(idle_worker(&queue), &config)
            .unwrap()
            .without_stale_job_reaper();
        let mut second = WorkerPool::from_hammerwork_config(idle_worker(&queue), &config)
            .unwrap()
            .without_stale_job_reaper();
        tokio::select! {
            result = first.start() => panic!("pool stopped early: {result:?}"),
            result = second.start() => panic!("pool stopped early: {result:?}"),
            done = async {
                let rotated = eventually(|| async { versions(key_id.clone()).await.0 >= 2 }).await;
                // Both pools run several passes meanwhile.
                tokio::time::sleep(Duration::from_millis(2500)).await;
                rotated
            } => assert!(done, "the configured pools rotated the due key"),
        }
        assert_eq!(
            versions(key_id.clone()).await,
            (2, 1),
            "exactly one new version, and one active version"
        );

        // Another due key while both pools keep running (dropping `start` does not
        // stop them): rotated once too.
        let second_key = due_key(&mut manager).await;
        assert!(eventually(|| async { versions(second_key.clone()).await.0 >= 2 }).await);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(versions(second_key.clone()).await, (2, 1));
        first.shutdown().await.unwrap();
        second.shutdown().await.unwrap();

        // After shutdown nothing is rotated any more.
        manager
            .schedule_key_rotation(&key_id, Utc::now() - chrono::Duration::minutes(1))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            versions(key_id.clone()).await,
            (2, 1),
            "stopped on shutdown"
        );
        let rotations = manager
            .audit_log(
                &hammerwork::encryption::KeyAuditFilter::new()
                    .with_key_id(&key_id)
                    .with_operation(hammerwork::encryption::KeyOperation::Rotate),
            )
            .await
            .unwrap();
        assert_eq!(rotations.len(), 1, "one rotation was audited");
    }

    /// `with_key_rotation` rotates the due keys of the given key manager.
    pub(super) async fn builder_pool_rotates_due_keys<DB, V, VFut>(
        queue: Arc<JobQueue<DB>>,
        versions: V,
    ) where
        DB: KeyManagerBackend + Send + Sync + 'static,
        JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
        V: Fn(String) -> VFut,
        VFut: Future<Output = (i64, i64)>,
    {
        let _serial = test_utils::serial().await;
        let mut manager = key_manager(queue.pool.clone()).await;
        let key_id = due_key(&mut manager).await;
        let mut pool = WorkerPool::new()
            .without_stale_job_reaper()
            .without_result_cleanup()
            .with_key_rotation(manager.clone(), Duration::from_millis(100));
        pool.add_worker(idle_worker(&queue));
        tokio::select! {
            result = pool.start() => panic!("pool stopped early: {result:?}"),
            done = eventually(|| async { versions(key_id.clone()).await.0 >= 2 }) => {
                assert!(done, "the pool rotated the due key");
            }
        }
        pool.shutdown().await.unwrap();
        assert_eq!(versions(key_id).await, (2, 1));
    }
}

#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;

    async fn queue() -> Arc<JobQueue<sqlx::Postgres>> {
        test_utils::setup_postgres_queue().await
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_pool_cleans_expired_results() {
        pool_cleans_expired_results(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_configured_pool_cleans_results() {
        configured_pool_cleans_results(queue().await).await;
    }

    #[cfg(feature = "encryption")]
    async fn versions(pool: sqlx::PgPool, key_id: String) -> (i64, i64) {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT COALESCE(MAX(key_version), 0)::BIGINT AS newest, \
             COUNT(*) FILTER (WHERE status = 'Active') AS active \
             FROM hammerwork_encryption_keys WHERE key_id = $1",
        )
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        (row.get("newest"), row.get("active"))
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_configured_pools_rotate_due_keys() {
        let queue = queue().await;
        let pool = queue.pool.clone();
        key_rotation::configured_pools_rotate_due_keys(queue, |key| versions(pool.clone(), key))
            .await;
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_builder_pool_rotates_due_keys() {
        let queue = queue().await;
        let pool = queue.pool.clone();
        key_rotation::builder_pool_rotates_due_keys(queue, |key| versions(pool.clone(), key)).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;

    async fn queue() -> Arc<JobQueue<sqlx::MySql>> {
        test_utils::setup_mysql_queue().await
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_pool_cleans_expired_results() {
        pool_cleans_expired_results(queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_configured_pool_cleans_results() {
        configured_pool_cleans_results(queue().await).await;
    }

    #[cfg(feature = "encryption")]
    async fn versions(pool: sqlx::MySqlPool, key_id: String) -> (i64, i64) {
        use sqlx::Row;
        let row = sqlx::query(
            "SELECT CAST(COALESCE(MAX(key_version), 0) AS SIGNED) AS newest, \
             CAST(COUNT(CASE WHEN status = 'Active' THEN 1 END) AS SIGNED) AS active \
             FROM hammerwork_encryption_keys WHERE key_id = ?",
        )
        .bind(key_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        (row.get("newest"), row.get("active"))
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_configured_pools_rotate_due_keys() {
        let queue = queue().await;
        let pool = queue.pool.clone();
        key_rotation::configured_pools_rotate_due_keys(queue, |key| versions(pool.clone(), key))
            .await;
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_builder_pool_rotates_due_keys() {
        let queue = queue().await;
        let pool = queue.pool.clone();
        key_rotation::builder_pool_rotates_due_keys(queue, |key| versions(pool.clone(), key)).await;
    }
}
