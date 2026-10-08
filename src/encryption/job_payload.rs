//! Encrypting job payloads at rest.
//!
//! [`JobQueue`](crate::JobQueue) uses these functions when it has an
//! [`EncryptionEngine`] (see [`JobQueue::with_encryption`](crate::JobQueue::with_encryption)):
//! jobs with an [`EncryptionConfig`](super::EncryptionConfig) are sealed before they are
//! written, and opened again before a worker hands them to a handler.
//!
//! What is stored in the `payload` column of an encrypted job:
//!
//! - **Whole-payload encryption** (the job has no PII fields): the placeholder
//!   `{"encrypted": true}` ([`encrypted_payload_placeholder`]).
//! - **PII field encryption** (`Job::with_pii_fields`): the payload with the value of each
//!   listed field replaced by the string `"[ENCRYPTED]"` ([`REDACTED_FIELD_VALUE`]). Only the
//!   listed fields' values are encrypted; the rest of the payload stays readable.
//!
//! PII field names are paths into the JSON payload: `"ssn"` is a top-level field and
//! `"customer.ssn"` the `ssn` field of the `customer` object. A key that itself contains a
//! dot (`"a.b"`) is matched literally first. Fields missing from a payload are skipped.
//!
//! # Binding the ciphertext to its job
//!
//! The ciphertext is encrypted with associated data ([`job_associated_data`]) made of the
//! job's id, queue name, key id, algorithm, PII fields and encrypted fields, and the
//! metadata records [`EncryptionMetadata::FORMAT_JOB_BOUND`]. Decryption recomputes the
//! associated data from the stored row, so moving a ciphertext (or its nonce and tag) to
//! another job, moving a job to another queue, or changing its key id, algorithm or PII
//! fields makes decryption fail. All of these values are kept by archiving and restoring.
//! Setting the stored format back to [`EncryptionMetadata::FORMAT_UNBOUND`] does not help
//! an attacker either: the ciphertext then fails to authenticate without the associated
//! data it was encrypted with.
//!
//! Payloads written before this binding have format
//! [`EncryptionMetadata::FORMAT_UNBOUND`] and are still decrypted, without associated
//! data. They are not re-encrypted.

use super::{
    EncryptionAlgorithm, EncryptionEngine, EncryptionError, EncryptionMetadata, RetentionPolicy,
};
use crate::job::Job;
use serde_json::{Map, Value};

/// Domain separation prefix of [`job_associated_data`].
const JOB_AAD_CONTEXT: &[u8] = b"hammerwork/job-payload/v1";

/// The associated data that binds an encrypted job payload to its job
/// ([`EncryptionMetadata::FORMAT_JOB_BOUND`]).
///
/// Each value is length-prefixed, so different values never encode to the same bytes:
/// a context string, the job id, queue name, key id, algorithm, the job's PII fields and
/// the fields that were actually encrypted (empty for whole-payload encryption).
pub fn job_associated_data(
    job_id: &uuid::Uuid,
    queue_name: &str,
    key_id: &str,
    algorithm: &EncryptionAlgorithm,
    pii_fields: &[String],
    encrypted_fields: &[String],
) -> Vec<u8> {
    fn push(out: &mut Vec<u8>, bytes: &[u8]) {
        out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        out.extend_from_slice(bytes);
    }
    fn push_list(out: &mut Vec<u8>, items: &[String]) {
        out.extend_from_slice(&(items.len() as u64).to_be_bytes());
        for item in items {
            push(out, item.as_bytes());
        }
    }

    let algorithm = match algorithm {
        EncryptionAlgorithm::AES256GCM => "AES256GCM",
        EncryptionAlgorithm::ChaCha20Poly1305 => "ChaCha20Poly1305",
    };
    let mut out = Vec::with_capacity(128);
    push(&mut out, JOB_AAD_CONTEXT);
    push(&mut out, job_id.as_bytes());
    push(&mut out, queue_name.as_bytes());
    push(&mut out, key_id.as_bytes());
    push(&mut out, algorithm.as_bytes());
    push_list(&mut out, pii_fields);
    push_list(&mut out, encrypted_fields);
    out
}

/// The value stored in place of an encrypted PII field.
pub const REDACTED_FIELD_VALUE: &str = "[ENCRYPTED]";

/// The placeholder payload for whole-payload encryption.
pub fn encrypted_payload_placeholder() -> Value {
    let mut map = Map::new();
    map.insert("encrypted".to_string(), Value::Bool(true));
    Value::Object(map)
}

/// Removes the values of `fields` from `payload`.
///
/// Returns the redacted payload (each found field's value replaced by
/// [`REDACTED_FIELD_VALUE`]), the removed values as `[[path, value], ...]` in extraction
/// order, and the paths that were found.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) fn redact_fields(
    payload: &Value,
    fields: &[String],
) -> Result<(Value, Value, Vec<String>), EncryptionError> {
    if !payload.is_object() {
        return Err(EncryptionError::FieldProcessing(
            "PII field encryption needs a JSON object payload; use whole-payload encryption \
             (no PII fields) for other payloads"
                .to_string(),
        ));
    }

    let mut redacted = payload.clone();
    let mut removed = Vec::new();
    let mut found = Vec::new();
    for field in fields {
        if found.contains(field) {
            continue;
        }
        if let Some(slot) = resolve_mut(&mut redacted, field) {
            let value = std::mem::replace(slot, Value::String(REDACTED_FIELD_VALUE.to_string()));
            removed.push(Value::Array(vec![Value::String(field.clone()), value]));
            found.push(field.clone());
        }
    }
    Ok((redacted, Value::Array(removed), found))
}

/// Puts the values removed by [`redact_fields`] back into `redacted`.
pub(crate) fn restore_fields(redacted: &Value, removed: &Value) -> Result<Value, EncryptionError> {
    let invalid = || {
        EncryptionError::FieldProcessing(
            "Decrypted PII fields are not in the expected [[path, value], ...] form".to_string(),
        )
    };
    let entries = removed.as_array().ok_or_else(invalid)?;

    let mut payload = redacted.clone();
    // Reverse order, so a field nested in another listed field is restored after its parent
    for entry in entries.iter().rev() {
        let pair = entry
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(invalid)?;
        let path = pair[0].as_str().ok_or_else(invalid)?;
        let slot = resolve_mut(&mut payload, path).ok_or_else(|| {
            EncryptionError::FieldProcessing(format!(
                "Encrypted PII field '{}' is missing from the stored payload",
                path
            ))
        })?;
        *slot = pair[1].clone();
    }
    Ok(payload)
}

/// The value at `path`: a top-level key equal to `path`, otherwise the dot-separated
/// path through nested objects.
fn resolve_mut<'a>(value: &'a mut Value, path: &str) -> Option<&'a mut Value> {
    if value.as_object()?.contains_key(path) {
        return value.as_object_mut()?.get_mut(path);
    }
    let mut current = value;
    for segment in path.split('.') {
        current = current.as_object_mut()?.get_mut(segment)?;
    }
    Some(current)
}

/// Encrypts `job`'s payload with `engine`, for storage.
///
/// Does nothing for a job without an encryption config or whose payload is already
/// encrypted. Otherwise replaces the payload with its redacted form and sets
/// `is_encrypted` and `encrypted_payload`.
#[cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]
pub(crate) async fn seal_job(
    engine: &EncryptionEngine,
    job: &mut Job,
) -> Result<(), EncryptionError> {
    let Some(config) = job.encryption_config.as_ref() else {
        return Ok(());
    };
    if job.is_encrypted {
        return Ok(());
    }

    if config.algorithm != *engine.algorithm() {
        return Err(EncryptionError::InvalidConfiguration(format!(
            "Job {} asks for {:?} but the queue's encryption engine uses {:?}",
            job.id,
            config.algorithm,
            engine.algorithm()
        )));
    }
    if let Some(key_id) = config.key_id.as_deref()
        && key_id != engine.key_id()
    {
        return Err(EncryptionError::InvalidConfiguration(format!(
            "Job {} asks for key '{}' but the queue's encryption engine encrypts with key '{}'",
            job.id,
            key_id,
            engine.key_id()
        )));
    }

    let retention = job
        .retention_policy
        .clone()
        .unwrap_or(RetentionPolicy::UseDefault);

    let aad = |encrypted_fields: &[String]| {
        job_associated_data(
            &job.id,
            &job.queue_name,
            engine.key_id(),
            engine.algorithm(),
            &job.pii_fields,
            encrypted_fields,
        )
    };
    let (stored_payload, mut encrypted) = if job.pii_fields.is_empty() {
        let encrypted = engine
            .encrypt_payload_with_associated_data(
                &job.payload,
                &[] as &[&str],
                retention,
                &aad(&[]),
            )
            .await?;
        (encrypted_payload_placeholder(), encrypted)
    } else {
        let (redacted, removed, found) = redact_fields(&job.payload, &job.pii_fields)?;
        let encrypted = engine
            .encrypt_payload_with_associated_data(&removed, &found, retention, &aad(&found))
            .await?;
        (redacted, encrypted)
    };
    encrypted.metadata.format_version = EncryptionMetadata::FORMAT_JOB_BOUND;

    job.payload = stored_payload;
    job.is_encrypted = true;
    job.encrypted_payload = Some(encrypted);
    Ok(())
}

/// Decrypts a stored job's payload with `engine`.
///
/// The returned job has its plaintext payload, `is_encrypted == false` and no
/// `encrypted_payload`. A job that is not encrypted is returned unchanged.
pub(crate) async fn open_job(
    engine: &EncryptionEngine,
    mut job: Job,
) -> Result<Job, EncryptionError> {
    if !job.is_encrypted {
        return Ok(job);
    }
    let encrypted = job.encrypted_payload.take().ok_or_else(|| {
        EncryptionError::DecryptionFailed(format!(
            "Job {} is marked encrypted but has no encrypted payload",
            job.id
        ))
    })?;

    let metadata = &encrypted.metadata;
    let decrypted = match metadata.format_version {
        EncryptionMetadata::FORMAT_UNBOUND => engine.decrypt_payload(&encrypted).await?,
        EncryptionMetadata::FORMAT_JOB_BOUND => {
            let aad = job_associated_data(
                &job.id,
                &job.queue_name,
                &metadata.key_id,
                &metadata.algorithm,
                &job.pii_fields,
                &metadata.encrypted_fields,
            );
            engine
                .decrypt_payload_with_associated_data(&encrypted, &aad)
                .await?
        }
        other => {
            return Err(EncryptionError::DecryptionFailed(format!(
                "Job {} has an encrypted payload in an unknown format ({}); it was written \
                 by a newer version of hammerwork",
                job.id, other
            )));
        }
    };
    job.payload = if job.pii_fields.is_empty() {
        decrypted
    } else {
        restore_fields(&job.payload, &decrypted)?
    };
    job.is_encrypted = false;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encryption::{EncryptionAlgorithm, EncryptionConfig, KeySource};
    use serde_json::json;

    const KEY: &str = "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=";

    fn fields(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    async fn engine(key_id: &str) -> EncryptionEngine {
        EncryptionEngine::new(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                .with_key_id(key_id)
                .with_key_source(KeySource::Static(KEY.to_string())),
        )
        .await
        .unwrap()
    }

    #[test]
    fn redacts_top_level_and_nested_fields() {
        let payload = json!({
            "ssn": "123-45-6789",
            "customer": {"email": "a@example.com", "name": "Ada"},
            "amount": 10
        });
        let (redacted, removed, found) =
            redact_fields(&payload, &fields(&["ssn", "customer.email", "missing"])).unwrap();

        assert_eq!(
            redacted,
            json!({
                "ssn": "[ENCRYPTED]",
                "customer": {"email": "[ENCRYPTED]", "name": "Ada"},
                "amount": 10
            })
        );
        assert_eq!(found, fields(&["ssn", "customer.email"]));
        assert_eq!(
            removed,
            json!([["ssn", "123-45-6789"], ["customer.email", "a@example.com"]])
        );
        assert_eq!(restore_fields(&redacted, &removed).unwrap(), payload);
    }

    #[test]
    fn redacts_non_string_values_and_whole_objects() {
        let payload =
            json!({"card": {"number": 4111, "cvv": 123}, "nested": {"deep": {"x": [1, 2]}}});
        let (redacted, removed, _) =
            redact_fields(&payload, &fields(&["card", "nested.deep.x"])).unwrap();
        assert_eq!(redacted["card"], json!("[ENCRYPTED]"));
        assert_eq!(redacted["nested"]["deep"]["x"], json!("[ENCRYPTED]"));
        assert!(!redacted.to_string().contains("4111"));
        assert_eq!(restore_fields(&redacted, &removed).unwrap(), payload);
    }

    #[test]
    fn overlapping_fields_restore_in_order() {
        let payload = json!({"user": {"ssn": "1", "name": "n"}});
        // Child first, then its parent: the parent's stored value contains the redacted child
        let (redacted, removed, found) =
            redact_fields(&payload, &fields(&["user.ssn", "user"])).unwrap();
        assert_eq!(found, fields(&["user.ssn", "user"]));
        assert_eq!(redacted, json!({"user": "[ENCRYPTED]"}));
        assert_eq!(restore_fields(&redacted, &removed).unwrap(), payload);

        // Parent first: the child is no longer reachable and is skipped
        let (redacted, removed, found) =
            redact_fields(&payload, &fields(&["user", "user.ssn"])).unwrap();
        assert_eq!(found, fields(&["user"]));
        assert_eq!(restore_fields(&redacted, &removed).unwrap(), payload);
    }

    #[test]
    fn literal_dotted_key_wins() {
        let payload = json!({"a.b": "literal", "a": {"b": "nested"}});
        let (redacted, _, _) = redact_fields(&payload, &fields(&["a.b"])).unwrap();
        assert_eq!(redacted["a.b"], json!("[ENCRYPTED]"));
        assert_eq!(redacted["a"]["b"], json!("nested"));
    }

    #[test]
    fn duplicate_fields_are_redacted_once() {
        let payload = json!({"ssn": "1"});
        let (_, removed, found) = redact_fields(&payload, &fields(&["ssn", "ssn"])).unwrap();
        assert_eq!(found, fields(&["ssn"]));
        assert_eq!(removed, json!([["ssn", "1"]]));
    }

    #[test]
    fn pii_fields_need_an_object_payload() {
        assert!(redact_fields(&json!([1, 2]), &fields(&["x"])).is_err());
        assert!(redact_fields(&json!("ssn"), &fields(&["x"])).is_err());
    }

    #[test]
    fn restore_rejects_malformed_data() {
        let redacted = json!({"ssn": "[ENCRYPTED]"});
        assert!(restore_fields(&redacted, &json!({"ssn": "1"})).is_err());
        assert!(restore_fields(&redacted, &json!([["ssn"]])).is_err());
        assert!(restore_fields(&redacted, &json!([["other", "1"]])).is_err());
    }

    #[tokio::test]
    async fn seal_and_open_whole_payload() {
        let engine = engine("k1").await;
        let payload = json!({"secret": "hunter2"});
        let mut job = Job::new("q".into(), payload.clone())
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM));
        seal_job(&engine, &mut job).await.unwrap();

        assert!(job.is_encrypted);
        assert_eq!(job.payload, encrypted_payload_placeholder());
        let encrypted = job.encrypted_payload.as_ref().unwrap();
        assert_eq!(encrypted.metadata.key_id, "k1");
        assert!(encrypted.metadata.payload_hash.starts_with("hmac-sha256:"));

        // Sealing again is a no-op
        let before = job.encrypted_payload.as_ref().unwrap().ciphertext.clone();
        seal_job(&engine, &mut job).await.unwrap();
        assert_eq!(job.encrypted_payload.as_ref().unwrap().ciphertext, before);

        let opened = open_job(&engine, job).await.unwrap();
        assert_eq!(opened.payload, payload);
        assert!(!opened.is_encrypted);
        assert!(opened.encrypted_payload.is_none());
    }

    #[tokio::test]
    async fn seal_and_open_pii_fields() {
        let engine = engine("k1").await;
        let payload = json!({"ssn": "123-45-6789", "amount": 5});
        let mut job = Job::new("q".into(), payload.clone())
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
            .with_pii_fields(vec!["ssn"]);
        seal_job(&engine, &mut job).await.unwrap();
        assert_eq!(job.payload, json!({"ssn": "[ENCRYPTED]", "amount": 5}));
        assert_eq!(
            job.encrypted_payload
                .as_ref()
                .unwrap()
                .metadata
                .encrypted_fields,
            fields(&["ssn"])
        );
        assert_eq!(open_job(&engine, job).await.unwrap().payload, payload);
    }

    #[tokio::test]
    async fn seal_rejects_mismatched_engine() {
        let engine = engine("k1").await;
        let mut job = Job::new("q".into(), json!({}))
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305));
        assert!(matches!(
            seal_job(&engine, &mut job).await,
            Err(EncryptionError::InvalidConfiguration(_))
        ));

        let mut job = Job::new("q".into(), json!({})).with_encryption(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM).with_key_id("other"),
        );
        assert!(matches!(
            seal_job(&engine, &mut job).await,
            Err(EncryptionError::InvalidConfiguration(_))
        ));
        assert!(!job.is_encrypted);
    }

    #[tokio::test]
    async fn open_fails_without_the_key() {
        let mut job = Job::new("q".into(), json!({"secret": 1}))
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM));
        seal_job(&engine("k1").await, &mut job).await.unwrap();

        // Another key id: the key is not found
        assert!(matches!(
            open_job(&engine("k2").await, job.clone()).await,
            Err(EncryptionError::KeyManagement(_))
        ));

        // Same key id, different key material: authentication fails
        let other = EncryptionEngine::new(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                .with_key_id("k1")
                .with_key_source(KeySource::Static(
                    "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=".to_string(),
                )),
        )
        .await
        .unwrap();
        assert!(matches!(
            open_job(&other, job.clone()).await,
            Err(EncryptionError::DecryptionFailed(_))
        ));

        // A decrypt-only key for the old id
        let rotated = EncryptionEngine::new(
            EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                .with_key_id("k2")
                .with_key_source(KeySource::Static(
                    "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=".to_string(),
                )),
        )
        .await
        .unwrap()
        .with_decryption_key("k1", &KeySource::Static(KEY.to_string()))
        .await
        .unwrap();
        assert_eq!(
            open_job(&rotated, job).await.unwrap().payload,
            json!({"secret": 1})
        );
    }

    #[tokio::test]
    async fn open_rejects_corrupted_nonce_without_panicking() {
        let engine = engine("k1").await;
        let mut job = Job::new("q".into(), json!({"secret": 1}))
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM));
        seal_job(&engine, &mut job).await.unwrap();
        job.encrypted_payload.as_mut().unwrap().nonce = "AAAA".to_string();
        assert!(open_job(&engine, job).await.is_err());
    }

    async fn sealed(engine: &EncryptionEngine, queue: &str, payload: Value) -> Job {
        let mut job = Job::new(queue.into(), payload)
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM));
        seal_job(engine, &mut job).await.unwrap();
        job
    }

    #[tokio::test]
    async fn sealed_jobs_are_bound_to_their_job() {
        let engine = engine("k1").await;
        let job = sealed(&engine, "q", json!({"secret": 1})).await;
        assert_eq!(
            job.encrypted_payload
                .as_ref()
                .unwrap()
                .metadata
                .format_version,
            EncryptionMetadata::FORMAT_JOB_BOUND
        );
        assert!(open_job(&engine, job).await.is_ok());
    }

    #[tokio::test]
    async fn swapped_ciphertexts_do_not_decrypt() {
        let engine = engine("k1").await;
        let mut a = sealed(&engine, "q", json!({"who": "a"})).await;
        let mut b = sealed(&engine, "q", json!({"who": "b"})).await;
        std::mem::swap(&mut a.encrypted_payload, &mut b.encrypted_payload);
        for job in [a, b] {
            assert!(matches!(
                open_job(&engine, job).await,
                Err(EncryptionError::DecryptionFailed(_))
            ));
        }
    }

    #[tokio::test]
    async fn changed_queue_or_fields_do_not_decrypt() {
        let engine = engine("k1").await;
        let mut job = sealed(&engine, "q", json!({"secret": 1})).await;
        job.queue_name = "other".into();
        assert!(open_job(&engine, job).await.is_err());

        let mut job = Job::new("q".into(), json!({"ssn": "1", "name": "n"}))
            .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
            .with_pii_fields(vec!["ssn"]);
        seal_job(&engine, &mut job).await.unwrap();
        let mut renamed = job.clone();
        renamed.pii_fields = fields(&["name"]);
        assert!(open_job(&engine, renamed).await.is_err());
        let mut whole = job.clone();
        whole.pii_fields.clear();
        assert!(open_job(&engine, whole).await.is_err());
        let mut listed = job.clone();
        listed
            .encrypted_payload
            .as_mut()
            .unwrap()
            .metadata
            .encrypted_fields = fields(&["name"]);
        assert!(open_job(&engine, listed).await.is_err());
        assert!(open_job(&engine, job).await.is_ok());
    }

    #[tokio::test]
    async fn format_downgrade_does_not_decrypt() {
        let engine = engine("k1").await;
        let mut job = sealed(&engine, "q", json!({"secret": 1})).await;
        job.encrypted_payload
            .as_mut()
            .unwrap()
            .metadata
            .format_version = EncryptionMetadata::FORMAT_UNBOUND;
        assert!(open_job(&engine, job).await.is_err());

        let mut job = sealed(&engine, "q", json!({"secret": 1})).await;
        job.encrypted_payload
            .as_mut()
            .unwrap()
            .metadata
            .format_version = 99;
        let err = open_job(&engine, job).await.unwrap_err();
        assert!(err.to_string().contains("unknown format"), "{err}");
    }

    #[tokio::test]
    async fn legacy_unbound_payloads_still_decrypt() {
        // What the previous release stored: no associated data, no format_version
        let engine = engine("k1").await;
        let payload = json!({"secret": "legacy"});
        let encrypted = engine
            .encrypt_payload(&payload, &[] as &[&str])
            .await
            .unwrap();
        let mut metadata = serde_json::to_value(&encrypted.metadata).unwrap();
        metadata.as_object_mut().unwrap().remove("format_version");
        assert_eq!(
            EncryptionMetadata::format_version_from_json(&metadata),
            EncryptionMetadata::FORMAT_UNBOUND
        );
        let legacy: crate::encryption::EncryptedPayload = serde_json::from_value(json!({
            "ciphertext": encrypted.ciphertext,
            "nonce": encrypted.nonce,
            "tag": encrypted.tag,
            "metadata": metadata,
        }))
        .unwrap();

        let mut job = Job::new("q".into(), encrypted_payload_placeholder());
        job.is_encrypted = true;
        job.encrypted_payload = Some(legacy);
        assert_eq!(open_job(&engine, job).await.unwrap().payload, payload);
    }

    #[test]
    fn associated_data_is_unambiguous() {
        let id = uuid::Uuid::new_v4();
        let alg = EncryptionAlgorithm::AES256GCM;
        let a = job_associated_data(&id, "ab", "c", &alg, &[], &[]);
        let b = job_associated_data(&id, "a", "bc", &alg, &[], &[]);
        assert_ne!(a, b);
        let c = job_associated_data(&id, "q", "k", &alg, &fields(&["x"]), &[]);
        let d = job_associated_data(&id, "q", "k", &alg, &[], &fields(&["x"]));
        assert_ne!(c, d);
        let e = job_associated_data(
            &id,
            "q",
            "k",
            &EncryptionAlgorithm::ChaCha20Poly1305,
            &[],
            &[],
        );
        assert_ne!(job_associated_data(&id, "q", "k", &alg, &[], &[]), e);
    }

    #[tokio::test]
    async fn plain_jobs_pass_through() {
        let engine = engine("k1").await;
        let mut job = Job::new("q".into(), json!({"a": 1})).with_pii_fields(vec!["a"]);
        seal_job(&engine, &mut job).await.unwrap();
        assert!(!job.is_encrypted);
        assert_eq!(job.payload, json!({"a": 1}));
        assert_eq!(
            open_job(&engine, job).await.unwrap().payload,
            json!({"a": 1})
        );
    }
}
