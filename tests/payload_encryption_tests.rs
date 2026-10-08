//! Job payload encryption at rest (#11).
//!
//! Each scenario is written once against the `DatabaseQueue` trait and run against both
//! backends. The stored rows are inspected directly: no column of an encrypted job may
//! contain the plaintext of its encrypted data.

#![cfg(all(feature = "encryption", any(feature = "postgres", feature = "mysql")))]

mod test_utils;

use chrono::Utc;
use hammerwork::{
    HammerworkError, Job, JobId, JobQueue, JobStatus, Worker,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason},
    batch::JobBatch,
    encryption::{
        EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeySource, RetentionPolicy,
    },
    queue::DatabaseQueue,
    worker::JobHandler,
    workflow::JobGroup,
};
use serde_json::{Value, json};
use std::{
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

const KEY_A: &str = "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=";
const KEY_B: &str = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=";

/// Direct access to a stored row, per backend.
trait Inspect: sqlx::Database {
    /// Every column of the row of job `id` in `table`, hex-encoded and concatenated
    /// (lowercase). Panics if the row does not exist.
    fn row_hex(
        pool: &sqlx::Pool<Self>,
        table: &'static str,
        id: JobId,
    ) -> impl Future<Output = String> + Send;
}

#[cfg(feature = "postgres")]
impl Inspect for sqlx::Postgres {
    async fn row_hex(pool: &sqlx::PgPool, table: &'static str, id: JobId) -> String {
        let columns: Vec<(String, String)> = sqlx::query_as(
            "SELECT column_name::text, data_type::text FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = $1",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .unwrap();
        assert!(columns.len() > 20, "{table} has too few columns");
        let parts = columns
            .iter()
            .map(|(column, data_type)| {
                if data_type == "bytea" {
                    format!("coalesce(encode({column}, 'hex'), '')")
                } else {
                    format!("coalesce(encode(convert_to({column}::text, 'UTF8'), 'hex'), '')")
                }
            })
            .collect::<Vec<_>>()
            .join(" || '|' || ");
        let hex: String = sqlx::query_scalar(&format!("SELECT {parts} FROM {table} WHERE id = $1"))
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
        hex.to_lowercase()
    }
}

#[cfg(feature = "mysql")]
impl Inspect for sqlx::MySql {
    async fn row_hex(pool: &sqlx::MySqlPool, table: &'static str, id: JobId) -> String {
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT CAST(column_name AS CHAR) FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = ?",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .unwrap();
        assert!(columns.len() > 20, "{table} has too few columns");
        let parts = columns
            .iter()
            .map(|column| format!("COALESCE(HEX(`{column}`), '')"))
            .collect::<Vec<_>>()
            .join(", ");
        let hex: String = sqlx::query_scalar(&format!(
            "SELECT CONCAT_WS('|', {parts}) FROM {table} WHERE id = ?"
        ))
        .bind(id.to_string())
        .fetch_one(pool)
        .await
        .unwrap();
        hex.to_lowercase()
    }
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

/// Asserts that no column of the stored row of `id` contains `secret`.
async fn assert_row_has_no_plaintext<DB: Inspect>(
    pool: &sqlx::Pool<DB>,
    table: &'static str,
    id: JobId,
    secret: &str,
) {
    let row = DB::row_hex(pool, table, id).await;
    assert!(
        !row.contains(&hex(secret)),
        "the {table} row of job {id} contains the plaintext {secret:?}"
    );
}

fn secret(label: &str) -> String {
    format!("{label}-{}", uuid::Uuid::new_v4().simple())
}

fn config(key_id: &str) -> EncryptionConfig {
    EncryptionConfig::new(EncryptionAlgorithm::AES256GCM).with_key_id(key_id)
}

async fn engine(key_id: &str, key: &str) -> EncryptionEngine {
    EncryptionEngine::new(config(key_id).with_key_source(KeySource::Static(key.to_string())))
        .await
        .unwrap()
}

/// The test queue's pool with `engine` for encryption.
async fn encrypted_queue<DB>(queue: &JobQueue<DB>, engine: EncryptionEngine) -> Arc<JobQueue<DB>>
where
    DB: sqlx::Database,
{
    Arc::new(JobQueue::new(queue.pool.clone()).with_encryption(engine))
}

/// Retention long enough that `purge_expired_encrypted_jobs` in another test never
/// deletes this job.
fn long_retention() -> RetentionPolicy {
    RetentionPolicy::DeleteAfter(Duration::from_secs(30 * 24 * 60 * 60))
}

/// Poll `get_job` until `predicate` holds or 15 seconds elapse; returns the last job.
async fn wait_for_job<DB, F>(queue: &Arc<JobQueue<DB>>, id: JobId, predicate: F) -> Job
where
    DB: sqlx::Database,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
    F: Fn(&Job) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let job = queue.get_job(id).await.unwrap().expect("job exists");
        if predicate(&job) || Instant::now() >= deadline {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Runs a worker on `queue_name` until job `id` is no longer pending or running.
/// Returns the job and the payloads the handler saw.
async fn run_worker_until_done<DB>(
    queue: &Arc<JobQueue<DB>>,
    queue_name: &str,
    id: JobId,
) -> (Job, Vec<Value>)
where
    DB: sqlx::Database + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let seen = Arc::new(Mutex::new(Vec::new()));
    let handler_seen = Arc::clone(&seen);
    let handler: JobHandler = Arc::new(move |job: Job| {
        let seen = Arc::clone(&handler_seen);
        Box::pin(async move {
            seen.lock().unwrap().push(job.payload.clone());
            Ok(())
        })
    });
    let worker = Worker::new(Arc::clone(queue), queue_name.to_string(), handler)
        .with_poll_interval(Duration::from_millis(50));
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let task = tokio::spawn(async move { worker.run(shutdown_rx).await });

    let job = wait_for_job(queue, id, |job| {
        !matches!(job.status, JobStatus::Pending | JobStatus::Running)
    })
    .await;

    shutdown_tx.send(()).await.unwrap();
    task.await.unwrap().unwrap();
    let seen = seen.lock().unwrap().clone();
    (job, seen)
}

/// Whole-payload encryption: the row holds only ciphertext and a placeholder, and a
/// real worker hands the plaintext to the handler.
async fn whole_payload_round_trip<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_whole");
    let card = secret("card");
    let payload = json!({"card": card, "note": "hello"});

    let id = queue
        .enqueue(
            Job::new(queue_name.clone(), payload.clone())
                .with_encryption(config("k1"))
                .with_retention_policy(long_retention()),
        )
        .await
        .unwrap();

    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", id, &card).await;
    // Not even the non-sensitive fields: the whole payload is encrypted
    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", id, "hello").await;

    // The queue returns the stored form; the encryption columns read back
    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert!(stored.is_encrypted);
    assert_eq!(stored.payload, json!({"encrypted": true}));
    let encrypted = stored.encrypted_payload.as_ref().expect("ciphertext");
    assert_eq!(encrypted.metadata.key_id, "k1");
    assert_eq!(encrypted.metadata.algorithm, EncryptionAlgorithm::AES256GCM);
    assert!(encrypted.metadata.payload_hash.starts_with("hmac-sha256:"));
    let delete_at = encrypted.metadata.delete_at.expect("retention_delete_at");
    assert!(delete_at > Utc::now() + chrono::Duration::days(29));
    assert_eq!(
        queue.decrypt_job(stored).await.unwrap().payload,
        payload,
        "decrypt_job restores the payload"
    );

    // A plain queue (e.g. the web dashboard or CLI) shows the redacted job and cannot decrypt
    let seen_by_plain = plain.get_job(id).await.unwrap().unwrap();
    assert_eq!(seen_by_plain.payload, json!({"encrypted": true}));
    assert!(matches!(
        plain.decrypt_job(seen_by_plain).await,
        Err(HammerworkError::Encryption { .. })
    ));

    let (job, seen) = run_worker_until_done(&queue, &queue_name, id).await;
    assert_eq!(job.status, JobStatus::Completed, "{:?}", job.error_message);
    assert_eq!(seen, vec![payload], "the handler sees the plaintext");
    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", id, &card).await;

    queue.delete_job(id).await.unwrap();
}

/// PII field encryption: only the listed fields are encrypted and redacted.
async fn pii_fields_round_trip<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_pii");
    let ssn = secret("ssn");
    let email = secret("email");
    let payload = json!({
        "user": {"ssn": ssn, "name": "Ada"},
        "email": email,
        "amount": 42
    });

    let id = queue
        .enqueue(
            Job::new(queue_name.clone(), payload.clone())
                .with_encryption(config("k1"))
                .with_pii_fields(vec!["user.ssn", "email", "not_present"])
                .with_retention_policy(long_retention()),
        )
        .await
        .unwrap();

    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", id, &ssn).await;
    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", id, &email).await;

    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert!(stored.is_encrypted);
    assert_eq!(
        stored.payload,
        json!({"user": {"ssn": "[ENCRYPTED]", "name": "Ada"}, "email": "[ENCRYPTED]", "amount": 42})
    );
    assert_eq!(stored.pii_fields, vec!["user.ssn", "email", "not_present"]);
    assert_eq!(
        stored
            .encrypted_payload
            .as_ref()
            .unwrap()
            .metadata
            .encrypted_fields,
        vec!["user.ssn", "email"]
    );

    let (job, seen) = run_worker_until_done(&queue, &queue_name, id).await;
    assert_eq!(job.status, JobStatus::Completed, "{:?}", job.error_message);
    assert_eq!(seen, vec![payload]);

    queue.delete_job(id).await.unwrap();
}

/// Batch and workflow enqueues encrypt every job that asks for it.
async fn batch_and_workflow_are_encrypted<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_batch");
    let secrets: Vec<String> = (0..3).map(|i| secret(&format!("batch{i}"))).collect();

    let mut jobs: Vec<Job> = secrets
        .iter()
        .map(|s| {
            Job::new(queue_name.clone(), json!({"token": s, "kind": "secret"}))
                .with_encryption(config("k1"))
                .with_pii_fields(vec!["token"])
                .with_retention_policy(long_retention())
        })
        .collect();
    jobs.push(Job::new(queue_name.clone(), json!({"kind": "plain"})));
    let batch_id = queue
        .enqueue_batch(JobBatch::new("encrypted batch").with_jobs(jobs))
        .await
        .unwrap();

    let stored = queue.get_batch_jobs(batch_id).await.unwrap();
    assert_eq!(stored.len(), 4);
    let mut decrypted = Vec::new();
    for job in stored {
        for s in &secrets {
            assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", job.id, s).await;
        }
        if job.payload["kind"] == "plain" {
            assert!(!job.is_encrypted);
            assert_eq!(job.payload, json!({"kind": "plain"}));
        } else {
            assert!(job.is_encrypted);
            assert_eq!(job.payload["token"], "[ENCRYPTED]");
            decrypted.push(queue.decrypt_job(job).await.unwrap().payload["token"].clone());
        }
    }
    decrypted.sort_by_key(|v| v.as_str().unwrap().to_string());
    let mut expected: Vec<Value> = secrets.iter().map(|s| json!(s)).collect();
    expected.sort_by_key(|v| v.as_str().unwrap().to_string());
    assert_eq!(decrypted, expected);
    queue.delete_batch(batch_id).await.unwrap();

    // Workflow
    let wf_secret = secret("workflow");
    let job = Job::new(queue_name.clone(), json!({"token": wf_secret}))
        .with_encryption(config("k1"))
        .with_retention_policy(long_retention());
    let job_id = job.id;
    queue
        .enqueue_workflow(JobGroup::new("encrypted workflow").add_job(job))
        .await
        .unwrap();
    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", job_id, &wf_secret).await;
    let stored = queue.get_job(job_id).await.unwrap().unwrap();
    assert!(stored.is_encrypted);
    assert_eq!(
        queue.decrypt_job(stored).await.unwrap().payload,
        json!({"token": wf_secret})
    );
    queue.delete_job(job_id).await.unwrap();
}

/// A queue without an encryption engine refuses jobs that ask for encryption, and
/// writes nothing.
async fn missing_engine_fails_closed<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue_name = test_utils::unique_queue("enc_missing");
    let job =
        Job::new(queue_name.clone(), json!({"secret": secret("x")})).with_encryption(config("k1"));
    let id = job.id;
    let error = plain.enqueue(job).await.unwrap_err();
    assert!(
        matches!(error, HammerworkError::Encryption { .. }),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("with_encryption"));
    assert!(plain.get_job(id).await.unwrap().is_none());

    // One job needing encryption fails the whole batch
    let plain_job = Job::new(queue_name.clone(), json!({"a": 1}));
    let plain_id = plain_job.id;
    let encrypted_job = Job::new(queue_name.clone(), json!({"b": 2})).with_encryption(config("k1"));
    assert!(matches!(
        plain
            .enqueue_batch(JobBatch::new("mixed").with_jobs(vec![plain_job, encrypted_job]))
            .await,
        Err(HammerworkError::Encryption { .. })
    ));
    assert!(plain.get_job(plain_id).await.unwrap().is_none());

    // A job asking for another key than the engine's is refused as well
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let job = Job::new(queue_name.clone(), json!({"c": 3})).with_encryption(config("other"));
    let id = job.id;
    assert!(matches!(
        queue.enqueue(job).await,
        Err(HammerworkError::Encryption { .. })
    ));
    assert!(queue.get_job(id).await.unwrap().is_none());
}

/// Workers without the right key fail the job with a clear error and never run the
/// handler; a decrypt-only key for the previous key id keeps old jobs readable.
async fn wrong_and_rotated_keys<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let writer = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_keys");
    let payload = json!({"secret": secret("rotated")});
    let enqueue = || {
        writer.enqueue(
            Job::new(queue_name.clone(), payload.clone())
                .with_encryption(config("k1"))
                .with_retention_policy(long_retention())
                .with_max_attempts(1),
        )
    };

    // Same key id, different key material: authentication fails
    let id = enqueue().await.unwrap();
    let wrong = encrypted_queue(&plain, engine("k1", KEY_B).await).await;
    let (job, seen) = run_worker_until_done(&wrong, &queue_name, id).await;
    assert_eq!(job.status, JobStatus::Dead);
    let error = job.error_message.clone().unwrap_or_default();
    assert!(error.contains("Cannot decrypt the payload"), "{error}");
    assert!(seen.is_empty(), "the handler must not run");
    assert_eq!(job.payload, json!({"encrypted": true}));
    writer.delete_job(id).await.unwrap();

    // No engine at all
    let id = enqueue().await.unwrap();
    let (job, seen) = run_worker_until_done(&plain, &queue_name, id).await;
    assert_eq!(job.status, JobStatus::Dead);
    let error = job.error_message.clone().unwrap_or_default();
    assert!(error.contains("no encryption engine"), "{error}");
    assert!(seen.is_empty());
    writer.delete_job(id).await.unwrap();

    // Rotated to a new key id: the job is encrypted with k1, the worker's engine
    // encrypts with k2 and keeps k1 for decryption
    let id = enqueue().await.unwrap();
    let rotated =
        EncryptionEngine::new(config("k2").with_key_source(KeySource::Static(KEY_B.to_string())))
            .await
            .unwrap();
    let without_old_key = encrypted_queue(&plain, rotated).await;
    assert!(matches!(
        without_old_key
            .decrypt_job(writer.get_job(id).await.unwrap().unwrap())
            .await,
        Err(HammerworkError::Encryption { .. })
    ));
    let rotated =
        EncryptionEngine::new(config("k2").with_key_source(KeySource::Static(KEY_B.to_string())))
            .await
            .unwrap()
            .with_decryption_key("k1", &KeySource::Static(KEY_A.to_string()))
            .await
            .unwrap();
    let rotated = encrypted_queue(&plain, rotated).await;
    let (job, seen) = run_worker_until_done(&rotated, &queue_name, id).await;
    assert_eq!(job.status, JobStatus::Completed, "{:?}", job.error_message);
    assert_eq!(seen, vec![payload.clone()]);
    writer.delete_job(id).await.unwrap();

    // New jobs on the rotated queue use k2
    let id = rotated
        .enqueue(Job::new(queue_name.clone(), payload).with_encryption(config("k2")))
        .await
        .unwrap();
    let stored = rotated.get_job(id).await.unwrap().unwrap();
    assert_eq!(stored.encrypted_payload.unwrap().metadata.key_id, "k2");
    rotated.delete_job(id).await.unwrap();
}

/// Expired encrypted jobs are deleted once finished; pending ones and ones still
/// within retention are kept.
async fn retention_purge<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_retention");
    let archive_queue = test_utils::unique_queue("enc_retention_archive");
    let past = Utc::now() - chrono::Duration::hours(1);
    let encrypted_on = |queue_name: &str, policy: RetentionPolicy| {
        Job::new(
            queue_name.to_string(),
            json!({"secret": secret("retention")}),
        )
        .with_encryption(config("k1"))
        .with_retention_policy(policy)
    };
    let encrypted = |policy: RetentionPolicy| encrypted_on(&queue_name, policy);

    let expired_pending = queue
        .enqueue(encrypted(RetentionPolicy::DeleteAt(past)))
        .await
        .unwrap();
    let expired_done = queue
        .enqueue(encrypted(RetentionPolicy::DeleteAt(past)))
        .await
        .unwrap();
    let immediate_done = queue
        .enqueue(encrypted(RetentionPolicy::DeleteImmediately))
        .await
        .unwrap();
    let kept_done = queue.enqueue(encrypted(long_retention())).await.unwrap();
    let indefinite_done = queue
        .enqueue(encrypted(RetentionPolicy::KeepIndefinitely))
        .await
        .unwrap();
    let expired_archived = queue
        .enqueue(encrypted_on(
            &archive_queue,
            RetentionPolicy::DeleteAt(past),
        ))
        .await
        .unwrap();
    for id in [
        expired_done,
        immediate_done,
        kept_done,
        indefinite_done,
        expired_archived,
    ] {
        queue.complete_job(id).await.unwrap();
    }
    // Archive one expired job; the purge removes it from the archive table too
    let stats = queue
        .archive_jobs(
            Some(archive_queue.as_str()),
            &ArchivalPolicy::new()
                .archive_completed_after(chrono::Duration::seconds(0))
                .enabled(true),
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            None,
        )
        .await
        .unwrap();
    assert_eq!(stats.jobs_archived, 1);

    // Purging needs no key: run it on the plain queue
    let purge = plain.purge_expired_encrypted_jobs().await.unwrap();
    assert!(purge.jobs >= 2, "{purge:?}");
    assert!(purge.archived_jobs >= 1, "{purge:?}");

    for id in [expired_done, immediate_done, expired_archived] {
        assert!(
            queue.get_job(id).await.unwrap().is_none(),
            "job {id} should have been purged"
        );
    }
    for id in [expired_pending, kept_done, indefinite_done] {
        assert!(
            queue.get_job(id).await.unwrap().is_some(),
            "job {id} must be kept"
        );
    }

    // Once the pending job finishes it is purged too
    queue.complete_job(expired_pending).await.unwrap();
    plain.purge_expired_encrypted_jobs().await.unwrap();
    assert!(queue.get_job(expired_pending).await.unwrap().is_none());

    for id in [kept_done, indefinite_done] {
        queue.delete_job(id).await.unwrap();
    }
}

/// Archiving and restoring move the ciphertext without decrypting it (the queue
/// doing it has no engine), and the restored job decrypts.
async fn archive_and_restore_keep_ciphertext<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_archive");
    let ssn = secret("archived");
    let payload = json!({"ssn": ssn, "visible": 1});
    let id = queue
        .enqueue(
            Job::new(queue_name.clone(), payload.clone())
                .with_encryption(config("k1"))
                .with_pii_fields(vec!["ssn"])
                .with_retention_policy(long_retention()),
        )
        .await
        .unwrap();
    let before = queue.get_job(id).await.unwrap().unwrap();
    let ciphertext = before.encrypted_payload.clone().unwrap().ciphertext;
    plain.complete_job(id).await.unwrap();

    let stats = plain
        .archive_jobs(
            Some(queue_name.as_str()),
            &ArchivalPolicy::new()
                .archive_completed_after(chrono::Duration::seconds(0))
                .enabled(true),
            &ArchivalConfig::new(),
            ArchivalReason::Manual,
            None,
        )
        .await
        .unwrap();
    assert_eq!(stats.jobs_archived, 1);
    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs_archive", id, &ssn).await;
    let archived_hex = DB::row_hex(&queue.pool, "hammerwork_jobs_archive", id).await;
    assert!(
        archived_hex.contains(&hex("[ENCRYPTED]")) || archived_hex.contains(&hex("hmac-sha256:")),
        "the archive row carries the encryption columns"
    );

    let archived = plain.get_job(id).await.unwrap().unwrap();
    assert_eq!(archived.status, JobStatus::Archived);
    assert!(archived.is_encrypted);
    assert_eq!(
        archived.payload,
        json!({"ssn": "[ENCRYPTED]", "visible": 1})
    );

    let restored = plain.restore_archived_job(id).await.unwrap();
    assert!(restored.is_encrypted);
    assert_row_has_no_plaintext(&queue.pool, "hammerwork_jobs", id, &ssn).await;
    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert!(stored.is_encrypted);
    assert_eq!(stored.pii_fields, vec!["ssn"]);
    assert_eq!(
        stored.encrypted_payload.as_ref().unwrap().ciphertext,
        ciphertext,
        "the ciphertext is carried over unchanged"
    );
    assert_eq!(queue.decrypt_job(stored).await.unwrap().payload, payload);

    queue.delete_job(id).await.unwrap();
}

/// Plain jobs on a queue with an engine are stored as before.
async fn plain_jobs_unchanged<DB>(plain: Arc<JobQueue<DB>>)
where
    DB: Inspect + Send + Sync + 'static,
    JobQueue<DB>: DatabaseQueue<Database = DB> + Send + Sync + 'static,
{
    let queue = encrypted_queue(&plain, engine("k1", KEY_A).await).await;
    let queue_name = test_utils::unique_queue("enc_plain");
    let marker = secret("visible");
    let payload = json!({"visible": marker});
    let id = queue
        .enqueue(Job::new(queue_name.clone(), payload.clone()))
        .await
        .unwrap();
    // Control for `assert_row_has_no_plaintext`: the inspection sees plaintext payloads
    assert!(
        DB::row_hex(&queue.pool, "hammerwork_jobs", id)
            .await
            .contains(&hex(&marker))
    );
    let stored = queue.get_job(id).await.unwrap().unwrap();
    assert!(!stored.is_encrypted);
    assert!(stored.encrypted_payload.is_none());
    assert_eq!(stored.payload, payload);
    let (job, seen) = run_worker_until_done(&plain, &queue_name, id).await;
    assert_eq!(job.status, JobStatus::Completed);
    assert_eq!(seen, vec![payload]);
    queue.delete_job(id).await.unwrap();
}

#[cfg(feature = "postgres")]
mod postgres {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_whole_payload_round_trip() {
        whole_payload_round_trip(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_pii_fields_round_trip() {
        pii_fields_round_trip(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_batch_and_workflow_are_encrypted() {
        batch_and_workflow_are_encrypted(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_missing_engine_fails_closed() {
        missing_engine_fails_closed(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_wrong_and_rotated_keys() {
        wrong_and_rotated_keys(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_retention_purge() {
        retention_purge(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_archive_and_restore_keep_ciphertext() {
        archive_and_restore_keep_ciphertext(test_utils::setup_postgres_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_postgres_plain_jobs_unchanged() {
        plain_jobs_unchanged(test_utils::setup_postgres_queue().await).await;
    }
}

#[cfg(feature = "mysql")]
mod mysql {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_whole_payload_round_trip() {
        whole_payload_round_trip(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_pii_fields_round_trip() {
        pii_fields_round_trip(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_batch_and_workflow_are_encrypted() {
        batch_and_workflow_are_encrypted(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_missing_engine_fails_closed() {
        missing_engine_fails_closed(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_wrong_and_rotated_keys() {
        wrong_and_rotated_keys(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_retention_purge() {
        retention_purge(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_archive_and_restore_keep_ciphertext() {
        archive_and_restore_keep_ciphertext(test_utils::setup_mysql_queue().await).await;
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_mysql_plain_jobs_unchanged() {
        plain_jobs_unchanged(test_utils::setup_mysql_queue().await).await;
    }
}
