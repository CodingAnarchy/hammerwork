//! Job payload encryption and PII protection for Hammerwork.
//!
//! This module provides encryption capabilities for job payloads to protect sensitive data
//! such as personally identifiable information (PII). It supports multiple encryption
//! algorithms, field-level encryption, and configurable retention policies.
//!
//! # Features
//!
//! - **Multiple Encryption Algorithms**: AES-256-GCM, ChaCha20-Poly1305
//! - **Field-Level Encryption**: Encrypt specific fields containing PII
//! - **Key Management**: Support for static keys, key rotation, and external key management
//! - **Retention Policies**: Automatic cleanup of encrypted data after specified periods
//! - **Performance Optimized**: Minimal overhead for non-encrypted jobs
//!
//! # Examples
//!
//! ## Basic Job Encryption
//!
//! ```rust,no_run
//! # #[cfg(feature = "encryption")]
//! # {
//! use hammerwork::{Job, encryption::{EncryptionConfig, EncryptionAlgorithm, RetentionPolicy}};
//! use serde_json::json;
//! use std::time::Duration;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let job = Job::new("payment_processing".to_string(), json!({
//!     "user_id": "user123",
//!     "credit_card": "4111-1111-1111-1111",
//!     "ssn": "123-45-6789",
//!     "amount": 99.99
//! }))
//! .with_encryption(EncryptionConfig::new(EncryptionAlgorithm::AES256GCM))
//! .with_pii_fields(vec!["credit_card", "ssn"])
//! .with_retention_policy(RetentionPolicy::DeleteAfter(Duration::from_secs(7 * 24 * 60 * 60))); // 7 days
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ## Custom Key Management
//!
//! ```rust,no_run
//! # #[cfg(feature = "encryption")]
//! # {
//! use hammerwork::encryption::{EncryptionConfig, KeySource, EncryptionAlgorithm};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305)
//!     .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()))
//!     .with_key_rotation_enabled(true);
//! # Ok(())
//! # }
//! # }
//! ```

pub mod engine;
pub mod key_manager;

pub use engine::EncryptionEngine;
pub use key_manager::{
    EncryptionKey, ExternalKmsConfig, KeyAuditRecord, KeyDerivationConfig, KeyManager,
    KeyManagerConfig, KeyManagerStats, KeyOperation, KeyPurpose, KeyStatus,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use thiserror::Error;

use base64::Engine;

/// Errors that can occur during encryption operations.
#[derive(Error, Debug)]
pub enum EncryptionError {
    /// Key management error (missing, invalid, or rotation failure).
    #[error("Key management error: {0}")]
    KeyManagement(String),

    /// Encryption operation failed.
    #[error("Encryption failed: {0}")]
    EncryptionFailed(String),

    /// Decryption operation failed.
    #[error("Decryption failed: {0}")]
    DecryptionFailed(String),

    /// Invalid algorithm or configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    /// Field processing error (PII detection, field extraction, etc.).
    #[error("Field processing error: {0}")]
    FieldProcessing(String),

    /// Serialization/deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Base64 encoding/decoding error.
    #[error("Base64 error: {0}")]
    Base64(#[from] base64::DecodeError),

    /// Cryptographic operation error.
    #[error("Cryptographic error: {0}")]
    Cryptographic(String),
}

/// Supported encryption algorithms.
///
/// Each algorithm has different characteristics in terms of performance,
/// security, and compatibility.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    /// AES-256 in Galois/Counter Mode (AES-256-GCM).
    ///
    /// Industry standard, widely supported, hardware acceleration available
    /// on most modern processors. Provides both confidentiality and authenticity.
    AES256GCM,

    /// ChaCha20-Poly1305 stream cipher.
    ///
    /// Modern alternative to AES, designed for software implementations.
    /// Resistant to timing attacks and performs well on devices without
    /// AES hardware acceleration.
    ChaCha20Poly1305,
}

impl Default for EncryptionAlgorithm {
    fn default() -> Self {
        Self::AES256GCM
    }
}

/// Source for encryption keys.
///
/// Determines where encryption keys are obtained from, enabling
/// different security models and key management strategies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeySource {
    /// Use a static key provided directly.
    ///
    /// **Security Note**: Only use for development/testing.
    /// Production systems should use environment variables or external key management.
    Static(String),

    /// Load key from an environment variable.
    ///
    /// The environment variable should contain a base64-encoded key
    /// of appropriate length for the selected algorithm.
    Environment(String),

    /// Retrieve key from an external key management service.
    ///
    /// This could be AWS KMS, HashiCorp Vault, Azure Key Vault, etc.
    /// The string contains the key identifier or service configuration.
    External(String),

    /// Generate a random key and store it in the specified location.
    ///
    /// Useful for initial setup or testing scenarios.
    Generated(String),
}

impl Default for KeySource {
    fn default() -> Self {
        Self::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string())
    }
}

/// Configuration for job payload encryption.
///
/// This struct contains all settings needed to encrypt and decrypt job payloads,
/// including algorithm selection, key management, and field-specific encryption rules.
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "encryption")]
/// # {
/// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm, KeySource};
/// use std::time::Duration;
///
/// // Basic configuration
/// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
///
/// // Advanced configuration
/// let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305)
///     .with_key_source(KeySource::Environment("MY_ENCRYPTION_KEY".to_string()))
///     .with_key_rotation_enabled(true)
///     .with_default_retention(Duration::from_secs(30 * 24 * 60 * 60)); // 30 days
/// # }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionConfig {
    /// The encryption algorithm to use.
    pub algorithm: EncryptionAlgorithm,

    /// Source for the encryption key.
    pub key_source: KeySource,

    /// Whether to enable automatic key rotation.
    pub key_rotation_enabled: bool,

    /// How often to rotate keys (if rotation is enabled).
    pub key_rotation_interval: Option<Duration>,

    /// Default retention period for encrypted data.
    pub default_retention: Option<Duration>,

    /// Whether to compress data before encryption.
    ///
    /// Can reduce storage size for large payloads but adds CPU overhead.
    pub compression_enabled: bool,

    /// Key identifier for the current encryption key.
    ///
    /// Used to track which key was used for encryption to support
    /// key rotation and proper decryption.
    pub key_id: Option<String>,

    /// Version of the encryption configuration.
    ///
    /// Incremented when encryption settings change to ensure
    /// backward compatibility during upgrades.
    pub version: u32,
}

impl EncryptionConfig {
    /// Creates a new encryption configuration with the specified algorithm.
    ///
    /// Uses default settings for key source (environment variable),
    /// no key rotation, and no default retention period.
    ///
    /// # Arguments
    ///
    /// * `algorithm` - The encryption algorithm to use
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
    /// assert_eq!(config.algorithm, EncryptionAlgorithm::AES256GCM);
    /// assert!(!config.key_rotation_enabled);
    /// ```
    pub fn new(algorithm: EncryptionAlgorithm) -> Self {
        Self {
            algorithm,
            key_source: KeySource::default(),
            key_rotation_enabled: false,
            key_rotation_interval: None,
            default_retention: None,
            compression_enabled: false,
            key_id: None,
            version: 1,
        }
    }

    /// Sets the key source for this configuration.
    ///
    /// # Arguments
    ///
    /// * `key_source` - Where to obtain the encryption key
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm, KeySource};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_source(KeySource::Environment("MY_SECRET_KEY".to_string()));
    /// # }
    /// ```
    pub fn with_key_source(mut self, key_source: KeySource) -> Self {
        self.key_source = key_source;
        self
    }

    /// Enables or disables automatic key rotation.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to enable key rotation
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_rotation_enabled(true);
    ///
    /// assert!(config.key_rotation_enabled);
    /// ```
    pub fn with_key_rotation_enabled(mut self, enabled: bool) -> Self {
        self.key_rotation_enabled = enabled;
        self
    }

    /// Sets the key rotation interval.
    ///
    /// Only effective if key rotation is enabled.
    ///
    /// # Arguments
    ///
    /// * `interval` - How often to rotate keys
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    /// use std::time::Duration;
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_rotation_enabled(true)
    ///     .with_key_rotation_interval(Duration::from_secs(30 * 24 * 60 * 60)); // 30 days
    /// ```
    pub fn with_key_rotation_interval(mut self, interval: Duration) -> Self {
        self.key_rotation_interval = Some(interval);
        self
    }

    /// Sets the default retention period for encrypted data.
    ///
    /// # Arguments
    ///
    /// * `retention` - How long to keep encrypted data before cleanup
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    /// use std::time::Duration;
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_default_retention(Duration::from_secs(7 * 24 * 60 * 60)); // 7 days
    /// ```
    pub fn with_default_retention(mut self, retention: Duration) -> Self {
        self.default_retention = Some(retention);
        self
    }

    /// Enables or disables compression before encryption.
    ///
    /// # Arguments
    ///
    /// * `enabled` - Whether to compress data before encryption
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_compression_enabled(true);
    ///
    /// assert!(config.compression_enabled);
    /// ```
    pub fn with_compression_enabled(mut self, enabled: bool) -> Self {
        self.compression_enabled = enabled;
        self
    }

    /// Sets the key identifier for this configuration.
    ///
    /// # Arguments
    ///
    /// * `key_id` - Identifier for the encryption key
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_id("key-2024-01");
    /// ```
    pub fn with_key_id(mut self, key_id: impl Into<String>) -> Self {
        self.key_id = Some(key_id.into());
        self
    }

    /// Gets the expected key size in bytes for the configured algorithm.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
    /// assert_eq!(config.key_size_bytes(), 32); // 256 bits = 32 bytes
    /// ```
    pub fn key_size_bytes(&self) -> usize {
        match self.algorithm {
            EncryptionAlgorithm::AES256GCM => 32,        // 256 bits
            EncryptionAlgorithm::ChaCha20Poly1305 => 32, // 256 bits
        }
    }

    /// Gets the nonce/IV size in bytes for the configured algorithm.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
    /// assert_eq!(config.nonce_size_bytes(), 12); // 96 bits for GCM
    /// ```
    pub fn nonce_size_bytes(&self) -> usize {
        match self.algorithm {
            EncryptionAlgorithm::AES256GCM => 12,        // 96 bits for GCM
            EncryptionAlgorithm::ChaCha20Poly1305 => 12, // 96 bits
        }
    }

    /// Gets the authentication tag size in bytes for the configured algorithm.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionConfig, EncryptionAlgorithm};
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
    /// assert_eq!(config.tag_size_bytes(), 16); // 128 bits
    /// ```
    pub fn tag_size_bytes(&self) -> usize {
        match self.algorithm {
            EncryptionAlgorithm::AES256GCM => 16,        // 128 bits
            EncryptionAlgorithm::ChaCha20Poly1305 => 16, // 128 bits
        }
    }
}

impl Default for EncryptionConfig {
    fn default() -> Self {
        Self::new(EncryptionAlgorithm::default())
    }
}

/// Policy for retaining encrypted data.
///
/// Determines how long encrypted job data should be kept before
/// automatic cleanup. Different retention policies can be applied
/// based on data sensitivity and compliance requirements.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RetentionPolicy {
    /// Keep data for a specific duration from job creation.
    DeleteAfter(Duration),

    /// Keep data until a specific date/time.
    DeleteAt(DateTime<Utc>),

    /// Keep data indefinitely (manual cleanup required).
    KeepIndefinitely,

    /// Delete immediately after job completion.
    DeleteImmediately,

    /// Use the default retention policy from the encryption configuration.
    UseDefault,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self::UseDefault
    }
}

impl RetentionPolicy {
    /// Calculates when the data should be deleted based on the policy.
    ///
    /// # Arguments
    ///
    /// * `created_at` - When the job was created
    /// * `completed_at` - When the job was completed (if applicable)
    /// * `default_retention` - Default retention period to use if policy is UseDefault
    ///
    /// # Returns
    ///
    /// The date/time when the data should be deleted, or None if it should be kept indefinitely.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::RetentionPolicy;
    /// use chrono::Utc;
    /// use std::time::Duration;
    ///
    /// let policy = RetentionPolicy::DeleteAfter(Duration::from_secs(7 * 24 * 60 * 60));
    /// let created_at = Utc::now();
    /// let delete_at = policy.calculate_deletion_time(created_at, None, None);
    ///
    /// assert!(delete_at.is_some());
    /// ```
    pub fn calculate_deletion_time(
        &self,
        created_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
        default_retention: Option<Duration>,
    ) -> Option<DateTime<Utc>> {
        match self {
            RetentionPolicy::DeleteAfter(duration) => {
                let duration_chrono = chrono::Duration::from_std(*duration).ok()?;
                Some(created_at + duration_chrono)
            }
            RetentionPolicy::DeleteAt(timestamp) => Some(*timestamp),
            RetentionPolicy::KeepIndefinitely => None,
            RetentionPolicy::DeleteImmediately => completed_at.or(Some(created_at)),
            RetentionPolicy::UseDefault => {
                if let Some(default_duration) = default_retention {
                    let duration_chrono = chrono::Duration::from_std(default_duration).ok()?;
                    Some(created_at + duration_chrono)
                } else {
                    None
                }
            }
        }
    }

    /// Checks if the data should be deleted now based on the policy.
    ///
    /// # Arguments
    ///
    /// * `created_at` - When the job was created
    /// * `completed_at` - When the job was completed (if applicable)
    /// * `default_retention` - Default retention period to use if policy is UseDefault
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::RetentionPolicy;
    /// use chrono::Utc;
    /// use std::time::Duration;
    ///
    /// let policy = RetentionPolicy::DeleteAfter(Duration::from_secs(1));
    /// let created_at = Utc::now() - chrono::Duration::seconds(2);
    ///
    /// assert!(policy.should_delete_now(created_at, None, None));
    /// ```
    pub fn should_delete_now(
        &self,
        created_at: DateTime<Utc>,
        completed_at: Option<DateTime<Utc>>,
        default_retention: Option<Duration>,
    ) -> bool {
        match self.calculate_deletion_time(created_at, completed_at, default_retention) {
            Some(delete_at) => Utc::now() >= delete_at,
            None => false,
        }
    }
}

/// Metadata about an encrypted payload.
///
/// Contains information needed to decrypt the payload and manage
/// its lifecycle according to retention policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionMetadata {
    /// The encryption algorithm used.
    pub algorithm: EncryptionAlgorithm,

    /// Identifier of the key used for encryption.
    pub key_id: String,

    /// Version of the encryption configuration.
    pub config_version: u32,

    /// Whether the data was compressed before encryption.
    pub compressed: bool,

    /// List of field names that contain PII and were encrypted.
    pub encrypted_fields: Vec<String>,

    /// Retention policy for this encrypted data.
    pub retention_policy: RetentionPolicy,

    /// When the data was encrypted.
    pub encrypted_at: DateTime<Utc>,

    /// When the data should be deleted (if applicable).
    pub delete_at: Option<DateTime<Utc>>,

    /// Hash of the original payload for integrity verification.
    pub payload_hash: String,
}

impl EncryptionMetadata {
    /// Creates new encryption metadata.
    ///
    /// # Arguments
    ///
    /// * `config` - The encryption configuration used
    /// * `encrypted_fields` - List of fields that were encrypted
    /// * `retention_policy` - Retention policy for the data
    /// * `payload_hash` - Hash of the original payload
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionMetadata, EncryptionConfig, EncryptionAlgorithm, RetentionPolicy};
    /// use std::time::Duration;
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_id("key-123");
    /// let metadata = EncryptionMetadata::new(
    ///     &config,
    ///     vec!["ssn".to_string(), "credit_card".to_string()],
    ///     RetentionPolicy::DeleteAfter(Duration::from_secs(86400)),
    ///     "hash123".to_string(),
    /// );
    ///
    /// assert_eq!(metadata.algorithm, EncryptionAlgorithm::AES256GCM);
    /// assert_eq!(metadata.encrypted_fields.len(), 2);
    /// ```
    pub fn new(
        config: &EncryptionConfig,
        encrypted_fields: Vec<String>,
        retention_policy: RetentionPolicy,
        payload_hash: String,
    ) -> Self {
        let now = Utc::now();
        let delete_at =
            retention_policy.calculate_deletion_time(now, None, config.default_retention);

        Self {
            algorithm: config.algorithm.clone(),
            key_id: config
                .key_id
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            config_version: config.version,
            compressed: config.compression_enabled,
            encrypted_fields,
            retention_policy,
            encrypted_at: now,
            delete_at,
            payload_hash,
        }
    }

    /// Checks if this encrypted data should be deleted now.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::encryption::{EncryptionMetadata, EncryptionConfig, EncryptionAlgorithm, RetentionPolicy};
    /// use std::time::Duration;
    ///
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
    /// let metadata = EncryptionMetadata::new(
    ///     &config,
    ///     vec![],
    ///     RetentionPolicy::DeleteAfter(Duration::from_secs(1)),
    ///     "hash".to_string(),
    /// );
    ///
    /// // Should not be deleted immediately
    /// assert!(!metadata.should_delete_now());
    /// ```
    pub fn should_delete_now(&self) -> bool {
        match self.delete_at {
            Some(delete_at) => Utc::now() >= delete_at,
            None => false,
        }
    }

    /// Updates the deletion time based on job completion.
    ///
    /// # Arguments
    ///
    /// * `completed_at` - When the job was completed
    /// * `default_retention` - Default retention period from config
    pub fn update_deletion_time(
        &mut self,
        completed_at: DateTime<Utc>,
        default_retention: Option<Duration>,
    ) {
        self.delete_at = self.retention_policy.calculate_deletion_time(
            self.encrypted_at,
            Some(completed_at),
            default_retention,
        );
    }
}

/// Container for encrypted job payload data.
///
/// Contains the encrypted payload along with all metadata needed
/// for decryption and lifecycle management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedPayload {
    /// Base64-encoded encrypted data.
    pub ciphertext: String,

    /// Base64-encoded nonce/IV used for encryption.
    pub nonce: String,

    /// Base64-encoded authentication tag.
    pub tag: String,

    /// Metadata about the encryption.
    pub metadata: EncryptionMetadata,
}

impl EncryptedPayload {
    /// Creates a new encrypted payload container.
    ///
    /// # Arguments
    ///
    /// * `ciphertext` - The encrypted data
    /// * `nonce` - The nonce/IV used for encryption
    /// * `tag` - The authentication tag
    /// * `metadata` - Encryption metadata
    pub fn new(
        ciphertext: Vec<u8>,
        nonce: Vec<u8>,
        tag: Vec<u8>,
        metadata: EncryptionMetadata,
    ) -> Self {
        Self {
            ciphertext: base64::engine::general_purpose::STANDARD.encode(ciphertext),
            nonce: base64::engine::general_purpose::STANDARD.encode(nonce),
            tag: base64::engine::general_purpose::STANDARD.encode(tag),
            metadata,
        }
    }

    /// Decodes the ciphertext from base64.
    pub fn decode_ciphertext(&self) -> Result<Vec<u8>, EncryptionError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.ciphertext)
            .map_err(EncryptionError::Base64)
    }

    /// Decodes the nonce from base64.
    pub fn decode_nonce(&self) -> Result<Vec<u8>, EncryptionError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.nonce)
            .map_err(EncryptionError::Base64)
    }

    /// Decodes the authentication tag from base64.
    pub fn decode_tag(&self) -> Result<Vec<u8>, EncryptionError> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.tag)
            .map_err(EncryptionError::Base64)
    }

    /// Checks if this encrypted payload should be deleted now.
    pub fn should_delete_now(&self) -> bool {
        self.metadata.should_delete_now()
    }

    /// Gets the size of the encrypted payload in bytes.
    pub fn size_bytes(&self) -> usize {
        self.ciphertext.len() + self.nonce.len() + self.tag.len()
    }

    /// Checks if the payload contains specific PII fields.
    pub fn contains_pii_field(&self, field_name: &str) -> bool {
        self.metadata
            .encrypted_fields
            .contains(&field_name.to_string())
    }

    /// Gets all PII fields contained in this payload.
    pub fn pii_fields(&self) -> &[String] {
        &self.metadata.encrypted_fields
    }
}

/// Statistics about encryption operations.
///
/// Provides metrics for monitoring encryption performance and usage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EncryptionStats {
    /// Total number of jobs encrypted.
    pub jobs_encrypted: u64,

    /// Total number of jobs decrypted.
    pub jobs_decrypted: u64,

    /// Total number of PII fields encrypted.
    pub pii_fields_encrypted: u64,

    /// Total size of data encrypted (in bytes).
    pub total_encrypted_bytes: u64,

    /// Total size of data decrypted (in bytes).
    pub total_decrypted_bytes: u64,

    /// Number of encryption errors.
    pub encryption_errors: u64,

    /// Number of decryption errors.
    pub decryption_errors: u64,

    /// Number of key rotation events.
    pub key_rotations: u64,

    /// Number of retention policy cleanups.
    pub retention_cleanups: u64,

    /// Average encryption time in milliseconds.
    pub avg_encryption_time_ms: f64,

    /// Average decryption time in milliseconds.
    pub avg_decryption_time_ms: f64,

    /// Breakdown by encryption algorithm.
    pub algorithm_usage: HashMap<String, u64>,

    /// When these statistics were last updated.
    pub last_updated: DateTime<Utc>,
}

impl EncryptionStats {
    /// Creates new encryption statistics.
    pub fn new() -> Self {
        Self {
            last_updated: Utc::now(),
            ..Default::default()
        }
    }

    /// Records a successful encryption operation.
    pub fn record_encryption(
        &mut self,
        algorithm: &EncryptionAlgorithm,
        size_bytes: usize,
        duration_ms: f64,
    ) {
        self.jobs_encrypted += 1;
        self.total_encrypted_bytes += size_bytes as u64;

        // Update average encryption time
        if self.jobs_encrypted == 1 {
            self.avg_encryption_time_ms = duration_ms;
        } else {
            let total_time =
                self.avg_encryption_time_ms * (self.jobs_encrypted - 1) as f64 + duration_ms;
            self.avg_encryption_time_ms = total_time / self.jobs_encrypted as f64;
        }

        // Update algorithm usage
        let algo_name = format!("{:?}", algorithm);
        *self.algorithm_usage.entry(algo_name).or_insert(0) += 1;

        self.last_updated = Utc::now();
    }

    /// Records a successful decryption operation.
    pub fn record_decryption(&mut self, size_bytes: usize, duration_ms: f64) {
        self.jobs_decrypted += 1;
        self.total_decrypted_bytes += size_bytes as u64;

        // Update average decryption time
        if self.jobs_decrypted == 1 {
            self.avg_decryption_time_ms = duration_ms;
        } else {
            let total_time =
                self.avg_decryption_time_ms * (self.jobs_decrypted - 1) as f64 + duration_ms;
            self.avg_decryption_time_ms = total_time / self.jobs_decrypted as f64;
        }

        self.last_updated = Utc::now();
    }

    /// Records PII field encryption.
    pub fn record_pii_encryption(&mut self, field_count: usize) {
        self.pii_fields_encrypted += field_count as u64;
        self.last_updated = Utc::now();
    }

    /// Records an encryption error.
    pub fn record_encryption_error(&mut self) {
        self.encryption_errors += 1;
        self.last_updated = Utc::now();
    }

    /// Records a decryption error.
    pub fn record_decryption_error(&mut self) {
        self.decryption_errors += 1;
        self.last_updated = Utc::now();
    }

    /// Records a key rotation event.
    pub fn record_key_rotation(&mut self) {
        self.key_rotations += 1;
        self.last_updated = Utc::now();
    }

    /// Records a retention cleanup operation.
    pub fn record_retention_cleanup(&mut self, jobs_cleaned: u64) {
        self.retention_cleanups += jobs_cleaned;
        self.last_updated = Utc::now();
    }

    /// Calculates the encryption success rate as a percentage.
    pub fn encryption_success_rate(&self) -> f64 {
        let total_attempts = self.jobs_encrypted + self.encryption_errors;
        if total_attempts == 0 {
            100.0
        } else {
            (self.jobs_encrypted as f64 / total_attempts as f64) * 100.0
        }
    }

    /// Calculates the decryption success rate as a percentage.
    pub fn decryption_success_rate(&self) -> f64 {
        let total_attempts = self.jobs_decrypted + self.decryption_errors;
        if total_attempts == 0 {
            100.0
        } else {
            (self.jobs_decrypted as f64 / total_attempts as f64) * 100.0
        }
    }

    /// Gets the most used encryption algorithm.
    pub fn most_used_algorithm(&self) -> Option<String> {
        self.algorithm_usage
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(algo, _)| algo.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_encryption_config_creation() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
        assert_eq!(config.algorithm, EncryptionAlgorithm::AES256GCM);
        assert!(!config.key_rotation_enabled);
        assert!(!config.compression_enabled);
        assert_eq!(config.version, 1);
    }

    #[test]
    fn test_encryption_config_builder() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305)
            .with_key_source(KeySource::Environment("TEST_KEY".to_string()))
            .with_key_rotation_enabled(true)
            .with_compression_enabled(true)
            .with_key_id("test-key-1");

        assert_eq!(config.algorithm, EncryptionAlgorithm::ChaCha20Poly1305);
        assert_eq!(
            config.key_source,
            KeySource::Environment("TEST_KEY".to_string())
        );
        assert!(config.key_rotation_enabled);
        assert!(config.compression_enabled);
        assert_eq!(config.key_id, Some("test-key-1".to_string()));
    }

    #[test]
    fn test_algorithm_key_sizes() {
        let aes_config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
        assert_eq!(aes_config.key_size_bytes(), 32);
        assert_eq!(aes_config.nonce_size_bytes(), 12);
        assert_eq!(aes_config.tag_size_bytes(), 16);

        let chacha_config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305);
        assert_eq!(chacha_config.key_size_bytes(), 32);
        assert_eq!(chacha_config.nonce_size_bytes(), 12);
        assert_eq!(chacha_config.tag_size_bytes(), 16);
    }

    #[test]
    fn test_retention_policy_calculation() {
        let now = Utc::now();
        let one_day = Duration::from_secs(24 * 60 * 60);

        let policy = RetentionPolicy::DeleteAfter(one_day);
        let delete_time = policy.calculate_deletion_time(now, None, None).unwrap();

        let expected = now + chrono::Duration::from_std(one_day).unwrap();
        assert!((delete_time - expected).num_seconds().abs() < 1);
    }

    #[test]
    fn test_retention_policy_should_delete() {
        let past = Utc::now() - chrono::Duration::seconds(2);
        let policy = RetentionPolicy::DeleteAfter(Duration::from_secs(1));

        assert!(policy.should_delete_now(past, None, None));

        let future = Utc::now() + chrono::Duration::seconds(10);
        assert!(!policy.should_delete_now(future, None, None));
    }

    #[test]
    fn test_encryption_metadata_creation() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM).with_key_id("test-key");

        let fields = vec!["ssn".to_string(), "credit_card".to_string()];
        let policy = RetentionPolicy::DeleteAfter(Duration::from_secs(86400));

        let metadata =
            EncryptionMetadata::new(&config, fields.clone(), policy, "hash123".to_string());

        assert_eq!(metadata.algorithm, EncryptionAlgorithm::AES256GCM);
        assert_eq!(metadata.key_id, "test-key");
        assert_eq!(metadata.encrypted_fields, fields);
        assert_eq!(metadata.payload_hash, "hash123");
        assert!(!metadata.compressed);
    }

    #[test]
    fn test_encrypted_payload_creation() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
        let metadata = EncryptionMetadata::new(
            &config,
            vec!["test_field".to_string()],
            RetentionPolicy::KeepIndefinitely,
            "hash".to_string(),
        );

        let payload = EncryptedPayload::new(
            b"encrypted_data".to_vec(),
            b"nonce123".to_vec(),
            b"tag12345".to_vec(),
            metadata,
        );

        assert!(payload.contains_pii_field("test_field"));
        assert!(!payload.contains_pii_field("other_field"));
        assert_eq!(payload.pii_fields().len(), 1);
        assert!(!payload.should_delete_now());
    }

    #[test]
    fn test_encryption_stats() {
        let mut stats = EncryptionStats::new();

        stats.record_encryption(&EncryptionAlgorithm::AES256GCM, 1024, 10.5);
        stats.record_encryption(&EncryptionAlgorithm::AES256GCM, 2048, 15.0);
        stats.record_pii_encryption(2);

        assert_eq!(stats.jobs_encrypted, 2);
        assert_eq!(stats.total_encrypted_bytes, 3072);
        assert_eq!(stats.pii_fields_encrypted, 2);
        assert_eq!(stats.avg_encryption_time_ms, 12.75); // (10.5 + 15.0) / 2
        assert_eq!(stats.encryption_success_rate(), 100.0);

        stats.record_encryption_error();
        assert!(stats.encryption_success_rate() < 100.0);
    }

    #[test]
    fn test_key_source_equality() {
        assert_eq!(
            KeySource::Environment("KEY1".to_string()),
            KeySource::Environment("KEY1".to_string())
        );
        assert_ne!(
            KeySource::Environment("KEY1".to_string()),
            KeySource::Environment("KEY2".to_string())
        );
        assert_ne!(
            KeySource::Environment("KEY1".to_string()),
            KeySource::Static("KEY1".to_string())
        );
    }

    #[test]
    fn test_serialization() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::ChaCha20Poly1305)
            .with_key_rotation_enabled(true)
            .with_compression_enabled(true);

        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: EncryptionConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(config.algorithm, deserialized.algorithm);
        assert_eq!(
            config.key_rotation_enabled,
            deserialized.key_rotation_enabled
        );
        assert_eq!(config.compression_enabled, deserialized.compression_enabled);
    }
}
