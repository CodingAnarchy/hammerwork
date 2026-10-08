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

use super::{EncryptionEngine, EncryptionError, RetentionPolicy};
use crate::job::Job;
use serde_json::{Map, Value};

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

    let (stored_payload, encrypted) = if job.pii_fields.is_empty() {
        let encrypted = engine
            .encrypt_payload_with_retention(&job.payload, &[] as &[&str], retention)
            .await?;
        (encrypted_payload_placeholder(), encrypted)
    } else {
        let (redacted, removed, found) = redact_fields(&job.payload, &job.pii_fields)?;
        let encrypted = engine
            .encrypt_payload_with_retention(&removed, &found, retention)
            .await?;
        (redacted, encrypted)
    };

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

    let decrypted = engine.decrypt_payload(&encrypted).await?;
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
