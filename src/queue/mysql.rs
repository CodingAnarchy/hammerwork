//! MySQL implementation of the job queue.
//!
//! This module provides the MySQL-specific implementation of the `DatabaseQueue` trait,
//! optimized for MySQL's JSON support and transactional capabilities.

use super::{DatabaseQueue, DeadJobSummary, QueueStats};
use crate::{
    Result,
    job::{Job, JobId, JobStatus},
    priority::JobPriority,
    rate_limit::ThrottleConfig,
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
    // Encryption fields
    pub is_encrypted: bool,
    pub encryption_key_id: Option<String>,
    pub encryption_algorithm: Option<String>,
    pub encrypted_payload: Option<Vec<u8>>,
    pub encryption_nonce: Option<Vec<u8>>,
    pub encryption_tag: Option<Vec<u8>>,
    pub encryption_metadata: Option<serde_json::Value>,
    pub payload_hash: Option<String>,
    pub pii_fields: Option<serde_json::Value>,
    pub retention_policy: Option<String>,
    pub retention_delete_at: Option<DateTime<Utc>>,
    pub encrypted_at: Option<DateTime<Utc>>,
}

/// Standard field list for selecting complete job data from hammerwork_jobs table.
const JOB_SELECT_FIELDS: &str = "id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context, is_encrypted, encryption_key_id, encryption_algorithm, encrypted_payload, encryption_nonce, encryption_tag, encryption_metadata, payload_hash, pii_fields, retention_policy, retention_delete_at, encrypted_at";

impl JobRow {
    pub fn into_job(self) -> Result<Job> {
        // Extract encryption data before moving self
        #[cfg(feature = "encryption")]
        let encryption_config = self.build_encryption_config()?;
        #[cfg(feature = "encryption")]
        let retention_policy = self.parse_retention_policy()?;
        #[cfg(feature = "encryption")]
        let encrypted_payload = self.build_encrypted_payload()?;
        let pii_fields = self.parse_pii_fields();
        
        Ok(Job {
            id: uuid::Uuid::parse_str(&self.id)?,
            queue_name: self.queue_name,
            payload: self.payload,
            status: serde_json::from_str(&self.status)?,
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
                .map(|s| std::time::Duration::from_secs(s as u64)),
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
                    .map(|s| std::time::Duration::from_secs(s as u64)),
                max_size_bytes: self.result_max_size_bytes.map(|b| b as usize),
            },
            result_data: self.result_data,
            result_stored_at: self.result_stored_at,
            result_expires_at: self.result_expires_at,
            retry_strategy: None,
            depends_on: self
                .depends_on
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            dependents: self
                .dependents
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            dependency_status: self
                .dependency_status
                .as_ref()
                .and_then(|s| crate::workflow::DependencyStatus::parse_from_db(s).ok())
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
                    "Unknown encryption algorithm: {}", alg
                )));
            }
            None => {
                return Err(crate::HammerworkError::Processing(
                    "Missing encryption algorithm for encrypted job".to_string()
                ));
            }
        };

        // Parse metadata if available
        let (key_id, compression_enabled, version) = if let Some(metadata) = &self.encryption_metadata {
            let key_id = metadata.get("key_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let compression_enabled = metadata.get("compressed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let version = metadata.get("config_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            (key_id, compression_enabled, version)
        } else {
            (self.encryption_key_id.clone(), false, 1)
        };

        let config = crate::encryption::EncryptionConfig {
            algorithm,
            key_source: crate::encryption::KeySource::External(
                key_id.unwrap_or_else(|| "unknown".to_string())
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

    /// Builds an EncryptionConfig from database fields - no-op when encryption is disabled.
    #[cfg(not(feature = "encryption"))]
    fn build_encryption_config(&self) -> Result<Option<()>> {
        Ok(None)
    }

    /// Parses the retention policy from the database string.
    #[cfg(feature = "encryption")]
    fn parse_retention_policy(&self) -> Result<Option<crate::encryption::RetentionPolicy>> {
        match self.retention_policy.as_deref() {
            None => Ok(None),
            Some("KeepIndefinitely") => Ok(Some(crate::encryption::RetentionPolicy::KeepIndefinitely)),
            Some("DeleteImmediately") => Ok(Some(crate::encryption::RetentionPolicy::DeleteImmediately)),
            Some("UseDefault") => Ok(Some(crate::encryption::RetentionPolicy::UseDefault)),
            Some(policy_str) => {
                // Handle DeleteAfter and DeleteAt policies
                if let Some(delete_at) = self.retention_delete_at {
                    if policy_str == "DeleteAt" {
                        Ok(Some(crate::encryption::RetentionPolicy::DeleteAt(delete_at)))
                    } else if policy_str == "DeleteAfter" {
                        // Calculate duration from creation to deletion time
                        let duration = delete_at.signed_duration_since(self.created_at);
                        if let Ok(std_duration) = duration.to_std() {
                            Ok(Some(crate::encryption::RetentionPolicy::DeleteAfter(std_duration)))
                        } else {
                            Ok(Some(crate::encryption::RetentionPolicy::UseDefault))
                        }
                    } else {
                        Err(crate::HammerworkError::Processing(format!(
                            "Unknown retention policy: {}", policy_str
                        )))
                    }
                } else {
                    Err(crate::HammerworkError::Processing(format!(
                        "Retention policy '{}' requires retention_delete_at timestamp", policy_str
                    )))
                }
            }
        }
    }

    /// Parses the retention policy - no-op when encryption is disabled.
    #[cfg(not(feature = "encryption"))]
    fn parse_retention_policy(&self) -> Result<Option<()>> {
        Ok(None)
    }

    /// Builds an EncryptedPayload from database fields if the job is encrypted.
    #[cfg(feature = "encryption")]
    fn build_encrypted_payload(&self) -> Result<Option<crate::encryption::EncryptedPayload>> {
        if !self.is_encrypted {
            return Ok(None);
        }

        let encrypted_data = self.encrypted_payload.as_ref().ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encrypted payload for encrypted job".to_string()
            )
        })?;

        let nonce = self.encryption_nonce.as_ref().ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encryption nonce for encrypted job".to_string()
            )
        })?;

        let tag = self.encryption_tag.as_ref().ok_or_else(|| {
            crate::HammerworkError::Processing(
                "Missing encryption tag for encrypted job".to_string()
            )
        })?;

        // Parse metadata
        let metadata = if let Some(metadata_json) = &self.encryption_metadata {
            // Build metadata from JSON
            let algorithm = match self.encryption_algorithm.as_deref() {
                Some("AES256GCM") => crate::encryption::EncryptionAlgorithm::AES256GCM,
                Some("ChaCha20Poly1305") => crate::encryption::EncryptionAlgorithm::ChaCha20Poly1305,
                _ => crate::encryption::EncryptionAlgorithm::AES256GCM, // Default fallback
            };

            let key_id = metadata_json.get("key_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| self.encryption_key_id.clone())
                .unwrap_or_else(|| "unknown".to_string());

            let config_version = metadata_json.get("config_version")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;

            let compressed = metadata_json.get("compressed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let encrypted_fields = metadata_json.get("encrypted_fields")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_else(|| self.parse_pii_fields());

            let retention_policy = self.parse_retention_policy()?
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
            }
        } else {
            // Build basic metadata from available fields
            let algorithm = match self.encryption_algorithm.as_deref() {
                Some("AES256GCM") => crate::encryption::EncryptionAlgorithm::AES256GCM,
                Some("ChaCha20Poly1305") => crate::encryption::EncryptionAlgorithm::ChaCha20Poly1305,
                _ => crate::encryption::EncryptionAlgorithm::AES256GCM, // Default fallback
            };

            crate::encryption::EncryptionMetadata {
                algorithm,
                key_id: self.encryption_key_id.clone().unwrap_or_else(|| "unknown".to_string()),
                config_version: 1,
                compressed: false,
                encrypted_fields: self.parse_pii_fields(),
                retention_policy: self.parse_retention_policy()?
                    .unwrap_or(crate::encryption::RetentionPolicy::UseDefault),
                encrypted_at: self.encrypted_at.unwrap_or(self.created_at),
                delete_at: self.retention_delete_at,
                payload_hash: self.payload_hash.clone().unwrap_or_default(),
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

    /// Builds an EncryptedPayload - no-op when encryption is disabled.
    #[cfg(not(feature = "encryption"))]
    fn build_encrypted_payload(&self) -> Result<Option<()>> {
        Ok(None)
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
            status: serde_json::from_str(&self.status).unwrap_or(JobStatus::Dead),
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
                .map(|s| std::time::Duration::from_secs(s as u64)),
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

#[async_trait]
impl DatabaseQueue for crate::queue::JobQueue<MySql> {
    type Database = MySql;

    async fn enqueue(&self, job: Job) -> Result<JobId> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_jobs 
            (id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_storage_type, result_ttl_seconds, result_max_size_bytes, depends_on, dependents, dependency_status, workflow_id, workflow_name, trace_id, correlation_id, parent_span_id, span_context)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(job.id.to_string())
        .bind(&job.queue_name)
        .bind(&job.payload)
        .bind(job.status)
        .bind(job.priority.as_i32())
        .bind(job.attempts)
        .bind(job.max_attempts)
        .bind(job.timeout.map(|t| t.as_secs() as i32))
        .bind(job.created_at)
        .bind(job.scheduled_at)
        .bind(job.started_at)
        .bind(job.completed_at)
        .bind(job.failed_at)
        .bind(job.timed_out_at)
        .bind(&job.error_message)
        .bind(&job.cron_schedule)
        .bind(job.next_run_at)
        .bind(job.recurring)
        .bind(&job.timezone)
        .bind(None::<String>) // batch_id is None for individual jobs
        .bind(match job.result_config.storage {
            crate::job::ResultStorage::Database => "database",
            crate::job::ResultStorage::Memory => "memory",
            crate::job::ResultStorage::None => "none",
        })
        .bind(job.result_config.ttl.map(|d| d.as_secs() as i64))
        .bind(job.result_config.max_size_bytes.map(|s| s as i64))
        .bind(serde_json::to_value(&job.depends_on)?)
        .bind(serde_json::to_value(&job.dependents)?)
        .bind(job.dependency_status.as_str())
        .bind(job.workflow_id.map(|id| id.to_string()))
        .bind(&job.workflow_name)
        .bind(&job.trace_id)
        .bind(&job.correlation_id)
        .bind(&job.parent_span_id)
        .bind(&job.span_context)
        .execute(&self.pool)
        .await?;

        Ok(job.id)
    }

    async fn dequeue(&self, queue_name: &str) -> Result<Option<Job>> {
        use crate::job::JobStatus;

        // MySQL doesn't support FOR UPDATE SKIP LOCKED in the same way
        // This is a simplified version - in production you might want advisory locks
        let mut tx = self.pool.begin().await?;

        let row = sqlx::query_as::<_, JobRow>(
            &format!(
            r#"
            SELECT {}
            FROM hammerwork_jobs 
            WHERE queue_name = ? 
            AND status = ? 
            AND scheduled_at <= ?
            ORDER BY priority DESC, scheduled_at ASC 
            LIMIT 1
            FOR UPDATE
            "#, JOB_SELECT_FIELDS)
        )
        .bind(queue_name)
        .bind(JobStatus::Pending)
        .bind(Utc::now())
        .fetch_optional(&mut *tx)
        .await?;

        if let Some(job_row) = row {
            let _job_id = uuid::Uuid::parse_str(&job_row.id)?;

            sqlx::query(
                "UPDATE hammerwork_jobs SET status = ?, started_at = ?, attempts = attempts + 1 WHERE id = ?"
            )
            .bind(JobStatus::Running)
            .bind(Utc::now())
            .bind(&job_row.id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;

            let mut job = job_row.into_job()?;
            job.status = JobStatus::Running;
            job.attempts += 1;
            job.started_at = Some(Utc::now());

            Ok(Some(job))
        } else {
            tx.rollback().await?;
            Ok(None)
        }
    }

    async fn dequeue_with_priority_weights(
        &self,
        queue_name: &str,
        weights: &crate::priority::PriorityWeights,
    ) -> Result<Option<Job>> {
        use crate::job::JobStatus;
        use crate::priority::JobPriority;

        if weights.is_strict() {
            // Use strict priority - same as regular dequeue
            return self.dequeue(queue_name).await;
        }

        let mut tx = self.pool.begin().await?;

        // Get available jobs by priority
        let available_jobs = sqlx::query_as::<_, JobRow>(
            &format!(
            r#"
            SELECT {}
            FROM hammerwork_jobs 
            WHERE queue_name = ? 
            AND status = ? 
            AND scheduled_at <= ?
            ORDER BY priority DESC, scheduled_at ASC 
            LIMIT 20
            FOR UPDATE
            "#, JOB_SELECT_FIELDS)
        )
        .bind(queue_name)
        .bind(JobStatus::Pending)
        .bind(Utc::now())
        .fetch_all(&mut *tx)
        .await?;

        if available_jobs.is_empty() {
            tx.rollback().await?;
            return Ok(None);
        }

        // Group jobs by priority and apply weighted selection
        let mut priority_jobs: std::collections::HashMap<JobPriority, Vec<_>> =
            std::collections::HashMap::new();

        for job_row in available_jobs {
            let priority = JobPriority::from_i32(job_row.priority).unwrap_or(JobPriority::Normal);
            priority_jobs.entry(priority).or_default().push(job_row);
        }

        // Calculate weighted selection
        let mut weighted_choices = Vec::new();
        for priority in priority_jobs.keys() {
            let weight = weights.get_weight(*priority);
            for _ in 0..weight {
                weighted_choices.push(priority);
            }
        }

        if weighted_choices.is_empty() {
            tx.rollback().await?;
            return Ok(None);
        }

        // Use a simple hash-based selection instead of thread_rng for Send compatibility
        let selection_index = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            queue_name.hash(&mut hasher);
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or(0)
                .hash(&mut hasher);
            (hasher.finish() as usize) % weighted_choices.len()
        };
        let selected_priority = weighted_choices[selection_index];

        // Select the oldest job from the selected priority
        if let Some(jobs) = priority_jobs.get(selected_priority) {
            if let Some(selected_job) = jobs.first() {
                // Update the selected job
                sqlx::query(
                    "UPDATE hammerwork_jobs SET status = ?, started_at = ?, attempts = attempts + 1 WHERE id = ?"
                )
                .bind(JobStatus::Running)
                .bind(Utc::now())
                .bind(&selected_job.id)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;

                let mut job = selected_job.clone().into_job()?;
                job.status = JobStatus::Running;
                job.attempts += 1;
                job.started_at = Some(Utc::now());

                return Ok(Some(job));
            }
        }

        tx.rollback().await?;
        Ok(None)
    }

    async fn complete_job(&self, job_id: JobId) -> Result<()> {
        use crate::job::JobStatus;

        sqlx::query("UPDATE hammerwork_jobs SET status = ?, completed_at = ? WHERE id = ?")
            .bind(JobStatus::Completed)
            .bind(Utc::now())
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn fail_job(&self, job_id: JobId, error_message: &str) -> Result<()> {
        use crate::job::JobStatus;

        sqlx::query(
            "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ? WHERE id = ?",
        )
        .bind(JobStatus::Failed)
        .bind(error_message)
        .bind(Utc::now())
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn retry_job(&self, job_id: JobId, retry_at: DateTime<Utc>) -> Result<()> {
        use crate::job::JobStatus;

        sqlx::query(
            "UPDATE hammerwork_jobs SET status = ?, scheduled_at = ?, started_at = NULL WHERE id = ?"
        )
        .bind(JobStatus::Pending)
        .bind(retry_at)
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_job(&self, job_id: JobId) -> Result<Option<Job>> {
        let row = sqlx::query_as::<_, JobRow>(
            &format!("SELECT {} FROM hammerwork_jobs WHERE id = ?", JOB_SELECT_FIELDS)
        )
        .bind(job_id.to_string())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(job_row) => Ok(Some(job_row.into_job()?)),
            None => Ok(None),
        }
    }

    async fn delete_job(&self, job_id: JobId) -> Result<()> {
        sqlx::query("DELETE FROM hammerwork_jobs WHERE id = ?")
            .bind(job_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn enqueue_batch(&self, batch: crate::batch::JobBatch) -> Result<crate::batch::BatchId> {
        use crate::batch::BatchStatus;

        // Validate the batch first
        batch.validate()?;

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
        .bind(serde_json::to_string(&BatchStatus::Pending)?)
        .bind(serde_json::to_string(&batch.failure_mode)?)
        .bind(batch.created_at)
        .bind(serde_json::to_value(&batch.metadata)?)
        .execute(&mut *tx)
        .await?;

        // Bulk insert jobs using multiple-row VALUES syntax
        if !batch.jobs.is_empty() {
            let chunk_size = 100; // MySQL has limits on max_allowed_packet and query size
            for chunk in batch.jobs.chunks(chunk_size) {
                // Build query with proper parameter bindings
                let mut query = "INSERT INTO hammerwork_jobs (id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds, created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message, cron_schedule, next_run_at, recurring, timezone, batch_id, result_storage_type, result_ttl_seconds, result_max_size_bytes) VALUES ".to_string();

                for (i, _) in chunk.iter().enumerate() {
                    if i > 0 {
                        query.push_str(", ");
                    }
                    query.push_str(
                        "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    );
                }

                let mut prepared_query = sqlx::query(&query);
                for job in chunk {
                    prepared_query = prepared_query
                        .bind(job.id.to_string())
                        .bind(&job.queue_name)
                        .bind(&job.payload)
                        .bind(job.status)
                        .bind(job.priority.as_i32())
                        .bind(job.attempts)
                        .bind(job.max_attempts)
                        .bind(job.timeout.map(|t| t.as_secs() as i32))
                        .bind(job.created_at)
                        .bind(job.scheduled_at)
                        .bind(job.started_at)
                        .bind(job.completed_at)
                        .bind(job.failed_at)
                        .bind(job.timed_out_at)
                        .bind(&job.error_message)
                        .bind(&job.cron_schedule)
                        .bind(job.next_run_at)
                        .bind(job.recurring)
                        .bind(&job.timezone)
                        .bind(batch.id.to_string())
                        .bind(match job.result_config.storage {
                            crate::job::ResultStorage::Database => Some("database".to_string()),
                            crate::job::ResultStorage::Memory => Some("memory".to_string()),
                            crate::job::ResultStorage::None => Some("none".to_string()),
                        })
                        .bind(job.result_config.ttl.map(|d| d.as_secs() as i64))
                        .bind(job.result_config.max_size_bytes.map(|b| b as i64));
                }

                prepared_query.execute(&mut *tx).await?;
            }
        }

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

        let total_jobs: i32 = batch_row.get("total_jobs");
        let completed_jobs: i32 = batch_row.get("completed_jobs");
        let failed_jobs: i32 = batch_row.get("failed_jobs");
        let pending_jobs: i32 = batch_row.get("pending_jobs");
        let status: String = batch_row.get("status");
        let created_at: DateTime<Utc> = batch_row.get("created_at");
        let completed_at: Option<DateTime<Utc>> = batch_row.get("completed_at");
        let error_summary: Option<String> = batch_row.get("error_summary");

        // Get job errors for CollectErrors mode
        let job_errors: Vec<(String, String)> = sqlx::query_as(
            "SELECT id, error_message FROM hammerwork_jobs WHERE batch_id = ? AND error_message IS NOT NULL"
        )
        .bind(batch_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let job_errors_map: HashMap<uuid::Uuid, String> = job_errors
            .into_iter()
            .filter_map(|(id_str, error)| uuid::Uuid::parse_str(&id_str).ok().map(|id| (id, error)))
            .collect();

        Ok(BatchResult {
            batch_id,
            total_jobs: total_jobs as u32,
            completed_jobs: completed_jobs as u32,
            failed_jobs: failed_jobs as u32,
            pending_jobs: pending_jobs as u32,
            status: serde_json::from_str(&status)?,
            created_at,
            completed_at,
            error_summary,
            job_errors: job_errors_map,
        })
    }

    async fn get_batch_jobs(&self, batch_id: crate::batch::BatchId) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(
            &format!("SELECT {} FROM hammerwork_jobs WHERE batch_id = ? ORDER BY created_at ASC", JOB_SELECT_FIELDS)
        )
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
        use crate::job::JobStatus;

        sqlx::query(
            "UPDATE hammerwork_jobs SET status = ?, error_message = ?, failed_at = ? WHERE id = ?",
        )
        .bind(JobStatus::Dead)
        .bind(error_message)
        .bind(Utc::now())
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn mark_job_timed_out(&self, job_id: JobId, error_message: &str) -> Result<()> {
        use crate::job::JobStatus;

        sqlx::query(
            "UPDATE hammerwork_jobs SET status = ?, error_message = ?, timed_out_at = ? WHERE id = ?"
        )
        .bind(JobStatus::TimedOut)
        .bind(error_message)
        .bind(Utc::now())
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

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
        use crate::job::JobStatus;

        sqlx::query(
            "UPDATE hammerwork_jobs SET status = ?, attempts = 0, scheduled_at = ?, started_at = NULL, failed_at = NULL WHERE id = ? AND status = ?"
        )
        .bind(JobStatus::Pending)
        .bind(Utc::now())
        .bind(job_id.to_string())
        .bind(JobStatus::Dead)
        .execute(&self.pool)
        .await?;

        Ok(())
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
        use std::collections::HashMap;

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

        let dead_jobs_by_queue: HashMap<String, u64> = dead_jobs_by_queue_rows
            .into_iter()
            .map(|(queue, count)| (queue, count as u64))
            .collect();

        let error_patterns: HashMap<String, u64> = error_patterns_rows
            .into_iter()
            .filter_map(|(error, count)| error.map(|e| (e, count as u64)))
            .collect();

        let (oldest_dead_job, newest_dead_job) = timestamps
            .first()
            .map(|(oldest, newest)| (*oldest, *newest))
            .unwrap_or((None, None));

        Ok(DeadJobSummary {
            total_dead_jobs: total_dead_jobs.0 as u64,
            dead_jobs_by_queue,
            oldest_dead_job,
            newest_dead_job,
            error_patterns,
        })
    }

    async fn get_queue_stats(&self, queue_name: &str) -> Result<QueueStats> {
        use crate::stats::JobStatistics;
        use std::collections::HashMap;

        // Get job counts by status
        let status_counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT status, COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? GROUP BY status",
        )
        .bind(queue_name)
        .fetch_all(&self.pool)
        .await?;

        let mut counts = HashMap::new();
        for (status, count) in status_counts {
            counts.insert(status, count as u64);
        }

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

        Ok(status_counts
            .into_iter()
            .map(|(status, count)| (status, count as u64))
            .collect())
    }

    async fn get_processing_times(
        &self,
        queue_name: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<i64>> {
        let times: Vec<(Option<i64>,)> = sqlx::query_as(
            r#"
            SELECT TIMESTAMPDIFF(MICROSECOND, started_at, completed_at) / 1000 as processing_time_ms
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

        Ok(error_frequencies
            .into_iter()
            .map(|(error, count)| (error, count as u64))
            .collect())
    }

    async fn enqueue_cron_job(&self, job: Job) -> Result<JobId> {
        // For cron jobs, we use the regular enqueue method
        // The job should already have the cron fields set
        self.enqueue(job).await
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
            AND (next_run_at IS NULL OR next_run_at <= ?)
            AND status = ?
            ORDER BY next_run_at ASC
            "#, JOB_SELECT_FIELDS)
        } else {
            format!(
            r#"
            SELECT {}
            FROM hammerwork_jobs 
            WHERE recurring = TRUE 
            AND (next_run_at IS NULL OR next_run_at <= ?)
            AND status = ?
            ORDER BY next_run_at ASC
            "#, JOB_SELECT_FIELDS)
        };

        let rows = if let Some(queue) = queue_name {
            sqlx::query_as::<_, JobRow>(&query)
                .bind(queue)
                .bind(Utc::now())
                .bind(JobStatus::Pending)
                .fetch_all(&self.pool)
                .await?
        } else {
            sqlx::query_as::<_, JobRow>(&query)
                .bind(Utc::now())
                .bind(JobStatus::Pending)
                .fetch_all(&self.pool)
                .await?
        };

        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn reschedule_cron_job(&self, job_id: JobId, next_run_at: DateTime<Utc>) -> Result<()> {
        use crate::job::JobStatus;

        sqlx::query(
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
                error_message = NULL
            WHERE id = ? AND recurring = TRUE
            "#,
        )
        .bind(JobStatus::Pending)
        .bind(next_run_at)
        .bind(next_run_at)
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
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

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM hammerwork_jobs WHERE queue_name = ? AND status = ?",
        )
        .bind(queue_name)
        .bind(JobStatus::Pending)
        .fetch_one(&self.pool)
        .await?;

        Ok(count.0 as u64)
    }

    // Job result storage and retrieval operations
    async fn store_job_result(
        &self,
        job_id: JobId,
        result_data: serde_json::Value,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE hammerwork_jobs SET result_data = ?, result_stored_at = ?, result_expires_at = ? WHERE id = ?"
        )
        .bind(result_data)
        .bind(Utc::now())
        .bind(expires_at)
        .bind(job_id.to_string())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_job_result(&self, job_id: JobId) -> Result<Option<serde_json::Value>> {
        let row = sqlx::query(
            "SELECT result_data FROM hammerwork_jobs WHERE id = ? AND result_data IS NOT NULL AND (result_expires_at IS NULL OR result_expires_at > ?)"
        )
        .bind(job_id.to_string())
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let result_data: Option<serde_json::Value> = row.get("result_data");
                Ok(result_data)
            }
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
            "UPDATE hammerwork_jobs SET result_data = NULL, result_stored_at = NULL, result_expires_at = NULL WHERE result_expires_at IS NOT NULL AND result_expires_at <= ?"
        )
        .bind(Utc::now())
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
        for job in &workflow.jobs {
            self.insert_job_in_transaction(&mut tx, job).await?;
        }
        
        tx.commit().await?;
        
        Ok(workflow.id)
    }

    async fn get_workflow_status(
        &self,
        workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Option<crate::workflow::JobGroup>> {
        use crate::workflow::{JobGroup, WorkflowStatus, FailurePolicy};
        
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
                id: uuid::Uuid::parse_str(row.get::<String, _>("id").as_str())?,
                name: row.get("name"),
                status: WorkflowStatus::parse_from_db(row.get("status"))?,
                created_at: row.get("created_at"),
                completed_at: row.get("completed_at"),
                failed_at: row.get("failed_at"),
                failure_policy: FailurePolicy::parse_from_db(row.get("failure_policy"))?,
                jobs,
                dependencies,
                total_jobs: row.get::<i32, _>("total_jobs") as usize,
                completed_jobs: row.get::<i32, _>("completed_jobs") as usize,
                failed_jobs: row.get::<i32, _>("failed_jobs") as usize,
                metadata: row.get("metadata"),
            };
            
            Ok(Some(workflow))
        } else {
            Ok(None)
        }
    }

    async fn resolve_job_dependencies(&self, completed_job_id: JobId) -> Result<Vec<JobId>> {
        // Find all jobs that depend on the completed job
        let dependent_jobs = sqlx::query(
            r#"
            SELECT id, depends_on
            FROM hammerwork_jobs
            WHERE JSON_CONTAINS(depends_on, ?)
            AND dependency_status = 'waiting'
            "#
        )
        .bind(serde_json::json!([completed_job_id.to_string()]))
        .fetch_all(&self.pool)
        .await?;
        
        let mut resolved_jobs = Vec::new();
        
        for job_row in dependent_jobs {
            let job_id = uuid::Uuid::parse_str(&job_row.get::<String, _>("id"))?;
            let depends_on_json: serde_json::Value = job_row.get("depends_on");
            let depends_on: Vec<String> = serde_json::from_value(depends_on_json)?;
            
            // Convert String UUIDs to uuid::Uuid for validation
            let depends_on_uuids: Result<Vec<uuid::Uuid>> = depends_on.iter()
                .map(|s| uuid::Uuid::parse_str(s).map_err(Into::into))
                .collect();
            let _depends_on_uuids = depends_on_uuids?;
            
            // Check if all dependencies are now satisfied
            let placeholders = depends_on.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            let query_str = format!(
                r#"
                SELECT COUNT(*)
                FROM hammerwork_jobs
                WHERE id IN ({})
                AND status = 'Completed'
                "#,
                placeholders
            );
            
            let satisfied_count = sqlx::query_scalar::<_, i64>(&query_str);
            
            let mut query = satisfied_count;
            for uuid_str in &depends_on {
                query = query.bind(uuid_str);
            }
            
            let satisfied_count = query.fetch_one(&self.pool).await?;
            
            if satisfied_count == depends_on.len() as i64 {
                // All dependencies satisfied, mark as ready
                sqlx::query(
                    r#"
                    UPDATE hammerwork_jobs
                    SET dependency_status = 'satisfied'
                    WHERE id = ?
                    "#
                )
                .bind(job_id.to_string())
                .execute(&self.pool)
                .await?;
                
                resolved_jobs.push(job_id);
            }
        }
        
        Ok(resolved_jobs)
    }

    async fn get_ready_jobs(&self, queue_name: &str, limit: u32) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(
            &format!(
                r#"
                SELECT {}
                FROM hammerwork_jobs
                WHERE queue_name = ?
                AND status = 'Pending'
                AND dependency_status IN ('none', 'satisfied')
                AND scheduled_at <= ?
                ORDER BY priority DESC, scheduled_at ASC
                LIMIT ?
                "#, JOB_SELECT_FIELDS
            )
        )
        .bind(queue_name)
        .bind(chrono::Utc::now())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        
        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn fail_job_dependencies(&self, failed_job_id: JobId) -> Result<Vec<JobId>> {
        // Find all jobs that depend on the failed job (directly or indirectly)
        let mut failed_jobs = Vec::new();
        let mut jobs_to_check = vec![failed_job_id];
        
        while let Some(current_job_id) = jobs_to_check.pop() {
            // Find jobs that depend on the current job
            let dependent_jobs = sqlx::query_scalar::<_, String>(
                r#"
                SELECT id
                FROM hammerwork_jobs
                WHERE JSON_CONTAINS(depends_on, ?)
                AND dependency_status IN ('waiting', 'satisfied')
                AND status = 'Pending'
                "#
            )
            .bind(serde_json::json!([current_job_id.to_string()]))
            .fetch_all(&self.pool)
            .await?;
            
            for dependent_job_id_str in dependent_jobs {
                let dependent_job_id = uuid::Uuid::parse_str(&dependent_job_id_str)?;
                if !failed_jobs.contains(&dependent_job_id) {
                    // Mark job as failed due to dependency
                    sqlx::query(
                        r#"
                        UPDATE hammerwork_jobs
                        SET dependency_status = 'failed',
                            status = 'Failed',
                            failed_at = ?,
                            error_message = CONCAT('Dependency failed: job ', ?, ' failed')
                        WHERE id = ?
                        "#
                    )
                    .bind(chrono::Utc::now())
                    .bind(current_job_id.to_string())
                    .bind(dependent_job_id.to_string())
                    .execute(&self.pool)
                    .await?;
                    
                    failed_jobs.push(dependent_job_id);
                    jobs_to_check.push(dependent_job_id);
                }
            }
        }
        
        Ok(failed_jobs)
    }

    async fn get_workflow_jobs(
        &self,
        workflow_id: crate::workflow::WorkflowId,
    ) -> Result<Vec<Job>> {
        let rows = sqlx::query_as::<_, JobRow>(
            &format!(
                r#"
                SELECT {}
                FROM hammerwork_jobs
                WHERE workflow_id = ?
                ORDER BY created_at ASC
                "#, JOB_SELECT_FIELDS
            )
        )
        .bind(workflow_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        
        rows.into_iter().map(|row| row.into_job()).collect()
    }

    async fn cancel_workflow(&self, workflow_id: crate::workflow::WorkflowId) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        
        // Cancel all pending jobs in the workflow
        sqlx::query(
            r#"
            UPDATE hammerwork_jobs
            SET status = 'Failed',
                failed_at = ?,
                error_message = 'Workflow cancelled'
            WHERE workflow_id = ?
            AND status = 'Pending'
            "#
        )
        .bind(chrono::Utc::now())
        .bind(workflow_id.to_string())
        .execute(&mut *tx)
        .await?;
        
        // Update workflow status
        sqlx::query(
            r#"
            UPDATE hammerwork_workflows
            SET status = 'cancelled',
                failed_at = ?
            WHERE id = ?
            "#
        )
        .bind(chrono::Utc::now())
        .bind(workflow_id.to_string())
        .execute(&mut *tx)
        .await?;
        
        tx.commit().await?;
        
        Ok(())
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
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        if !policy.enabled {
            return Ok(ArchivalStats::default());
        }

        let start_time = Utc::now();
        let mut jobs_archived = 0u64;
        let mut bytes_archived = 0u64;
        let mut total_compression_ratio = 0.0;

        // Build query to find jobs eligible for archival
        let mut query = "SELECT * FROM hammerwork_jobs WHERE archived_at IS NULL".to_string();
        let mut conditions = Vec::new();

        if queue_name.is_some() {
            conditions.push("queue_name = ?".to_string());
        }

        // Add status-based conditions for archival eligibility
        let mut status_conditions = Vec::new();

        if policy.archive_completed_after.is_some() {
            status_conditions.push(
                "(status = 'Completed' AND completed_at IS NOT NULL AND completed_at <= ?)"
                    .to_string(),
            );
        }

        if policy.archive_failed_after.is_some() {
            status_conditions.push(
                "(status = 'Failed' AND failed_at IS NOT NULL AND failed_at <= ?)".to_string(),
            );
        }

        if policy.archive_dead_after.is_some() {
            status_conditions
                .push("(status = 'Dead' AND failed_at IS NOT NULL AND failed_at <= ?)".to_string());
        }

        if policy.archive_timed_out_after.is_some() {
            status_conditions.push(
                "(status = 'TimedOut' AND timed_out_at IS NOT NULL AND timed_out_at <= ?)"
                    .to_string(),
            );
        }

        if status_conditions.is_empty() {
            return Ok(ArchivalStats::default());
        }

        conditions.push(format!("({})", status_conditions.join(" OR ")));

        if !conditions.is_empty() {
            query.push_str(" AND ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(&format!(
            " ORDER BY created_at ASC LIMIT {}",
            policy.batch_size
        ));

        // Start transaction
        let mut tx = self.pool.begin().await?;

        // Build and execute query with parameters
        let mut sql_query = sqlx::query_as::<_, JobRow>(&query);

        if let Some(queue) = queue_name {
            sql_query = sql_query.bind(queue);
        }

        if let Some(completed_after) = policy.archive_completed_after {
            let threshold = Utc::now() - completed_after;
            sql_query = sql_query.bind(threshold);
        }

        if let Some(failed_after) = policy.archive_failed_after {
            let threshold = Utc::now() - failed_after;
            sql_query = sql_query.bind(threshold);
        }

        if let Some(dead_after) = policy.archive_dead_after {
            let threshold = Utc::now() - dead_after;
            sql_query = sql_query.bind(threshold);
        }

        if let Some(timed_out_after) = policy.archive_timed_out_after {
            let threshold = Utc::now() - timed_out_after;
            sql_query = sql_query.bind(threshold);
        }

        let jobs_to_archive = sql_query.fetch_all(&mut *tx).await?;

        for job_row in jobs_to_archive {
            let job = job_row.into_job()?;
            let payload_json = serde_json::to_vec(&job.payload)?;
            let original_size = payload_json.len();

            let (final_payload, is_compressed) = if policy.compress_payloads {
                let mut encoder =
                    GzEncoder::new(Vec::new(), Compression::new(config.compression_level));
                encoder.write_all(&payload_json)?;
                let compressed = encoder.finish()?;

                if compressed.len() < original_size {
                    total_compression_ratio += original_size as f64 / compressed.len() as f64;
                    (compressed, true)
                } else {
                    (payload_json, false)
                }
            } else {
                (payload_json, false)
            };

            // Insert into archive table
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
            .bind(Utc::now())
            .bind(job.error_message)
            .bind(job.result_data)
            .bind(job.result_expires_at)
            .bind(
                job.retry_strategy
                    .map(|rs| serde_json::to_string(&rs).unwrap_or_default()),
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
            .bind(serde_json::to_string(&job.dependency_status)?)
            .bind(serde_json::to_value(&job.result_config)?)
            .bind(job.trace_id)
            .bind(job.correlation_id)
            .bind(job.parent_span_id)
            .bind(job.span_context)
            .bind(reason.to_string())
            .bind(archived_by)
            .bind("hammerwork_jobs")
            .execute(&mut *tx)
            .await?;

            // Update main table to mark as archived
            sqlx::query(
                "UPDATE hammerwork_jobs SET archived_at = ?, archival_reason = ?, archival_policy_applied = ? WHERE id = ?"
            )
            .bind(Utc::now())
            .bind(reason.to_string())
            .bind(archived_by)
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
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut tx = self.pool.begin().await?;

        // Get archived job
        let archived_row = sqlx::query(
            r#"
            SELECT 
                id, queue_name, payload, payload_compressed, original_payload_size,
                status, priority, attempts, max_attempts, created_at, scheduled_at,
                started_at, completed_at, failed_at, timed_out_at,
                error_message, result, result_ttl, retry_strategy, timeout_seconds,
                cron_schedule, next_run_at, recurring, timezone, batch_id,
                depends_on, dependency_status, result_config,
                trace_id, correlation_id, parent_span_id, span_context
            FROM hammerwork_jobs_archive 
            WHERE id = ?
        "#,
        )
        .bind(job_id.to_string())
        .fetch_one(&mut *tx)
        .await?;

        // Decompress payload if needed
        let payload_bytes: Vec<u8> = archived_row.get("payload");
        let is_compressed: bool = archived_row.get("payload_compressed");

        let payload_json = if is_compressed {
            let mut decoder = GzDecoder::new(&payload_bytes[..]);
            let mut decompressed = Vec::new();
            decoder.read_to_end(&mut decompressed)?;
            decompressed
        } else {
            payload_bytes
        };

        let payload: serde_json::Value = serde_json::from_slice(&payload_json)?;

        // Create job object
        let job = Job {
            id: uuid::Uuid::parse_str(&archived_row.get::<String, _>("id"))?,
            queue_name: archived_row.get("queue_name"),
            payload,
            status: JobStatus::Pending, // Reset to pending for re-processing
            priority: archived_row
                .get::<String, _>("priority")
                .parse()
                .unwrap_or(JobPriority::Normal),
            attempts: 0, // Reset attempts
            max_attempts: archived_row.get("max_attempts"),
            created_at: archived_row.get("created_at"),
            scheduled_at: Utc::now(), // Schedule for immediate processing
            started_at: None,
            completed_at: None,
            failed_at: None,
            timed_out_at: None,
            timeout: archived_row
                .get::<Option<i32>, _>("timeout_seconds")
                .map(|s| std::time::Duration::from_secs(s as u64)),
            error_message: None, // Clear error message
            cron_schedule: archived_row.get("cron_schedule"),
            next_run_at: archived_row.get("next_run_at"),
            recurring: archived_row.get("recurring"),
            timezone: archived_row.get("timezone"),
            batch_id: archived_row
                .get::<Option<String>, _>("batch_id")
                .and_then(|s| uuid::Uuid::parse_str(&s).ok()),
            result_config: archived_row
                .get::<Option<serde_json::Value>, _>("result_config")
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            result_data: None, // Clear result data
            result_stored_at: None,
            result_expires_at: None,
            retry_strategy: archived_row
                .get::<Option<String>, _>("retry_strategy")
                .and_then(|s| serde_json::from_str(&s).ok()),
            depends_on: archived_row
                .get::<Option<serde_json::Value>, _>("depends_on")
                .map(|v| serde_json::from_value(v).unwrap_or_default())
                .unwrap_or_default(),
            dependents: Vec::new(),
            dependency_status: archived_row
                .get::<Option<String>, _>("dependency_status")
                .map(|s| serde_json::from_str(&s).unwrap_or_default())
                .unwrap_or_default(),
            workflow_id: None,
            workflow_name: None,
            trace_id: archived_row.get("trace_id"),
            correlation_id: archived_row.get("correlation_id"),
            parent_span_id: archived_row.get("parent_span_id"),
            span_context: archived_row.get("span_context"),
            #[cfg(feature = "encryption")]
            encryption_config: None,
            pii_fields: Vec::new(),
            #[cfg(feature = "encryption")]
            retention_policy: None,
            is_encrypted: false,
            #[cfg(feature = "encryption")]
            encrypted_payload: None,
        };

        // Insert back into main table using existing enqueue method
        self.enqueue_with_tx(&mut tx, job.clone()).await?;

        // Remove from archive table
        sqlx::query("DELETE FROM hammerwork_jobs_archive WHERE id = ?")
            .bind(job_id.to_string())
            .execute(&mut *tx)
            .await?;

        // Clear archival metadata from main table
        sqlx::query("UPDATE hammerwork_jobs SET archived_at = NULL, archival_reason = NULL, archival_policy_applied = NULL WHERE id = ?")
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
                id: uuid::Uuid::parse_str(&row.get::<String, _>("id"))?,
                queue_name: row.get("queue_name"),
                status: serde_json::from_str::<JobStatus>(&row.get::<String, _>("status"))
                    .unwrap_or(JobStatus::Dead),
                created_at: row.get("created_at"),
                archived_at: row.get("archived_at"),
                archival_reason: serde_json::from_str(&row.get::<String, _>("archival_reason"))
                    .unwrap_or(crate::archive::ArchivalReason::Automatic),
                original_payload_size: row
                    .get::<Option<i32>, _>("original_payload_size")
                    .map(|s| s as usize),
                payload_compressed: row.get("payload_compressed"),
                archived_by: row.get("archived_by"),
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
            COALESCE(SUM(original_payload_size), 0) as total_original_size,
            COALESCE(SUM(LENGTH(payload)), 0) as total_compressed_size,
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

        let job_count: i64 = row.get("job_count");
        let total_original_size: i64 = row.get("total_original_size");
        let total_compressed_size: i64 = row.get("total_compressed_size");
        let last_archived_at: Option<DateTime<Utc>> = row.get("last_archived_at");

        let compression_ratio = if total_compressed_size > 0 {
            total_original_size as f64 / total_compressed_size as f64
        } else {
            1.0
        };

        Ok(ArchivalStats {
            jobs_archived: job_count as u64,
            jobs_purged: 0, // This would need separate tracking
            bytes_archived: total_compressed_size as u64,
            bytes_purged: 0,
            compression_ratio,
            operation_duration: std::time::Duration::from_secs(0),
            last_run_at: last_archived_at.unwrap_or(Utc::now()),
        })
    }
}

// Helper method for enqueueing with an existing transaction
impl crate::queue::JobQueue<sqlx::MySql> {
    async fn enqueue_with_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
        job: Job,
    ) -> Result<JobId> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_jobs (
                id, queue_name, payload, status, priority, attempts, max_attempts, 
                timeout_seconds, created_at, scheduled_at, error_message, 
                cron_schedule, next_run_at, recurring, timezone, batch_id,
                result_storage_type, result_ttl_seconds, result_max_size_bytes,
                depends_on, dependents, dependency_status, workflow_id, workflow_name,
                trace_id, correlation_id, parent_span_id, span_context
            ) VALUES (
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?
            )
            "#,
        )
        .bind(job.id.to_string())
        .bind(&job.queue_name)
        .bind(&job.payload)
        .bind(job.status)
        .bind(job.priority as i32)
        .bind(job.attempts)
        .bind(job.max_attempts)
        .bind(job.timeout.map(|d| d.as_secs() as i32))
        .bind(job.created_at)
        .bind(job.scheduled_at)
        .bind(job.error_message)
        .bind(job.cron_schedule)
        .bind(job.next_run_at)
        .bind(job.recurring)
        .bind(job.timezone)
        .bind(job.batch_id.map(|id| id.to_string()))
        .bind(match job.result_config.storage {
            crate::job::ResultStorage::Database => Some("database"),
            crate::job::ResultStorage::Memory => Some("memory"),
            crate::job::ResultStorage::None => Some("none"),
        })
        .bind(job.result_config.ttl.map(|t| t.as_secs() as i64))
        .bind(job.result_config.max_size_bytes.map(|s| s as i64))
        .bind(if job.depends_on.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&job.depends_on)?)
        })
        .bind(if job.dependents.is_empty() {
            None
        } else {
            Some(serde_json::to_value(&job.dependents)?)
        })
        .bind(serde_json::to_string(&job.dependency_status)?)
        .bind(job.workflow_id.map(|id| id.to_string()))
        .bind(job.workflow_name)
        .bind(job.trace_id)
        .bind(job.correlation_id)
        .bind(job.parent_span_id)
        .bind(job.span_context)
        .execute(&mut **tx)
        .await?;

        Ok(job.id)
    }
}

/// Helper methods for MySQL JobQueue
impl crate::queue::JobQueue<MySql> {
    /// Helper method to insert a job within a transaction
    async fn insert_job_in_transaction(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::MySql>,
        job: &Job,
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_jobs (
                id, queue_name, payload, status, priority, attempts, max_attempts, timeout_seconds,
                created_at, scheduled_at, started_at, completed_at, failed_at, timed_out_at, error_message,
                cron_schedule, next_run_at, recurring, timezone, batch_id,
                result_data, result_stored_at, result_expires_at, result_storage_type, result_ttl_seconds, result_max_size_bytes,
                depends_on, dependents, dependency_status, workflow_id, workflow_name,
                trace_id, correlation_id, parent_span_id, span_context
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#
        )
        .bind(job.id.to_string())
        .bind(&job.queue_name)
        .bind(&job.payload)
        .bind(job.status)
        .bind(job.priority.as_i32())
        .bind(job.attempts)
        .bind(job.max_attempts)
        .bind(job.timeout.map(|t| t.as_secs() as i32))
        .bind(job.created_at)
        .bind(job.scheduled_at)
        .bind(job.started_at)
        .bind(job.completed_at)
        .bind(job.failed_at)
        .bind(job.timed_out_at)
        .bind(&job.error_message)
        .bind(&job.cron_schedule)
        .bind(job.next_run_at)
        .bind(job.recurring)
        .bind(&job.timezone)
        .bind(job.batch_id.map(|id| id.to_string()))
        .bind(&job.result_data)
        .bind(job.result_stored_at)
        .bind(job.result_expires_at)
        .bind(match job.result_config.storage {
            crate::job::ResultStorage::Database => "database",
            crate::job::ResultStorage::Memory => "memory",
            crate::job::ResultStorage::None => "none",
        })
        .bind(job.result_config.ttl.map(|t| t.as_secs() as i64))
        .bind(job.result_config.max_size_bytes.map(|s| s as i64))
        .bind(serde_json::to_value(&job.depends_on)?)
        .bind(serde_json::to_value(&job.dependents)?)
        .bind(job.dependency_status.as_str())
        .bind(job.workflow_id.map(|id| id.to_string()))
        .bind(&job.workflow_name)
        .bind(&job.trace_id)
        .bind(&job.correlation_id)
        .bind(&job.parent_span_id)
        .bind(&job.span_context)
        .execute(&mut **tx)
        .await?;
        
        Ok(())
    }
}
