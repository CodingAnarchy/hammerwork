//! MySQL implementation of the job queue.
//!
//! This module provides the MySQL-specific implementation of the `DatabaseQueue` trait,
//! optimized for MySQL's JSON support and transactional capabilities.

use super::lifecycle::{self, Guard, ParentState, Target, TransitionResult};
use super::{
    DatabaseQueue, DeadJobSummary, JobOutcome, JobTransition, QueueStats, RecordedOutcome,
};
use crate::{
    Result,
    batch::{BatchProgress, BatchStatus, PartialFailureMode},
    job::{Job, JobId, JobStatus},
    priority::JobPriority,
    rate_limit::ThrottleConfig,
    workflow::{FailurePolicy, WorkflowProgress, WorkflowStatus},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, MySql, Row};
use std::{collections::HashMap, time::Duration};

#[derive(FromRow, Clone)]
pub(crate) struct JobRow {
    pub id: String, // MySQL uses CHAR(36) for UUID
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i32,
    pub attempts: i32,
    pub max_attempts: i32,
    pub timeout_seconds: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub timed_out_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
    pub cron_schedule: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub recurring: bool,
    pub timezone: Option<String>,
    pub batch_id: Option<String>,
    pub result_data: Option<serde_json::Value>,
    pub result_stored_at: Option<DateTime<Utc>>,
    pub result_expires_at: Option<DateTime<Utc>>,
    pub result_storage_type: Option<String>,
    pub result_ttl_seconds: Option<i64>,
    pub result_max_size_bytes: Option<i64>,
    pub depends_on: Option<serde_json::Value>,
    pub dependents: Option<serde_json::Value>,
    pub dependency_status: Option<String>,
    pub workflow_id: Option<String>,
    pub workflow_name: Option<String>,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub span_context: Option<String>,
    pub retry_strategy: Option<serde_json::Value>,
    // Encryption fields (written by `insert_jobs`, see `JobQueue::with_encryption`).
    // Only `is_encrypted` and `pii_fields` are mapped into `Job` unconditionally; the
    // rest are decoded only with the `encryption` feature.
    pub is_encrypted: bool,
    #[cfg(feature = "encryption")]
    pub encryption_key_id: Option<String>,
    #[cfg(feature = "encryption")]
    pub encryption_algorithm: Option<String>,
    #[cfg(feature = "encryption")]
    pub encrypted_payload: Option<Vec<u8>>,
    #[cfg(feature = "encryption")]
    pub encryption_nonce: Option<Vec<u8>>,
    #[cfg(feature = "encryption")]
    pub encryption_tag: Option<Vec<u8>>,
    #[cfg(feature = "encryption")]
    pub encryption_metadata: Option<serde_json::Value>,
    #[cfg(feature = "encryption")]
    pub payload_hash: Option<String>,
    pub pii_fields: Option<serde_json::Value>,
    #[cfg(feature = "encryption")]
    pub retention_policy: Option<String>,
    #[cfg(feature = "encryption")]
    pub retention_delete_at: Option<DateTime<Utc>>,
    #[cfg(feature = "encryption")]
    pub encrypted_at: Option<DateTime<Utc>>,
}

/// Standard field list for selecting complete job data from hammerwork_jobs table.
const JOB_SELECT_FIELDS: &str = "id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context, retry_strategy, is_encrypted, encryption_key_id, encryption_algorithm, encrypted_payload, encryption_nonce, encryption_tag, encryption_metadata, payload_hash, pii_fields, retention_policy, retention_delete_at, encrypted_at";

/// Columns of hammerwork_jobs_archive needed to rebuild a [`Job`].
const ARCHIVED_JOB_FIELDS: &str = "id, queue_name, payload, payload_compressed, status, priority, attempts, max_attempts, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, result, result_ttl, retry_strategy, timeout_seconds, cron_schedule, next_run_at, recurring, timezone, batch_id, depends_on, dependency_status, result_config, trace_id, correlation_id, parent_span_id, span_context, is_encrypted, pii_fields";

/// Encryption columns of hammerwork_jobs and hammerwork_jobs_archive (migration 011).
/// Archiving and restoring copy them between the tables in SQL, so encrypted payloads
/// move without being decrypted, with or without the `encryption` feature.
const ENCRYPTION_COLUMNS: [&str; 12] = [
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

/// Copies the encryption columns of job `id` from table `from` to table `to`.
async fn copy_encryption_columns(
    conn: &mut sqlx::MySqlConnection,
    from: &str,
    to: &str,
    id: JobId,
) -> Result<()> {
    let assignments = ENCRYPTION_COLUMNS
        .iter()
        .map(|column| format!("dst.{column} = src.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    sqlx::query(&format!(
        "UPDATE {to} dst JOIN {from} src ON dst.id = src.id SET {assignments} WHERE src.id = ?"
    ))
    .bind(id.to_string())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Rebuilds a job from a hammerwork_jobs_archive row selected with [`ARCHIVED_JOB_FIELDS`].
///
/// The job keeps the data it had when it was archived, with status [`JobStatus::Archived`].
fn archived_job_from_row(row: &sqlx::mysql::MySqlRow) -> Result<Job> {
    let payload = crate::archive::decode_archived_payload(
        &row.try_get::<Vec<u8>, _>("payload")?,
        row.try_get("payload_compressed")?,
    )?;

    Ok(Job {
        id: uuid::Uuid::parse_str(&row.try_get::<String, _>("id")?)?,
        queue_name: row.try_get("queue_name")?,
        payload,
        status: JobStatus::Archived,
        priority: row
            .try_get::<String, _>("priority")?
            .parse()
            .unwrap_or(JobPriority::Normal),
        attempts: row.try_get("attempts")?,
        max_attempts: row.try_get("max_attempts")?,
        created_at: row.try_get("created_at")?,
        scheduled_at: row.try_get("scheduled_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        failed_at: row.try_get("failed_at")?,
        timed_out_at: row.try_get("timed_out_at")?,
        timeout: row
            .try_get::<Option<i32>, _>("timeout_seconds")?
            .map(|s| super::db_seconds(s, "timeout_seconds"))
            .transpose()?,
        error_message: row.try_get("error_message")?,
        cron_schedule: row.try_get("cron_schedule")?,
        next_run_at: row.try_get("next_run_at")?,
        recurring: row.try_get("recurring")?,
        timezone: row.try_get("timezone")?,
        batch_id: row
            .try_get::<Option<String>, _>("batch_id")?
            .map(|s| uuid::Uuid::parse_str(&s))
            .transpose()?,
        result_config: row
            .try_get::<Option<serde_json::Value>, _>("result_config")?
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default(),
        result_data: row.try_get("result")?,
        result_stored_at: None,
        result_expires_at: row.try_get("result_ttl")?,
        retry_strategy: row
            .try_get::<Option<String>, _>("retry_strategy")?
            .map(|s| serde_json::from_str(&s))
            .transpose()?,
        depends_on: json_list(row.try_get("depends_on")?, "depends_on")?,
        dependents: Vec::new(),
        dependency_status: crate::archive::parse_archived_dependency_status(
            row.try_get::<Option<String>, _>("dependency_status")?
                .as_deref(),
        ),
        workflow_id: None,
        workflow_name: None,
        trace_id: row.try_get("trace_id")?,
        correlation_id: row.try_get("correlation_id")?,
        parent_span_id: row.try_get("parent_span_id")?,
        span_context: row.try_get("span_context")?,
        #[cfg(feature = "encryption")]
        encryption_config: None,
        pii_fields: json_list(row.try_get("pii_fields")?, "pii_fields")?,
        #[cfg(feature = "encryption")]
        retention_policy: None,
        // The ciphertext stays in the archive row; restore the job to decrypt it
        is_encrypted: row.try_get("is_encrypted")?,
        #[cfg(feature = "encryption")]
        encrypted_payload: None,
    })
}

impl JobRow {
    pub fn into_job(self) -> Result<Job> {
        // Extract encryption data before moving self
        #[cfg(feature = "encryption")]
        let encryption_config = self.build_encryption_config()?;
        #[cfg(feature = "encryption")]
        let retention_policy = self.parse_retention_policy()?;
        #[cfg(feature = "encryption")]
        let encrypted_payload = self.build_encrypted_payload()?;
        let pii_fields = json_list(self.pii_fields.clone(), "pii_fields")?;

        Ok(Job {
            id: uuid::Uuid::parse_str(&self.id)?,
            queue_name: self.queue_name,
            payload: self.payload,
            status: {
                // Handle both quoted (old format) and unquoted (new format) status values
                let cleaned_str = self.status.trim_matches('"');
                match cleaned_str {
                    "Pending" => JobStatus::Pending,
                    "Running" => JobStatus::Running,
                    "Completed" => JobStatus::Completed,
                    "Failed" => JobStatus::Failed,
                    "Dead" => JobStatus::Dead,
                    "TimedOut" => JobStatus::TimedOut,
                    "Retrying" => JobStatus::Retrying,
                    "Archived" => JobStatus::Archived,
                    _ => {
                        return Err(crate::error::HammerworkError::Processing(format!(
                            "Unknown job status: {}",
                            cleaned_str
                        )));
                    }
                }
            },
            priority: JobPriority::from_i32(self.priority).unwrap_or(JobPriority::Normal),
            attempts: self.attempts,
            max_attempts: self.max_attempts,
            created_at: self.created_at,
            scheduled_at: self.scheduled_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
            failed_at: self.failed_at,
            timed_out_at: self.timed_out_at,
            timeout: self
                .timeout_seconds
                .map(|s| super::db_seconds(s, "timeout_seconds"))
                .transpose()?,
            error_message: self.error_message,
            cron_schedule: self.cron_schedule,
            next_run_at: self.next_run_at,
            recurring: self.recurring,
            timezone: self.timezone,
            batch_id: self
                .batch_id
                .map(|s| uuid::Uuid::parse_str(&s))
                .transpose()?,
            result_config: crate::job::ResultConfig {
                storage: self
                    .result_storage_type
                    .as_ref()
                    .map(|s| match s.as_str() {
                        "database" => crate::job::ResultStorage::Database,
                        "memory" => crate::job::ResultStorage::Memory,
                        "none" => crate::job::ResultStorage::None,
                        _ => crate::job::ResultStorage::None,
                    })
                    .unwrap_or(crate::job::ResultStorage::None),
                ttl: self
                    .result_ttl_seconds
                    .map(|s| super::db_seconds(s, "result_ttl_seconds"))
                    .transpose()?,
                max_size_bytes: self
                    .result_max_size_bytes
                    .map(|b| super::db_int(b, "result_max_size_bytes"))
                    .transpose()?,
            },
            result_data: self.result_data,
            result_stored_at: self.result_stored_at,
            result_expires_at: self.result_expires_at,
            retry_strategy: super::retry_strategy_from_json(self.retry_strategy.clone())?,
            depends_on: json_list(self.depends_on, "depends_on")?,
            dependents: json_list(self.dependents, "dependents")?,
            dependency_status: self
                .dependency_status
                .as_deref()
                .map(crate::workflow::DependencyStatus::parse_from_db)
                .transpose()?
                .unwrap_or(crate::workflow::DependencyStatus::None),
            workflow_id: self
                .workflow_id
                .map(|s| uuid::Uuid::parse_str(&s))
                .transpose()?,
            workflow_name: self.workflow_name,
            trace_id: self.trace_id,
            correlation_id: self.correlation_id,
            parent_span_id: self.parent_span_id,
            span_context: self.span_context,
            #[cfg(feature = "encryption")]
            encryption_config,
            pii_fields,
            #[cfg(feature = "encryption")]
            retention_policy,
            is_encrypted: self.is_encrypted,
            #[cfg(feature = "encryption")]
            encrypted_payload,
        })
    }

    /// Parses PII fields from JSON value.
    #[cfg(feature = "encryption")]
    fn parse_pii_fields(&self) -> Vec<String> {
        self.pii_fields
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Builds an EncryptionConfig from database fields if encryption is enabled.
    #[cfg(feature = "encryption")]
    fn build_encryption_config(&self) -> Result<Option<crate::encryption::EncryptionConfig>> {
        if !self.is_encrypted {
            return Ok(None);
        }

        // Parse algorithm
        let algorithm = match self.encryption_algorithm.as_deref() {
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
            if let Some(metadata) = &self.encryption_metadata {
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
                (self.encryption_key_id.clone(), false, 1)
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
            key_id: self.encryption_key_id.clone(),
            version,
        };

        Ok(Some(config))
    }

    /// Parses the retention policy from the database string.
    #[cfg(feature = "encryption")]
    fn parse_retention_policy(&self) -> Result<Option<crate::encryption::RetentionPolicy>> {
        match self.retention_policy.as_deref() {
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
    #[cfg(feature = "encryption")]
    fn build_encrypted_payload(&self) -> Result<Option<crate::encryption::EncryptedPayload>> {
        if !self.is_encrypted {
            return Ok(None);
        }

        let encrypted_data = self.encrypted_payload.as_ref().ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encrypted payload for encrypted job".to_string(),
            )
        })?;

        let nonce = self.encryption_nonce.as_ref().ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encryption nonce for encrypted job".to_string(),
            )
        })?;

        let tag = self.encryption_tag.as_ref().ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encryption tag for encrypted job".to_string(),
            )
        })?;

        // Parse metadata
        let metadata = if let Some(metadata_json) = &self.encryption_metadata {
            // Build metadata from JSON
            let algorithm = match self.encryption_algorithm.as_deref() {
                Some("AES256GCM") => crate::encryption::EncryptionAlgorithm::AES256GCM,
                Some("ChaCha20Poly1305") => {
                    crate::encryption::EncryptionAlgorithm::ChaCha20Poly1305
                }
                _ => crate::encryption::EncryptionAlgorithm::AES256GCM, // Default fallback
            };

            let key_id = metadata_json
                .get("key_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| self.encryption_key_id.clone())
                .unwrap_or_else(|| "unknown".to_string());

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
                .unwrap_or_else(|| self.parse_pii_fields());

            let retention_policy = self
                .parse_retention_policy()?
                .unwrap_or(crate::encryption::RetentionPolicy::UseDefault);

            let encrypted_at = self.encrypted_at.unwrap_or(self.created_at);

            crate::encryption::EncryptionMetadata {
                algorithm,
                key_id,
                config_version,
                compressed,
                encrypted_fields,
                retention_policy,
                encrypted_at,
                delete_at: self.retention_delete_at,
                payload_hash: self.payload_hash.clone().unwrap_or_default(),
                format_version: crate::encryption::EncryptionMetadata::format_version_from_json(
                    metadata_json,
                ),
            }
        } else {
            // Build basic metadata from available fields
            let algorithm = match self.encryption_algorithm.as_deref() {
                Some("AES256GCM") => crate::encryption::EncryptionAlgorithm::AES256GCM,
                Some("ChaCha20Poly1305") => {
                    crate::encryption::EncryptionAlgorithm::ChaCha20Poly1305
                }
                _ => crate::encryption::EncryptionAlgorithm::AES256GCM, // Default fallback
            };

            crate::encryption::EncryptionMetadata {
                algorithm,
                key_id: self
                    .encryption_key_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
                config_version: 1,
                compressed: false,
                encrypted_fields: self.parse_pii_fields(),
                retention_policy: self
                    .parse_retention_policy()?
                    .unwrap_or(crate::encryption::RetentionPolicy::UseDefault),
                encrypted_at: self.encrypted_at.unwrap_or(self.created_at),
                delete_at: self.retention_delete_at,
                payload_hash: self.payload_hash.clone().unwrap_or_default(),
                format_version: crate::encryption::EncryptionMetadata::FORMAT_UNBOUND,
            }
        };

        // Encode binary data to base64
        use base64::Engine;
        let ciphertext = base64::engine::general_purpose::STANDARD.encode(encrypted_data);
        let nonce_b64 = base64::engine::general_purpose::STANDARD.encode(nonce);
        let tag_b64 = base64::engine::general_purpose::STANDARD.encode(tag);

        Ok(Some(crate::encryption::EncryptedPayload {
            ciphertext,
            nonce: nonce_b64,
            tag: tag_b64,
            metadata,
        }))
    }
}

#[derive(FromRow)]
pub(crate) struct DeadJobRow {
    pub id: String,
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub status: String,
    pub priority: i32,
    pub attempts: i32,
    pub max_attempts: i32,
    pub timeout_seconds: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_at: Option<DateTime<Utc>>,
    pub timed_out_at: Option<DateTime<Utc>>,
    pub error_message: Option<String>,
}

impl DeadJobRow {
    pub fn into_job(self) -> Result<Job> {
        Ok(Job {
            id: uuid::Uuid::parse_str(&self.id)?,
            queue_name: self.queue_name,
            payload: self.payload,
            status: {
                // Handle both quoted (old format) and unquoted (new format) status values
                let cleaned_str = self.status.trim_matches('"');
                match cleaned_str {
                    "Pending" => JobStatus::Pending,
                    "Running" => JobStatus::Running,
                    "Completed" => JobStatus::Completed,
                    "Failed" => JobStatus::Failed,
                    "Dead" => JobStatus::Dead,
                    "TimedOut" => JobStatus::TimedOut,
                    "Retrying" => JobStatus::Retrying,
                    "Archived" => JobStatus::Archived,
                    _ => JobStatus::Dead, // Default fallback for unknown status
                }
            },
            priority: JobPriority::from_i32(self.priority).unwrap_or(JobPriority::Normal),
            attempts: self.attempts,
            max_attempts: self.max_attempts,
            created_at: self.created_at,
            scheduled_at: self.scheduled_at,
            started_at: self.started_at,
            completed_at: self.completed_at,
            failed_at: self.failed_at,
            timed_out_at: self.timed_out_at,
            timeout: self
                .timeout_seconds
                .map(|s| super::db_seconds(s, "timeout_seconds"))
                .transpose()?,
            error_message: self.error_message,
            cron_schedule: None,
            next_run_at: None,
            recurring: false,
            timezone: None,
            batch_id: None,
            result_config: crate::job::ResultConfig::default(),
            result_data: None,
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: None,
            depends_on: Vec::new(),
            dependents: Vec::new(),
            dependency_status: crate::workflow::DependencyStatus::None,
            workflow_id: None,
            workflow_name: None,
            trace_id: None,
            correlation_id: None,
            parent_span_id: None,
            span_context: None,
            #[cfg(feature = "encryption")]
            encryption_config: None,
            pii_fields: Vec::new(),
            #[cfg(feature = "encryption")]
            retention_policy: None,
            is_encrypted: false,
            #[cfg(feature = "encryption")]
            encrypted_payload: None,
        })
    }
}

/// Number of attempts for a job claim that keeps hitting InnoDB deadlocks.
const CLAIM_DEADLOCK_ATTEMPTS: u32 = 5;

/// Whether an error is an InnoDB deadlock (SQLSTATE 40001, error 1213), which is safe to retry.
fn is_deadlock(error: &crate::HammerworkError) -> bool {
    matches!(
        error,
        crate::HammerworkError::Database(sqlx::Error::Database(db_error))
            if db_error.code().as_deref() == Some("40001")
    )
}

/// Whether MySQL refused to change transaction characteristics because the session is
/// still inside a transaction (ER_CANT_CHANGE_TX_CHARACTERISTICS, 1568 / SQLSTATE 25001).
fn is_open_transaction_error(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db_error) if db_error.code().as_deref() == Some("25001"))
}

/// Runs a job claim, retrying with a short backoff when InnoDB aborts it as a deadlock
/// victim. Concurrent `SELECT ... FOR UPDATE SKIP LOCKED` + `UPDATE` claims can still
/// deadlock on secondary index locks; the aborted transaction has been rolled back, so
/// retrying is safe.
async fn retry_on_deadlock<F, Fut, T>(mut claim: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 1;
    loop {
        match claim().await {
            Err(error) if attempt < CLAIM_DEADLOCK_ATTEMPTS && is_deadlock(&error) => {
                tokio::time::sleep(Duration::from_millis(5 * u64::from(attempt))).await;
                attempt += 1;
            }
            result => return result,
        }
    }
}

impl crate::queue::JobQueue<MySql> {
    /// Starts the transaction used to claim jobs.
    ///
    /// It runs at READ COMMITTED: under the default REPEATABLE READ, the claim's locking
    /// read takes next-key (gap) locks that make concurrent workers deadlock far more often.
    async fn begin_claim_transaction(
        conn: &mut sqlx::pool::PoolConnection<MySql>,
    ) -> Result<sqlx::Transaction<'_, MySql>> {
        const SET_READ_COMMITTED: &str = "SET TRANSACTION ISOLATION LEVEL READ COMMITTED";
        if let Err(error) = sqlx::query(SET_READ_COMMITTED).execute(&mut **conn).await {
            // ER_CANT_CHANGE_TX_CHARACTERISTICS (1568): the pooled connection is still
            // inside a transaction that was abandoned without COMMIT/ROLLBACK (e.g. a
            // future dropped mid-transaction). Roll it back so its row locks are released,
            // then continue.
            if !is_open_transaction_error(&error) {
                return Err(error.into());
            }
            tracing::warn!(
                "pooled MySQL connection had an abandoned open transaction; rolling it back"
            );
            sqlx::Executor::execute(&mut **conn, "ROLLBACK").await?;
            sqlx::query(SET_READ_COMMITTED).execute(&mut **conn).await?;
        }
        Ok(sqlx::Connection::begin(&mut **conn).await?)
    }

    /// One attempt at claiming the next runnable job of `queue_name` (of `priority`
    /// when given), highest priority and oldest first.
    ///
    /// Lock and claim happen in one transaction. SKIP LOCKED (MySQL 8.0+) lets
    /// concurrent workers move past rows another worker is claiming instead of
    /// blocking on them. The claim time comes from the database clock.
    async fn claim_attempt(
        &self,
        queue_name: &str,
        priority: Option<JobPriority>,
    ) -> Result<Option<Job>> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = Self::begin_claim_transaction(&mut conn).await?;
        let result = async {
            let query = format!(
                "SELECT {JOB_SELECT_FIELDS}, UTC_TIMESTAMP(6) AS claimed_at \
                 FROM hammerwork_jobs j WHERE {RUNNABLE_IN_QUEUE_SQL}{} \
                 ORDER BY j.priority DESC, j.scheduled_at ASC LIMIT 1 FOR UPDATE SKIP LOCKED",
                if priority.is_some() {
                    " AND j.priority = ?"
                } else {
                    ""
                }
            );
            let mut select = sqlx::query(&query).bind(queue_name).bind(queue_name);
            if let Some(priority) = priority {
                select = select.bind(priority.as_i32());
            }
            let Some(row) = select.fetch_optional(&mut *tx).await? else {
                return Ok(None);
            };
            let job_row = JobRow::from_row(&row)?;
            let started_at: DateTime<Utc> = row.try_get("claimed_at")?;

            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, started_at = ?, attempts = attempts + 1 \
                 WHERE id = ?",
            )
            .bind(JobStatus::Running)
            .bind(started_at)
            .bind(&job_row.id)
            .execute(&mut *tx)
            .await?;

            let mut job = job_row.into_job()?;
            job.status = JobStatus::Running;
            job.attempts += 1;
            job.started_at = Some(started_at);
            Ok(Some(job))
        }
        .await;
        super::end_transaction(tx, result).await
    }

    async fn dequeue_with_priority_weights_inner(
        &self,
        queue_name: &str,
        weights: &crate::priority::PriorityWeights,
    ) -> Result<Option<Job>> {
        // Which priority levels have a runnable job: one index probe per level, so
        // every level is considered however many jobs of other levels are queued.
        let levels: Vec<i64> =
            sqlx::query_scalar(&super::runnable_priorities_sql(RUNNABLE_IN_QUEUE_SQL))
                .bind(queue_name)
                .bind(queue_name)
                .fetch_all(&self.pool)
                .await?;
        let mut candidates: Vec<JobPriority> = levels
            .into_iter()
            .filter_map(|level| i32::try_from(level).ok())
            .filter_map(|level| JobPriority::from_i32(level).ok())
            .collect();

        // Pick a level by weight and claim its oldest runnable job. If other workers
        // hold all of that level's jobs (SKIP LOCKED found none), try another level.
        let mut attempt = 0;
        while let Some(priority) = super::pick_weighted_priority(
            &candidates,
            weights,
            super::weighted_seed(queue_name, attempt),
        ) {
            if let Some(job) =
                retry_on_deadlock(|| self.claim_attempt(queue_name, Some(priority))).await?
            {
                return Ok(Some(job));
            }
            candidates.retain(|candidate| *candidate != priority);
            attempt += 1;
        }
        Ok(None)
    }
}

#[async_trait]
impl DatabaseQueue for crate::queue::JobQueue<MySql> {
    type Database = MySql;

    async fn enqueue(&self, mut job: Job) -> Result<JobId> {
        self.seal_jobs(std::slice::from_mut(&mut job)).await?;
        if job.depends_on.is_empty() {
            let mut conn = self.pool.acquire().await?;
            insert_jobs(&mut conn, std::slice::from_ref(&job)).await?;
            return Ok(job.id);
        }
        // A job with dependencies is inserted together with a check of its
        // dependencies' current state (see `insert_dependent_jobs`).
        super::retry_on_conflict(|| async {
            let mut conn = self.pool.acquire().await?;
            let mut tx = Self::begin_claim_transaction(&mut conn).await?;
            let mut jobs = vec![job.clone()];
            let result = insert_dependent_jobs(&mut tx, &mut jobs).await;
            super::end_transaction(tx, result).await
        })
        .await?;
        Ok(job.id)
    }

    async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
        retry_on_deadlock(|| self.claim_attempt(queue_name, None)).await
    }

    async fn dequeue_with_priority_weights(
        &self,
        queue_name: &str,
        weights: &crate::priority::PriorityWeights,
    ) -> Result<Option<Job>> {
        if weights.is_strict() {
            return self.dequeue(queue_name).await;
        }
        self.dequeue_with_priority_weights_inner(queue_name, weights)
            .await
    }

    async fn complete_job(&self, job_id: JobId) -> Result<()> {
        let transition = JobTransition::Complete;
        self.transition_job(job_id, Guard::Manual(transition), &Target::Completed, false)
            .await?
            .into_manual(job_id, transition)?;
        Ok(())
    }

    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
        let transition = JobTransition::Fail;
        let target = Target::Failed(error_message.to_string());
        self.transition_job(job_id, Guard::Manual(transition), &target, false)
            .await?
            .into_manual(job_id, transition)?;
        Ok(())
    }

    async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
        check_timestamp(retry_at, "retry time")?;
        let transition = JobTransition::Retry;
        let target = Target::Retry {
            retry_at,
            error: None,
            timed_out: false,
        };
        self.transition_job(job_id, Guard::Manual(transition), &target, false)
            .await?
            .into_manual(job_id, transition)?;
        Ok(())
    }

    async fn finish_job_run(
        &self,
        run: &Job,
        outcome: JobOutcome,
    ) -> Result<Option<RecordedOutcome>> {
        let target = Target::from(outcome);
        Ok(self
            .transition_job(run.id, Guard::Run(run), &target, true)
            .await?
            .into_run())
    }

    async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
        let row = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {} FROM hammerwork_jobs WHERE id = ?",
            JOB_SELECT_FIELDS
        ))
        .bind(job_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        if let Some(job_row) = row {
            return Ok(Some(job_row.into_job()?));
        }

        // Archiving moves a job out of hammerwork_jobs; report it with status Archived.
        let archived = sqlx::query(&format!(
            "SELECT {ARCHIVED_JOB_FIELDS} FROM hammerwork_jobs_archive WHERE id = ?"
        ))
        .bind(job_id.to_string())
        .fetch_optional(&self.pool)
        .await?;
        archived.as_ref().map(archived_job_from_row).transpose()
    }

    async fn delete_job(&self, job_id: JobId) -> Result<()> {
        sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ?")
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn enqueue_batch(&self, batch: crate::batch::JobBatch) -> Result<crate::batch::BatchId> {
        // Validate the batch first
        batch.validate()?;

        // Encrypt payloads before anything is written, so a job that cannot be encrypted
        // fails the whole batch.
        let mut jobs: Vec<Job> = batch
            .jobs
            .iter()
            .cloned()
            .map(|mut job| {
                job.batch_id = Some(batch.id);
                job
            })
            .collect();
        self.seal_jobs(&mut jobs).await?;

        let mut tx = self.pool.begin().await?;

        // Insert batch metadata
        sqlx::query(
            r#"
            INSERT INTO hammerwork_batches
            (id, batch_name, total_jobs, completed_jobs, failed_jobs, pending_jobs, status, failure_mode, created_at, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(batch.id.to_string())
        .bind(&batch.name)
        .bind(batch.jobs.len() as i32)
        .bind(0i32) // completed_jobs
        .bind(0i32) // failed_jobs
        .bind(batch.jobs.len() as i32) // pending_jobs
        .bind(crate::batch::BatchStatus::Pending)
        .bind(serde_json::to_string(&batch.failure_mode)?)
        .bind(batch.created_at)
        .bind(serde_json::to_value(&batch.metadata)?)
        .execute(&mut *tx)
        .await?;

        // Insert the jobs with the same columns as `enqueue` (result config,
        // dependencies, workflow, tracing, retry strategy and encryption included).
        insert_dependent_jobs(&mut tx, &mut jobs).await?;

        tx.commit().await?;
        Ok(batch.id)
    }

    async fn get_batch_status(
        &self,
        batch_id: crate::batch::BatchId,
    ) -> Result<crate::batch::BatchResult> {
        use crate::batch::BatchResult;
        use std::collections::HashMap;

        // Get batch metadata
        let batch_row = sqlx::query(
            "SELECT batch_name, total_jobs, completed_jobs, failed_jobs, pending_jobs, status, failure_mode, created_at, completed_at, error_summary, metadata FROM hammerwork_batches WHERE id = ?"
        )
        .bind(batch_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        let batch_row = batch_row.ok_or_else(|| crate::HammerworkError::JobNotFound {
            id: batch_id.to_string(),
        })?;

        let total_jobs: u32 =
            super::db_int(batch_row.try_get::<i32, _>("total_jobs")?, "total_jobs")?;
        let failure_mode =
            crate::batch::parse_failure_mode(&batch_row.try_get::<String, _>("failure_mode")?);
        let created_at: DateTime<Utc> = batch_row.try_get("created_at")?;
        let completed_at: Option<DateTime<Utc>> = batch_row.try_get("completed_at")?;
        let error_summary: Option<String> = batch_row.try_get("error_summary")?;

        // The counters in hammerwork_batches are never updated after enqueue; tally the
        // batch's jobs (including archived ones) instead.
        let status_counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM (
                SELECT status FROM hammerwork_jobs WHERE batch_id = ?
                UNION ALL
                SELECT status FROM hammerwork_jobs_archive WHERE batch_id = ?
            ) batch_jobs GROUP BY status",
        )
        .bind(batch_id.to_string())
        .bind(batch_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        let progress = crate::batch::BatchProgress::from_status_counts(
            status_counts,
            total_jobs,
            &failure_mode,
        );

        // Get job errors for CollectErrors mode
        let job_errors: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, error_message FROM hammerwork_jobs WHERE batch_id = ? AND error_message IS NOT NULL"
        )
        .bind(batch_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let job_errors_map: HashMap<uuid::Uuid, String> = job_errors
            .into_iter()
            .map(|(id, error)| Ok((uuid::Uuid::parse_str(&id)?, error)))
            .collect::<Result<_>>()?;

        Ok(BatchResult {
            batch_id,
            total_jobs,
            completed_jobs: progress.completed,
            failed_jobs: progress.failed,
            pending_jobs: progress.pending,
            status: progress.status,
            created_at,
            completed_at,
            error_summary,
            job_errors: job_errors_map,
        })
    }

    async fn get_batch_jobs(&self, batch_id: crate::batch::BatchId) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {} FROM hammerwork_jobs WHERE batch_id = ? ORDER BY created_at ASC",
            JOB_SELECT_FIELDS
        ))
        .bind(batch_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn delete_batch(&self, batch_id: crate::batch::BatchId) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Delete all jobs in the batch
        sqlx::query("DELETE FROM hammerwork_jobs WHERE batch_id = ?")
            .bind(batch_id.to_string())
            .execute(&mut *tx)
            .await?;

        // Delete the batch metadata
        sqlx::query("DELETE FROM hammerwork_batches WHERE id = ?")
            .bind(batch_id.to_string())
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn mark_job_dead(&self, job_id: JobId, error_message: &str) -> Result<()> {
        let transition = JobTransition::MarkDead;
        let target = Target::Dead(error_message.to_string());
        self.transition_job(job_id, Guard::Manual(transition), &target, false)
            .await?
            .into_manual(job_id, transition)?;
        Ok(())
    }

    async fn mark_job_timed_out(&self, job_id: JobId, error_message: &str) -> Result<()> {
        let transition = JobTransition::MarkTimedOut;
        let target = Target::TimedOut(error_message.to_string());
        self.transition_job(job_id, Guard::Manual(transition), &target, false)
            .await?
            .into_manual(job_id, transition)?;
        Ok(())
    }

    async fn get_dead_jobs(&self, limit: Option<u32>, offset: Option<u32>) -> Result<Vec<Job>> {
        use crate::job::JobStatus;

        let limit = limit.unwrap_or(100) as i64;
        let offset = offset.unwrap_or(0) as i64;

        let rows = sqlx::query_as::<_, DeadJobRow>(
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = ? ORDER BY failed_at DESC LIMIT ? OFFSET ?"
        )
        .bind(JobStatus::Dead)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn get_dead_jobs_by_queue(
        &self,
        queue_name: &str,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<Job>> {
        use crate::job::JobStatus;

        let limit = limit.unwrap_or(100) as i64;
        let offset = offset.unwrap_or(0) as i64;

        let rows = sqlx::query_as::<_, DeadJobRow>(
            "SELECT id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message FROM hammerwork_jobs WHERE status = ? AND queue_name = ? ORDER BY failed_at DESC LIMIT ? OFFSET ?"
        )
        .bind(JobStatus::Dead)
        .bind(queue_name)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn retry_dead_job(&self, job_id: JobId) -> Result<()> {
        let update = sqlx::query(
            "UPDATE hammerwork_jobs SET status = ?, attempts = 0, \
             scheduled_at = UTC_TIMESTAMP(6), started_at = NULL, failed_at = NULL, \
             timed_out_at = NULL, last_heartbeat_at = NULL, lease_expires_at = NULL WHERE id = ?",
        )
        .bind(JobStatus::Pending)
        .bind(job_id.to_string());
        self.guarded_update(job_id, JobTransition::RetryDead, update)
            .await
    }

    async fn purge_dead_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        use crate::job::JobStatus;

        let result = sqlx::query("DELETE FROM hammerwork_jobs WHERE status = ? AND failed_at < ?")
            .bind(JobStatus::Dead)
            .bind(older_than)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    async fn get_dead_job_summary(&self) -> Result<DeadJobSummary> {
        use crate::job::JobStatus;

        // Get total dead job count
        let total_dead_jobs: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM hammerwork_jobs WHERE status = ?")
                .bind(JobStatus::Dead)
                .fetch_one(&self.pool)
                .await?;

        // Get dead jobs by queue
        let dead_jobs_by_queue_rows: Vec<(String, i64)> = sqlx::query_as(
            "SELECT queue_name, COUNT(*) FROM hammerwork_jobs WHERE status = ? GROUP BY queue_name",
        )
        .bind(JobStatus::Dead)
        .fetch_all(&self.pool)
        .await?;

        // Get oldest and newest dead jobs
        let timestamps: Vec<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT MIN(failed_at), MAX(failed_at) FROM hammerwork_jobs WHERE status = ? AND failed_at IS NOT NULL"
        )
        .bind(JobStatus::Dead)
        .fetch_all(&self.pool)
        .await?;

        // Get error patterns
        let error_patterns_rows: Vec<(Option<String>, i64)> = sqlx::query_as(
            "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE status = ? AND error_message IS NOT NULL GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 20"
        )
        .bind(JobStatus::Dead)
        .fetch_all(&self.pool)
        .await?;

        let dead_jobs_by_queue = super::count_map(dead_jobs_by_queue_rows, "dead job count")?;

        let error_patterns = super::count_map(
            error_patterns_rows
                .into_iter()
                .filter_map(|(error, count)| error.map(|e| (e, count)))
                .collect(),
            "error count",
        )?;

        let (oldest_dead_job, newest_dead_job) = timestamps
            .first()
            .map(|(oldest, newest)| (*oldest, *newest))
            .unwrap_or((None, None));

        Ok(DeadJobSummary {
            total_dead_jobs: super::db_int(total_dead_jobs.0, "dead job count")?,
            dead_jobs_by_queue,
            oldest_dead_job,
            newest_dead_job,
            error_patterns,
        })
    }

    async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
        use crate::stats::JobStatistics;

        // Get job counts by status
        let status_counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status",
        )
        .bind(queue_name)
        .fetch_all(&self.pool)
        .await?;

        let counts = super::count_map(status_counts, "job count")?;

        let pending_count = counts.get("Pending").copied().unwrap_or(0);
        let running_count = counts.get("Running").copied().unwrap_or(0);
        let dead_count = counts.get("Dead").copied().unwrap_or(0);
        let timed_out_count = counts.get("TimedOut").copied().unwrap_or(0);
        let completed_count = counts.get("Completed").copied().unwrap_or(0);

        // Basic statistics (more detailed stats would require the statistics collector)
        let statistics = JobStatistics {
            total_processed: completed_count + dead_count,
            completed: completed_count,
            failed: counts.get("Failed").copied().unwrap_or(0),
            dead: dead_count,
            timed_out: timed_out_count,
            running: running_count,
            time_window: Duration::from_secs(3600), // Default 1 hour window
            calculated_at: Utc::now(),
            ..Default::default()
        };

        Ok(QueueStats {
            queue_name: queue_name.to_string(),
            pending_count,
            running_count,
            dead_count,
            timed_out_count,
            completed_count,
            statistics,
        })
    }

    async fn get_all_queue_stats(&self) -> Result<Vec<QueueStats>> {
        // Get all unique queue names
        let queue_names: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT queue_name FROM hammerwork_jobs")
                .fetch_all(&self.pool)
                .await?;

        let mut results = Vec::new();
        for (queue_name,) in queue_names {
            let stats = self.get_queue_stats(&queue_name).await?;
            results.push(stats);
        }

        Ok(results)
    }

    async fn get_job_counts_by_status(
        &self,
        queue_name: &str,
    ) -> Result<std::collections::HashMap<String, u64>> {
        let status_counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status",
        )
        .bind(queue_name)
        .fetch_all(&self.pool)
        .await?;

        super::count_map(status_counts, "job count")
    }

    async fn get_priority_stats(&self, queue_name: &str) -> Result<crate::priority::PriorityStats> {
        let priority_counts: Vec<(i32, i64)> = sqlx::query_as(
            "SELECT priority, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY priority",
        )
        .bind(queue_name)
        .fetch_all(&self.pool)
        .await?;

        let mut stats = crate::priority::PriorityStats::new();

        for (priority_num, count) in priority_counts {
            let priority = match priority_num {
                0 => crate::priority::JobPriority::Background,
                1 => crate::priority::JobPriority::Low,
                2 => crate::priority::JobPriority::Normal,
                3 => crate::priority::JobPriority::High,
                4 => crate::priority::JobPriority::Critical,
                _ => crate::priority::JobPriority::Normal, // Default fallback
            };

            *stats.job_counts.entry(priority).or_insert(0) = super::db_int(count, "job count")?;
        }

        // Calculate additional statistics if we have processing time data
        // Dividing gives DECIMAL; cast it so it decodes as a float.
        let processing_times: Vec<(i32, f64)> = sqlx::query_as(
            r#"SELECT priority,
                      CAST(TIMESTAMPDIFF(MICROSECOND, started_at, completed_at) / 1000 AS DOUBLE)
                          AS processing_ms
               FROM hammerwork_jobs
               WHERE queue_name = ? AND started_at IS NOT NULL AND completed_at IS NOT NULL"#,
        )
        .bind(queue_name)
        .fetch_all(&self.pool)
        .await?;

        // Group processing times by priority and calculate averages
        let mut priority_times: std::collections::HashMap<crate::priority::JobPriority, Vec<f64>> =
            std::collections::HashMap::new();

        for (priority_num, processing_ms) in processing_times {
            let priority = match priority_num {
                0 => crate::priority::JobPriority::Background,
                1 => crate::priority::JobPriority::Low,
                2 => crate::priority::JobPriority::Normal,
                3 => crate::priority::JobPriority::High,
                4 => crate::priority::JobPriority::Critical,
                _ => crate::priority::JobPriority::Normal,
            };

            priority_times
                .entry(priority)
                .or_default()
                .push(processing_ms);
        }

        // Calculate average processing times for each priority
        for (priority, times) in priority_times {
            if !times.is_empty() {
                let avg = times.iter().sum::<f64>() / times.len() as f64;
                stats.avg_processing_times.insert(priority, avg);
            }
        }

        stats.calculate_distribution();
        Ok(stats)
    }

    async fn get_processing_times(
        &self,
        queue_name: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<i64>> {
        let times: Vec<(Option<i64>,)> = sqlx::query_as(
            r#"
            SELECT TIMESTAMPDIFF(MICROSECOND, started_at, completed_at) DIV 1000
                       AS processing_time_ms
            FROM hammerwork_jobs
            WHERE queue_name = ?
            AND started_at IS NOT NULL
            AND completed_at IS NOT NULL
            AND completed_at >= ?
            ORDER BY completed_at DESC
            LIMIT 1000
            "#,
        )
        .bind(queue_name)
        .bind(since)
        .fetch_all(&self.pool)
        .await?;

        Ok(times.into_iter().filter_map(|(time,)| time).collect())
    }

    async fn get_error_frequencies(
        &self,
        queue_name: Option<&str>,
        since: DateTime<Utc>,
    ) -> Result<std::collections::HashMap<String, u64>> {
        let query = if queue_name.is_some() {
            "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? AND error_message IS NOT NULL AND failed_at >= ? GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
        } else {
            "SELECT error_message, COUNT(*) FROM hammerwork_jobs WHERE error_message IS NOT NULL AND failed_at >= ? GROUP BY error_message ORDER BY COUNT(*) DESC LIMIT 50"
        };

        let error_frequencies: Vec<(String, i64)> = if let Some(queue) = queue_name {
            sqlx::query_as(query)
                .bind(queue)
                .bind(since)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query_as(query)
                .bind(since)
                .fetch_all(&self.pool)
                .await?
        };

        super::count_map(error_frequencies, "error count")
    }

    async fn get_jobs_completed_in_range(
        &self,
        queue_name: Option<&str>,
        start_time: DateTime<Utc>,
        end_time: DateTime<Utc>,
        limit: Option<u32>,
    ) -> Result<Vec<Job>> {
        let limit_clause = if let Some(limit) = limit {
            format!("LIMIT {}", limit)
        } else {
            "".to_string()
        };

        let query = if queue_name.is_some() {
            format!(
                r#"
                SELECT {}
                FROM hammerwork_jobs
                WHERE queue_name = ?
                  AND status = 'Completed'
                  AND completed_at >= ?
                  AND completed_at < ?
                ORDER BY completed_at DESC
                {}
                "#,
                JOB_SELECT_FIELDS, limit_clause
            )
        } else {
            format!(
                r#"
                SELECT {}
                FROM hammerwork_jobs
                WHERE status = 'Completed'
                  AND completed_at >= ?
                  AND completed_at < ?
                ORDER BY completed_at DESC
                {}
                "#,
                JOB_SELECT_FIELDS, limit_clause
            )
        };

        let rows: Vec<JobRow> = if let Some(queue) = queue_name {
            sqlx::query_as(&query)
                .bind(queue)
                .bind(start_time)
                .bind(end_time)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query_as(&query)
                .bind(start_time)
                .bind(end_time)
                .fetch_all(&self.pool)
                .await?
        };

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn enqueue_cron_job(&self, job: Job) -> Result<JobId> {
        self.enqueue(super::prepare_cron_job(job)?).await
    }

    async fn get_due_cron_jobs(&self, queue_name: Option<&str>) -> Result<Vec<Job>> {
        use crate::job::JobStatus;

        let query = if queue_name.is_some() {
            format!(
                r#"
            SELECT {}
            FROM hammerwork_jobs
            WHERE recurring = TRUE
            AND queue_name = ?
            AND (next_run_at IS NULL OR next_run_at <= UTC_TIMESTAMP(6))
            AND status = ?
            ORDER BY next_run_at ASC
            "#,
                JOB_SELECT_FIELDS
            )
        } else {
            format!(
                r#"
            SELECT {}
            FROM hammerwork_jobs
            WHERE recurring = TRUE
            AND (next_run_at IS NULL OR next_run_at <= UTC_TIMESTAMP(6))
            AND status = ?
            ORDER BY next_run_at ASC
            "#,
                JOB_SELECT_FIELDS
            )
        };

        let rows = if let Some(queue) = queue_name {
            sqlx::query_as::<_, JobRow>(&query)
                .bind(queue)
                .bind(JobStatus::Pending)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query_as::<_, JobRow>(&query)
                .bind(JobStatus::Pending)
                .fetch_all(&self.pool)
                .await?
        };

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn reschedule_cron_job(&self, job_id: JobId, next_run_at: DateTime<Utc>) -> Result<()> {
        check_timestamp(next_run_at, "next run time")?;
        let update = sqlx::query(
            r#"
            UPDATE hammerwork_jobs
            SET status = ?,
                scheduled_at = ?,
                next_run_at = ?,
                attempts = 0,
                started_at = NULL,
                completed_at = NULL,
                failed_at = NULL,
                timed_out_at = NULL,
                error_message = NULL,
                last_heartbeat_at = NULL,
                lease_expires_at = NULL
            WHERE id = ?
            "#,
        )
        .bind(JobStatus::Pending)
        .bind(next_run_at)
        .bind(next_run_at)
        .bind(job_id.to_string());
        self.guarded_update(job_id, JobTransition::RescheduleCron, update)
            .await
    }

    async fn get_recurring_jobs(&self, queue_name: &str) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(
            &format!("SELECT {} FROM hammerwork_jobs WHERE queue_name = ? AND recurring = TRUE ORDER BY next_run_at ASC", JOB_SELECT_FIELDS)
        )
        .bind(queue_name)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn disable_recurring_job(&self, job_id: JobId) -> Result<()> {
        sqlx::query("UPDATE hammerwork_jobs SET recurring = FALSE WHERE id = ?")
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn enable_recurring_job(&self, job_id: JobId) -> Result<()> {
        sqlx::query("UPDATE hammerwork_jobs SET recurring = TRUE WHERE id = ?")
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn set_throttle_config(&self, queue_name: &str, config: ThrottleConfig) -> Result<()> {
        self.set_throttle(queue_name, config).await
    }

    async fn get_throttle_config(&self, queue_name: &str) -> Result<Option<ThrottleConfig>> {
        Ok(self.get_throttle(queue_name).await)
    }

    async fn remove_throttle_config(&self, queue_name: &str) -> Result<()> {
        self.remove_throttle(queue_name).await
    }

    async fn get_all_throttle_configs(&self) -> Result<HashMap<String, ThrottleConfig>> {
        Ok(self.get_all_throttles().await)
    }

    async fn get_queue_depth(&self, queue_name: &str) -> Result<u64> {
        use crate::job::JobStatus;

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? AND status = ?",
        )
        .bind(queue_name)
        .bind(JobStatus::Pending)
        .fetch_one(&self.pool)
        .await?;

        super::db_int(count, "queue depth")
    }

    // Job result storage and retrieval operations
    async fn store_job_result(
        &self,
        job_id: JobId,
        result_data: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        if let Some(expires_at) = expires_at {
            check_timestamp(expires_at, "result expiry time")?;
        }
        sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = ?, result_stored_at = UTC_TIMESTAMP(6), result_expires_at = ? WHERE id = ?"
        )
        .bind(result_data)
        .bind(expires_at)
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_job_result(&self, job_id: JobId) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            "SELECT result_data FROM hammerwork_jobs WHERE id = ? AND result_data IS NOT NULL AND (result_expires_at IS NULL OR result_expires_at > UTC_TIMESTAMP(6))"
        )
        .bind(job_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(row.try_get("result_data")?),
            None => Ok(None),
        }
    }

    async fn delete_job_result(&self, job_id: JobId) -> Result<()> {
        sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = NULL, result_stored_at = NULL, result_expires_at = NULL WHERE id = ?"
        )
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn cleanup_expired_results(&self) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = NULL, result_stored_at = NULL, result_expires_at = NULL WHERE result_expires_at IS NOT NULL AND result_expires_at <= UTC_TIMESTAMP(6)"
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    // Workflow and dependency management methods
    async fn enqueue_workflow(
        &self,
        workflow: crate::workflow::JobGroup,
    ) -> Result<crate::workflow::WorkflowId> {
        // Validate workflow before enqueuing
        workflow.validate()?;

        let mut jobs = workflow.jobs.clone();
        self.seal_jobs(&mut jobs).await?;

        let mut tx = self.pool.begin().await?;

        // Insert workflow metadata
        sqlx::query(
            r#"
            INSERT INTO hammerwork_workflows
            (id, name, status, created_at, completed_at, failed_at, total_jobs, completed_jobs, failed_jobs, failure_policy, metadata)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(workflow.id.to_string())
        .bind(&workflow.name)
        .bind(workflow.status.as_str())
        .bind(workflow.created_at)
        .bind(workflow.completed_at)
        .bind(workflow.failed_at)
        .bind(workflow.total_jobs as i32)
        .bind(workflow.completed_jobs as i32)
        .bind(workflow.failed_jobs as i32)
        .bind(workflow.failure_policy.as_str())
        .bind(&workflow.metadata)
        .execute(&mut *tx)
        .await?;

        // Insert all jobs in the workflow
        insert_dependent_jobs(&mut tx, &mut jobs).await?;

        tx.commit().await?;

        Ok(workflow.id)
    }

    async fn get_workflow_status(
        &self,
        workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Option<crate::workflow::JobGroup>> {
        use crate::workflow::{FailurePolicy, JobGroup, WorkflowStatus};

        // Get workflow metadata
        let workflow_row = sqlx::query(
            r#"
            SELECT id, name, status, created_at, completed_at, failed_at, total_jobs, completed_jobs, failed_jobs, failure_policy, metadata
            FROM hammerwork_workflows
            WHERE id = ?
            "#
        )
        .bind(workflow_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        if let Some(row) = workflow_row {
            // Get all jobs in the workflow
            let jobs = self.get_workflow_jobs(workflow_id).await?;

            // Build dependencies map
            let mut dependencies = std::collections::HashMap::new();
            for job in &jobs {
                if !job.depends_on.is_empty() {
                    dependencies.insert(job.id, job.depends_on.clone());
                }
            }

            let workflow = JobGroup {
                id: uuid::Uuid::parse_str(row.try_get::<String, _>("id")?.as_str())?,
                name: row.try_get("name")?,
                status: WorkflowStatus::parse_from_db(row.try_get("status")?)?,
                created_at: row.try_get("created_at")?,
                completed_at: row.try_get("completed_at")?,
                failed_at: row.try_get("failed_at")?,
                failure_policy: FailurePolicy::parse_from_db(row.try_get("failure_policy")?)?,
                jobs,
                dependencies,
                total_jobs: super::db_int(row.try_get::<i32, _>("total_jobs")?, "total_jobs")?,
                completed_jobs: super::db_int(
                    row.try_get::<i32, _>("completed_jobs")?,
                    "completed_jobs",
                )?,
                failed_jobs: super::db_int(row.try_get::<i32, _>("failed_jobs")?, "failed_jobs")?,
                metadata: row.try_get("metadata")?,
            };

            Ok(Some(workflow))
        } else {
            Ok(None)
        }
    }

    async fn resolve_job_dependencies(&self, completed_job_id: JobId) -> Result<Vec<JobId>> {
        super::retry_on_conflict(|| async {
            let mut conn = self.pool.acquire().await?;
            let mut tx = Self::begin_claim_transaction(&mut conn).await?;
            let result = Self::resolve_dependents(&mut tx, completed_job_id).await;
            super::end_transaction(tx, result).await
        })
        .await
    }

    async fn get_ready_jobs(&self, queue_name: &str, limit: u32) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(&format!(
            r#"
                SELECT {}
                FROM hammerwork_jobs
                WHERE queue_name = ?
                AND status = 'Pending'
                AND dependency_status IN ('none', 'satisfied')
                AND scheduled_at <= UTC_TIMESTAMP(6)
                ORDER BY priority DESC, scheduled_at ASC
                LIMIT ?
                "#,
            JOB_SELECT_FIELDS
        ))
        .bind(queue_name)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn fail_job_dependencies(&self, failed_job_id: JobId) -> Result<Vec<JobId>> {
        super::retry_on_conflict(|| async {
            let mut conn = self.pool.acquire().await?;
            let mut tx = Self::begin_claim_transaction(&mut conn).await?;
            let result = Self::fail_job_dependencies_in_tx(&mut tx, failed_job_id).await;
            super::end_transaction(tx, result).await
        })
        .await
    }

    async fn get_workflow_jobs(
        &self,
        workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(&format!(
            r#"
                SELECT {}
                FROM hammerwork_jobs
                WHERE workflow_id = ?
                ORDER BY created_at ASC
                "#,
            JOB_SELECT_FIELDS
        ))
        .bind(workflow_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn cancel_workflow(&self, workflow_id: crate::workflow::WorkflowId) -> Result<()> {
        super::retry_on_conflict(|| async {
            let mut conn = self.pool.acquire().await?;
            let mut tx = Self::begin_claim_transaction(&mut conn).await?;
            let result = Self::cancel_workflow_in_tx(&mut tx, workflow_id).await;
            super::end_transaction(tx, result).await
        })
        .await
    }

    // Job archival operations
    async fn archive_jobs(
        &self,
        queue_name: Option<&str>,
        policy: &crate::archive::ArchivalPolicy,
        config: &crate::archive::ArchivalConfig,
        reason: crate::archive::ArchivalReason,
        archived_by: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats> {
        use crate::archive::ArchivalStats;

        if !policy.enabled {
            return Ok(ArchivalStats::default());
        }

        let start_time = Utc::now();
        let mut jobs_archived = 0u64;
        let mut bytes_archived = 0u64;
        let mut total_compression_ratio = 0.0;

        // One condition per job status the policy archives, each with its own age threshold.
        let now = Utc::now();
        let mut thresholds = Vec::new();
        let mut status_conditions = Vec::new();
        for (after, status, column) in crate::archive::archival_candidates(policy) {
            if let Some(after) = after {
                status_conditions.push(format!(
                    "(status = '{status}' AND {column} IS NOT NULL AND {column} <= ?)"
                ));
                thresholds.push(now - after);
            }
        }
        if status_conditions.is_empty() || policy.batch_size == 0 {
            return Ok(ArchivalStats::default());
        }

        // Lock the candidate rows so concurrent archivers (and workers) skip them
        // instead of archiving the same job twice (SKIP LOCKED needs MySQL 8.0+).
        let select = format!(
            "SELECT {} FROM hammerwork_jobs WHERE archived_at IS NULL{} AND ({}) \
             ORDER BY created_at ASC LIMIT {} FOR UPDATE SKIP LOCKED",
            JOB_SELECT_FIELDS,
            if queue_name.is_some() {
                " AND queue_name = ?"
            } else {
                ""
            },
            status_conditions.join(" OR "),
            policy.batch_size
        );

        // Archiving a job moves its row: insert into the archive table and delete from
        // the main table in the same transaction.
        let mut tx = self.pool.begin().await?;

        let mut query = sqlx::query_as::<_, JobRow>(&select);
        if let Some(queue) = queue_name {
            query = query.bind(queue);
        }
        for threshold in thresholds {
            query = query.bind(threshold);
        }
        let jobs_to_archive = query.fetch_all(&mut *tx).await?;

        let archived_at = Utc::now();
        for job_row in jobs_to_archive {
            let job = job_row.into_job()?;
            let (final_payload, is_compressed, original_size) =
                crate::archive::encode_archived_payload(&job.payload, policy, config)?;
            if is_compressed {
                total_compression_ratio += original_size as f64 / final_payload.len() as f64;
            }

            sqlx::query(
                r#"
                INSERT INTO hammerwork_jobs_archive (
                    id, queue_name, payload, payload_compressed, original_payload_size,
                    status, priority, attempts, max_attempts, created_at, scheduled_at,
                    started_at, completed_at, failed_at, timed_out_at, archived_at,
                    error_message, result, result_ttl, retry_strategy, timeout_seconds,
                    priority_weight, cron_schedule, next_run_at, recurring, timezone,
                    batch_id, depends_on, dependency_status, result_config,
                    trace_id, correlation_id, parent_span_id, span_context,
                    archival_reason, archived_by, original_table
                ) VALUES (
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    ?, ?, ?, ?, ?, ?, ?
                )
            "#,
            )
            .bind(job.id.to_string())
            .bind(&job.queue_name)
            .bind(&final_payload)
            .bind(is_compressed)
            .bind(original_size as i32)
            .bind(job.status)
            .bind(job.priority.to_string())
            .bind(job.attempts)
            .bind(job.max_attempts)
            .bind(job.created_at)
            .bind(job.scheduled_at)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(job.failed_at)
            .bind(job.timed_out_at)
            .bind(archived_at)
            .bind(job.error_message)
            .bind(job.result_data)
            .bind(job.result_expires_at)
            .bind(
                job.retry_strategy
                    .map(|rs| serde_json::to_string(&rs))
                    .transpose()?,
            )
            .bind(job.timeout.map(|t| t.as_secs() as i32))
            .bind(job.priority.weight() as i32)
            .bind(job.cron_schedule)
            .bind(job.next_run_at)
            .bind(job.recurring)
            .bind(job.timezone)
            .bind(job.batch_id.map(|id| id.to_string()))
            .bind(if job.depends_on.is_empty() {
                None
            } else {
                Some(serde_json::to_value(&job.depends_on)?)
            })
            .bind(job.dependency_status.as_str())
            .bind(serde_json::to_value(&job.result_config)?)
            .bind(job.trace_id)
            .bind(job.correlation_id)
            .bind(job.parent_span_id)
            .bind(job.span_context)
            .bind(reason.as_str())
            .bind(archived_by)
            .bind("hammerwork_jobs")
            .execute(&mut *tx)
            .await?;

            // Move an encrypted payload as it is, without decrypting it
            if job.is_encrypted || !job.pii_fields.is_empty() {
                copy_encryption_columns(
                    &mut tx,
                    "hammerwork_jobs",
                    "hammerwork_jobs_archive",
                    job.id,
                )
                .await?;
            }

            sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ?")
                .bind(job.id.to_string())
                .execute(&mut *tx)
                .await?;

            jobs_archived += 1;
            bytes_archived += final_payload.len() as u64;
        }

        tx.commit().await?;

        let operation_duration = Utc::now() - start_time;
        let compression_ratio = if jobs_archived > 0 && total_compression_ratio > 0.0 {
            total_compression_ratio / jobs_archived as f64
        } else {
            1.0
        };

        Ok(ArchivalStats {
            jobs_archived,
            jobs_purged: 0,
            bytes_archived,
            bytes_purged: 0,
            compression_ratio,
            operation_duration: operation_duration.to_std().unwrap_or_default(),
            last_run_at: Utc::now(),
        })
    }

    async fn restore_archived_job(&self, job_id: JobId) -> Result<Job> {
        let mut tx = self.pool.begin().await?;

        // Lock the archived row so two concurrent restores cannot both re-insert it.
        let archived_row = sqlx::query(&format!(
            "SELECT {ARCHIVED_JOB_FIELDS} FROM hammerwork_jobs_archive WHERE id = ? FOR UPDATE"
        ))
        .bind(job_id.to_string())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| crate::HammerworkError::JobNotFound {
            id: job_id.to_string(),
        })?;

        let job = crate::archive::reset_for_restore(archived_job_from_row(&archived_row)?);

        // Older versions archived a job without deleting its row from hammerwork_jobs
        // (they only set archived_at). Drop such a leftover so the restore does not
        // collide with it.
        sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ? AND archived_at IS NOT NULL")
            .bind(job_id.to_string())
            .execute(&mut *tx)
            .await?;

        // Insert back into main table. An encrypted payload is copied back as it is,
        // without decrypting it.
        let mut row = job.clone();
        row.is_encrypted = false;
        self.enqueue_with_tx(&mut tx, row).await?;
        if job.is_encrypted || !job.pii_fields.is_empty() {
            copy_encryption_columns(
                &mut tx,
                "hammerwork_jobs_archive",
                "hammerwork_jobs",
                job_id,
            )
            .await?;
        }

        // Remove from archive table
        sqlx::query("DELETE FROM hammerwork_jobs_archive WHERE id = ?")
            .bind(job_id.to_string())
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(job)
    }

    async fn list_archived_jobs(
        &self,
        queue_name: Option<&str>,
        limit: Option<u32>,
        offset: Option<u32>,
    ) -> Result<Vec<crate::archive::ArchivedJob>> {
        use crate::archive::ArchivedJob;

        let mut query = "SELECT id, queue_name, status, created_at, archived_at, archival_reason, original_payload_size, payload_compressed, archived_by FROM hammerwork_jobs_archive".to_string();
        let mut conditions = Vec::new();

        if queue_name.is_some() {
            conditions.push("queue_name = ?".to_string());
        }

        if !conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(" ORDER BY archived_at DESC");

        if let Some(limit) = limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        if let Some(offset) = offset {
            query.push_str(&format!(" OFFSET {}", offset));
        }

        let mut sql_query = sqlx::query(&query);

        if let Some(queue) = queue_name {
            sql_query = sql_query.bind(queue);
        }

        let rows = sql_query.fetch_all(&self.pool).await?;

        let mut archived_jobs = Vec::new();
        for row in rows {
            archived_jobs.push(ArchivedJob {
                id: uuid::Uuid::parse_str(&row.try_get::<String, _>("id")?)?,
                queue_name: row.try_get("queue_name")?,
                status: crate::archive::parse_archived_status(&row.try_get::<String, _>("status")?),
                created_at: row.try_get("created_at")?,
                archived_at: row.try_get("archived_at")?,
                archival_reason: crate::archive::ArchivalReason::parse_from_db(
                    &row.try_get::<String, _>("archival_reason")?,
                )
                .unwrap_or_default(),
                original_payload_size: row
                    .try_get::<Option<i32>, _>("original_payload_size")?
                    .map(|s| super::db_int(s, "original_payload_size"))
                    .transpose()?,
                payload_compressed: row.try_get("payload_compressed")?,
                archived_by: row.try_get("archived_by")?,
            });
        }

        Ok(archived_jobs)
    }

    async fn purge_archived_jobs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let result = sqlx::query("DELETE FROM hammerwork_jobs_archive WHERE archived_at <= ?")
            .bind(older_than)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected())
    }

    async fn get_archival_stats(
        &self,
        queue_name: Option<&str>,
    ) -> Result<crate::archive::ArchivalStats> {
        use crate::archive::ArchivalStats;

        let mut base_query = "SELECT
            COUNT(*) as job_count,
            CAST(COALESCE(SUM(original_payload_size), 0) AS SIGNED) as total_original_size,
            CAST(COALESCE(SUM(LENGTH(payload)), 0) AS SIGNED) as total_compressed_size,
            MAX(archived_at) as last_archived_at
            FROM hammerwork_jobs_archive"
            .to_string();

        if queue_name.is_some() {
            base_query.push_str(" WHERE queue_name = ?");
        }

        let mut query = sqlx::query(&base_query);
        if let Some(queue) = queue_name {
            query = query.bind(queue);
        }

        let row = query.fetch_one(&self.pool).await?;

        let job_count: i64 = row.try_get("job_count")?;
        let total_original_size: i64 = row.try_get("total_original_size")?;
        let total_compressed_size: i64 = row.try_get("total_compressed_size")?;
        let last_archived_at: Option<DateTime<Utc>> = row.try_get("last_archived_at")?;

        let compression_ratio = if total_compressed_size > 0 {
            total_original_size as f64 / total_compressed_size as f64
        } else {
            1.0
        };

        Ok(ArchivalStats {
            jobs_archived: super::db_int(job_count, "archived job count")?,
            jobs_purged: 0, // This would need separate tracking
            bytes_archived: super::db_int(total_compressed_size, "archived bytes")?,
            bytes_purged: 0,
            compression_ratio,
            operation_duration: std::time::Duration::from_secs(0),
            last_run_at: last_archived_at.unwrap_or(Utc::now()),
        })
    }

    // Queue management operations
    async fn pause_queue(&self, queue_name: &str, paused_by: Option<&str>) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_queue_pause (queue_name, paused_by, paused_at, created_at, updated_at)
            VALUES (?, ?, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))
            ON DUPLICATE KEY UPDATE
                paused_by = VALUES(paused_by),
                paused_at = UTC_TIMESTAMP(6),
                updated_at = UTC_TIMESTAMP(6)
            "#,
        )
        .bind(queue_name)
        .bind(paused_by)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn resume_queue(&self, queue_name: &str, _resumed_by: Option<&str>) -> Result<()> {
        sqlx::query("DELETE FROM hammerwork_queue_pause WHERE queue_name = ?")
            .bind(queue_name)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn is_queue_paused(&self, queue_name: &str) -> Result<bool> {
        let row = sqlx::query("SELECT 1 FROM hammerwork_queue_pause WHERE queue_name = ?")
            .bind(queue_name)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.is_some())
    }

    async fn get_queue_pause_info(
        &self,
        queue_name: &str,
    ) -> Result<Option<super::QueuePauseInfo>> {
        let row = sqlx::query(
            "SELECT queue_name, paused_at, paused_by, reason FROM hammerwork_queue_pause WHERE queue_name = ?"
        )
        .bind(queue_name)
        .fetch_optional(&self.pool)
        .await?;

        row.as_ref().map(pause_info_from_row).transpose()
    }

    async fn get_paused_queues(&self) -> Result<Vec<super::QueuePauseInfo>> {
        let rows = sqlx::query(
            "SELECT queue_name, paused_at, paused_by, reason FROM hammerwork_queue_pause ORDER BY paused_at DESC"
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(pause_info_from_row).collect()
    }

    async fn heartbeat_job(&self, job_id: JobId, lease: std::time::Duration) -> Result<bool> {
        // The lease is measured on the database clock, like the reaper that checks it,
        // and capped at the latest time a TIMESTAMP column holds.
        let lease = super::clamp_interval(lease)
            .num_microseconds()
            .unwrap_or(i64::MAX);
        let result = sqlx::query(&format!(
            "UPDATE hammerwork_jobs SET last_heartbeat_at = UTC_TIMESTAMP(6), \
             lease_expires_at = LEAST(DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? MICROSECOND), \
             CAST('{TIMESTAMP_MAX_SQL}' AS DATETIME(6))) WHERE id = ? AND status = ?"
        ))
        .bind(lease)
        .bind(job_id.to_string())
        .bind(JobStatus::Running)
        .execute(&self.pool)
        .await?;

        // MySQL reports "rows changed", not "rows matched"; the timestamps always change
        // (microsecond precision), so 0 means the job is no longer Running.
        Ok(result.rows_affected() > 0)
    }

    async fn requeue_stale_jobs(
        &self,
        older_than: std::time::Duration,
    ) -> Result<super::StaleJobRecovery> {
        let recovery =
            super::retry_on_conflict(|| self.requeue_stale_jobs_once(older_than)).await?;

        if !recovery.is_empty() {
            tracing::warn!(
                requeued = recovery.requeued.len(),
                dead = recovery.dead.len(),
                "Reclaimed stale Running jobs whose lease expired"
            );
        }

        Ok(recovery)
    }

    async fn purge_expired_encrypted_jobs(&self) -> Result<super::EncryptedJobPurge> {
        let mut tx = self.pool.begin().await?;
        let now = db_now(&mut tx).await?;
        let jobs = sqlx::query(&format!(
            "DELETE FROM hammerwork_jobs WHERE is_encrypted = true \
             AND retention_delete_at IS NOT NULL AND retention_delete_at <= ? \
             AND status IN ({})",
            super::FINISHED_STATUS_SQL
        ))
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let archived_jobs = sqlx::query(
            "DELETE FROM hammerwork_jobs_archive WHERE is_encrypted = true \
             AND retention_delete_at IS NOT NULL AND retention_delete_at <= ?",
        )
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;

        let purge = super::EncryptedJobPurge {
            jobs,
            archived_jobs,
        };
        if purge.total() > 0 {
            tracing::info!(
                jobs = purge.jobs,
                archived_jobs = purge.archived_jobs,
                "Deleted encrypted jobs whose retention period ended"
            );
        }
        Ok(purge)
    }
}

impl crate::queue::JobQueue<MySql> {
    async fn requeue_stale_jobs_once(
        &self,
        older_than: std::time::Duration,
    ) -> Result<super::StaleJobRecovery> {
        let mut conn = self.pool.acquire().await?;
        // READ COMMITTED, like the dequeue: no gap locks on the Running range.
        let mut tx = Self::begin_claim_transaction(&mut conn).await?;
        let result = Self::requeue_stale_jobs_in_tx(&mut tx, older_than).await;
        super::end_transaction(tx, result).await
    }

    async fn requeue_stale_jobs_in_tx(
        conn: &mut sqlx::MySqlConnection,
        older_than: std::time::Duration,
    ) -> Result<super::StaleJobRecovery> {
        let now = db_now(conn).await?;
        let cutoff = super::saturating_sub_from(now, older_than);

        // Claim stale rows with SKIP LOCKED so concurrent reapers never wait on (or
        // double-process) the same row. Each UPDATE re-checks `status` as well.
        let candidates = sqlx::query_as::<_, JobRow>(&format!(
            r#"
            SELECT {JOB_SELECT_FIELDS} FROM hammerwork_jobs
            WHERE status = ?
              AND (
                (last_heartbeat_at IS NOT NULL
                    AND last_heartbeat_at >= started_at
                    AND lease_expires_at < ?)
                OR ((last_heartbeat_at IS NULL OR last_heartbeat_at < started_at)
                    AND started_at < ?)
              )
            FOR UPDATE SKIP LOCKED
            "#
        ))
        .bind(JobStatus::Running)
        .bind(now)
        .bind(cutoff)
        .fetch_all(&mut *conn)
        .await?;

        let mut recovery = super::StaleJobRecovery::default();
        for row in candidates {
            let job = row.into_job()?;
            let id_str = job.id.to_string();

            let exhausted = job.attempts >= job.max_attempts;
            let result = if exhausted {
                sqlx::query(
                    "UPDATE hammerwork_jobs SET status = ?, failed_at = ?, error_message = ?, \
                     last_heartbeat_at = NULL, lease_expires_at = NULL \
                     WHERE id = ? AND status = ?",
                )
                .bind(JobStatus::Dead)
                .bind(now)
                .bind(super::STALE_JOB_ERROR_MESSAGE)
                .bind(&id_str)
                .bind(JobStatus::Running)
                .execute(&mut *conn)
                .await?
            } else {
                sqlx::query(
                    "UPDATE hammerwork_jobs SET status = ?, scheduled_at = ?, started_at = NULL, \
                     error_message = ?, last_heartbeat_at = NULL, lease_expires_at = NULL \
                     WHERE id = ? AND status = ?",
                )
                .bind(JobStatus::Pending)
                .bind(now)
                .bind(super::STALE_JOB_ERROR_MESSAGE)
                .bind(&id_str)
                .bind(JobStatus::Running)
                .execute(&mut *conn)
                .await?
            };

            if result.rows_affected() == 1 {
                if exhausted {
                    // A reclaimed job that died is a terminal failure like any other:
                    // reschedule it if recurring, otherwise apply it to its dependents,
                    // workflow and batch.
                    Self::after_terminal(conn, &job, JobStatus::Dead, now, true).await?;
                    recovery.dead.push(job.id);
                } else {
                    recovery.requeued.push(job.id);
                }
            }
        }

        Ok(recovery)
    }
}

// Helpers for enqueueing within an existing transaction
impl crate::queue::JobQueue<sqlx::MySql> {
    async fn enqueue_with_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
        job: Job,
    ) -> Result<JobId> {
        insert_jobs(tx, std::slice::from_ref(&job)).await?;
        Ok(job.id)
    }
}

/// Columns written by [`insert_jobs`], in bind order.
const INSERT_JOB_COLUMNS: &str = "id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_storage_type, result_ttl_seconds, result_max_size_bytes, retry_strategy, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context, is_encrypted, encryption_key_id, encryption_algorithm, encrypted_payload, encryption_nonce, encryption_tag, encryption_metadata, payload_hash, pii_fields, retention_policy, retention_delete_at, encrypted_at";

/// Rows per multi-row INSERT; 45 placeholders per row stays far below MySQL's 65535
/// and keeps statements well under the default `max_allowed_packet`.
const INSERT_CHUNK_ROWS: usize = 200;

/// Ids per `id IN (...)` statement when locking or failing jobs by id.
const ID_CHUNK: usize = 500;

fn result_storage_str(storage: &crate::job::ResultStorage) -> &'static str {
    match storage {
        crate::job::ResultStorage::Database => "database",
        crate::job::ResultStorage::Memory => "memory",
        crate::job::ResultStorage::None => "none",
    }
}

/// Insert `jobs` with every persisted field. Used by `enqueue`, `enqueue_batch`,
/// `enqueue_workflow` and archive restore, so all of them store the same columns.
async fn insert_jobs(conn: &mut sqlx::MySqlConnection, jobs: &[Job]) -> Result<()> {
    // Encode the fallible fields first so a bad job fails before anything is written.
    let mut encoded = Vec::with_capacity(jobs.len());
    for job in jobs {
        check_job_timestamps(job)?;
        encoded.push((
            super::retry_strategy_json(job)?,
            serde_json::to_value(&job.depends_on)?,
            serde_json::to_value(&job.dependents)?,
            super::EncryptionColumns::for_job(job)?,
            super::pii_fields_column(job)
                .map(serde_json::to_value)
                .transpose()?,
        ));
    }

    for (chunk, encoded) in jobs
        .chunks(INSERT_CHUNK_ROWS)
        .zip(encoded.chunks(INSERT_CHUNK_ROWS))
    {
        let mut builder = sqlx::QueryBuilder::<MySql>::new(format!(
            "INSERT INTO hammerwork_jobs ({INSERT_JOB_COLUMNS}) "
        ));
        builder.push_values(
            chunk.iter().zip(encoded),
            |mut row, (job, (strategy, depends_on, dependents, encryption, pii_fields))| {
                row.push_bind(job.id.to_string())
                    .push_bind(job.queue_name.clone())
                    .push_bind(job.payload.clone())
                    .push_bind(job.status)
                    .push_bind(job.priority.as_i32())
                    .push_bind(job.attempts)
                    .push_bind(job.max_attempts)
                    .push_bind(job.timeout.map(|d| d.as_secs() as i32))
                    .push_bind(job.created_at)
                    .push_bind(job.scheduled_at)
                    .push_bind(job.started_at)
                    .push_bind(job.completed_at)
                    .push_bind(job.failed_at)
                    .push_bind(job.timed_out_at)
                    .push_bind(job.error_message.clone())
                    .push_bind(job.cron_schedule.clone())
                    .push_bind(job.next_run_at)
                    .push_bind(job.recurring)
                    .push_bind(job.timezone.clone())
                    .push_bind(job.batch_id.map(|id| id.to_string()))
                    .push_bind(result_storage_str(&job.result_config.storage))
                    .push_bind(job.result_config.ttl.map(|d| d.as_secs() as i64))
                    .push_bind(job.result_config.max_size_bytes.map(|s| s as i64))
                    .push_bind(strategy.clone())
                    .push_bind(depends_on.clone())
                    .push_bind(dependents.clone())
                    .push_bind(job.dependency_status.as_str())
                    .push_bind(job.workflow_id.map(|id| id.to_string()))
                    .push_bind(job.workflow_name.clone())
                    .push_bind(job.trace_id.clone())
                    .push_bind(job.correlation_id.clone())
                    .push_bind(job.parent_span_id.clone())
                    .push_bind(job.span_context.clone())
                    .push_bind(encryption.is_encrypted)
                    .push_bind(encryption.key_id.clone())
                    .push_bind(encryption.algorithm)
                    .push_bind(encryption.ciphertext.clone())
                    .push_bind(encryption.nonce.clone())
                    .push_bind(encryption.tag.clone())
                    .push_bind(encryption.metadata.clone())
                    .push_bind(encryption.payload_hash.clone())
                    .push_bind(pii_fields.clone())
                    .push_bind(encryption.retention_policy)
                    .push_bind(encryption.retention_delete_at)
                    .push_bind(encryption.encrypted_at);
            },
        );
        builder.build().execute(&mut *conn).await?;
    }
    Ok(())
}

/// The latest instant a MySQL `TIMESTAMP` column can hold, as SQL.
const TIMESTAMP_MAX_SQL: &str = "2038-01-19 03:14:07.999999";

/// The range of instants a MySQL `TIMESTAMP` column can hold:
/// 1970-01-01 00:00:01 to 2038-01-19 03:14:07.999999 UTC.
fn timestamp_range() -> (DateTime<Utc>, DateTime<Utc>) {
    (
        DateTime::from_timestamp(1, 0).unwrap_or(DateTime::<Utc>::MIN_UTC),
        DateTime::from_timestamp(i64::from(i32::MAX), 999_999_000)
            .unwrap_or(DateTime::<Utc>::MAX_UTC),
    )
}

/// Reject a time that the MySQL schema's `TIMESTAMP` columns cannot store (after
/// 2038-01-19 03:14:07 UTC, the "year 2038" limit), with an error naming the value,
/// instead of letting MySQL reject the statement.
fn check_timestamp(value: DateTime<Utc>, what: &str) -> Result<()> {
    let (min, max) = timestamp_range();
    if value < min || value > max {
        return Err(crate::HammerworkError::InvalidJobPayload {
            message: format!(
                "{what} {value} is outside the range MySQL TIMESTAMP columns can store \
                 ({min} to {max}); the MySQL backend cannot schedule jobs after \
                 2038-01-19 03:14:07 UTC"
            ),
        });
    }
    Ok(())
}

/// [`check_timestamp`] for every time stored with a new job.
fn check_job_timestamps(job: &Job) -> Result<()> {
    check_timestamp(job.created_at, "created_at")?;
    check_timestamp(job.scheduled_at, "scheduled_at")?;
    for (value, what) in [
        (job.next_run_at, "next_run_at"),
        (job.started_at, "started_at"),
        (job.completed_at, "completed_at"),
        (job.failed_at, "failed_at"),
        (job.timed_out_at, "timed_out_at"),
        (job.result_expires_at, "result_expires_at"),
    ] {
        if let Some(value) = value {
            check_timestamp(value, what)?;
        }
    }
    Ok(())
}

/// Decode a JSON array column (`depends_on`, `dependents`, `pii_fields`); `NULL` is an
/// empty list, anything that is not a list of the expected items is an error.
fn json_list<T: serde::de::DeserializeOwned>(
    value: Option<serde_json::Value>,
    column: &str,
) -> Result<Vec<T>> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(value) => {
            serde_json::from_value(value).map_err(|error| crate::HammerworkError::Queue {
                message: format!("Invalid {column} column: {error}"),
            })
        }
    }
}

/// The current time by the database clock (UTC).
///
/// Due times, run timestamps, leases and cron schedules are all compared with and
/// derived from the database clock, so clock skew between application servers does
/// not shift scheduling. `UTC_TIMESTAMP` rather than `NOW()` so the values are UTC
/// whatever the session `time_zone` is.
async fn db_now(conn: &mut sqlx::MySqlConnection) -> Result<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT UTC_TIMESTAMP(6)")
        .fetch_one(&mut *conn)
        .await?)
}

/// A `hammerwork_queue_pause` row.
fn pause_info_from_row(row: &sqlx::mysql::MySqlRow) -> Result<super::QueuePauseInfo> {
    Ok(super::QueuePauseInfo {
        queue_name: row.try_get("queue_name")?,
        paused_at: row.try_get("paused_at")?,
        paused_by: row.try_get("paused_by")?,
        reason: row.try_get("reason")?,
    })
}

/// The condition for a job of a queue (alias `j`; bind the queue name twice) that can
/// be dequeued now: pending, due by the database clock, not waiting on dependencies,
/// and its queue not paused. The pause check is uncorrelated and looks up the pause
/// table's primary key.
const RUNNABLE_IN_QUEUE_SQL: &str = "j.queue_name = ? AND j.status = 'Pending' \
     AND j.scheduled_at <= UTC_TIMESTAMP(6) AND j.dependency_status IN ('none', 'satisfied') \
     AND NOT EXISTS (SELECT 1 FROM hammerwork_queue_pause p WHERE p.queue_name = ?)";

/// Insert `jobs`, settling the dependency state of those that depend on jobs that
/// already finished (see [`lifecycle::settle_new_dependents`]).
///
/// The dependencies are locked `FOR SHARE` before their state is read and stay locked
/// until the caller's transaction commits. A dependency finishing concurrently either
/// finishes first (its transition holds the row lock, so we read its final state) or
/// waits for this insert to commit, after which its transition (READ COMMITTED) sees
/// the new jobs and releases or fails them as usual. Either way no new job is left
/// waiting forever.
async fn insert_dependent_jobs(conn: &mut sqlx::MySqlConnection, jobs: &mut [Job]) -> Result<()> {
    let parents = lifecycle::external_dependencies(jobs);
    if parents.is_empty() {
        return insert_jobs(conn, jobs).await;
    }

    let mut states = HashMap::new();
    for chunk in parents.chunks(ID_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let query = format!(
            "SELECT j.id, j.status, w.failure_policy FROM hammerwork_jobs j \
             LEFT JOIN hammerwork_workflows w ON w.id = j.workflow_id \
             WHERE j.id IN ({placeholders}) ORDER BY j.id FOR SHARE OF j"
        );
        let mut select = sqlx::query_as::<_, (String, String, Option<String>)>(&query);
        for id in chunk {
            select = select.bind(id.to_string());
        }
        for (id, status, policy) in select.fetch_all(&mut *conn).await? {
            states.insert(
                uuid::Uuid::parse_str(&id)?,
                ParentState::from_db(&status, policy.as_deref()),
            );
        }

        // Finished dependencies may already have been archived.
        let query =
            format!("SELECT id, status FROM hammerwork_jobs_archive WHERE id IN ({placeholders})");
        let mut select = sqlx::query_as::<_, (String, String)>(&query);
        for id in chunk {
            select = select.bind(id.to_string());
        }
        for (id, status) in select.fetch_all(&mut *conn).await? {
            states
                .entry(uuid::Uuid::parse_str(&id)?)
                .or_insert_with(|| ParentState::from_archived(&status));
        }
    }

    let now = db_now(conn).await?;
    let failed = lifecycle::settle_new_dependents(jobs, &states, now);
    insert_jobs(conn, jobs).await?;

    // Jobs inserted as failed count towards their workflow's progress.
    let mut workflows: Vec<uuid::Uuid> = jobs
        .iter()
        .filter(|job| failed.contains(&job.id))
        .filter_map(|job| job.workflow_id)
        .collect();
    workflows.sort_unstable();
    workflows.dedup();
    for id in workflows {
        if let Some(workflow) = crate::queue::JobQueue::<MySql>::lock_workflow(conn, id).await? {
            crate::queue::JobQueue::<MySql>::refresh_workflow(conn, &workflow, now).await?;
        }
    }
    Ok(())
}

/// The locked `hammerwork_workflows` row of a job's workflow.
struct LockedWorkflow {
    id: String,
    policy: FailurePolicy,
    status: WorkflowStatus,
    total_jobs: usize,
}

/// Parse the `id` column of a list of rows.
fn parse_ids(ids: Vec<String>) -> Result<Vec<JobId>> {
    ids.iter()
        .map(|id| uuid::Uuid::parse_str(id).map_err(Into::into))
        .collect()
}

/// Job lifecycle transitions and their side effects (dependencies, workflows, batches,
/// cron). See [`crate::queue::lifecycle`].
///
/// Transactions run at READ COMMITTED (like the dequeue), so each statement sees the
/// latest committed data: a parent completing after another one's transaction
/// committed sees that completion when it counts a shared child's dependencies.
impl crate::queue::JobQueue<MySql> {
    /// Apply `target` to a job if `guard` admits its current state, together with the
    /// side effects of the new state, in one transaction.
    async fn transition_job(
        &self,
        job_id: JobId,
        guard: Guard<'_>,
        target: &Target,
        reschedule_recurring: bool,
    ) -> Result<TransitionResult> {
        super::retry_on_conflict(|| {
            self.transition_job_once(job_id, guard, target, reschedule_recurring)
        })
        .await
    }

    async fn transition_job_once(
        &self,
        job_id: JobId,
        guard: Guard<'_>,
        target: &Target,
        reschedule_recurring: bool,
    ) -> Result<TransitionResult> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = Self::begin_claim_transaction(&mut conn).await?;
        let result =
            Self::transition_in_tx(&mut tx, job_id, guard, target, reschedule_recurring).await;
        super::end_transaction(tx, result).await
    }

    async fn transition_in_tx(
        conn: &mut sqlx::MySqlConnection,
        job_id: JobId,
        guard: Guard<'_>,
        target: &Target,
        reschedule_recurring: bool,
    ) -> Result<TransitionResult> {
        let row = sqlx::query_as::<_, JobRow>(&format!(
            "SELECT {JOB_SELECT_FIELDS} FROM hammerwork_jobs WHERE id = ? FOR UPDATE"
        ))
        .bind(job_id.to_string())
        .fetch_optional(&mut *conn)
        .await?;
        let Some(row) = row else {
            return Ok(TransitionResult::NotFound);
        };
        let job = row.into_job()?;
        if !guard.admits(&job) {
            return Ok(TransitionResult::Rejected(job.status));
        }

        let now = db_now(conn).await?;
        let target = target.on_db_clock(&guard, now);
        Self::write_target(conn, job_id, &target, now).await?;
        let recorded = match target.terminal_status() {
            Some(status) => {
                Self::after_terminal(conn, &job, status, now, reschedule_recurring).await?
            }
            None => RecordedOutcome::new(JobStatus::Pending),
        };
        Ok(TransitionResult::Applied(recorded))
    }

    /// Write the new status and its timestamps. Clears the lease: the run is over.
    async fn write_target(
        conn: &mut sqlx::MySqlConnection,
        job_id: JobId,
        target: &Target,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let id = job_id.to_string();
        let query = match target {
            Target::Completed => sqlx::query(
                "UPDATE hammerwork_jobs SET status = 'Completed', completed_at = ?, \
                 last_heartbeat_at = NULL, lease_expires_at = NULL WHERE id = ?",
            )
            .bind(now)
            .bind(id),
            Target::Failed(error) | Target::Dead(error) => sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ?, \
                 last_heartbeat_at = NULL, lease_expires_at = NULL WHERE id = ?",
            )
            .bind(target.terminal_status())
            .bind(error)
            .bind(now)
            .bind(id),
            Target::TimedOut(error) => sqlx::query(
                "UPDATE hammerwork_jobs SET status = 'TimedOut', error_message = ?, \
                 timed_out_at = ?, last_heartbeat_at = NULL, lease_expires_at = NULL \
                 WHERE id = ?",
            )
            .bind(error)
            .bind(now)
            .bind(id),
            Target::Retry {
                retry_at,
                error,
                timed_out,
            } => sqlx::query(
                "UPDATE hammerwork_jobs SET status = 'Pending', scheduled_at = ?, \
                 started_at = NULL, error_message = COALESCE(?, error_message), \
                 timed_out_at = COALESCE(?, timed_out_at), \
                 last_heartbeat_at = NULL, lease_expires_at = NULL WHERE id = ?",
            )
            // A backoff beyond the TIMESTAMP range retries at the latest storable time.
            .bind((*retry_at).min(timestamp_range().1))
            .bind(error)
            .bind(timed_out.then_some(now))
            .bind(id),
        };
        query.execute(&mut *conn).await?;
        Ok(())
    }

    /// Side effects of `job` reaching the terminal `status`.
    ///
    /// A recurring job (when `reschedule_recurring`) is rescheduled for its next run
    /// instead. Otherwise dependents are resolved or failed, the workflow policy and
    /// counters are applied, and the batch's failure mode and status.
    async fn after_terminal(
        conn: &mut sqlx::MySqlConnection,
        job: &Job,
        status: JobStatus,
        now: DateTime<Utc>,
        reschedule_recurring: bool,
    ) -> Result<RecordedOutcome> {
        if reschedule_recurring && job.recurring {
            if let Some(next_run_at) = lifecycle::next_cron_run(job, now)
                .filter(|next_run_at| check_timestamp(*next_run_at, "next run time").is_ok())
            {
                Self::reschedule_after_run(conn, job.id, next_run_at, status).await?;
                let mut recorded = RecordedOutcome::new(JobStatus::Pending);
                recorded.next_run_at = Some(next_run_at);
                return Ok(recorded);
            }
            tracing::warn!(
                "Could not calculate the next run time for recurring job {}; it stays {}",
                job.id,
                status.as_str()
            );
        }

        let mut recorded = RecordedOutcome::new(status);

        // Lock the workflow row first: it serializes terminal transitions within a
        // workflow, so concurrent completions cannot both miss each other.
        let workflow = match job.workflow_id {
            Some(id) => Self::lock_workflow(conn, id).await?,
            None => None,
        };

        if status == JobStatus::Completed {
            recorded.unblocked = Self::resolve_dependents(conn, job.id).await?;
        } else {
            recorded.cancelled =
                Self::apply_failure_policy(conn, workflow.as_ref(), job.id, now).await?;
        }

        if let Some(workflow) = &workflow {
            Self::refresh_workflow(conn, workflow, now).await?;
        }
        if let Some(batch_id) = job.batch_id {
            let cancelled = Self::apply_batch_outcome(conn, batch_id, job.id, status, now).await?;
            recorded.cancelled.extend(cancelled);
        }
        Ok(recorded)
    }

    /// Apply the [`FailurePolicy`] of `workflow` (the failed job's locked workflow, if
    /// any) to the failure of `failed_job_id`. Returns the jobs that were failed.
    ///
    /// - `FailFast` fails the workflow's other `Pending` and `Retrying` jobs, then the
    ///   jobs that (transitively) depend on the failed one;
    /// - `ContinueOnFailure`, and jobs with dependencies outside any workflow, fail
    ///   only the jobs that (transitively) depend on the failed one;
    /// - `Manual` leaves the dependents waiting for an operator.
    async fn apply_failure_policy(
        conn: &mut sqlx::MySqlConnection,
        workflow: Option<&LockedWorkflow>,
        failed_job_id: JobId,
        now: DateTime<Utc>,
    ) -> Result<Vec<JobId>> {
        match workflow {
            Some(w) if w.policy == FailurePolicy::Manual => Ok(Vec::new()),
            Some(w) if w.policy == FailurePolicy::FailFast => {
                let mut failed =
                    Self::fail_workflow_pending(conn, &w.id, failed_job_id, now).await?;
                for id in Self::fail_dependents(conn, failed_job_id, now).await? {
                    if !failed.contains(&id) {
                        failed.push(id);
                    }
                }
                Ok(failed)
            }
            _ => Self::fail_dependents(conn, failed_job_id, now).await,
        }
    }

    /// [`DatabaseQueue::fail_job_dependencies`] in one transaction: lock the failed job
    /// and its workflow (in the same order as a terminal transition), apply the
    /// workflow's failure policy and update the workflow's counters and status.
    async fn fail_job_dependencies_in_tx(
        conn: &mut sqlx::MySqlConnection,
        failed_job_id: JobId,
    ) -> Result<Vec<JobId>> {
        let workflow_id: Option<Option<String>> =
            sqlx::query_scalar("SELECT workflow_id FROM hammerwork_jobs WHERE id = ? FOR UPDATE")
                .bind(failed_job_id.to_string())
                .fetch_optional(&mut *conn)
                .await?;
        let workflow = match workflow_id.flatten() {
            Some(id) => Self::lock_workflow(conn, uuid::Uuid::parse_str(&id)?).await?,
            None => None,
        };
        let now = db_now(conn).await?;
        let failed =
            Self::apply_failure_policy(conn, workflow.as_ref(), failed_job_id, now).await?;
        if let Some(workflow) = &workflow {
            Self::refresh_workflow(conn, workflow, now).await?;
        }
        Ok(failed)
    }

    /// Put a recurring job back to `Pending` for its next run. A failed run's error and
    /// failure time stay visible until the next run; a successful run clears them.
    async fn reschedule_after_run(
        conn: &mut sqlx::MySqlConnection,
        job_id: JobId,
        next_run_at: DateTime<Utc>,
        run_status: JobStatus,
    ) -> Result<()> {
        let keep_failure = run_status != JobStatus::Completed;
        sqlx::query(
            r#"
            UPDATE hammerwork_jobs
            SET status = 'Pending',
                scheduled_at = ?,
                next_run_at = ?,
                attempts = 0,
                started_at = NULL,
                completed_at = NULL,
                failed_at = CASE WHEN ? THEN failed_at ELSE NULL END,
                timed_out_at = CASE WHEN ? THEN timed_out_at ELSE NULL END,
                error_message = CASE WHEN ? THEN error_message ELSE NULL END,
                last_heartbeat_at = NULL,
                lease_expires_at = NULL
            WHERE id = ?
            "#,
        )
        .bind(next_run_at)
        .bind(next_run_at)
        .bind(keep_failure)
        .bind(keep_failure)
        .bind(keep_failure)
        .bind(job_id.to_string())
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    async fn lock_workflow(
        conn: &mut sqlx::MySqlConnection,
        workflow_id: uuid::Uuid,
    ) -> Result<Option<LockedWorkflow>> {
        let id = workflow_id.to_string();
        let row = sqlx::query(
            "SELECT failure_policy, status, total_jobs FROM hammerwork_workflows \
             WHERE id = ? FOR UPDATE",
        )
        .bind(&id)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(LockedWorkflow {
            id,
            policy: FailurePolicy::parse_from_db(&row.try_get::<String, _>("failure_policy")?)?,
            status: WorkflowStatus::parse_from_db(&row.try_get::<String, _>("status")?)?,
            total_jobs: usize::try_from(row.try_get::<i32, _>("total_jobs")?).unwrap_or(0),
        }))
    }

    /// Lock the rows among `candidates` that still match `condition`, by primary key
    /// and in id order, and return their `columns`.
    ///
    /// Callers find candidates with a non-locking read first. A locking read that has
    /// to scan (e.g. `JSON_CONTAINS`, which no index serves) would lock or wait on
    /// every row it scans, including rows that concurrent dequeues and transitions
    /// hold; locking by primary key touches only the rows we change.
    async fn lock_rows_by_id(
        conn: &mut sqlx::MySqlConnection,
        candidates: &[String],
        columns: &str,
        condition: &str,
    ) -> Result<Vec<sqlx::mysql::MySqlRow>> {
        let mut rows = Vec::new();
        for chunk in candidates.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let query = format!(
                "SELECT {columns} FROM hammerwork_jobs WHERE id IN ({placeholders}) \
                 AND {condition} ORDER BY id FOR UPDATE"
            );
            let mut select = sqlx::query(&query);
            for id in chunk {
                select = select.bind(id);
            }
            rows.extend(select.fetch_all(&mut *conn).await?);
        }
        Ok(rows)
    }

    /// Set `status = 'Failed'` on the (already locked) jobs `ids`, recording `error`.
    /// Jobs still waiting on dependencies get `dependency_status = 'failed'`.
    async fn fail_jobs_by_id(
        conn: &mut sqlx::MySqlConnection,
        ids: &[String],
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<()> {
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let query = format!(
                "UPDATE hammerwork_jobs SET status = 'Failed', failed_at = ?, error_message = ?, \
                 dependency_status = CASE WHEN dependency_status IN ('waiting', 'satisfied') \
                 THEN 'failed' ELSE dependency_status END WHERE id IN ({placeholders})"
            );
            let mut update = sqlx::query(&query).bind(now).bind(error);
            for id in chunk {
                update = update.bind(id);
            }
            update.execute(&mut *conn).await?;
        }
        Ok(())
    }

    /// Mark `waiting` dependents of `completed_job_id` whose dependencies have all
    /// completed as `satisfied`, so they can be dequeued.
    ///
    /// The dependents are locked (in id order) before their dependencies are checked.
    /// Two parents completing at once therefore take turns on a shared child, and the
    /// second one sees the first one's committed completion.
    ///
    /// The dependencies of all dependents are checked with one query, and the satisfied
    /// dependents updated with one statement, so the number of queries does not grow
    /// with the number of dependents. Finding the dependents uses `JSON_CONTAINS`,
    /// which no index serves: it scans the queue's rows (see `lock_rows_by_id`).
    async fn resolve_dependents(
        conn: &mut sqlx::MySqlConnection,
        completed_job_id: JobId,
    ) -> Result<Vec<JobId>> {
        let candidates: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM hammerwork_jobs WHERE JSON_CONTAINS(depends_on, ?) \
             AND dependency_status = 'waiting' AND status = 'Pending'",
        )
        .bind(serde_json::json!([completed_job_id.to_string()]))
        .fetch_all(&mut *conn)
        .await?;
        let dependents = Self::lock_rows_by_id(
            conn,
            &candidates,
            "id, depends_on",
            "dependency_status = 'waiting' AND status = 'Pending'",
        )
        .await?
        .iter()
        .map(|row| {
            let id: String = row.try_get("id")?;
            let depends_on: Vec<String> = json_list(row.try_get("depends_on")?, "depends_on")?;
            Ok((id, depends_on))
        })
        .collect::<Result<Vec<_>>>()?;

        // Completed dependencies of all dependents; they may already have been archived.
        let mut dependencies: Vec<&String> = dependents
            .iter()
            .flat_map(|(_, depends_on)| depends_on)
            .collect();
        dependencies.sort_unstable();
        dependencies.dedup();
        let mut completed = std::collections::HashSet::new();
        for chunk in dependencies.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let query = format!(
                "SELECT id FROM hammerwork_jobs WHERE id IN ({placeholders}) \
                 AND status = 'Completed' \
                 UNION SELECT id FROM hammerwork_jobs_archive WHERE id IN ({placeholders}) \
                 AND status = 'Completed'"
            );
            let mut select = sqlx::query_scalar::<_, String>(&query);
            for _ in 0..2 {
                for id in chunk {
                    select = select.bind(*id);
                }
            }
            completed.extend(select.fetch_all(&mut *conn).await?);
        }

        let satisfied: Vec<String> = dependents
            .into_iter()
            .filter(|(_, depends_on)| depends_on.iter().all(|id| completed.contains(id)))
            .map(|(id, _)| id)
            .collect();
        for chunk in satisfied.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let query = format!(
                "UPDATE hammerwork_jobs SET dependency_status = 'satisfied' \
                 WHERE id IN ({placeholders})"
            );
            let mut update = sqlx::query(&query);
            for id in chunk {
                update = update.bind(id);
            }
            update.execute(&mut *conn).await?;
        }
        parse_ids(satisfied)
    }

    /// Cancel a workflow: its unfinished jobs (pending, retrying and **running**) become
    /// `Failed` with "Workflow cancelled", and the workflow `cancelled`.
    ///
    /// A running job keeps running in its worker (a handler cannot be interrupted), but
    /// its outcome is discarded: `finish_job_run` only records outcomes for jobs that
    /// are still `Running` the same run, and heartbeats stop extending the lease.
    async fn cancel_workflow_in_tx(
        conn: &mut sqlx::MySqlConnection,
        workflow_id: crate::workflow::WorkflowId,
    ) -> Result<()> {
        // Serializes with the terminal transitions of the workflow's jobs.
        Self::lock_workflow(conn, workflow_id).await?;
        let now = db_now(conn).await?;
        let id = workflow_id.to_string();
        let candidates: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT id FROM hammerwork_jobs WHERE workflow_id = ? AND status IN ({})",
            lifecycle::UNFINISHED_STATUS_SQL
        ))
        .bind(&id)
        .fetch_all(&mut *conn)
        .await?;
        let ids: Vec<String> = Self::lock_rows_by_id(
            conn,
            &candidates,
            "id",
            &format!("status IN ({})", lifecycle::UNFINISHED_STATUS_SQL),
        )
        .await?
        .iter()
        .map(|row| row.try_get("id"))
        .collect::<std::result::Result<_, _>>()?;
        for chunk in ids.chunks(ID_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(", ");
            let query = format!(
                "UPDATE hammerwork_jobs SET status = 'Failed', failed_at = ?, error_message = ?, \
                 dependency_status = CASE WHEN dependency_status = 'waiting' \
                 THEN 'failed' ELSE dependency_status END, \
                 last_heartbeat_at = NULL, lease_expires_at = NULL WHERE id IN ({placeholders})"
            );
            let mut update = sqlx::query(&query)
                .bind(now)
                .bind(lifecycle::WORKFLOW_CANCELLED_MESSAGE);
            for id in chunk {
                update = update.bind(id);
            }
            update.execute(&mut *conn).await?;
        }

        sqlx::query(
            "UPDATE hammerwork_workflows SET status = 'cancelled', \
             failed_at = COALESCE(failed_at, ?) WHERE id = ?",
        )
        .bind(now)
        .bind(&id)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Fail every pending job that (transitively) depends on `failed_job_id`.
    async fn fail_dependents(
        conn: &mut sqlx::MySqlConnection,
        failed_job_id: JobId,
        now: DateTime<Utc>,
    ) -> Result<Vec<JobId>> {
        const CONDITION: &str =
            "dependency_status IN ('waiting', 'satisfied') AND status = 'Pending'";
        let mut failed = Vec::new();
        let mut to_check = vec![failed_job_id];
        while let Some(current) = to_check.pop() {
            let candidates: Vec<String> = sqlx::query_scalar(&format!(
                "SELECT id FROM hammerwork_jobs WHERE JSON_CONTAINS(depends_on, ?) AND {CONDITION}"
            ))
            .bind(serde_json::json!([current.to_string()]))
            .fetch_all(&mut *conn)
            .await?;
            let ids: Vec<String> = Self::lock_rows_by_id(conn, &candidates, "id", CONDITION)
                .await?
                .iter()
                .map(|row| row.try_get("id"))
                .collect::<std::result::Result<_, _>>()?;
            if ids.is_empty() {
                continue;
            }
            Self::fail_jobs_by_id(
                conn,
                &ids,
                &lifecycle::dependency_failed_message(current),
                now,
            )
            .await?;

            for id in parse_ids(ids)? {
                if !failed.contains(&id) {
                    failed.push(id);
                    to_check.push(id);
                }
            }
        }
        Ok(failed)
    }

    /// Fail the jobs among `candidates` that have not started yet (`Pending` or
    /// `Retrying`), recording `error`. Returns the jobs failed.
    async fn fail_unstarted(
        conn: &mut sqlx::MySqlConnection,
        candidates: &[String],
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<JobId>> {
        let ids: Vec<String> =
            Self::lock_rows_by_id(conn, candidates, "id", "status IN ('Pending', 'Retrying')")
                .await?
                .iter()
                .map(|row| row.try_get("id"))
                .collect::<std::result::Result<_, _>>()?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Self::fail_jobs_by_id(conn, &ids, error, now).await?;
        parse_ids(ids)
    }

    /// Fail the pending jobs of a fail-fast workflow after `failed_job_id` failed.
    async fn fail_workflow_pending(
        conn: &mut sqlx::MySqlConnection,
        workflow_id: &str,
        failed_job_id: JobId,
        now: DateTime<Utc>,
    ) -> Result<Vec<JobId>> {
        let candidates: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM hammerwork_jobs WHERE workflow_id = ? AND id <> ? \
             AND status IN ('Pending', 'Retrying')",
        )
        .bind(workflow_id)
        .bind(failed_job_id.to_string())
        .fetch_all(&mut *conn)
        .await?;
        Self::fail_unstarted(
            conn,
            &candidates,
            &lifecycle::workflow_failed_message(failed_job_id),
            now,
        )
        .await
    }

    /// Recompute a workflow's counters and status from its jobs.
    async fn refresh_workflow(
        conn: &mut sqlx::MySqlConnection,
        workflow: &LockedWorkflow,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE workflow_id = ? GROUP BY status",
        )
        .bind(&workflow.id)
        .fetch_all(&mut *conn)
        .await?;
        let progress = WorkflowProgress::from_status_counts(counts);
        let status = progress.status(workflow.total_jobs, &workflow.policy, &workflow.status);

        sqlx::query(
            r#"
            UPDATE hammerwork_workflows
            SET completed_jobs = ?,
                failed_jobs = ?,
                status = ?,
                completed_at = COALESCE(completed_at, ?),
                failed_at = COALESCE(failed_at, ?)
            WHERE id = ?
            "#,
        )
        .bind(progress.completed as i32)
        .bind(progress.failed as i32)
        .bind(status.as_str())
        .bind((status == WorkflowStatus::Completed).then_some(now))
        .bind((status == WorkflowStatus::Failed).then_some(now))
        .bind(&workflow.id)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Apply a batch job's terminal `status` to its batch: a `FailFast` batch fails its
    /// pending jobs on the first failure, and the batch's counters and status are
    /// persisted once the batch has finished (or failed fast). Returns the jobs failed.
    async fn apply_batch_outcome(
        conn: &mut sqlx::MySqlConnection,
        batch_id: uuid::Uuid,
        job_id: JobId,
        status: JobStatus,
        now: DateTime<Utc>,
    ) -> Result<Vec<JobId>> {
        let batch = batch_id.to_string();
        // Locking the batch row serializes its jobs' terminal transitions, so exactly
        // one of them sees the batch finish.
        let row = sqlx::query(
            "SELECT total_jobs, failure_mode FROM hammerwork_batches WHERE id = ? FOR UPDATE",
        )
        .bind(&batch)
        .fetch_optional(&mut *conn)
        .await?;
        let Some(row) = row else {
            return Ok(Vec::new());
        };
        let total_jobs: i32 = row.try_get("total_jobs")?;
        let mode = crate::batch::parse_failure_mode(&row.try_get::<String, _>("failure_mode")?);

        let fail_fast = status != JobStatus::Completed && mode == PartialFailureMode::FailFast;
        let mut cancelled = Vec::new();
        if fail_fast {
            let candidates: Vec<String> = sqlx::query_scalar(
                "SELECT id FROM hammerwork_jobs WHERE batch_id = ? \
                 AND status IN ('Pending', 'Retrying')",
            )
            .bind(&batch)
            .fetch_all(&mut *conn)
            .await?;
            cancelled = Self::fail_unstarted(
                conn,
                &candidates,
                &lifecycle::batch_failed_message(job_id),
                now,
            )
            .await?;
        }

        let unfinished: i64 = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM hammerwork_jobs WHERE batch_id = ? \
             AND status IN ({}))",
            lifecycle::UNFINISHED_STATUS_SQL
        ))
        .bind(&batch)
        .fetch_one(&mut *conn)
        .await?;

        if unfinished != 0 && !fail_fast {
            sqlx::query("UPDATE hammerwork_batches SET status = ? WHERE id = ? AND status = ?")
                .bind(BatchStatus::Processing)
                .bind(&batch)
                .bind(BatchStatus::Pending)
                .execute(&mut *conn)
                .await?;
            return Ok(cancelled);
        }

        let counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM (
                SELECT status FROM hammerwork_jobs WHERE batch_id = ?
                UNION ALL
                SELECT status FROM hammerwork_jobs_archive WHERE batch_id = ?
            ) batch_jobs GROUP BY status",
        )
        .bind(&batch)
        .bind(&batch)
        .fetch_all(&mut *conn)
        .await?;
        let progress = BatchProgress::from_status_counts(
            counts,
            super::db_int(total_jobs, "total_jobs")?,
            &mode,
        );

        sqlx::query(
            r#"
            UPDATE hammerwork_batches
            SET completed_jobs = ?,
                failed_jobs = ?,
                pending_jobs = ?,
                status = ?,
                completed_at = COALESCE(completed_at, ?),
                error_summary = COALESCE(?, error_summary)
            WHERE id = ?
            "#,
        )
        .bind(progress.completed as i32)
        .bind(progress.failed as i32)
        .bind(progress.pending as i32)
        .bind(progress.status)
        .bind((progress.pending == 0).then_some(now))
        .bind(
            (progress.failed > 0)
                .then(|| format!("{} of {} jobs failed", progress.failed, total_jobs)),
        )
        .bind(&batch)
        .execute(&mut *conn)
        .await?;
        Ok(cancelled)
    }

    /// Apply a manual transition that writes no side effects (`retry_dead_job`,
    /// `reschedule_cron_job`): lock the row, check the guard, then run `update`.
    ///
    /// The status is checked under the row lock rather than by the UPDATE's
    /// affected-row count, because MySQL reports rows *changed*, not rows matched.
    async fn guarded_update(
        &self,
        job_id: JobId,
        transition: JobTransition,
        update: sqlx::query::Query<'_, MySql, sqlx::mysql::MySqlArguments>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let result = async {
            let row = sqlx::query(
                "SELECT status, recurring FROM hammerwork_jobs WHERE id = ? FOR UPDATE",
            )
            .bind(job_id.to_string())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(row) = row else {
                return Err(lifecycle::rejection_error(job_id, transition, None));
            };
            let status: String = row.try_get("status")?;
            let recurring: bool = row.try_get("recurring")?;
            let allowed = lifecycle::job_status_from_db(&status)
                .is_some_and(|status| transition.is_allowed_from(status))
                && (transition != JobTransition::RescheduleCron || recurring);
            if !allowed {
                return Err(lifecycle::rejection_error(
                    job_id,
                    transition,
                    Some(&status),
                ));
            }
            update.execute(&mut *tx).await?;
            Ok(())
        }
        .await;
        super::end_transaction(tx, result).await
    }
}
