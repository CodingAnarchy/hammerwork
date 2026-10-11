//! The parts of reading and writing `hammerwork_jobs` rows that do not depend on the SQL
//! dialect, shared by the PostgreSQL and MySQL backends: the column lists, and decoding
//! the status, result configuration and encryption columns of a row into a [`Job`].
//!
//! Each backend's `JobRow` decodes the columns whose types differ between the databases
//! (ids, arrays) and hands the rest to these helpers.
//!
//! [`Job`]: crate::job::Job
#![cfg_attr(not(any(feature = "postgres", feature = "mysql")), allow(dead_code))]

use crate::{
    Result,
    job::{JobStatus, ResultStorage},
    workflow::DependencyStatus,
};
#[cfg(feature = "encryption")]
use chrono::{DateTime, Utc};

/// Standard field list for selecting complete job data from hammerwork_jobs table.
pub(crate) const JOB_SELECT_FIELDS: &str = "id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context, retry_strategy, is_encrypted, encryption_key_id, encryption_algorithm, encrypted_payload, encryption_nonce, encryption_tag, encryption_metadata, payload_hash, pii_fields, retention_policy, retention_delete_at, encrypted_at";

/// Columns of hammerwork_jobs_archive needed to rebuild a [`Job`](crate::job::Job).
pub(crate) const ARCHIVED_JOB_FIELDS: &str = "id, queue_name, payload, payload_compressed, status, priority, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, result, result_ttl, retry_strategy, timeout_seconds, cron_schedule, next_run_at, recurring, timezone, batch_id, depends_on, dependency_status, result_config, trace_id, correlation_id, parent_span_id, span_context, is_encrypted, pii_fields";

/// The columns written by each backend's `insert_jobs`, in bind order.
pub(crate) const INSERT_JOB_COLUMNS: &str = "id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_storage_type, result_ttl_seconds, result_max_size_bytes, retry_strategy, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context, is_encrypted, encryption_key_id, encryption_algorithm, encrypted_payload, encryption_nonce, encryption_tag, encryption_metadata, payload_hash, pii_fields, retention_policy, retention_delete_at, encrypted_at";

/// Encryption columns of hammerwork_jobs and hammerwork_jobs_archive (migration 011).
/// Archiving and restoring copy them between the tables in SQL, so encrypted payloads
/// move without being decrypted, with or without the `encryption` feature.
pub(crate) const ENCRYPTION_COLUMNS: [&str; 12] = [
    "is_encrypted",
    "encryption_key_id",
    "encryption_algorithm",
    "encrypted_payload",
    "encryption_nonce",
    "encryption_tag",
    "encryption_metadata",
    "payload_hash",
    "pii_fields",
    "retention_policy",
    "retention_delete_at",
    "encrypted_at",
];

/// The `result_storage_type` column value of `storage`.
pub(crate) fn result_storage_str(storage: &ResultStorage) -> &'static str {
    match storage {
        ResultStorage::Database => "database",
        ResultStorage::Memory => "memory",
        ResultStorage::None => "none",
    }
}

/// Decode a `result_storage_type` column; unknown values (and `NULL`) are no storage.
pub(crate) fn result_storage_from_db(value: Option<&str>) -> ResultStorage {
    match value {
        Some("database") => ResultStorage::Database,
        Some("memory") => ResultStorage::Memory,
        _ => ResultStorage::None,
    }
}

/// Decode a `status` column (old rows may hold it JSON-quoted); an unknown status is an
/// error.
pub(crate) fn job_status_from_row(value: &str) -> Result<JobStatus> {
    super::lifecycle::job_status_from_db(value).ok_or_else(|| {
        crate::error::HammerworkError::Processing(format!(
            "Unknown job status: {}",
            value.trim_matches('"')
        ))
    })
}

/// Decode a `dependency_status` column; `NULL` is [`DependencyStatus::None`].
pub(crate) fn dependency_status_from_row(value: Option<&str>) -> Result<DependencyStatus> {
    Ok(value
        .map(DependencyStatus::parse_from_db)
        .transpose()?
        .unwrap_or(DependencyStatus::None))
}

/// The encryption columns of a `hammerwork_jobs` row, borrowed from a backend's
/// `JobRow` (with `pii_fields` already decoded).
#[cfg(feature = "encryption")]
pub(crate) struct EncryptionRow<'a> {
    pub is_encrypted: bool,
    pub created_at: DateTime<Utc>,
    pub encryption_key_id: Option<&'a str>,
    pub encryption_algorithm: Option<&'a str>,
    pub encrypted_payload: Option<&'a [u8]>,
    pub encryption_nonce: Option<&'a [u8]>,
    pub encryption_tag: Option<&'a [u8]>,
    pub encryption_metadata: Option<&'a serde_json::Value>,
    pub payload_hash: Option<&'a str>,
    pub pii_fields: &'a [String],
    pub retention_policy: Option<&'a str>,
    pub retention_delete_at: Option<DateTime<Utc>>,
    pub encrypted_at: Option<DateTime<Utc>>,
}

/// The encryption fields of a [`Job`](crate::job::Job): its encryption configuration,
/// retention policy and encrypted payload.
#[cfg(feature = "encryption")]
pub(crate) type DecodedEncryption = (
    Option<crate::encryption::EncryptionConfig>,
    Option<crate::encryption::RetentionPolicy>,
    Option<crate::encryption::EncryptedPayload>,
);

#[cfg(feature = "encryption")]
impl EncryptionRow<'_> {
    /// Decode the encryption configuration, retention policy and encrypted payload.
    pub(crate) fn decode(&self) -> Result<DecodedEncryption> {
        Ok((
            self.encryption_config()?,
            self.retention_policy()?,
            self.encrypted_payload()?,
        ))
    }

    /// Builds an EncryptionConfig from database fields if encryption is enabled.
    fn encryption_config(&self) -> Result<Option<crate::encryption::EncryptionConfig>> {
        if !self.is_encrypted {
            return Ok(None);
        }

        // Parse algorithm
        let algorithm = match self.encryption_algorithm {
            Some("AES256GCM") => crate::encryption::EncryptionAlgorithm::AES256GCM,
            Some("ChaCha20Poly1305") => crate::encryption::EncryptionAlgorithm::ChaCha20Poly1305,
            Some(alg) => {
                return Err(crate::HammerworkError::Processing(format!(
                    "Unknown encryption algorithm: {}",
                    alg
                )));
            }
            None => {
                return Err(crate::HammerworkError::Processing(
                    "Missing encryption algorithm for encrypted job".to_string(),
                ));
            }
        };

        // Parse metadata if available
        let (key_id, compression_enabled, version) =
            if let Some(metadata) = self.encryption_metadata {
                let key_id = metadata
                    .get("key_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let compression_enabled = metadata
                    .get("compressed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let version = metadata
                    .get("config_version")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(1) as u32;
                (key_id, compression_enabled, version)
            } else {
                (self.encryption_key_id.map(str::to_string), false, 1)
            };

        let config = crate::encryption::EncryptionConfig {
            algorithm,
            key_source: crate::encryption::KeySource::External(
                key_id.unwrap_or_else(|| "unknown".to_string()),
            ),
            key_rotation_enabled: false, // Not stored in database
            key_rotation_interval: None,
            default_retention: None, // Will be parsed separately
            compression_enabled,
            key_id: self.encryption_key_id.map(str::to_string),
            version,
        };

        Ok(Some(config))
    }

    /// Parses the retention policy from the database string.
    fn retention_policy(&self) -> Result<Option<crate::encryption::RetentionPolicy>> {
        match self.retention_policy {
            None => Ok(None),
            Some("KeepIndefinitely") => {
                Ok(Some(crate::encryption::RetentionPolicy::KeepIndefinitely))
            }
            Some("DeleteImmediately") => {
                Ok(Some(crate::encryption::RetentionPolicy::DeleteImmediately))
            }
            Some("UseDefault") => Ok(Some(crate::encryption::RetentionPolicy::UseDefault)),
            Some(policy_str) => {
                // Handle DeleteAfter and DeleteAt policies
                if let Some(delete_at) = self.retention_delete_at {
                    if policy_str == "DeleteAt" {
                        Ok(Some(crate::encryption::RetentionPolicy::DeleteAt(
                            delete_at,
                        )))
                    } else if policy_str == "DeleteAfter" {
                        // Calculate duration from creation to deletion time
                        let duration = delete_at.signed_duration_since(self.created_at);
                        if let Ok(std_duration) = duration.to_std() {
                            Ok(Some(crate::encryption::RetentionPolicy::DeleteAfter(
                                std_duration,
                            )))
                        } else {
                            Ok(Some(crate::encryption::RetentionPolicy::UseDefault))
                        }
                    } else {
                        Err(crate::HammerworkError::Processing(format!(
                            "Unknown retention policy: {}",
                            policy_str
                        )))
                    }
                } else {
                    Err(crate::HammerworkError::Processing(format!(
                        "Retention policy '{}' requires retention_delete_at timestamp",
                        policy_str
                    )))
                }
            }
        }
    }

    /// Builds an EncryptedPayload from database fields if the job is encrypted.
    fn encrypted_payload(&self) -> Result<Option<crate::encryption::EncryptedPayload>> {
        if !self.is_encrypted {
            return Ok(None);
        }

        let encrypted_data = self.encrypted_payload.ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encrypted payload for encrypted job".to_string(),
            )
        })?;

        let nonce = self.encryption_nonce.ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encryption nonce for encrypted job".to_string(),
            )
        })?;

        let tag = self.encryption_tag.ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encryption tag for encrypted job".to_string(),
            )
        })?;

        // An unknown algorithm falls back to AES-256-GCM here.
        let algorithm = match self.encryption_algorithm {
            Some("ChaCha20Poly1305") => crate::encryption::EncryptionAlgorithm::ChaCha20Poly1305,
            _ => crate::encryption::EncryptionAlgorithm::AES256GCM,
        };
        let retention_policy = self
            .retention_policy()?
            .unwrap_or(crate::encryption::RetentionPolicy::UseDefault);
        let encrypted_at = self.encrypted_at.unwrap_or(self.created_at);
        let payload_hash = self.payload_hash.unwrap_or_default().to_string();

        let metadata = if let Some(metadata_json) = self.encryption_metadata {
            // Build metadata from JSON
            let key_id = metadata_json
                .get("key_id")
                .and_then(|v| v.as_str())
                .or(self.encryption_key_id)
                .unwrap_or("unknown")
                .to_string();

            let config_version = metadata_json
                .get("config_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;

            let compressed = metadata_json
                .get("compressed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let encrypted_fields = metadata_json
                .get("encrypted_fields")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_else(|| self.pii_fields.to_vec());

            crate::encryption::EncryptionMetadata {
                algorithm,
                key_id,
                config_version,
                compressed,
                encrypted_fields,
                retention_policy,
                encrypted_at,
                delete_at: self.retention_delete_at,
                payload_hash,
                format_version: crate::encryption::EncryptionMetadata::format_version_from_json(
                    metadata_json,
                ),
            }
        } else {
            // Build basic metadata from available fields
            crate::encryption::EncryptionMetadata {
                algorithm,
                key_id: self.encryption_key_id.unwrap_or("unknown").to_string(),
                config_version: 1,
                compressed: false,
                encrypted_fields: self.pii_fields.to_vec(),
                retention_policy,
                encrypted_at,
                delete_at: self.retention_delete_at,
                payload_hash,
                format_version: crate::encryption::EncryptionMetadata::FORMAT_UNBOUND,
            }
        };

        // Encode binary data to base64
        use base64::Engine;
        let engine = base64::engine::general_purpose::STANDARD;
        Ok(Some(crate::encryption::EncryptedPayload {
            ciphertext: engine.encode(encrypted_data),
            nonce: engine.encode(nonce),
            tag: engine.encode(tag),
            metadata,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_storage_round_trips_and_defaults_to_none() {
        for storage in [
            ResultStorage::Database,
            ResultStorage::Memory,
            ResultStorage::None,
        ] {
            let decoded = result_storage_from_db(Some(result_storage_str(&storage)));
            assert_eq!(result_storage_str(&decoded), result_storage_str(&storage));
        }
        assert!(matches!(
            result_storage_from_db(Some("bogus")),
            ResultStorage::None
        ));
        assert!(matches!(result_storage_from_db(None), ResultStorage::None));
    }

    #[test]
    fn job_status_accepts_quoted_values_and_rejects_unknown_ones() {
        assert_eq!(job_status_from_row("Running").unwrap(), JobStatus::Running);
        assert_eq!(job_status_from_row("\"Dead\"").unwrap(), JobStatus::Dead);
        let error = job_status_from_row("\"Bogus\"").unwrap_err().to_string();
        assert!(error.contains("Unknown job status: Bogus"), "{error}");
    }

    #[test]
    fn dependency_status_defaults_to_none() {
        assert_eq!(
            dependency_status_from_row(None).unwrap(),
            DependencyStatus::None
        );
        assert!(dependency_status_from_row(Some("bogus")).is_err());
    }
}
