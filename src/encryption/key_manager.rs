//! Advanced key management system for Hammerwork encryption.
//!
//! This module provides comprehensive key management capabilities including:
//! - Secure key generation and storage
//! - Key rotation and lifecycle management
//! - Master key encryption (Key Encryption Keys)
//! - External key management service integration (AWS KMS, Azure Key Vault, GCP KMS, HashiCorp Vault)
//! - Azure Key Vault integration for master key retrieval with automatic credential resolution
//! - Audit trails and key usage tracking
//!
//! # Security Considerations
//!
//! - Keys are never stored in plain text in the database: each key is encrypted with
//!   AES-256-GCM under a master key before it is written
//! - The master key comes from [`KeyManagerConfig::master_key_source`]. Loading it fails
//!   closed: if the environment variable, KMS or vault is unavailable, or the KMS cargo
//!   feature for an `aws://`, `gcp://`, `vault://` or `azure://` source is not enabled,
//!   [`KeyManager::new`] returns an error. There is no fallback key.
//! - `aws://` and `gcp://` master keys use envelope encryption: on first use the KMS
//!   generates a data key and only its KMS-encrypted form is stored (in
//!   `hammerwork_kms_data_keys`); later loads decrypt it with the KMS, so every process and
//!   restart uses the same master key. Rotate it with [`KeyManager::rotate_kms_master_key`].
//! - Key creation, access and rotation are recorded in `hammerwork_key_audit_log` when
//!   auditing is enabled
//!
//! For development without a KMS, use [`KeySource::Static`] with a base64-encoded 32-byte
//! key, or [`KeySource::Generated`] for a random in-memory key that does not survive a
//! restart.
//!
//! # Examples
//!
//! ## Basic Key Management
//!
//! ```rust,no_run
//! # #[cfg(all(feature = "encryption", feature = "postgres"))]
//! # {
//! use hammerwork::encryption::{KeyManager, EncryptionAlgorithm, KeyManagerConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let database_url = "postgres://user:pass@localhost/hammerwork";
//! # let pool = sqlx::PgPool::connect(database_url).await?;
//! let config = KeyManagerConfig::new()
//!     .with_master_key_env("MASTER_KEY")
//!     .with_auto_rotation_enabled(true);
//!
//! let mut key_manager = KeyManager::new(config, pool).await?;
//!
//! // Generate a new encryption key
//! let key_id = key_manager.generate_key("payment-encryption", EncryptionAlgorithm::AES256GCM).await?;
//!
//! // Use the key for encryption operations
//! let key_material = key_manager.get_key(&key_id).await?;
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ## Key Rotation Workflow
//!
//! ```rust,no_run
//! # #[cfg(all(feature = "encryption", feature = "postgres"))]
//! # {
//! use hammerwork::encryption::{KeyManager, EncryptionAlgorithm, KeyManagerConfig};
//! use chrono::Duration;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let database_url = "postgres://user:pass@localhost/hammerwork";
//! # let pool = sqlx::PgPool::connect(database_url).await?;
//! // Configure key manager with automatic rotation
//! let config = KeyManagerConfig::new()
//!     .with_master_key_env("MASTER_KEY")
//!     .with_auto_rotation_enabled(true)
//!     .with_rotation_interval(Duration::days(30))
//!     .with_max_key_versions(5);
//!
//! let mut key_manager = KeyManager::new(config, pool).await?;
//!
//! // Generate initial key
//! let key_id = key_manager.generate_key("user-data-key", EncryptionAlgorithm::AES256GCM).await?;
//! println!("Initial key generated: {}", key_id);
//!
//! // Check if key needs rotation
//! if key_manager.is_key_due_for_rotation(&key_id).await? {
//!     println!("Key is due for rotation");
//!
//!     // Rotate the key
//!     let new_version = key_manager.rotate_key(&key_id).await?;
//!     println!("Key rotated to version: {}", new_version);
//!
//!     // Update rotation schedule
//!     key_manager.update_key_rotation_schedule(&key_id, None).await?;
//! }
//!
//! // Perform automatic rotation for all keys
//! let rotated_keys = key_manager.perform_automatic_rotation().await?;
//! println!("Automatically rotated {} keys", rotated_keys.len());
//!
//! // Get key management statistics
//! let stats = key_manager.get_stats().await;
//! println!("Total keys: {}, Rotations performed: {}",
//!          stats.total_keys, stats.rotations_performed);
//! # Ok(())
//! # }
//! # }
//! ```
//!
//! ## Azure Key Vault Master Key Integration
//!
//! ```rust,no_run
//! # #[cfg(all(feature = "encryption", feature = "azure-kv", feature = "postgres"))]
//! # {
//! use hammerwork::encryption::{KeyManager, KeyManagerConfig, KeySource};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let database_url = "postgres://user:pass@localhost/hammerwork";
//! # let pool = sqlx::PgPool::connect(database_url).await?;
//! // Azure credentials are read from the environment: AZURE_TENANT_ID, AZURE_CLIENT_ID
//! // and AZURE_CLIENT_SECRET, workload identity, managed identity or the Azure CLI.
//!
//! // Configure key manager with Azure Key Vault for master key
//! let config = KeyManagerConfig::new()
//!     .with_master_key_source(KeySource::External(
//!         "azure://my-vault.vault.azure.net/keys/master-key".to_string()
//!     ))
//!     .with_auto_rotation_enabled(true);
//!
//! let mut key_manager = KeyManager::new(config, pool).await?;
//!
//! // The master key is loaded from Azure Key Vault. If the vault is unreachable or the
//! // credentials are rejected, KeyManager::new returns an error.
//! let key_id = key_manager.generate_key("payment-key",
//!     hammerwork::encryption::EncryptionAlgorithm::AES256GCM).await?;
//!
//! println!("Generated key with Azure Key Vault master key: {}", key_id);
//! # Ok(())
//! # }
//! # }
//! ```

use super::envelope::{self, DbStore, KmsKeyWrapper};
use super::{EncryptionAlgorithm, EncryptionError, KeySource, kms};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use sqlx::{Database, Pool, Row};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Configuration for the key management system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyManagerConfig {
    /// Source for the master key used to encrypt data encryption keys
    pub master_key_source: KeySource,

    /// Whether to enable automatic key rotation
    pub auto_rotation_enabled: bool,

    /// Default rotation interval for automatically rotated keys
    pub default_rotation_interval: Duration,

    /// Maximum number of key versions to keep for each key ID
    pub max_key_versions: u32,

    /// Whether to enable key usage auditing
    pub audit_enabled: bool,

    /// External key management service configuration
    pub external_kms_config: Option<ExternalKmsConfig>,

    /// Key derivation configuration for password-based keys
    pub key_derivation_config: KeyDerivationConfig,
}

/// Configuration for external Key Management Service integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalKmsConfig {
    /// KMS service type (AWS, GCP, Azure, HashiCorp Vault, etc.)
    pub service_type: String,

    /// Service endpoint URL
    pub endpoint: String,

    /// Authentication configuration
    pub auth_config: HashMap<String, String>,

    /// Region or availability zone
    pub region: Option<String>,

    /// Key namespace or project ID
    pub namespace: Option<String>,
}

/// Configuration for key derivation from passwords or passphrases
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyDerivationConfig {
    /// Argon2 memory cost parameter (in KB)
    pub memory_cost: u32,

    /// Argon2 time cost parameter (iterations)
    pub time_cost: u32,

    /// Argon2 parallelism parameter (threads)
    pub parallelism: u32,

    /// Salt length for key derivation
    pub salt_length: usize,
}

/// Represents an encryption key with its metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptionKey {
    /// Unique identifier for the key
    pub id: Uuid,

    /// Human-readable key identifier
    pub key_id: String,

    /// Key version number
    pub version: u32,

    /// Encryption algorithm this key is used for
    pub algorithm: EncryptionAlgorithm,

    /// Encrypted key material (never stored in plain text)
    pub encrypted_key_material: Vec<u8>,

    /// Salt used for key derivation (if applicable)
    pub derivation_salt: Option<Vec<u8>>,

    /// How this key was created
    pub source: KeySource,

    /// Purpose of this key
    pub purpose: KeyPurpose,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,

    /// Who or what created this key
    pub created_by: Option<String>,

    /// When this key expires (if applicable)
    pub expires_at: Option<DateTime<Utc>>,

    /// When this key was rotated (if applicable)
    pub rotated_at: Option<DateTime<Utc>>,

    /// When this key was retired
    pub retired_at: Option<DateTime<Utc>>,

    /// Current status of the key
    pub status: KeyStatus,

    /// How often to rotate this key automatically
    pub rotation_interval: Option<Duration>,

    /// When the next rotation is scheduled
    pub next_rotation_at: Option<DateTime<Utc>>,

    /// Key strength in bits
    pub key_strength: u32,

    /// ID of the master key used to encrypt this key
    pub master_key_id: Option<Uuid>,

    /// Audit trail information
    pub last_used_at: Option<DateTime<Utc>>,
    pub usage_count: u64,
}

/// Purpose of an encryption key
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyPurpose {
    /// Data encryption key for encrypting job payloads
    Encryption,
    /// Message Authentication Code key
    MAC,
    /// Key Encryption Key (master key for encrypting other keys)
    KEK,
}

/// Current status of an encryption key
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum KeyStatus {
    /// Key is active and can be used for encryption and decryption
    Active,
    /// Key has been retired but can still be used for decryption
    Retired,
    /// Key has been revoked and should not be used
    Revoked,
    /// Key has expired based on its expiration time
    Expired,
}

/// Key usage audit record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyAuditRecord {
    /// Unique audit record ID
    pub id: Uuid,

    /// Key that was accessed
    pub key_id: String,

    /// Type of operation performed
    pub operation: KeyOperation,

    /// When the operation occurred
    pub timestamp: DateTime<Utc>,

    /// Who or what performed the operation
    pub actor: Option<String>,

    /// Additional context about the operation
    pub context: HashMap<String, String>,

    /// Whether the operation was successful
    pub success: bool,

    /// Error message if the operation failed
    pub error_message: Option<String>,
}

/// Types of key operations that can be audited
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KeyOperation {
    /// Key was created
    Create,
    /// Key material was retrieved for use
    Access,
    /// Key was rotated to a new version
    Rotate,
    /// Key was retired
    Retire,
    /// Key was revoked
    Revoke,
    /// Key was deleted
    Delete,
    /// Key metadata was updated
    Update,
}

/// Statistics about key management operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyManagerStats {
    /// Total number of keys managed
    pub total_keys: u64,

    /// Number of active keys
    pub active_keys: u64,

    /// Number of retired keys
    pub retired_keys: u64,

    /// Number of revoked keys
    pub revoked_keys: u64,

    /// Number of expired keys
    pub expired_keys: u64,

    /// Total key access operations
    pub total_access_operations: u64,

    /// Number of key rotations performed
    pub rotations_performed: u64,

    /// Average key age in days
    pub average_key_age_days: f64,

    /// Keys approaching expiration (within 7 days)
    pub keys_expiring_soon: u64,

    /// Keys due for rotation
    pub keys_due_for_rotation: u64,
}

/// Main key management system
type KeyCacheEntry = (Vec<u8>, DateTime<Utc>); // (decrypted_material, cached_at)
type KeyCache = Arc<Mutex<HashMap<String, KeyCacheEntry>>>;
type RootKey = (Uuid, Vec<u8>); // (derived ID, key material)

/// Name under which [`KeyManager`] stores its KMS-wrapped master key in
/// `hammerwork_kms_data_keys` (for `aws://` and `gcp://` master key sources).
const KMS_MASTER_KEY_NAME: &str = "key-manager/master";

/// A data key stored in `hammerwork_kms_data_keys`, encrypted by an AWS or GCP KMS key.
///
/// Used by `aws://` and `gcp://` key sources (envelope encryption): only the KMS-encrypted
/// key is stored, and it is decrypted with the KMS on load.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct KmsWrappedKey {
    /// Row ID
    pub id: Uuid,
    /// What the key is for, e.g. `key-manager/master` or `engine/<key-id>`
    pub key_name: String,
    /// `aws` or `gcp`
    pub kms_provider: String,
    /// KMS key that wraps this key (AWS key id/ARN/alias, GCP CryptoKey resource name)
    pub kms_key_id: String,
    /// Version, starting at 1 and incremented on rotation
    pub version: u32,
    /// Plaintext key length in bytes
    pub key_size: u32,
    /// The KMS-encrypted key
    pub wrapped_key: Vec<u8>,
    /// Whether this is the active version (older versions are retired)
    pub active: bool,
    /// When this version was created
    pub created_at: DateTime<Utc>,
    /// When this version was retired
    pub retired_at: Option<DateTime<Utc>>,
}

/// Generates, stores, rotates and audits encryption keys.
///
/// Keys are stored in `hammerwork_encryption_keys`, encrypted with a master key; plaintext
/// key material is never written to the database. The master key is either the key loaded
/// from [`KeyManagerConfig::master_key_source`] or, after
/// [`KeyManager::generate_master_key`], a key-encryption key stored in the database
/// encrypted with that configured key.
///
/// Supported databases are PostgreSQL and MySQL (see [`KeyManagerBackend`]).
pub struct KeyManager<DB: Database> {
    config: KeyManagerConfig,
    pool: Pool<DB>,
    /// Key loaded from `master_key_source`, with its derived ID
    root_key: Arc<Mutex<Option<RootKey>>>,
    /// Key that wraps newly generated keys (the active KEK, or the root key)
    master_key: Arc<Mutex<Option<Vec<u8>>>>,
    master_key_id: Arc<Mutex<Option<Uuid>>>,
    /// KMS that wraps the master key, for `aws://` and `gcp://` master key sources
    kms: Option<Arc<dyn KmsKeyWrapper>>,
    /// Earlier versions of a KMS-wrapped master key, by derived ID
    retired_root_keys: Arc<Mutex<HashMap<Uuid, Vec<u8>>>>,
    key_cache: KeyCache,
    stats: Arc<Mutex<KeyManagerStats>>,
}

impl<DB: Database> Clone for KeyManager<DB> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pool: self.pool.clone(),
            root_key: self.root_key.clone(),
            master_key: self.master_key.clone(),
            master_key_id: self.master_key_id.clone(),
            kms: self.kms.clone(),
            retired_root_keys: self.retired_root_keys.clone(),
            key_cache: self.key_cache.clone(),
            stats: self.stats.clone(),
        }
    }
}

impl Default for KeyManagerConfig {
    fn default() -> Self {
        Self {
            master_key_source: KeySource::Environment("HAMMERWORK_MASTER_KEY".to_string()),
            auto_rotation_enabled: false,
            default_rotation_interval: Duration::days(90), // 3 months
            max_key_versions: 10,
            audit_enabled: true,
            external_kms_config: None,
            key_derivation_config: KeyDerivationConfig::default(),
        }
    }
}

impl Default for KeyDerivationConfig {
    fn default() -> Self {
        Self {
            memory_cost: 65536, // 64 MB
            time_cost: 3,       // 3 iterations
            parallelism: 4,     // 4 threads
            salt_length: 32,    // 32 bytes
        }
    }
}

impl KeyManagerConfig {
    /// Create a new key manager configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManagerConfig, KeySource};
    /// use chrono::Duration;
    ///
    /// // Create default configuration
    /// let config = KeyManagerConfig::new();
    /// assert_eq!(config.auto_rotation_enabled, false);
    /// assert_eq!(config.max_key_versions, 10);
    /// assert_eq!(config.audit_enabled, true);
    /// # }
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the master key source
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManagerConfig, KeySource};
    ///
    /// let config = KeyManagerConfig::new()
    ///     .with_master_key_source(KeySource::Static("my-master-key".to_string()));
    /// # }
    /// ```
    pub fn with_master_key_source(mut self, source: KeySource) -> Self {
        self.master_key_source = source;
        self
    }

    /// Set the master key from an environment variable
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManagerConfig, KeySource};
    ///
    /// let config = KeyManagerConfig::new()
    ///     .with_master_key_env("MASTER_KEY");
    ///
    /// // This is equivalent to:
    /// let config2 = KeyManagerConfig::new()
    ///     .with_master_key_source(KeySource::Environment("MASTER_KEY".to_string()));
    /// # }
    /// ```
    pub fn with_master_key_env(mut self, env_var: &str) -> Self {
        self.master_key_source = KeySource::Environment(env_var.to_string());
        self
    }

    /// Enable or disable automatic key rotation
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::KeyManagerConfig;
    /// use chrono::Duration;
    ///
    /// let config = KeyManagerConfig::new()
    ///     .with_auto_rotation_enabled(true)
    ///     .with_rotation_interval(Duration::days(30));
    ///
    /// assert_eq!(config.auto_rotation_enabled, true);
    /// # }
    /// ```
    pub fn with_auto_rotation_enabled(mut self, enabled: bool) -> Self {
        self.auto_rotation_enabled = enabled;
        self
    }

    /// Set the default rotation interval
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::KeyManagerConfig;
    /// use chrono::Duration;
    ///
    /// // Rotate keys every 30 days
    /// let config = KeyManagerConfig::new()
    ///     .with_rotation_interval(Duration::days(30));
    ///
    /// // Or rotate keys every 24 hours for high-security scenarios
    /// let config2 = KeyManagerConfig::new()
    ///     .with_rotation_interval(Duration::hours(24));
    /// # }
    /// ```
    pub fn with_rotation_interval(mut self, interval: Duration) -> Self {
        self.default_rotation_interval = interval;
        self
    }

    /// Set the maximum number of key versions to retain
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::KeyManagerConfig;
    ///
    /// // Keep only 5 versions of each key
    /// let config = KeyManagerConfig::new()
    ///     .with_max_key_versions(5);
    ///
    /// // Keep up to 50 versions for compliance requirements
    /// let config2 = KeyManagerConfig::new()
    ///     .with_max_key_versions(50);
    /// # }
    /// ```
    pub fn with_max_key_versions(mut self, max_versions: u32) -> Self {
        self.max_key_versions = max_versions;
        self
    }

    /// Enable or disable key usage auditing
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::KeyManagerConfig;
    ///
    /// // Disable auditing for performance-critical applications
    /// let config = KeyManagerConfig::new()
    ///     .with_audit_enabled(false);
    ///
    /// // Enable auditing for compliance (default)
    /// let config2 = KeyManagerConfig::new()
    ///     .with_audit_enabled(true);
    /// # }
    /// ```
    pub fn with_audit_enabled(mut self, enabled: bool) -> Self {
        self.audit_enabled = enabled;
        self
    }

    /// Configure external KMS integration
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManagerConfig, ExternalKmsConfig};
    /// use std::collections::HashMap;
    ///
    /// let mut auth_config = HashMap::new();
    /// auth_config.insert("access_key".to_string(), "AKIA...".to_string());
    /// auth_config.insert("secret_key".to_string(), "secret...".to_string());
    ///
    /// let kms_config = ExternalKmsConfig {
    ///     service_type: "aws-kms".to_string(),
    ///     endpoint: "https://kms.us-east-1.amazonaws.com".to_string(),
    ///     auth_config,
    ///     region: Some("us-east-1".to_string()),
    ///     namespace: None,
    /// };
    ///
    /// let config = KeyManagerConfig::new()
    ///     .with_external_kms(kms_config);
    /// # }
    /// ```
    pub fn with_external_kms(mut self, config: ExternalKmsConfig) -> Self {
        self.external_kms_config = Some(config);
        self
    }
}

/// Database backends that [`KeyManager`] can persist keys to.
///
/// This trait is sealed: it is implemented for [`sqlx::Postgres`] (with the `postgres`
/// feature) and [`sqlx::MySql`] (with the `mysql` feature) and cannot be implemented
/// outside Hammerwork. Its methods are an implementation detail of [`KeyManager`]; use the
/// `KeyManager` methods instead of calling them directly.
#[async_trait::async_trait]
pub trait KeyManagerBackend: Database + sealed::Sealed {
    /// Insert a new key row.
    #[doc(hidden)]
    async fn insert_key(pool: &Pool<Self>, key: &EncryptionKey) -> Result<(), EncryptionError>;

    /// Insert a new key version and retire `previous_version`, in one transaction.
    #[doc(hidden)]
    async fn insert_rotated_key(
        pool: &Pool<Self>,
        key: &EncryptionKey,
        previous_version: u32,
    ) -> Result<(), EncryptionError>;

    /// Retire every active key-encryption key and insert `key` as the active one,
    /// in one transaction.
    #[doc(hidden)]
    async fn insert_master_key(
        pool: &Pool<Self>,
        key: &EncryptionKey,
    ) -> Result<(), EncryptionError>;

    /// Load the newest version of a key.
    #[doc(hidden)]
    async fn load_latest_key(
        pool: &Pool<Self>,
        key_id: &str,
    ) -> Result<Option<EncryptionKey>, EncryptionError>;

    /// Load a specific version of a key.
    #[doc(hidden)]
    async fn load_key_version(
        pool: &Pool<Self>,
        key_id: &str,
        version: u32,
    ) -> Result<Option<EncryptionKey>, EncryptionError>;

    /// Load the active key-encryption key (master key), if one has been generated.
    #[doc(hidden)]
    async fn load_active_master_key(
        pool: &Pool<Self>,
    ) -> Result<Option<EncryptionKey>, EncryptionError>;

    /// Delete all but the newest `keep` versions of a key.
    #[doc(hidden)]
    async fn delete_old_key_versions(
        pool: &Pool<Self>,
        key_id: &str,
        keep: u32,
    ) -> Result<(), EncryptionError>;

    /// Bump the usage counter and last-used time of the active version of a key.
    #[doc(hidden)]
    async fn record_key_usage(pool: &Pool<Self>, key_id: &str) -> Result<(), EncryptionError>;

    /// Append a record to the key audit log.
    #[doc(hidden)]
    async fn record_audit_event(
        pool: &Pool<Self>,
        key_id: &str,
        operation: &KeyOperation,
        success: bool,
        error_message: Option<&str>,
    ) -> Result<(), EncryptionError>;

    /// Key IDs (excluding key-encryption keys) whose active version is due for rotation.
    #[doc(hidden)]
    async fn keys_due_for_rotation(pool: &Pool<Self>) -> Result<Vec<String>, EncryptionError>;

    /// Whether the active version of a key is due for rotation.
    #[doc(hidden)]
    async fn is_key_due_for_rotation(
        pool: &Pool<Self>,
        key_id: &str,
    ) -> Result<bool, EncryptionError>;

    /// Set the rotation interval and next rotation time of the active version of a key.
    #[doc(hidden)]
    async fn update_rotation_schedule(
        pool: &Pool<Self>,
        key_id: &str,
        rotation_interval: Option<Duration>,
        next_rotation_at: Option<DateTime<Utc>>,
    ) -> Result<(), EncryptionError>;

    /// Set the next rotation time of the active version of a key.
    #[doc(hidden)]
    async fn schedule_rotation(
        pool: &Pool<Self>,
        key_id: &str,
        rotation_time: DateTime<Utc>,
    ) -> Result<(), EncryptionError>;

    /// Next rotation time of the active version of a key.
    #[doc(hidden)]
    async fn rotation_schedule(
        pool: &Pool<Self>,
        key_id: &str,
    ) -> Result<Option<DateTime<Utc>>, EncryptionError>;

    /// Active keys scheduled for rotation within `[from_time, to_time]`.
    #[doc(hidden)]
    async fn scheduled_rotations(
        pool: &Pool<Self>,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
    ) -> Result<Vec<(String, DateTime<Utc>)>, EncryptionError>;

    /// Key counts and ages from the key table. `total_access_operations` and
    /// `rotations_performed` are left at zero.
    #[doc(hidden)]
    async fn statistics(pool: &Pool<Self>) -> Result<KeyManagerStats, EncryptionError>;

    /// Insert a KMS-wrapped key unless its `(key_name, kms_provider, kms_key_id, version)`
    /// already exists.
    #[doc(hidden)]
    async fn insert_kms_wrapped_key_if_absent(
        pool: &Pool<Self>,
        key: &KmsWrappedKey,
    ) -> Result<(), EncryptionError>;

    /// Retire the active KMS-wrapped keys in `key`'s scope and insert `key`, in one
    /// transaction. Fails if the version already exists.
    #[doc(hidden)]
    async fn insert_rotated_kms_wrapped_key(
        pool: &Pool<Self>,
        key: &KmsWrappedKey,
    ) -> Result<(), EncryptionError>;

    /// Every version of a KMS-wrapped key, newest first.
    #[doc(hidden)]
    async fn load_kms_wrapped_keys(
        pool: &Pool<Self>,
        key_name: &str,
        kms_provider: &str,
        kms_key_id: &str,
    ) -> Result<Vec<KmsWrappedKey>, EncryptionError>;
}

mod sealed {
    pub trait Sealed {}

    #[cfg(feature = "postgres")]
    impl Sealed for sqlx::Postgres {}

    #[cfg(feature = "mysql")]
    impl Sealed for sqlx::MySql {}
}

impl<DB: KeyManagerBackend> KeyManager<DB> {
    /// Create a new key manager instance
    ///
    /// Loads the master key from `config.master_key_source` and fails if it cannot be
    /// loaded. There is no fallback key: an unreachable KMS, missing credentials, or a KMS
    /// source whose cargo feature (`aws-kms`, `gcp-kms`, `vault-kms`, `azure-kv`) is not
    /// enabled are all errors.
    ///
    /// If a key-encryption key was created earlier with [`KeyManager::generate_master_key`],
    /// it is loaded from the database and decrypted with the configured master key.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the key manager
    /// * `pool` - Database connection pool
    ///
    /// # Returns
    ///
    /// A new `KeyManager` instance with initialized master key and statistics
    ///
    /// # Examples
    ///
    /// ## Basic PostgreSQL setup
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, KeyManagerConfig};
    /// use sqlx::PgPool;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // MASTER_KEY must hold a base64-encoded 32-byte key
    /// let pool = PgPool::connect("postgresql://user:pass@localhost/hammerwork").await?;
    /// let config = KeyManagerConfig::new()
    ///     .with_master_key_env("MASTER_KEY")
    ///     .with_auto_rotation_enabled(true);
    ///
    /// let key_manager = KeyManager::new(config, pool).await?;
    /// println!("Key manager initialized successfully");
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    ///
    /// ## MySQL setup with static master key
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "mysql"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, KeyManagerConfig, KeySource};
    /// use sqlx::MySqlPool;
    /// use chrono::Duration;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = MySqlPool::connect("mysql://user:pass@localhost/hammerwork").await?;
    /// // Static keys are base64-encoded 32-byte keys (development and testing only)
    /// let config = KeyManagerConfig::new()
    ///     .with_master_key_source(KeySource::Static(
    ///         "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=".to_string(),
    ///     ))
    ///     .with_auto_rotation_enabled(true)
    ///     .with_rotation_interval(Duration::days(30));
    ///
    /// let key_manager = KeyManager::new(config, pool).await?;
    /// println!("MySQL key manager initialized with 30-day rotation");
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn new(config: KeyManagerConfig, pool: Pool<DB>) -> Result<Self, EncryptionError> {
        let kms = match envelope::parse_kms_source(&config.master_key_source)? {
            Some(source) => Some(source.connect().await?),
            None => None,
        };
        Self::with_kms(config, pool, kms).await
    }

    /// Create a key manager whose master key is wrapped by `kms` (when set) instead of
    /// being loaded from `config.master_key_source`.
    async fn with_kms(
        config: KeyManagerConfig,
        pool: Pool<DB>,
        kms: Option<Arc<dyn KmsKeyWrapper>>,
    ) -> Result<Self, EncryptionError> {
        let manager = Self {
            config,
            pool,
            root_key: Arc::new(Mutex::new(None)),
            master_key: Arc::new(Mutex::new(None)),
            master_key_id: Arc::new(Mutex::new(None)),
            kms,
            retired_root_keys: Arc::new(Mutex::new(HashMap::new())),
            key_cache: Arc::new(Mutex::new(HashMap::new())),
            stats: Arc::new(Mutex::new(KeyManagerStats::default())),
        };

        // Initialize the master key
        manager.load_master_key().await?;

        // Load initial statistics
        manager.refresh_stats().await?;

        Ok(manager)
    }

    /// Generate a new encryption key
    ///
    /// The key material is encrypted with the current master key before it is stored;
    /// plaintext key material never reaches the database.
    ///
    /// # Arguments
    ///
    /// * `key_id` - Human-readable identifier for the key
    /// * `algorithm` - Encryption algorithm to use for this key
    ///
    /// # Returns
    ///
    /// The generated key ID string that can be used to retrieve the key later
    ///
    /// # Examples
    ///
    /// ## Generate different types of encryption keys
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionAlgorithm};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Generate a key for payment processing
    /// let payment_key = key_manager.generate_key(
    ///     "payment-encryption-v1",
    ///     EncryptionAlgorithm::AES256GCM
    /// ).await?;
    /// println!("Payment key generated: {}", payment_key);
    ///
    /// // Generate a key for user data
    /// let user_data_key = key_manager.generate_key(
    ///     "user-data-encryption",
    ///     EncryptionAlgorithm::ChaCha20Poly1305
    /// ).await?;
    /// println!("User data key generated: {}", user_data_key);
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    ///
    /// ## Generate with key ID pattern
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionAlgorithm};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Use consistent naming pattern for keys
    /// let keys = vec![
    ///     ("prod-api-encryption-2024", EncryptionAlgorithm::AES256GCM),
    ///     ("prod-db-encryption-2024", EncryptionAlgorithm::AES256GCM),
    ///     ("prod-file-encryption-2024", EncryptionAlgorithm::ChaCha20Poly1305),
    /// ];
    ///
    /// for (key_id, algorithm) in keys {
    ///     let generated_key = key_manager.generate_key(key_id, algorithm).await?;
    ///     println!("Generated key: {} -> {}", key_id, generated_key);
    /// }
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn generate_key(
        &mut self,
        key_id: &str,
        algorithm: EncryptionAlgorithm,
    ) -> Result<String, EncryptionError> {
        self.generate_key_with_options(
            key_id,
            algorithm,
            KeyPurpose::Encryption,
            None, // No expiration
            None, // No rotation interval
        )
        .await
    }

    /// Generate a new encryption key with detailed options
    ///
    /// Fails if a key with the same `key_id` already exists; use
    /// [`KeyManager::rotate_key`] to create a new version of an existing key.
    pub async fn generate_key_with_options(
        &mut self,
        key_id: &str,
        algorithm: EncryptionAlgorithm,
        purpose: KeyPurpose,
        expires_at: Option<DateTime<Utc>>,
        rotation_interval: Option<Duration>,
    ) -> Result<String, EncryptionError> {
        info!("Generating new encryption key: {}", key_id);

        let key_material = random_key_material(&algorithm);
        let key_strength = (key_material.len() * 8) as u32;
        let (master_key_id, master_key) = self.current_master_key()?;
        let encrypted_key_material = wrap_key_material(&master_key, &key_material)?;

        let now = Utc::now();
        let key_record = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: key_id.to_string(),
            version: 1,
            algorithm,
            encrypted_key_material,
            derivation_salt: None,
            source: KeySource::Generated("database".to_string()),
            purpose,
            created_at: now,
            created_by: Some("hammerwork".to_string()),
            expires_at,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval,
            next_rotation_at: rotation_interval.map(|interval| now + interval),
            key_strength,
            master_key_id: Some(master_key_id),
            last_used_at: None,
            usage_count: 0,
        };

        self.store_key(&key_record).await?;
        self.cache_key(key_id, key_material);

        if self.config.audit_enabled {
            self.record_audit_event(key_id, KeyOperation::Create, true, None)
                .await?;
        }

        self.update_stats(|stats| {
            stats.total_keys += 1;
            stats.active_keys += 1;
        });

        info!("Successfully generated encryption key: {}", key_id);
        Ok(key_id.to_string())
    }

    /// Retrieve key material for encryption/decryption operations
    ///
    /// Returns the newest version of the key. Use [`KeyManager::get_key_version`] to
    /// retrieve an older version, for example to decrypt data encrypted before a rotation.
    ///
    /// # Arguments
    ///
    /// * `key_id` - The identifier of the key to retrieve
    ///
    /// # Returns
    ///
    /// The decrypted key material ready for use in encryption/decryption operations
    ///
    /// # Examples
    ///
    /// ## Retrieve a key for encryption
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionAlgorithm};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// // First generate a key
    /// let key_id = key_manager.generate_key(
    ///     "api-encryption-key",
    ///     EncryptionAlgorithm::AES256GCM
    /// ).await?;
    ///
    /// // Retrieve the key material for use
    /// let key_material = key_manager.get_key(&key_id).await?;
    /// println!("Retrieved key material: {} bytes", key_material.len());
    ///
    /// // Key material can now be used for encryption operations
    /// assert_eq!(key_material.len(), 32); // AES-256 key size
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    ///
    /// ## Handle key retrieval errors
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionError};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Try to retrieve a non-existent key
    /// match key_manager.get_key("non-existent-key").await {
    ///     Ok(key_material) => {
    ///         println!("Key retrieved successfully: {} bytes", key_material.len());
    ///     }
    ///     Err(EncryptionError::KeyManagement(msg)) => {
    ///         // Unknown, revoked or expired key
    ///         println!("Key management error: {}", msg);
    ///     }
    ///     Err(e) => {
    ///         println!("Other error: {}", e);
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn get_key(&mut self, key_id: &str) -> Result<Vec<u8>, EncryptionError> {
        // Check cache first
        if let Some(cached_key) = self.get_cached_key(key_id) {
            self.record_key_usage(key_id).await?;
            return Ok(cached_key);
        }

        let key_record = self.load_key(key_id).await?;
        check_key_usable(&key_record)?;
        let key_material = self.unwrap_key_material(&key_record).await?;

        self.cache_key(key_id, key_material.clone());
        self.record_key_usage(key_id).await?;

        if self.config.audit_enabled {
            self.record_audit_event(key_id, KeyOperation::Access, true, None)
                .await?;
        }

        Ok(key_material)
    }

    /// Retrieve the key material of a specific version of a key
    ///
    /// Retired versions can still be retrieved (they remain usable for decryption);
    /// revoked or expired versions cannot. Versions removed by
    /// [`KeyManagerConfig::max_key_versions`] cleanup are gone.
    pub async fn get_key_version(
        &mut self,
        key_id: &str,
        version: u32,
    ) -> Result<Vec<u8>, EncryptionError> {
        let key_record = DB::load_key_version(&self.pool, key_id, version)
            .await?
            .ok_or_else(|| {
                EncryptionError::KeyManagement(format!(
                    "Key not found: {} (version {})",
                    key_id, version
                ))
            })?;
        check_key_usable(&key_record)?;
        let key_material = self.unwrap_key_material(&key_record).await?;

        if self.config.audit_enabled {
            self.record_audit_event(key_id, KeyOperation::Access, true, None)
                .await?;
        }
        self.update_stats(|stats| stats.total_access_operations += 1);

        Ok(key_material)
    }

    /// Rotate a key to a new version
    ///
    /// Creates a new version of the specified key and retires the previous one. Retired
    /// versions stay in the database (up to [`KeyManagerConfig::max_key_versions`]) and can
    /// be retrieved with [`KeyManager::get_key_version`] to decrypt older data.
    ///
    /// # Arguments
    ///
    /// * `key_id` - The identifier of the key to rotate
    ///
    /// # Returns
    ///
    /// The new version number of the rotated key
    ///
    /// # Examples
    ///
    /// ## Basic key rotation
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionAlgorithm};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Generate initial key
    /// let key_id = key_manager.generate_key(
    ///     "payment-processing-key",
    ///     EncryptionAlgorithm::AES256GCM
    /// ).await?;
    ///
    /// // Rotate the key to a new version
    /// let new_version = key_manager.rotate_key(&key_id).await?;
    /// println!("Key rotated to version: {}", new_version);
    ///
    /// // Old version is still available for decryption
    /// let old_key = key_manager.get_key_version(&key_id, 1).await?;
    /// assert_eq!(new_version, 2); // Should be version 2 after first rotation
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    ///
    /// ## Key rotation with usage tracking
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "postgres"))]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionAlgorithm};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// let key_id = "user-data-encryption";
    ///
    /// // Generate initial key
    /// let initial_key = key_manager.generate_key(key_id, EncryptionAlgorithm::AES256GCM).await?;
    ///
    /// // Check if rotation is needed
    /// if key_manager.is_key_due_for_rotation(&initial_key).await? {
    ///     let new_version = key_manager.rotate_key(&initial_key).await?;
    ///     println!("Key {} rotated to version {}", key_id, new_version);
    ///
    ///     // Update rotation schedule
    ///     key_manager.update_key_rotation_schedule(&initial_key, None).await?;
    /// }
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn rotate_key(&mut self, key_id: &str) -> Result<u32, EncryptionError> {
        info!("Rotating encryption key: {}", key_id);

        let current_key = self.load_key(key_id).await?;
        if current_key.purpose == KeyPurpose::KEK {
            return Err(EncryptionError::KeyManagement(format!(
                "Key {} is a key-encryption key; use generate_master_key to replace it",
                key_id
            )));
        }

        let new_key_material = random_key_material(&current_key.algorithm);
        let (master_key_id, master_key) = self.current_master_key()?;
        let encrypted_key_material = wrap_key_material(&master_key, &new_key_material)?;

        let now = Utc::now();
        let new_version = current_key.version + 1;
        let new_key_record = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: key_id.to_string(),
            version: new_version,
            algorithm: current_key.algorithm,
            encrypted_key_material,
            derivation_salt: None,
            source: KeySource::Generated("rotation".to_string()),
            purpose: current_key.purpose,
            created_at: now,
            created_by: Some("hammerwork-rotation".to_string()),
            expires_at: current_key.expires_at,
            rotated_at: Some(now),
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: current_key.rotation_interval,
            next_rotation_at: current_key.rotation_interval.map(|interval| now + interval),
            key_strength: current_key.key_strength,
            master_key_id: Some(master_key_id),
            last_used_at: None,
            usage_count: 0,
        };

        // Store the new version and retire the old one atomically
        DB::insert_rotated_key(&self.pool, &new_key_record, current_key.version).await?;

        self.cache_key(key_id, new_key_material);
        self.cleanup_old_key_versions(key_id).await?;

        if self.config.audit_enabled {
            self.record_audit_event(key_id, KeyOperation::Rotate, true, None)
                .await?;
        }

        self.update_stats(|stats| stats.rotations_performed += 1);

        info!(
            "Successfully rotated key {} to version {}",
            key_id, new_version
        );
        Ok(new_version)
    }

    /// Rotate every key whose `next_rotation_at` has passed
    ///
    /// Returns the IDs of the rotated keys. Does nothing when automatic rotation is
    /// disabled. Every due key is attempted; if any rotation fails, the error lists the
    /// keys that could not be rotated (the others stay rotated).
    pub async fn perform_automatic_rotation(&mut self) -> Result<Vec<String>, EncryptionError> {
        if !self.config.auto_rotation_enabled {
            return Ok(vec![]);
        }

        let due_keys = DB::keys_due_for_rotation(&self.pool).await?;
        let mut rotated = Vec::with_capacity(due_keys.len());
        let mut failures = Vec::new();

        for key_id in due_keys {
            match self.rotate_key(&key_id).await {
                Ok(_) => rotated.push(key_id),
                Err(e) => {
                    error!("Automatic rotation of key {} failed: {}", key_id, e);
                    failures.push(format!("{}: {}", key_id, e));
                }
            }
        }

        if failures.is_empty() {
            Ok(rotated)
        } else {
            Err(EncryptionError::KeyManagement(format!(
                "automatic rotation failed for {} key(s) ({} rotated): {}",
                failures.len(),
                rotated.len(),
                failures.join("; ")
            )))
        }
    }

    /// Start automated key rotation service that runs in the background
    /// Returns a future that should be spawned as a background task
    pub async fn start_rotation_service(
        &self,
        check_interval: Duration,
    ) -> Result<impl std::future::Future<Output = ()> + Send + 'static, EncryptionError> {
        if !self.config.auto_rotation_enabled {
            return Err(EncryptionError::InvalidConfiguration(
                "Auto rotation is not enabled".to_string(),
            ));
        }

        let period = check_interval
            .to_std()
            .ok()
            .filter(|period| !period.is_zero())
            .ok_or_else(|| {
                EncryptionError::InvalidConfiguration(
                    "Rotation check interval must be positive".to_string(),
                )
            })?;

        let mut rotation_manager = self.clone();

        let rotation_service = async move {
            let mut interval_timer = tokio::time::interval(period);

            loop {
                interval_timer.tick().await;

                match rotation_manager.perform_automatic_rotation().await {
                    Ok(rotated_keys) => {
                        if !rotated_keys.is_empty() {
                            info!(
                                "Background rotation service rotated {} keys: {:?}",
                                rotated_keys.len(),
                                rotated_keys
                            );
                        }
                    }
                    Err(e) => {
                        error!("Background rotation service failed: {:?}", e);
                    }
                }
            }
        };

        Ok(rotation_service)
    }

    /// Get current key management statistics
    pub async fn get_stats(&self) -> KeyManagerStats {
        self.stats
            .lock()
            .map(|stats| stats.clone())
            .unwrap_or_default()
    }

    /// Refresh statistics by querying the database
    ///
    /// Key counts and ages come from the database; `total_access_operations` and
    /// `rotations_performed` are counted by this instance.
    pub async fn refresh_stats(&self) -> Result<(), EncryptionError> {
        let db_stats = DB::statistics(&self.pool).await?;
        self.update_stats(|stats| {
            *stats = KeyManagerStats {
                total_access_operations: stats.total_access_operations,
                rotations_performed: stats.rotations_performed,
                ..db_stats
            };
        });
        Ok(())
    }

    /// Get the current master key ID
    pub async fn get_master_key_id(&self) -> Option<Uuid> {
        self.master_key_id.lock().map(|id| *id).unwrap_or(None)
    }

    /// Set the master key ID
    pub async fn set_master_key_id(&self, key_id: Uuid) -> Result<(), EncryptionError> {
        *self
            .master_key_id
            .lock()
            .map_err(|_| lock_error("master key ID"))? = Some(key_id);
        Ok(())
    }

    /// Generate and store a new master key (key-encryption key)
    ///
    /// The new key is encrypted with the configured master key
    /// ([`KeyManagerConfig::master_key_source`]) and stored in the database, and is used to
    /// encrypt keys generated or rotated from now on. Any previous key-encryption key is
    /// retired but kept, so keys it encrypted can still be decrypted. Other `KeyManager`
    /// instances load the new key the next time they are created.
    pub async fn generate_master_key(&mut self) -> Result<Uuid, EncryptionError> {
        let (root_key_id, root_key) = self.root_key()?;

        let master_key_id = Uuid::new_v4();
        let master_key_material = random_key_material(&EncryptionAlgorithm::AES256GCM);
        let now = Utc::now();
        let record = EncryptionKey {
            id: master_key_id,
            key_id: master_key_id.to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: wrap_key_material(&root_key, &master_key_material)?,
            derivation_salt: None,
            source: KeySource::Generated("master-key".to_string()),
            purpose: KeyPurpose::KEK,
            created_at: now,
            created_by: Some("hammerwork".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: Some(root_key_id),
            last_used_at: None,
            usage_count: 0,
        };

        DB::insert_master_key(&self.pool, &record).await?;

        *self
            .master_key
            .lock()
            .map_err(|_| lock_error("master key"))? = Some(master_key_material);
        self.set_master_key_id(master_key_id).await?;

        if self.config.audit_enabled {
            self.record_audit_event(&master_key_id.to_string(), KeyOperation::Create, true, None)
                .await?;
        }

        self.update_stats(|stats| {
            stats.total_keys += 1;
            stats.active_keys += 1;
        });

        info!("Generated new master key: {}", master_key_id);
        Ok(master_key_id)
    }

    /// Rotate a KMS-wrapped master key (`aws://` and `gcp://` master key sources).
    ///
    /// Generates a new data key with the KMS, stores its KMS-encrypted form as the new
    /// active version in `hammerwork_kms_data_keys` and retires the previous version.
    /// Earlier versions stay stored, so keys they encrypted can still be decrypted. If no
    /// key-encryption key was created with [`KeyManager::generate_master_key`], keys
    /// generated or rotated from now on are encrypted with the new version; otherwise call
    /// `generate_master_key` to create a key-encryption key under it.
    ///
    /// Other running `KeyManager` instances keep using the version they loaded until they
    /// are recreated; keys they encrypt stay readable because no version is deleted.
    ///
    /// Returns the new version number. Fails with
    /// [`EncryptionError::InvalidConfiguration`] for other master key sources.
    pub async fn rotate_kms_master_key(&mut self) -> Result<u32, EncryptionError> {
        let kms = self.kms.clone().ok_or_else(|| {
            EncryptionError::InvalidConfiguration(
                "rotate_kms_master_key needs an aws:// or gcp:// master key source".to_string(),
            )
        })?;
        let (old_id, old_key) = self.root_key()?;
        let rotated =
            envelope::rotate(&DbStore(&self.pool), kms.as_ref(), KMS_MASTER_KEY_NAME, 32).await?;
        let new_id = derive_master_key_id(&rotated.key);

        self.retired_root_keys
            .lock()
            .map_err(|_| lock_error("retired root keys"))?
            .insert(old_id, old_key);
        *self.root_key.lock().map_err(|_| lock_error("root key"))? =
            Some((new_id, rotated.key.clone()));
        if self.get_master_key_id().await == Some(old_id) {
            // No key-encryption key: the root key wraps new keys directly
            *self
                .master_key
                .lock()
                .map_err(|_| lock_error("master key"))? = Some(rotated.key);
            self.set_master_key_id(new_id).await?;
        }

        if self.config.audit_enabled {
            self.record_audit_event(KMS_MASTER_KEY_NAME, KeyOperation::Rotate, true, None)
                .await?;
        }
        self.update_stats(|stats| stats.rotations_performed += 1);

        info!(
            "Rotated KMS-wrapped master key to version {}",
            rotated.version
        );
        Ok(rotated.version)
    }

    /// Check if the active version of a key is due for rotation
    pub async fn is_key_due_for_rotation(&self, key_id: &str) -> Result<bool, EncryptionError> {
        DB::is_key_due_for_rotation(&self.pool, key_id).await
    }

    /// Update the rotation interval of a key; the next rotation is scheduled one interval
    /// from now (or cleared when `rotation_interval` is `None`)
    pub async fn update_key_rotation_schedule(
        &self,
        key_id: &str,
        rotation_interval: Option<Duration>,
    ) -> Result<(), EncryptionError> {
        let next_rotation_at = rotation_interval.map(|interval| Utc::now() + interval);
        DB::update_rotation_schedule(&self.pool, key_id, rotation_interval, next_rotation_at)
            .await?;
        info!(
            "Updated rotation schedule for key {}: interval={:?}, next_rotation={:?}",
            key_id, rotation_interval, next_rotation_at
        );
        Ok(())
    }

    /// Get the next scheduled rotation time for a key
    pub async fn get_key_rotation_schedule(
        &self,
        key_id: &str,
    ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
        DB::rotation_schedule(&self.pool, key_id).await
    }

    /// Schedule a key for rotation at a specific time
    pub async fn schedule_key_rotation(
        &self,
        key_id: &str,
        rotation_time: DateTime<Utc>,
    ) -> Result<(), EncryptionError> {
        DB::schedule_rotation(&self.pool, key_id, rotation_time).await?;
        info!("Scheduled rotation for key {} at {}", key_id, rotation_time);
        Ok(())
    }

    /// Get all keys scheduled for rotation within a time window
    pub async fn get_scheduled_rotations(
        &self,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
    ) -> Result<Vec<(String, DateTime<Utc>)>, EncryptionError> {
        DB::scheduled_rotations(&self.pool, from_time, to_time).await
    }

    /// Query key statistics from the database
    ///
    /// `total_access_operations` and `rotations_performed` are not stored in the database
    /// and are returned as zero; [`KeyManager::get_stats`] includes them.
    pub async fn query_database_statistics(&self) -> Result<KeyManagerStats, EncryptionError> {
        DB::statistics(&self.pool).await
    }

    /// Get the IDs of keys whose active version is due for rotation
    pub async fn get_keys_due_for_rotation(&self) -> Result<Vec<String>, EncryptionError> {
        DB::keys_due_for_rotation(&self.pool).await
    }

    // Private helper methods

    /// Load the configured master key and, if one exists, the active key-encryption key.
    async fn load_master_key(&self) -> Result<(), EncryptionError> {
        let root_key = match &self.kms {
            Some(kms) => {
                envelope::load_or_create(
                    &DbStore(&self.pool),
                    kms.as_ref(),
                    KMS_MASTER_KEY_NAME,
                    32,
                )
                .await?
                .key
            }
            None => load_master_key_material(&self.config.master_key_source).await?,
        };
        if root_key.len() != 32 {
            return Err(EncryptionError::KeyManagement(format!(
                "Master key must be 32 bytes, got {}",
                root_key.len()
            )));
        }
        let root_key_id = derive_master_key_id(&root_key);
        *self.root_key.lock().map_err(|_| lock_error("root key"))? =
            Some((root_key_id, root_key.clone()));

        let (master_key_id, master_key) = match DB::load_active_master_key(&self.pool).await? {
            Some(kek) => {
                let material = self.unwrap_kek(&kek).await.map_err(|e| {
                    EncryptionError::KeyManagement(format!(
                        "The configured master key cannot decrypt the active key-encryption \
                         key {}; check master_key_source ({})",
                        kek.key_id, e
                    ))
                })?;
                (kek.id, material)
            }
            None => (root_key_id, root_key),
        };

        *self
            .master_key
            .lock()
            .map_err(|_| lock_error("master key"))? = Some(master_key);
        self.set_master_key_id(master_key_id).await?;

        debug!("Master key loaded successfully with ID: {}", master_key_id);
        Ok(())
    }

    /// The master key that new keys are encrypted with, and its ID.
    fn current_master_key(&self) -> Result<(Uuid, Vec<u8>), EncryptionError> {
        let id = (*self
            .master_key_id
            .lock()
            .map_err(|_| lock_error("master key ID"))?)
        .ok_or_else(|| EncryptionError::KeyManagement("Master key not loaded".to_string()))?;
        let key = self
            .master_key
            .lock()
            .map_err(|_| lock_error("master key"))?
            .clone()
            .ok_or_else(|| EncryptionError::KeyManagement("Master key not loaded".to_string()))?;
        Ok((id, key))
    }

    /// The configured master key (from `master_key_source`) and its ID.
    fn root_key(&self) -> Result<(Uuid, Vec<u8>), EncryptionError> {
        self.root_key
            .lock()
            .map_err(|_| lock_error("root key"))?
            .clone()
            .ok_or_else(|| EncryptionError::KeyManagement("Master key not loaded".to_string()))
    }

    /// Decrypt a stored key with the master key that encrypted it.
    async fn unwrap_key_material(&self, key: &EncryptionKey) -> Result<Vec<u8>, EncryptionError> {
        let (current_id, current_key) = self.current_master_key()?;
        let wrapping_key = match key.master_key_id {
            None => current_key,
            Some(id) if id == current_id => current_key,
            Some(id) => match self.root_key_by_id(id).await? {
                Some(root_key) => root_key,
                None => {
                    let kek = DB::load_latest_key(&self.pool, &id.to_string())
                        .await?
                        .filter(|kek| kek.purpose == KeyPurpose::KEK)
                        .ok_or_else(|| {
                            EncryptionError::KeyManagement(format!(
                                "Master key {} that encrypted key {} was not found",
                                id, key.key_id
                            ))
                        })?;
                    self.unwrap_kek(&kek).await?
                }
            },
        };
        unwrap_key_material(&wrapping_key, &key.encrypted_key_material)
    }

    /// Decrypt a key-encryption key with the root key that encrypted it.
    async fn unwrap_kek(&self, kek: &EncryptionKey) -> Result<Vec<u8>, EncryptionError> {
        let (root_id, root_key) = self.root_key()?;
        let wrapping_key = match kek.master_key_id {
            Some(id) if id != root_id => self.root_key_by_id(id).await?.unwrap_or(root_key),
            _ => root_key,
        };
        unwrap_key_material(&wrapping_key, &kek.encrypted_key_material)
    }

    /// The configured master key (root key) with derived ID `id`: the current one or, for
    /// a KMS-wrapped master key, an earlier (rotated) version.
    async fn root_key_by_id(&self, id: Uuid) -> Result<Option<Vec<u8>>, EncryptionError> {
        let (root_id, root_key) = self.root_key()?;
        if id == root_id {
            return Ok(Some(root_key));
        }
        let Some(kms) = &self.kms else {
            return Ok(None);
        };
        if let Some(key) = self
            .retired_root_keys
            .lock()
            .map_err(|_| lock_error("retired root keys"))?
            .get(&id)
        {
            return Ok(Some(key.clone()));
        }

        // Not seen yet (rotated by another process, or before this one started): decrypt
        // every stored version with the KMS and remember them.
        let versions =
            envelope::load_all(&DbStore(&self.pool), kms.as_ref(), KMS_MASTER_KEY_NAME, 32).await?;
        let mut retired = self
            .retired_root_keys
            .lock()
            .map_err(|_| lock_error("retired root keys"))?;
        for version in versions {
            let version_id = derive_master_key_id(&version.key);
            if version_id != root_id {
                retired.insert(version_id, version.key);
            }
        }
        Ok(retired.get(&id).cloned())
    }

    fn cache_key(&self, key_id: &str, key_material: Vec<u8>) {
        if let Ok(mut cache) = self.key_cache.lock() {
            cache.insert(key_id.to_string(), (key_material, Utc::now()));
        }
    }

    fn get_cached_key(&self, key_id: &str) -> Option<Vec<u8>> {
        let cache = self.key_cache.lock().ok()?;
        // Keys are cached for up to an hour
        cache
            .get(key_id)
            .filter(|(_, cached_at)| Utc::now() - *cached_at < Duration::hours(1))
            .map(|(key_material, _)| key_material.clone())
    }

    fn update_stats(&self, update: impl FnOnce(&mut KeyManagerStats)) {
        if let Ok(mut stats) = self.stats.lock() {
            update(&mut stats);
        }
    }

    async fn store_key(&self, key: &EncryptionKey) -> Result<(), EncryptionError> {
        DB::insert_key(&self.pool, key).await
    }

    async fn load_key(&self, key_id: &str) -> Result<EncryptionKey, EncryptionError> {
        DB::load_latest_key(&self.pool, key_id)
            .await?
            .ok_or_else(|| EncryptionError::KeyManagement(format!("Key not found: {}", key_id)))
    }

    async fn cleanup_old_key_versions(&self, key_id: &str) -> Result<(), EncryptionError> {
        if self.config.max_key_versions == 0 {
            return Ok(());
        }
        DB::delete_old_key_versions(&self.pool, key_id, self.config.max_key_versions).await
    }

    async fn record_key_usage(&self, key_id: &str) -> Result<(), EncryptionError> {
        DB::record_key_usage(&self.pool, key_id).await?;
        self.update_stats(|stats| stats.total_access_operations += 1);
        Ok(())
    }

    async fn record_audit_event(
        &self,
        key_id: &str,
        operation: KeyOperation,
        success: bool,
        error_message: Option<String>,
    ) -> Result<(), EncryptionError> {
        DB::record_audit_event(
            &self.pool,
            key_id,
            &operation,
            success,
            error_message.as_deref(),
        )
        .await
    }

    /// ID of the active key-encryption key in the database, if any
    #[cfg(test)]
    async fn find_master_key_id_in_database(&self) -> Result<Option<Uuid>, EncryptionError> {
        Ok(DB::load_active_master_key(&self.pool)
            .await?
            .map(|key| key.id))
    }

    /// The active key-encryption key's ID, or an ID derived from `key_material`
    #[cfg(test)]
    async fn get_or_create_master_key_id(
        &self,
        key_material: &[u8],
    ) -> Result<Uuid, EncryptionError> {
        match self.find_master_key_id_in_database().await? {
            Some(key_id) => Ok(key_id),
            None => Ok(derive_master_key_id(key_material)),
        }
    }
}

fn lock_error(what: &str) -> EncryptionError {
    EncryptionError::KeyManagement(format!("Failed to acquire {} lock", what))
}

fn check_key_usable(key: &EncryptionKey) -> Result<(), EncryptionError> {
    if key.status == KeyStatus::Revoked {
        return Err(EncryptionError::KeyManagement(format!(
            "Key {} has been revoked",
            key.key_id
        )));
    }
    if key.status == KeyStatus::Expired || key.expires_at.is_some_and(|at| Utc::now() > at) {
        return Err(EncryptionError::KeyManagement(format!(
            "Key {} has expired",
            key.key_id
        )));
    }
    Ok(())
}

fn random_key_material(algorithm: &EncryptionAlgorithm) -> Vec<u8> {
    let key_length = match algorithm {
        EncryptionAlgorithm::AES256GCM => 32,
        EncryptionAlgorithm::ChaCha20Poly1305 => 32,
    };
    let mut key_material = vec![0u8; key_length];
    OsRng.fill_bytes(&mut key_material);
    key_material
}

/// Deterministic ID for a master key, derived from its material.
fn derive_master_key_id(key_material: &[u8]) -> Uuid {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(key_material);
    hasher.update(b"hammerwork-master-key-v1"); // Version tag for future compatibility
    let hash = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    Uuid::from_bytes(bytes)
}

/// Encrypt key material with AES-256-GCM; the output is `nonce || ciphertext || tag`.
fn wrap_key_material(wrapping_key: &[u8], key_material: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    let cipher = Aes256Gcm::new_from_slice(wrapping_key)
        .map_err(|e| EncryptionError::KeyManagement(format!("Invalid master key: {}", e)))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), key_material)
        .map_err(|e| EncryptionError::EncryptionFailed(format!("Key encryption failed: {}", e)))?;

    let mut encrypted = nonce_bytes.to_vec();
    encrypted.extend_from_slice(&ciphertext);
    Ok(encrypted)
}

/// Decrypt key material produced by [`wrap_key_material`].
fn unwrap_key_material(wrapping_key: &[u8], encrypted: &[u8]) -> Result<Vec<u8>, EncryptionError> {
    if encrypted.len() < 12 {
        return Err(EncryptionError::DecryptionFailed(
            "Encrypted key data too short".to_string(),
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(wrapping_key)
        .map_err(|e| EncryptionError::KeyManagement(format!("Invalid master key: {}", e)))?;
    let (nonce, ciphertext) = encrypted.split_at(12);
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|e| EncryptionError::DecryptionFailed(format!("Key decryption failed: {}", e)))
}

/// Label stored in the `key_source` column. Only the kind of source is stored, never its
/// detail: for [`KeySource::Static`] the detail is the key itself.
fn key_source_label(source: &KeySource) -> &'static str {
    match source {
        KeySource::Environment(_) => "Environment",
        KeySource::Static(_) => "Static",
        KeySource::Generated(_) => "Generated",
        KeySource::External(_) => "External",
    }
}

fn db_error(context: &str) -> impl FnOnce(sqlx::Error) -> EncryptionError + '_ {
    move |e| EncryptionError::DatabaseError(format!("{}: {}", context, e))
}

fn column<'r, R, T>(row: &'r R, name: &str) -> Result<T, EncryptionError>
where
    R: Row,
    T: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    for<'n> &'n str: sqlx::ColumnIndex<R>,
{
    row.try_get(name).map_err(|e| {
        EncryptionError::DatabaseError(format!("Failed to decode column {}: {}", name, e))
    })
}

fn to_i32(value: u32, what: &str) -> Result<i32, EncryptionError> {
    i32::try_from(value)
        .map_err(|_| EncryptionError::KeyManagement(format!("{} {} is out of range", what, value)))
}

fn to_u32(value: i32, what: &str) -> Result<u32, EncryptionError> {
    u32::try_from(value)
        .map_err(|_| EncryptionError::KeyManagement(format!("{} {} is out of range", what, value)))
}

/// Load the master key material named by a [`KeySource`].
///
/// Fails closed: there is no fallback key when the source is unavailable.
async fn load_master_key_material(source: &KeySource) -> Result<Vec<u8>, EncryptionError> {
    match source {
        KeySource::Environment(env_var) => {
            let key_str = std::env::var(env_var).map_err(|_| {
                EncryptionError::KeyManagement(format!(
                    "Master key environment variable {} not found",
                    env_var
                ))
            })?;
            base64::engine::general_purpose::STANDARD
                .decode(key_str.trim())
                .map_err(|e| {
                    EncryptionError::KeyManagement(format!("Invalid base64 master key: {}", e))
                })
        }
        KeySource::Static(key_str) => base64::engine::general_purpose::STANDARD
            .decode(key_str)
            .map_err(|e| {
                EncryptionError::KeyManagement(format!("Invalid base64 master key: {}", e))
            }),
        KeySource::Generated(_) => {
            // A random key that only lives in memory (development only)
            warn!(
                "Generating a random in-memory master key: keys it encrypts cannot be \
                 decrypted after a restart. Do not use KeySource::Generated in production."
            );
            Ok(random_key_material(&EncryptionAlgorithm::AES256GCM))
        }
        KeySource::External(service_config) => {
            if service_config.starts_with("aws://") || service_config.starts_with("gcp://") {
                // KMS sources use envelope encryption and are loaded by KeyManager with
                // its database (see `envelope`); there is no stateless way to load them.
                Err(EncryptionError::KeyManagement(format!(
                    "{} master keys are stored KMS-wrapped in the database and are loaded \
                     by KeyManager::new",
                    &service_config[..service_config.find("://").unwrap_or(0) + 3]
                )))
            } else if service_config.starts_with("vault://") {
                load_master_key_from_vault(service_config).await
            } else if service_config.starts_with("azure://") {
                load_master_key_from_azure(service_config).await
            } else {
                Err(EncryptionError::KeyManagement(format!(
                    "Unknown external master key service: {}",
                    service_config
                )))
            }
        }
    }
}

/// Load the master key from a HashiCorp Vault KV v2 secret.
///
/// Format: `vault://<mount>/<path>?addr=<vault-address>`. The address falls back to
/// `VAULT_ADDR`; the token is read from `VAULT_TOKEN`. The secret must have a string `key`
/// field: a base64-encoded 32-byte key, or a passphrase that is hashed with SHA-256.
async fn load_master_key_from_vault(service_config: &str) -> Result<Vec<u8>, EncryptionError> {
    let (secret_path, params) = kms::parse_service_config(service_config, "vault://");
    let vault_addr = kms::vault_address(&params)?;
    let (mount, secret) = kms::vault_mount_and_path(secret_path)?;

    info!(
        "Loading master key from HashiCorp Vault: path={}, addr={}",
        secret_path, vault_addr
    );

    #[cfg(not(feature = "vault-kms"))]
    {
        let _ = (mount, secret);
        Err(kms::kms_feature_disabled("vault://", "vault-kms"))
    }

    #[cfg(feature = "vault-kms")]
    {
        let key_str = kms::vault_read_key_field(&vault_addr, mount, secret).await?;

        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&key_str)
            && decoded.len() == 32
        {
            info!("Successfully loaded master key from HashiCorp Vault");
            return Ok(decoded);
        }

        // Not a base64 32-byte key: treat it as a passphrase
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(key_str.as_bytes());
        info!("Successfully loaded and hashed master key from HashiCorp Vault");
        Ok(hash.to_vec())
    }
}

/// Load the master key from Azure Key Vault.
///
/// Format: `azure://<vault-host>/keys/<key-name>` (key name defaults to `master-key`).
/// Credentials are resolved from the environment (client secret, workload identity,
/// managed identity, Azure CLI). Key material shorter than 32 bytes is expanded with
/// HMAC-SHA256; longer material is truncated.
async fn load_master_key_from_azure(service_config: &str) -> Result<Vec<u8>, EncryptionError> {
    let (vault_url, key_name) = kms::azure_vault_and_key(service_config, "master-key")?;

    info!(
        "Loading master key from Azure Key Vault: vault={}, key={}",
        vault_url, key_name
    );

    #[cfg(not(feature = "azure-kv"))]
    {
        let _ = (vault_url, key_name);
        Err(kms::kms_feature_disabled("azure://", "azure-kv"))
    }

    #[cfg(feature = "azure-kv")]
    {
        let key_material = load_from_azure_key_vault(&vault_url, &key_name)
            .await
            .map_err(|e| {
                EncryptionError::KeyManagement(format!(
                    "Failed to load master key from Azure Key Vault: {}",
                    e
                ))
            })?;
        info!("Successfully loaded master key from Azure Key Vault");
        Ok(key_material)
    }
}

/// Load key material from Azure Key Vault and normalize it to 32 bytes.
///
/// Material of 32 bytes or more is truncated to 32 bytes; shorter material is used as the
/// HMAC-SHA256 key to derive 32 bytes.
#[cfg(feature = "azure-kv")]
async fn load_from_azure_key_vault(vault_url: &str, key_name: &str) -> Result<Vec<u8>, String> {
    let decoded_key = super::azure::fetch_key_material(vault_url, key_name).await?;

    if decoded_key.len() >= 32 {
        Ok(decoded_key[0..32].to_vec())
    } else {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(&decoded_key)
            .map_err(|e| format!("Failed to create HMAC: {}", e))?;
        hmac.update(b"azure-kv-master-key-derivation");
        hmac.update(vault_url.as_bytes());
        hmac.update(key_name.as_bytes());
        let result = hmac.finalize();
        Ok(result.into_bytes()[0..32].to_vec())
    }
}

// Database-specific implementations

/// Columns selected for an [`EncryptionKey`] row (PostgreSQL).
#[cfg(feature = "postgres")]
macro_rules! pg_select_key {
    () => {
        r#"SELECT id, key_id, key_version, algorithm, key_material, key_derivation_salt,
                  key_source, key_purpose, created_at, created_by, expires_at, rotated_at,
                  retired_at, status,
                  EXTRACT(EPOCH FROM rotation_interval)::BIGINT AS rotation_interval_seconds,
                  next_rotation_at, key_strength, master_key_id, last_used_at, usage_count
           FROM hammerwork_encryption_keys "#
    };
}

#[cfg(feature = "postgres")]
macro_rules! pg_insert_kms_key {
    () => {
        r#"INSERT INTO hammerwork_kms_data_keys (
               id, key_name, kms_provider, kms_key_id, key_version, key_size, wrapped_key,
               status, created_at, retired_at
           ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#
    };
}

#[cfg(feature = "postgres")]
mod postgres_backend {
    use super::*;
    use sqlx::postgres::{PgRow, Postgres};

    fn row_to_key(row: &PgRow) -> Result<EncryptionKey, EncryptionError> {
        Ok(EncryptionKey {
            id: column(row, "id")?,
            key_id: column(row, "key_id")?,
            version: to_u32(column(row, "key_version")?, "key version")?,
            algorithm: parse_algorithm(&column::<_, String>(row, "algorithm")?)?,
            encrypted_key_material: column(row, "key_material")?,
            derivation_salt: column(row, "key_derivation_salt")?,
            source: parse_key_source(&column::<_, String>(row, "key_source")?)?,
            purpose: parse_key_purpose(&column::<_, String>(row, "key_purpose")?)?,
            created_at: column(row, "created_at")?,
            created_by: column(row, "created_by")?,
            expires_at: column(row, "expires_at")?,
            rotated_at: column(row, "rotated_at")?,
            retired_at: column(row, "retired_at")?,
            status: parse_key_status(&column::<_, String>(row, "status")?)?,
            rotation_interval: column::<_, Option<i64>>(row, "rotation_interval_seconds")?
                .map(Duration::seconds),
            next_rotation_at: column(row, "next_rotation_at")?,
            key_strength: to_u32(column(row, "key_strength")?, "key strength")?,
            master_key_id: column(row, "master_key_id")?,
            last_used_at: column(row, "last_used_at")?,
            usage_count: u64::try_from(column::<_, i64>(row, "usage_count")?).unwrap_or(0),
        })
    }

    async fn insert<'e, E: sqlx::PgExecutor<'e>>(
        executor: E,
        key: &EncryptionKey,
    ) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_encryption_keys (
                id, key_id, key_version, algorithm, key_material, key_derivation_salt,
                key_source, key_purpose, created_at, created_by, expires_at, rotated_at,
                retired_at, status, rotation_interval, next_rotation_at, key_strength,
                master_key_id, last_used_at, usage_count
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                $15::BIGINT * INTERVAL '1 second', $16, $17, $18, $19, $20
            )
            "#,
        )
        .bind(key.id)
        .bind(&key.key_id)
        .bind(to_i32(key.version, "key version")?)
        .bind(key.algorithm.to_string())
        .bind(&key.encrypted_key_material)
        .bind(&key.derivation_salt)
        .bind(key_source_label(&key.source))
        .bind(key.purpose.to_string())
        .bind(key.created_at)
        .bind(&key.created_by)
        .bind(key.expires_at)
        .bind(key.rotated_at)
        .bind(key.retired_at)
        .bind(key.status.to_string())
        .bind(key.rotation_interval.map(|d| d.num_seconds()))
        .bind(key.next_rotation_at)
        .bind(to_i32(key.key_strength, "key strength")?)
        .bind(key.master_key_id)
        .bind(key.last_used_at)
        .bind(i64::try_from(key.usage_count).unwrap_or(i64::MAX))
        .execute(executor)
        .await
        .map_err(db_error("Failed to store key"))?;
        Ok(())
    }

    async fn insert_kms_wrapped_key<'e, E: sqlx::PgExecutor<'e>>(
        executor: E,
        key: &KmsWrappedKey,
        if_absent: bool,
    ) -> Result<(), EncryptionError> {
        let sql = if if_absent {
            concat!(
                pg_insert_kms_key!(),
                " ON CONFLICT (key_name, kms_provider, kms_key_id, key_version) DO NOTHING"
            )
        } else {
            pg_insert_kms_key!()
        };
        sqlx::query(sql)
            .bind(key.id)
            .bind(&key.key_name)
            .bind(&key.kms_provider)
            .bind(&key.kms_key_id)
            .bind(to_i32(key.version, "key version")?)
            .bind(to_i32(key.key_size, "key size")?)
            .bind(&key.wrapped_key)
            .bind(if key.active { "Active" } else { "Retired" })
            .bind(key.created_at)
            .bind(key.retired_at)
            .execute(executor)
            .await
            .map_err(db_error("Failed to store KMS data key"))?;
        Ok(())
    }

    #[async_trait::async_trait]
    impl KeyManagerBackend for Postgres {
        async fn insert_key(pool: &Pool<Self>, key: &EncryptionKey) -> Result<(), EncryptionError> {
            insert(pool, key).await
        }

        async fn insert_rotated_key(
            pool: &Pool<Self>,
            key: &EncryptionKey,
            previous_version: u32,
        ) -> Result<(), EncryptionError> {
            let mut tx = pool
                .begin()
                .await
                .map_err(db_error("Failed to start transaction"))?;
            insert(&mut *tx, key).await?;
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET status = 'Retired', retired_at = NOW()
                WHERE key_id = $1 AND key_version = $2 AND status = 'Active'
                "#,
            )
            .bind(&key.key_id)
            .bind(to_i32(previous_version, "key version")?)
            .execute(&mut *tx)
            .await
            .map_err(db_error("Failed to retire key version"))?;
            tx.commit()
                .await
                .map_err(db_error("Failed to commit key rotation"))
        }

        async fn insert_master_key(
            pool: &Pool<Self>,
            key: &EncryptionKey,
        ) -> Result<(), EncryptionError> {
            let mut tx = pool
                .begin()
                .await
                .map_err(db_error("Failed to start transaction"))?;
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET status = 'Retired', retired_at = NOW()
                WHERE key_purpose = 'KEK' AND status = 'Active'
                "#,
            )
            .execute(&mut *tx)
            .await
            .map_err(db_error("Failed to retire master key"))?;
            insert(&mut *tx, key).await?;
            tx.commit()
                .await
                .map_err(db_error("Failed to commit master key"))
        }

        async fn load_latest_key(
            pool: &Pool<Self>,
            key_id: &str,
        ) -> Result<Option<EncryptionKey>, EncryptionError> {
            sqlx::query(concat!(
                pg_select_key!(),
                "WHERE key_id = $1 ORDER BY key_version DESC LIMIT 1"
            ))
            .bind(key_id)
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to load key"))?
            .as_ref()
            .map(row_to_key)
            .transpose()
        }

        async fn load_key_version(
            pool: &Pool<Self>,
            key_id: &str,
            version: u32,
        ) -> Result<Option<EncryptionKey>, EncryptionError> {
            sqlx::query(concat!(
                pg_select_key!(),
                "WHERE key_id = $1 AND key_version = $2"
            ))
            .bind(key_id)
            .bind(to_i32(version, "key version")?)
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to load key version"))?
            .as_ref()
            .map(row_to_key)
            .transpose()
        }

        async fn load_active_master_key(
            pool: &Pool<Self>,
        ) -> Result<Option<EncryptionKey>, EncryptionError> {
            sqlx::query(concat!(
                pg_select_key!(),
                "WHERE key_purpose = 'KEK' AND status = 'Active' ORDER BY created_at DESC LIMIT 1"
            ))
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to load master key"))?
            .as_ref()
            .map(row_to_key)
            .transpose()
        }

        async fn delete_old_key_versions(
            pool: &Pool<Self>,
            key_id: &str,
            keep: u32,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                DELETE FROM hammerwork_encryption_keys
                WHERE key_id = $1 AND key_version <= (
                    SELECT MAX(key_version) FROM hammerwork_encryption_keys WHERE key_id = $1
                ) - $2
                "#,
            )
            .bind(key_id)
            .bind(i64::from(keep))
            .execute(pool)
            .await
            .map_err(db_error("Failed to clean up old key versions"))?;
            Ok(())
        }

        async fn record_key_usage(pool: &Pool<Self>, key_id: &str) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET last_used_at = NOW(), usage_count = usage_count + 1
                WHERE key_id = $1 AND status = 'Active'
                "#,
            )
            .bind(key_id)
            .execute(pool)
            .await
            .map_err(db_error("Failed to record key usage"))?;
            Ok(())
        }

        async fn record_audit_event(
            pool: &Pool<Self>,
            key_id: &str,
            operation: &KeyOperation,
            success: bool,
            error_message: Option<&str>,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_key_audit_log (key_id, operation, success, error_message, timestamp)
                VALUES ($1, $2, $3, $4, NOW())
                "#,
            )
            .bind(key_id)
            .bind(operation.to_string())
            .bind(success)
            .bind(error_message)
            .execute(pool)
            .await
            .map_err(db_error("Failed to record audit event"))?;
            Ok(())
        }

        async fn keys_due_for_rotation(pool: &Pool<Self>) -> Result<Vec<String>, EncryptionError> {
            let rows = sqlx::query(
                r#"
                SELECT DISTINCT key_id
                FROM hammerwork_encryption_keys
                WHERE status = 'Active'
                  AND key_purpose <> 'KEK'
                  AND next_rotation_at IS NOT NULL
                  AND next_rotation_at <= NOW()
                ORDER BY key_id
                "#,
            )
            .fetch_all(pool)
            .await
            .map_err(db_error("Failed to get keys due for rotation"))?;
            rows.iter().map(|row| column(row, "key_id")).collect()
        }

        async fn is_key_due_for_rotation(
            pool: &Pool<Self>,
            key_id: &str,
        ) -> Result<bool, EncryptionError> {
            let row = sqlx::query(
                r#"
                SELECT COUNT(*) AS count
                FROM hammerwork_encryption_keys
                WHERE key_id = $1
                  AND status = 'Active'
                  AND next_rotation_at IS NOT NULL
                  AND next_rotation_at <= NOW()
                "#,
            )
            .bind(key_id)
            .fetch_one(pool)
            .await
            .map_err(db_error("Failed to check rotation status"))?;
            Ok(column::<_, i64>(&row, "count")? > 0)
        }

        async fn update_rotation_schedule(
            pool: &Pool<Self>,
            key_id: &str,
            rotation_interval: Option<Duration>,
            next_rotation_at: Option<DateTime<Utc>>,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET rotation_interval = $2::BIGINT * INTERVAL '1 second', next_rotation_at = $3
                WHERE key_id = $1 AND status = 'Active'
                "#,
            )
            .bind(key_id)
            .bind(rotation_interval.map(|d| d.num_seconds()))
            .bind(next_rotation_at)
            .execute(pool)
            .await
            .map_err(db_error("Failed to update rotation schedule"))?;
            Ok(())
        }

        async fn schedule_rotation(
            pool: &Pool<Self>,
            key_id: &str,
            rotation_time: DateTime<Utc>,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET next_rotation_at = $2
                WHERE key_id = $1 AND status = 'Active'
                "#,
            )
            .bind(key_id)
            .bind(rotation_time)
            .execute(pool)
            .await
            .map_err(db_error("Failed to schedule rotation"))?;
            Ok(())
        }

        async fn rotation_schedule(
            pool: &Pool<Self>,
            key_id: &str,
        ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
            let row = sqlx::query(
                r#"
                SELECT next_rotation_at
                FROM hammerwork_encryption_keys
                WHERE key_id = $1 AND status = 'Active'
                ORDER BY key_version DESC
                LIMIT 1
                "#,
            )
            .bind(key_id)
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to get rotation schedule"))?;
            match row {
                Some(row) => column(&row, "next_rotation_at"),
                None => Ok(None),
            }
        }

        async fn scheduled_rotations(
            pool: &Pool<Self>,
            from_time: DateTime<Utc>,
            to_time: DateTime<Utc>,
        ) -> Result<Vec<(String, DateTime<Utc>)>, EncryptionError> {
            let rows = sqlx::query(
                r#"
                SELECT key_id, next_rotation_at
                FROM hammerwork_encryption_keys
                WHERE status = 'Active'
                  AND next_rotation_at IS NOT NULL
                  AND next_rotation_at BETWEEN $1 AND $2
                ORDER BY next_rotation_at ASC
                "#,
            )
            .bind(from_time)
            .bind(to_time)
            .fetch_all(pool)
            .await
            .map_err(db_error("Failed to get scheduled rotations"))?;
            rows.iter()
                .map(|row| Ok((column(row, "key_id")?, column(row, "next_rotation_at")?)))
                .collect()
        }

        async fn insert_kms_wrapped_key_if_absent(
            pool: &Pool<Self>,
            key: &KmsWrappedKey,
        ) -> Result<(), EncryptionError> {
            insert_kms_wrapped_key(pool, key, true).await
        }

        async fn insert_rotated_kms_wrapped_key(
            pool: &Pool<Self>,
            key: &KmsWrappedKey,
        ) -> Result<(), EncryptionError> {
            let mut tx = pool
                .begin()
                .await
                .map_err(db_error("Failed to start transaction"))?;
            sqlx::query(
                r#"
                UPDATE hammerwork_kms_data_keys
                SET status = 'Retired', retired_at = NOW()
                WHERE key_name = $1 AND kms_provider = $2 AND kms_key_id = $3
                  AND status = 'Active'
                "#,
            )
            .bind(&key.key_name)
            .bind(&key.kms_provider)
            .bind(&key.kms_key_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error("Failed to retire KMS data key"))?;
            insert_kms_wrapped_key(&mut *tx, key, false).await?;
            tx.commit()
                .await
                .map_err(db_error("Failed to commit KMS data key rotation"))
        }

        async fn load_kms_wrapped_keys(
            pool: &Pool<Self>,
            key_name: &str,
            kms_provider: &str,
            kms_key_id: &str,
        ) -> Result<Vec<KmsWrappedKey>, EncryptionError> {
            let rows = sqlx::query(
                r#"
                SELECT id, key_name, kms_provider, kms_key_id, key_version, key_size,
                       wrapped_key, status, created_at, retired_at
                FROM hammerwork_kms_data_keys
                WHERE key_name = $1 AND kms_provider = $2 AND kms_key_id = $3
                ORDER BY key_version DESC
                "#,
            )
            .bind(key_name)
            .bind(kms_provider)
            .bind(kms_key_id)
            .fetch_all(pool)
            .await
            .map_err(db_error("Failed to load KMS data keys"))?;
            rows.iter()
                .map(|row| {
                    Ok(KmsWrappedKey {
                        id: column(row, "id")?,
                        key_name: column(row, "key_name")?,
                        kms_provider: column(row, "kms_provider")?,
                        kms_key_id: column(row, "kms_key_id")?,
                        version: to_u32(column(row, "key_version")?, "key version")?,
                        key_size: to_u32(column(row, "key_size")?, "key size")?,
                        wrapped_key: column(row, "wrapped_key")?,
                        active: column::<_, String>(row, "status")? == "Active",
                        created_at: column(row, "created_at")?,
                        retired_at: column(row, "retired_at")?,
                    })
                })
                .collect()
        }

        async fn statistics(pool: &Pool<Self>) -> Result<KeyManagerStats, EncryptionError> {
            let row = sqlx::query(
                r#"
                SELECT
                    COUNT(*) AS total_keys,
                    COUNT(*) FILTER (WHERE status = 'Active') AS active_keys,
                    COUNT(*) FILTER (WHERE status = 'Retired') AS retired_keys,
                    COUNT(*) FILTER (WHERE status = 'Revoked') AS revoked_keys,
                    COUNT(*) FILTER (WHERE status = 'Expired') AS expired_keys,
                    COALESCE(
                        AVG(EXTRACT(EPOCH FROM (NOW() - created_at)) / 86400.0)
                            FILTER (WHERE status IN ('Active', 'Retired')),
                        0
                    )::FLOAT8 AS avg_age_days,
                    COUNT(*) FILTER (
                        WHERE status = 'Active'
                          AND expires_at IS NOT NULL
                          AND expires_at <= NOW() + INTERVAL '7 days'
                    ) AS expiring_soon,
                    COUNT(*) FILTER (
                        WHERE status = 'Active'
                          AND next_rotation_at IS NOT NULL
                          AND next_rotation_at <= NOW()
                    ) AS due_for_rotation
                FROM hammerwork_encryption_keys
                "#,
            )
            .fetch_one(pool)
            .await
            .map_err(db_error("Failed to query key statistics"))?;
            stats_from_row(&row)
        }
    }
}

#[cfg(feature = "mysql")]
macro_rules! mysql_select_key {
    () => {
        r#"SELECT id, key_id, key_version, algorithm, key_material, key_derivation_salt,
                  key_source, key_purpose, created_at, created_by, expires_at, rotated_at,
                  retired_at, status, rotation_interval_seconds, next_rotation_at,
                  key_strength, master_key_id, last_used_at, usage_count
           FROM hammerwork_encryption_keys "#
    };
}

#[cfg(feature = "mysql")]
macro_rules! mysql_insert_kms_key {
    () => {
        r#"INSERT INTO hammerwork_kms_data_keys (
               id, key_name, kms_provider, kms_key_id, key_version, key_size, wrapped_key,
               status, created_at, retired_at
           ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#
    };
}

#[cfg(feature = "mysql")]
mod mysql_backend {
    use super::*;
    use sqlx::mysql::{MySql, MySqlRow};

    fn parse_uuid(value: &str, what: &str) -> Result<Uuid, EncryptionError> {
        Uuid::parse_str(value)
            .map_err(|e| EncryptionError::KeyManagement(format!("Invalid {} UUID: {}", what, e)))
    }

    fn row_to_key(row: &MySqlRow) -> Result<EncryptionKey, EncryptionError> {
        Ok(EncryptionKey {
            id: parse_uuid(&column::<_, String>(row, "id")?, "key")?,
            key_id: column(row, "key_id")?,
            version: to_u32(column(row, "key_version")?, "key version")?,
            algorithm: parse_algorithm(&column::<_, String>(row, "algorithm")?)?,
            encrypted_key_material: column(row, "key_material")?,
            derivation_salt: column(row, "key_derivation_salt")?,
            source: parse_key_source(&column::<_, String>(row, "key_source")?)?,
            purpose: parse_key_purpose(&column::<_, String>(row, "key_purpose")?)?,
            created_at: column(row, "created_at")?,
            created_by: column(row, "created_by")?,
            expires_at: column(row, "expires_at")?,
            rotated_at: column(row, "rotated_at")?,
            retired_at: column(row, "retired_at")?,
            status: parse_key_status(&column::<_, String>(row, "status")?)?,
            rotation_interval: column::<_, Option<i64>>(row, "rotation_interval_seconds")?
                .map(Duration::seconds),
            next_rotation_at: column(row, "next_rotation_at")?,
            key_strength: to_u32(column(row, "key_strength")?, "key strength")?,
            master_key_id: column::<_, Option<String>>(row, "master_key_id")?
                .map(|id| parse_uuid(&id, "master key"))
                .transpose()?,
            last_used_at: column(row, "last_used_at")?,
            usage_count: u64::try_from(column::<_, i64>(row, "usage_count")?).unwrap_or(0),
        })
    }

    async fn insert<'e, E: sqlx::MySqlExecutor<'e>>(
        executor: E,
        key: &EncryptionKey,
    ) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_encryption_keys (
                id, key_id, key_version, algorithm, key_material, key_derivation_salt,
                key_source, key_purpose, created_at, created_by, expires_at, rotated_at,
                retired_at, status, rotation_interval_seconds, next_rotation_at, key_strength,
                master_key_id, last_used_at, usage_count
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(key.id.to_string())
        .bind(&key.key_id)
        .bind(to_i32(key.version, "key version")?)
        .bind(key.algorithm.to_string())
        .bind(&key.encrypted_key_material)
        .bind(&key.derivation_salt)
        .bind(key_source_label(&key.source))
        .bind(key.purpose.to_string())
        .bind(key.created_at)
        .bind(&key.created_by)
        .bind(key.expires_at)
        .bind(key.rotated_at)
        .bind(key.retired_at)
        .bind(key.status.to_string())
        .bind(key.rotation_interval.map(|d| d.num_seconds()))
        .bind(key.next_rotation_at)
        .bind(to_i32(key.key_strength, "key strength")?)
        .bind(key.master_key_id.map(|id| id.to_string()))
        .bind(key.last_used_at)
        .bind(i64::try_from(key.usage_count).unwrap_or(i64::MAX))
        .execute(executor)
        .await
        .map_err(db_error("Failed to store key"))?;
        Ok(())
    }

    async fn insert_kms_wrapped_key<'e, E: sqlx::MySqlExecutor<'e>>(
        executor: E,
        key: &KmsWrappedKey,
        if_absent: bool,
    ) -> Result<(), EncryptionError> {
        let sql = if if_absent {
            // A no-op update instead of INSERT IGNORE, which would also hide other errors
            concat!(mysql_insert_kms_key!(), " ON DUPLICATE KEY UPDATE id = id")
        } else {
            mysql_insert_kms_key!()
        };
        sqlx::query(sql)
            .bind(key.id.to_string())
            .bind(&key.key_name)
            .bind(&key.kms_provider)
            .bind(&key.kms_key_id)
            .bind(to_i32(key.version, "key version")?)
            .bind(to_i32(key.key_size, "key size")?)
            .bind(&key.wrapped_key)
            .bind(if key.active { "Active" } else { "Retired" })
            .bind(key.created_at)
            .bind(key.retired_at)
            .execute(executor)
            .await
            .map_err(db_error("Failed to store KMS data key"))?;
        Ok(())
    }

    #[async_trait::async_trait]
    impl KeyManagerBackend for MySql {
        async fn insert_key(pool: &Pool<Self>, key: &EncryptionKey) -> Result<(), EncryptionError> {
            insert(pool, key).await
        }

        async fn insert_rotated_key(
            pool: &Pool<Self>,
            key: &EncryptionKey,
            previous_version: u32,
        ) -> Result<(), EncryptionError> {
            let mut tx = pool
                .begin()
                .await
                .map_err(db_error("Failed to start transaction"))?;
            insert(&mut *tx, key).await?;
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET status = 'Retired', retired_at = NOW(6)
                WHERE key_id = ? AND key_version = ? AND status = 'Active'
                "#,
            )
            .bind(&key.key_id)
            .bind(to_i32(previous_version, "key version")?)
            .execute(&mut *tx)
            .await
            .map_err(db_error("Failed to retire key version"))?;
            tx.commit()
                .await
                .map_err(db_error("Failed to commit key rotation"))
        }

        async fn insert_master_key(
            pool: &Pool<Self>,
            key: &EncryptionKey,
        ) -> Result<(), EncryptionError> {
            let mut tx = pool
                .begin()
                .await
                .map_err(db_error("Failed to start transaction"))?;
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET status = 'Retired', retired_at = NOW(6)
                WHERE key_purpose = 'KEK' AND status = 'Active'
                "#,
            )
            .execute(&mut *tx)
            .await
            .map_err(db_error("Failed to retire master key"))?;
            insert(&mut *tx, key).await?;
            tx.commit()
                .await
                .map_err(db_error("Failed to commit master key"))
        }

        async fn load_latest_key(
            pool: &Pool<Self>,
            key_id: &str,
        ) -> Result<Option<EncryptionKey>, EncryptionError> {
            sqlx::query(concat!(
                mysql_select_key!(),
                "WHERE key_id = ? ORDER BY key_version DESC LIMIT 1"
            ))
            .bind(key_id)
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to load key"))?
            .as_ref()
            .map(row_to_key)
            .transpose()
        }

        async fn load_key_version(
            pool: &Pool<Self>,
            key_id: &str,
            version: u32,
        ) -> Result<Option<EncryptionKey>, EncryptionError> {
            sqlx::query(concat!(
                mysql_select_key!(),
                "WHERE key_id = ? AND key_version = ?"
            ))
            .bind(key_id)
            .bind(to_i32(version, "key version")?)
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to load key version"))?
            .as_ref()
            .map(row_to_key)
            .transpose()
        }

        async fn load_active_master_key(
            pool: &Pool<Self>,
        ) -> Result<Option<EncryptionKey>, EncryptionError> {
            sqlx::query(concat!(
                mysql_select_key!(),
                "WHERE key_purpose = 'KEK' AND status = 'Active' ORDER BY created_at DESC LIMIT 1"
            ))
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to load master key"))?
            .as_ref()
            .map(row_to_key)
            .transpose()
        }

        async fn delete_old_key_versions(
            pool: &Pool<Self>,
            key_id: &str,
            keep: u32,
        ) -> Result<(), EncryptionError> {
            // MySQL cannot read the table it deletes from in a subquery, so find the
            // newest version first.
            let row = sqlx::query(
                "SELECT MAX(key_version) AS newest FROM hammerwork_encryption_keys WHERE key_id = ?",
            )
            .bind(key_id)
            .fetch_one(pool)
            .await
            .map_err(db_error("Failed to clean up old key versions"))?;
            let Some(newest) = column::<_, Option<i32>>(&row, "newest")? else {
                return Ok(());
            };
            sqlx::query(
                "DELETE FROM hammerwork_encryption_keys WHERE key_id = ? AND key_version <= ?",
            )
            .bind(key_id)
            .bind(i64::from(newest) - i64::from(keep))
            .execute(pool)
            .await
            .map_err(db_error("Failed to clean up old key versions"))?;
            Ok(())
        }

        async fn record_key_usage(pool: &Pool<Self>, key_id: &str) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET last_used_at = NOW(6), usage_count = usage_count + 1
                WHERE key_id = ? AND status = 'Active'
                "#,
            )
            .bind(key_id)
            .execute(pool)
            .await
            .map_err(db_error("Failed to record key usage"))?;
            Ok(())
        }

        async fn record_audit_event(
            pool: &Pool<Self>,
            key_id: &str,
            operation: &KeyOperation,
            success: bool,
            error_message: Option<&str>,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                INSERT INTO hammerwork_key_audit_log (id, key_id, operation, success, error_message, timestamp)
                VALUES (?, ?, ?, ?, ?, NOW(6))
                "#,
            )
            .bind(Uuid::new_v4().to_string())
            .bind(key_id)
            .bind(operation.to_string())
            .bind(success)
            .bind(error_message)
            .execute(pool)
            .await
            .map_err(db_error("Failed to record audit event"))?;
            Ok(())
        }

        async fn keys_due_for_rotation(pool: &Pool<Self>) -> Result<Vec<String>, EncryptionError> {
            let rows = sqlx::query(
                r#"
                SELECT DISTINCT key_id
                FROM hammerwork_encryption_keys
                WHERE status = 'Active'
                  AND key_purpose <> 'KEK'
                  AND next_rotation_at IS NOT NULL
                  AND next_rotation_at <= NOW(6)
                ORDER BY key_id
                "#,
            )
            .fetch_all(pool)
            .await
            .map_err(db_error("Failed to get keys due for rotation"))?;
            rows.iter().map(|row| column(row, "key_id")).collect()
        }

        async fn is_key_due_for_rotation(
            pool: &Pool<Self>,
            key_id: &str,
        ) -> Result<bool, EncryptionError> {
            let row = sqlx::query(
                r#"
                SELECT COUNT(*) AS count
                FROM hammerwork_encryption_keys
                WHERE key_id = ?
                  AND status = 'Active'
                  AND next_rotation_at IS NOT NULL
                  AND next_rotation_at <= NOW(6)
                "#,
            )
            .bind(key_id)
            .fetch_one(pool)
            .await
            .map_err(db_error("Failed to check rotation status"))?;
            Ok(column::<_, i64>(&row, "count")? > 0)
        }

        async fn update_rotation_schedule(
            pool: &Pool<Self>,
            key_id: &str,
            rotation_interval: Option<Duration>,
            next_rotation_at: Option<DateTime<Utc>>,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET rotation_interval_seconds = ?, next_rotation_at = ?
                WHERE key_id = ? AND status = 'Active'
                "#,
            )
            .bind(rotation_interval.map(|d| d.num_seconds()))
            .bind(next_rotation_at)
            .bind(key_id)
            .execute(pool)
            .await
            .map_err(db_error("Failed to update rotation schedule"))?;
            Ok(())
        }

        async fn schedule_rotation(
            pool: &Pool<Self>,
            key_id: &str,
            rotation_time: DateTime<Utc>,
        ) -> Result<(), EncryptionError> {
            sqlx::query(
                r#"
                UPDATE hammerwork_encryption_keys
                SET next_rotation_at = ?
                WHERE key_id = ? AND status = 'Active'
                "#,
            )
            .bind(rotation_time)
            .bind(key_id)
            .execute(pool)
            .await
            .map_err(db_error("Failed to schedule rotation"))?;
            Ok(())
        }

        async fn rotation_schedule(
            pool: &Pool<Self>,
            key_id: &str,
        ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
            let row = sqlx::query(
                r#"
                SELECT next_rotation_at
                FROM hammerwork_encryption_keys
                WHERE key_id = ? AND status = 'Active'
                ORDER BY key_version DESC
                LIMIT 1
                "#,
            )
            .bind(key_id)
            .fetch_optional(pool)
            .await
            .map_err(db_error("Failed to get rotation schedule"))?;
            match row {
                Some(row) => column(&row, "next_rotation_at"),
                None => Ok(None),
            }
        }

        async fn scheduled_rotations(
            pool: &Pool<Self>,
            from_time: DateTime<Utc>,
            to_time: DateTime<Utc>,
        ) -> Result<Vec<(String, DateTime<Utc>)>, EncryptionError> {
            let rows = sqlx::query(
                r#"
                SELECT key_id, next_rotation_at
                FROM hammerwork_encryption_keys
                WHERE status = 'Active'
                  AND next_rotation_at IS NOT NULL
                  AND next_rotation_at BETWEEN ? AND ?
                ORDER BY next_rotation_at ASC
                "#,
            )
            .bind(from_time)
            .bind(to_time)
            .fetch_all(pool)
            .await
            .map_err(db_error("Failed to get scheduled rotations"))?;
            rows.iter()
                .map(|row| Ok((column(row, "key_id")?, column(row, "next_rotation_at")?)))
                .collect()
        }

        async fn insert_kms_wrapped_key_if_absent(
            pool: &Pool<Self>,
            key: &KmsWrappedKey,
        ) -> Result<(), EncryptionError> {
            insert_kms_wrapped_key(pool, key, true).await
        }

        async fn insert_rotated_kms_wrapped_key(
            pool: &Pool<Self>,
            key: &KmsWrappedKey,
        ) -> Result<(), EncryptionError> {
            let mut tx = pool
                .begin()
                .await
                .map_err(db_error("Failed to start transaction"))?;
            sqlx::query(
                r#"
                UPDATE hammerwork_kms_data_keys
                SET status = 'Retired', retired_at = NOW(6)
                WHERE key_name = ? AND kms_provider = ? AND kms_key_id = ?
                  AND status = 'Active'
                "#,
            )
            .bind(&key.key_name)
            .bind(&key.kms_provider)
            .bind(&key.kms_key_id)
            .execute(&mut *tx)
            .await
            .map_err(db_error("Failed to retire KMS data key"))?;
            insert_kms_wrapped_key(&mut *tx, key, false).await?;
            tx.commit()
                .await
                .map_err(db_error("Failed to commit KMS data key rotation"))
        }

        async fn load_kms_wrapped_keys(
            pool: &Pool<Self>,
            key_name: &str,
            kms_provider: &str,
            kms_key_id: &str,
        ) -> Result<Vec<KmsWrappedKey>, EncryptionError> {
            let rows = sqlx::query(
                r#"
                SELECT id, key_name, kms_provider, kms_key_id, key_version, key_size,
                       wrapped_key, status, created_at, retired_at
                FROM hammerwork_kms_data_keys
                WHERE key_name = ? AND kms_provider = ? AND kms_key_id = ?
                ORDER BY key_version DESC
                "#,
            )
            .bind(key_name)
            .bind(kms_provider)
            .bind(kms_key_id)
            .fetch_all(pool)
            .await
            .map_err(db_error("Failed to load KMS data keys"))?;
            rows.iter()
                .map(|row| {
                    Ok(KmsWrappedKey {
                        id: parse_uuid(&column::<_, String>(row, "id")?, "KMS data key id")?,
                        key_name: column(row, "key_name")?,
                        kms_provider: column(row, "kms_provider")?,
                        kms_key_id: column(row, "kms_key_id")?,
                        version: to_u32(column(row, "key_version")?, "key version")?,
                        key_size: to_u32(column(row, "key_size")?, "key size")?,
                        wrapped_key: column(row, "wrapped_key")?,
                        active: column::<_, String>(row, "status")? == "Active",
                        created_at: column(row, "created_at")?,
                        retired_at: column(row, "retired_at")?,
                    })
                })
                .collect()
        }

        async fn statistics(pool: &Pool<Self>) -> Result<KeyManagerStats, EncryptionError> {
            let row = sqlx::query(
                r#"
                SELECT
                    COUNT(*) AS total_keys,
                    COUNT(CASE WHEN status = 'Active' THEN 1 END) AS active_keys,
                    COUNT(CASE WHEN status = 'Retired' THEN 1 END) AS retired_keys,
                    COUNT(CASE WHEN status = 'Revoked' THEN 1 END) AS revoked_keys,
                    COUNT(CASE WHEN status = 'Expired' THEN 1 END) AS expired_keys,
                    CAST(COALESCE(AVG(CASE WHEN status IN ('Active', 'Retired')
                        THEN TIMESTAMPDIFF(SECOND, created_at, NOW(6)) END) / 86400, 0) AS DOUBLE)
                        AS avg_age_days,
                    COUNT(CASE WHEN status = 'Active'
                        AND expires_at IS NOT NULL
                        AND expires_at <= DATE_ADD(NOW(6), INTERVAL 7 DAY) THEN 1 END)
                        AS expiring_soon,
                    COUNT(CASE WHEN status = 'Active'
                        AND next_rotation_at IS NOT NULL
                        AND next_rotation_at <= NOW(6) THEN 1 END) AS due_for_rotation
                FROM hammerwork_encryption_keys
                "#,
            )
            .fetch_one(pool)
            .await
            .map_err(db_error("Failed to query key statistics"))?;
            stats_from_row(&row)
        }
    }
}

/// Build [`KeyManagerStats`] from a statistics query row.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn stats_from_row<R>(row: &R) -> Result<KeyManagerStats, EncryptionError>
where
    R: Row,
    for<'n> &'n str: sqlx::ColumnIndex<R>,
    for<'r> i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    for<'r> f64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    let count = |name: &str| -> Result<u64, EncryptionError> {
        Ok(u64::try_from(column::<_, i64>(row, name)?).unwrap_or(0))
    };
    Ok(KeyManagerStats {
        total_keys: count("total_keys")?,
        active_keys: count("active_keys")?,
        retired_keys: count("retired_keys")?,
        revoked_keys: count("revoked_keys")?,
        expired_keys: count("expired_keys")?,
        total_access_operations: 0,
        rotations_performed: 0,
        average_key_age_days: column(row, "avg_age_days")?,
        keys_expiring_soon: count("expiring_soon")?,
        keys_due_for_rotation: count("due_for_rotation")?,
    })
}

impl Default for KeyManagerStats {
    fn default() -> Self {
        Self {
            total_keys: 0,
            active_keys: 0,
            retired_keys: 0,
            revoked_keys: 0,
            expired_keys: 0,
            total_access_operations: 0,
            rotations_performed: 0,
            average_key_age_days: 0.0,
            keys_expiring_soon: 0,
            keys_due_for_rotation: 0,
        }
    }
}

// Helper functions for parsing database values
pub fn parse_algorithm(s: &str) -> Result<EncryptionAlgorithm, EncryptionError> {
    match s {
        "AES256GCM" => Ok(EncryptionAlgorithm::AES256GCM),
        "ChaCha20Poly1305" => Ok(EncryptionAlgorithm::ChaCha20Poly1305),
        _ => Err(EncryptionError::KeyManagement(format!(
            "Unknown algorithm: {}",
            s
        ))),
    }
}

/// Parse a key source.
///
/// Accepts the `Display` form (`Environment(VAR)`, `External(aws://...)`, ...) and the bare
/// labels stored in the `key_source` column (`Environment`, `External`, `Generated`,
/// `Derived`, `Static`). The database stores only the label, so a bare label parses to the
/// matching variant with an empty detail string (`Derived` maps to `Generated`).
pub fn parse_key_source(s: &str) -> Result<KeySource, EncryptionError> {
    let detail = |prefix: &str| {
        s.strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(')'))
            .map(str::to_string)
    };

    if let Some(env_var) = detail("Environment(") {
        Ok(KeySource::Environment(env_var))
    } else if let Some(static_key) = detail("Static(") {
        Ok(KeySource::Static(static_key))
    } else if let Some(generated_type) = detail("Generated(") {
        Ok(KeySource::Generated(generated_type))
    } else if let Some(external_id) = detail("External(") {
        Ok(KeySource::External(external_id))
    } else {
        match s {
            "Environment" => Ok(KeySource::Environment(String::new())),
            "Static" => Ok(KeySource::Static(String::new())),
            "Generated" | "Derived" => Ok(KeySource::Generated(String::new())),
            "External" => Ok(KeySource::External(String::new())),
            _ => Err(EncryptionError::KeyManagement(format!(
                "Unknown key source: {}",
                s
            ))),
        }
    }
}

pub fn parse_key_purpose(s: &str) -> Result<KeyPurpose, EncryptionError> {
    match s {
        "Encryption" => Ok(KeyPurpose::Encryption),
        "MAC" => Ok(KeyPurpose::MAC),
        "KEK" => Ok(KeyPurpose::KEK),
        _ => Err(EncryptionError::KeyManagement(format!(
            "Unknown key purpose: {}",
            s
        ))),
    }
}

pub fn parse_key_status(s: &str) -> Result<KeyStatus, EncryptionError> {
    match s {
        "Active" => Ok(KeyStatus::Active),
        "Retired" => Ok(KeyStatus::Retired),
        "Revoked" => Ok(KeyStatus::Revoked),
        "Expired" => Ok(KeyStatus::Expired),
        _ => Err(EncryptionError::KeyManagement(format!(
            "Unknown key status: {}",
            s
        ))),
    }
}

// Add Display implementations for enum serialization
impl std::fmt::Display for KeyPurpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyPurpose::Encryption => write!(f, "Encryption"),
            KeyPurpose::MAC => write!(f, "MAC"),
            KeyPurpose::KEK => write!(f, "KEK"),
        }
    }
}

impl std::fmt::Display for KeyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyStatus::Active => write!(f, "Active"),
            KeyStatus::Retired => write!(f, "Retired"),
            KeyStatus::Revoked => write!(f, "Revoked"),
            KeyStatus::Expired => write!(f, "Expired"),
        }
    }
}

impl std::fmt::Display for EncryptionAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncryptionAlgorithm::AES256GCM => write!(f, "AES256GCM"),
            EncryptionAlgorithm::ChaCha20Poly1305 => write!(f, "ChaCha20Poly1305"),
        }
    }
}

impl std::fmt::Display for KeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySource::Environment(env_var) => write!(f, "Environment({})", env_var),
            KeySource::Static(key) => write!(f, "Static({})", key),
            KeySource::Generated(gen_type) => write!(f, "Generated({})", gen_type),
            KeySource::External(ext_id) => write!(f, "External({})", ext_id),
        }
    }
}

impl std::fmt::Display for KeyOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyOperation::Create => write!(f, "Create"),
            KeyOperation::Access => write!(f, "Access"),
            KeyOperation::Rotate => write!(f, "Rotate"),
            KeyOperation::Retire => write!(f, "Retire"),
            KeyOperation::Revoke => write!(f, "Revoke"),
            KeyOperation::Delete => write!(f, "Delete"),
            KeyOperation::Update => write!(f, "Update"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_key_manager_config_creation() {
        let config = KeyManagerConfig::new()
            .with_master_key_env("TEST_MASTER_KEY")
            .with_auto_rotation_enabled(true)
            .with_rotation_interval(Duration::days(30))
            .with_audit_enabled(true);

        assert_eq!(
            config.master_key_source,
            KeySource::Environment("TEST_MASTER_KEY".to_string())
        );
        assert!(config.auto_rotation_enabled);
        assert_eq!(config.default_rotation_interval, Duration::days(30));
        assert!(config.audit_enabled);
    }

    #[test]
    fn test_key_purpose_serialization() {
        let purpose = KeyPurpose::Encryption;
        let serialized = serde_json::to_string(&purpose).unwrap();
        let deserialized: KeyPurpose = serde_json::from_str(&serialized).unwrap();
        assert_eq!(purpose, deserialized);
    }

    #[test]
    fn test_key_status_transitions() {
        let status = KeyStatus::Active;
        assert_eq!(status, KeyStatus::Active);

        let status = KeyStatus::Retired;
        assert_ne!(status, KeyStatus::Active);
    }

    #[test]
    fn test_external_kms_config() {
        let mut auth_config = HashMap::new();
        auth_config.insert("access_key_id".to_string(), "test_key".to_string());

        let kms_config = ExternalKmsConfig {
            service_type: "AWS".to_string(),
            endpoint: "https://kms.us-east-1.amazonaws.com".to_string(),
            auth_config,
            region: Some("us-east-1".to_string()),
            namespace: Some("hammerwork".to_string()),
        };

        assert_eq!(kms_config.service_type, "AWS");
        assert!(kms_config.auth_config.contains_key("access_key_id"));
    }

    /// Key manager config with an explicit, test-only master key so tests never
    /// depend on `HAMMERWORK_MASTER_KEY` being set in the environment.
    fn test_config() -> KeyManagerConfig {
        KeyManagerConfig::default().with_master_key_source(KeySource::Static(
            base64::engine::general_purpose::STANDARD.encode([0x42u8; 32]),
        ))
    }

    /// Database-backed key manager tests share a single database (and the
    /// `hammerwork_encryption_keys` table), and several assert on table-wide
    /// statistics. Each backend's tests are serialized with a lock and start
    /// from empty key tables.
    #[cfg(all(feature = "encryption", feature = "postgres"))]
    static POSTGRES_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    static MYSQL_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// Connect to `DATABASE_URL`, apply migrations, and clear key tables.
    /// The returned guard must be held for the duration of the test.
    #[cfg(all(feature = "encryption", feature = "postgres"))]
    async fn postgres_test_pool() -> (tokio::sync::MutexGuard<'static, ()>, sqlx::PgPool) {
        use crate::migrations::{MigrationManager, postgres::PostgresMigrationRunner};

        let guard = POSTGRES_DB_LOCK.lock().await;
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must point at a PostgreSQL test database");
        let pool = sqlx::PgPool::connect(&url)
            .await
            .expect("failed to connect to PostgreSQL");
        MigrationManager::new(Box::new(PostgresMigrationRunner::new(pool.clone())))
            .run_migrations()
            .await
            .expect("failed to run PostgreSQL migrations");
        for table in ["hammerwork_key_audit_log", "hammerwork_encryption_keys"] {
            sqlx::query(&format!("DELETE FROM {table}"))
                .execute(&pool)
                .await
                .expect("failed to clear key tables");
        }
        (guard, pool)
    }

    /// Connect to `MYSQL_DATABASE_URL`, apply migrations, and clear key tables.
    /// The returned guard must be held for the duration of the test.
    #[cfg(all(feature = "encryption", feature = "mysql"))]
    async fn mysql_test_pool() -> (tokio::sync::MutexGuard<'static, ()>, sqlx::MySqlPool) {
        use crate::migrations::{MigrationManager, mysql::MySqlMigrationRunner};

        let guard = MYSQL_DB_LOCK.lock().await;
        let url = std::env::var("MYSQL_DATABASE_URL")
            .expect("MYSQL_DATABASE_URL must point at a MySQL test database");
        let pool = sqlx::MySqlPool::connect(&url)
            .await
            .expect("failed to connect to MySQL");
        MigrationManager::new(Box::new(MySqlMigrationRunner::new(pool.clone())))
            .run_migrations()
            .await
            .expect("failed to run MySQL migrations");
        for table in ["hammerwork_key_audit_log", "hammerwork_encryption_keys"] {
            sqlx::query(&format!("DELETE FROM {table}"))
                .execute(&pool)
                .await
                .expect("failed to clear key tables");
        }
        (guard, pool)
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_master_key_storage_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config();
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Test generating a master key (which handles storage internally)
        let master_key_id = key_manager.generate_master_key().await.unwrap();

        // Test finding the stored key ID
        let found_id = key_manager.find_master_key_id_in_database().await.unwrap();
        assert!(found_id.is_some(), "Should find the stored master key ID");
        assert_eq!(
            found_id.unwrap(),
            master_key_id,
            "Found ID should match stored ID"
        );
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_master_key_storage_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config();
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Test generating a master key (which handles storage internally)
        let master_key_id = key_manager.generate_master_key().await.unwrap();

        // Test finding the stored key ID
        let found_id = key_manager.find_master_key_id_in_database().await.unwrap();
        assert!(found_id.is_some(), "Should find the stored master key ID");
        assert_eq!(
            found_id.unwrap(),
            master_key_id,
            "Found ID should match stored ID"
        );
    }

    #[cfg(feature = "encryption")]
    #[test]
    fn test_system_key_derivation() {
        let config = test_config();
        // Create a minimal struct just for testing the derivation logic
        let key_manager = TestKeyManager { config };

        let salt = [1u8; 32];
        let result = key_manager.derive_system_encryption_key(&salt);

        assert!(result.is_ok());
        let derived_key = result.unwrap();
        assert_eq!(derived_key.len(), 32); // Should be 32 bytes for AES-256

        // Test deterministic nature - same salt should produce same key
        let result2 = key_manager.derive_system_encryption_key(&salt);
        assert!(result2.is_ok());
        assert_eq!(derived_key, result2.unwrap());
    }

    #[cfg(feature = "encryption")]
    #[test]
    fn test_system_key_encryption_decryption() {
        let config = test_config();
        let key_manager = TestKeyManager { config };

        let system_key = [0u8; 32]; // Test key
        let plaintext = b"test master key material";

        let result = key_manager.encrypt_with_system_key(&system_key, plaintext);
        assert!(result.is_ok());

        let encrypted = result.unwrap();
        assert!(encrypted.len() > plaintext.len()); // Should be larger due to nonce + auth tag
        assert_ne!(&encrypted[12..], plaintext); // Encrypted data should be different
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_get_or_create_master_key_id_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        let key_material = b"test_key_material_for_id_generation";

        // First call should create a new ID
        let id1 = key_manager
            .get_or_create_master_key_id(key_material)
            .await
            .unwrap();

        // Second call with same material should return the same ID
        let id2 = key_manager
            .get_or_create_master_key_id(key_material)
            .await
            .unwrap();
        assert_eq!(id1, id2, "Should return the same ID for same key material");

        // Different material should produce different ID
        let different_material = b"different_test_key_material_here";
        let id3 = key_manager
            .get_or_create_master_key_id(different_material)
            .await
            .unwrap();
        assert_ne!(
            id1, id3,
            "Different key material should produce different ID"
        );
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_get_or_create_master_key_id_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        let key_material = b"test_key_material_for_id_generation";

        // First call should create a new ID
        let id1 = key_manager
            .get_or_create_master_key_id(key_material)
            .await
            .unwrap();

        // Second call with same material should return the same ID
        let id2 = key_manager
            .get_or_create_master_key_id(key_material)
            .await
            .unwrap();
        assert_eq!(id1, id2, "Should return the same ID for same key material");

        // Different material should produce different ID
        let different_material = b"different_test_key_material_here";
        let id3 = key_manager
            .get_or_create_master_key_id(different_material)
            .await
            .unwrap();
        assert_ne!(
            id1, id3,
            "Different key material should produce different ID"
        );
    }

    #[cfg(feature = "encryption")]
    #[test]
    fn test_master_key_id_generation() {
        let key_material = b"test key material for ID generation";

        // Test deterministic ID generation
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(key_material);
        hasher.update(b"hammerwork-master-key-v1");
        let hash = hasher.finalize();

        let expected_id = Uuid::from_bytes([
            hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7], hash[8],
            hash[9], hash[10], hash[11], hash[12], hash[13], hash[14], hash[15],
        ]);

        // Same material should produce same ID
        let mut hasher2 = Sha256::new();
        hasher2.update(key_material);
        hasher2.update(b"hammerwork-master-key-v1");
        let hash2 = hasher2.finalize();

        let id2 = Uuid::from_bytes([
            hash2[0], hash2[1], hash2[2], hash2[3], hash2[4], hash2[5], hash2[6], hash2[7],
            hash2[8], hash2[9], hash2[10], hash2[11], hash2[12], hash2[13], hash2[14], hash2[15],
        ]);

        assert_eq!(expected_id, id2);
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_key_rotation_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config().with_auto_rotation_enabled(true);
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Generate a test key with rotation schedule
        let key_id = key_manager
            .generate_key_with_options(
                "rotation-test-key",
                EncryptionAlgorithm::AES256GCM,
                KeyPurpose::Encryption,
                None,                     // expires_at
                Some(Duration::days(30)), // 30-day rotation interval
            )
            .await
            .unwrap();

        // Verify initial key version
        let initial_key = key_manager.load_key(&key_id).await.unwrap();
        assert_eq!(initial_key.version, 1);
        assert!(initial_key.next_rotation_at.is_some());

        // Perform manual rotation
        let new_version = key_manager.rotate_key(&key_id).await.unwrap();
        assert_eq!(new_version, 2);

        // Verify rotation worked
        let rotated_key = key_manager.load_key(&key_id).await.unwrap();
        assert_eq!(rotated_key.version, 2);
        assert_eq!(rotated_key.status, KeyStatus::Active);
        assert!(rotated_key.rotated_at.is_some());
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_key_rotation_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config().with_auto_rotation_enabled(true);
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Generate a test key with rotation schedule
        let key_id = key_manager
            .generate_key_with_options(
                "rotation-test-key",
                EncryptionAlgorithm::AES256GCM,
                KeyPurpose::Encryption,
                None,                     // expires_at
                Some(Duration::days(30)), // 30-day rotation interval
            )
            .await
            .unwrap();

        // Verify initial key version
        let initial_key = key_manager.load_key(&key_id).await.unwrap();
        assert_eq!(initial_key.version, 1);
        assert!(initial_key.next_rotation_at.is_some());

        // Perform manual rotation
        let new_version = key_manager.rotate_key(&key_id).await.unwrap();
        assert_eq!(new_version, 2);

        // Verify rotation worked
        let rotated_key = key_manager.load_key(&key_id).await.unwrap();
        assert_eq!(rotated_key.version, 2);
        assert_eq!(rotated_key.status, KeyStatus::Active);
        assert!(rotated_key.rotated_at.is_some());
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_rotation_scheduling_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create a test key first
        let test_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "schedule-test-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now(),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(90)),
            next_rotation_at: None, // Initially no rotation scheduled
            key_strength: 256,
            master_key_id: None,
            last_used_at: None,
            usage_count: 0,
        };

        key_manager.store_key(&test_key).await.unwrap();

        // Schedule rotation for 1 hour from now
        let rotation_time = Utc::now() + Duration::hours(1);
        key_manager
            .schedule_key_rotation("schedule-test-key", rotation_time)
            .await
            .unwrap();

        // Verify the schedule was set
        let schedule = key_manager
            .get_key_rotation_schedule("schedule-test-key")
            .await
            .unwrap();
        assert!(schedule.is_some());
        let scheduled_time = schedule.unwrap();

        // Allow for small time differences due to test execution time
        let time_diff = (scheduled_time - rotation_time).num_seconds().abs();
        assert!(
            time_diff < 5,
            "Scheduled time should be close to requested time"
        );

        // Test querying scheduled rotations
        let from_time = Utc::now();
        let to_time = Utc::now() + Duration::hours(2);
        let scheduled_rotations = key_manager
            .get_scheduled_rotations(from_time, to_time)
            .await
            .unwrap();

        assert_eq!(scheduled_rotations.len(), 1);
        assert_eq!(scheduled_rotations[0].0, "schedule-test-key");
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_rotation_scheduling_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create a test key first
        let test_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "schedule-test-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now(),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(90)),
            next_rotation_at: None, // Initially no rotation scheduled
            key_strength: 256,
            master_key_id: None,
            last_used_at: None,
            usage_count: 0,
        };

        key_manager.store_key(&test_key).await.unwrap();

        // Schedule rotation for 1 hour from now
        let rotation_time = Utc::now() + Duration::hours(1);
        key_manager
            .schedule_key_rotation("schedule-test-key", rotation_time)
            .await
            .unwrap();

        // Verify the schedule was set
        let schedule = key_manager
            .get_key_rotation_schedule("schedule-test-key")
            .await
            .unwrap();
        assert!(schedule.is_some());
        let scheduled_time = schedule.unwrap();

        // Allow for small time differences due to test execution time
        let time_diff = (scheduled_time - rotation_time).num_seconds().abs();
        assert!(
            time_diff < 5,
            "Scheduled time should be close to requested time"
        );

        // Test querying scheduled rotations
        let from_time = Utc::now();
        let to_time = Utc::now() + Duration::hours(2);
        let scheduled_rotations = key_manager
            .get_scheduled_rotations(from_time, to_time)
            .await
            .unwrap();

        assert_eq!(scheduled_rotations.len(), 1);
        assert_eq!(scheduled_rotations[0].0, "schedule-test-key");
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_automatic_rotation_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config().with_auto_rotation_enabled(true);
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create a key that is due for rotation (next_rotation_at in the past)
        let test_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "auto-rotation-test-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(90),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(30)),
            next_rotation_at: Some(Utc::now() - Duration::hours(1)), // Due for rotation
            key_strength: 256,
            master_key_id: None,
            last_used_at: None,
            usage_count: 0,
        };

        key_manager.store_key(&test_key).await.unwrap();

        // Verify key is due for rotation
        let is_due = key_manager
            .is_key_due_for_rotation("auto-rotation-test-key")
            .await
            .unwrap();
        assert!(is_due, "Key should be due for rotation");

        // Perform automatic rotation
        let rotated_keys = key_manager.perform_automatic_rotation().await.unwrap();
        assert_eq!(rotated_keys.len(), 1);
        assert_eq!(rotated_keys[0], "auto-rotation-test-key");

        // Verify the key was rotated
        let rotated_key = key_manager
            .load_key("auto-rotation-test-key")
            .await
            .unwrap();
        assert_eq!(rotated_key.version, 2);
        assert!(rotated_key.rotated_at.is_some());
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_automatic_rotation_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config().with_auto_rotation_enabled(true);
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create a key that is due for rotation (next_rotation_at in the past)
        let test_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "auto-rotation-test-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(90),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(30)),
            next_rotation_at: Some(Utc::now() - Duration::hours(1)), // Due for rotation
            key_strength: 256,
            master_key_id: None,
            last_used_at: None,
            usage_count: 0,
        };

        key_manager.store_key(&test_key).await.unwrap();

        // Verify key is due for rotation
        let is_due = key_manager
            .is_key_due_for_rotation("auto-rotation-test-key")
            .await
            .unwrap();
        assert!(is_due, "Key should be due for rotation");

        // Perform automatic rotation
        let rotated_keys = key_manager.perform_automatic_rotation().await.unwrap();
        assert_eq!(rotated_keys.len(), 1);
        assert_eq!(rotated_keys[0], "auto-rotation-test-key");

        // Verify the key was rotated
        let rotated_key = key_manager
            .load_key("auto-rotation-test-key")
            .await
            .unwrap();
        assert_eq!(rotated_key.version, 2);
        assert!(rotated_key.rotated_at.is_some());
    }

    // ---- Fail-closed master key loading (#16); none of these need cloud credentials ----

    #[tokio::test]
    async fn test_unknown_external_master_key_source_fails() {
        let err = load_master_key_material(&KeySource::External("ftp://key".to_string()))
            .await
            .unwrap_err();
        assert!(matches!(err, EncryptionError::KeyManagement(_)), "{err}");
    }

    #[tokio::test]
    async fn test_missing_or_invalid_local_master_key_fails() {
        let err = load_master_key_material(&KeySource::Environment(
            "HAMMERWORK_TEST_UNSET_MASTER_KEY_VAR".to_string(),
        ))
        .await
        .unwrap_err();
        assert!(matches!(err, EncryptionError::KeyManagement(_)), "{err}");

        let err = load_master_key_material(&KeySource::Static("not base64!".to_string()))
            .await
            .unwrap_err();
        assert!(matches!(err, EncryptionError::KeyManagement(_)), "{err}");
    }

    #[tokio::test]
    async fn test_kms_master_key_source_is_not_loaded_statelessly() {
        // aws:// and gcp:// master keys are only loaded by KeyManager (envelope encryption)
        for source in [
            "aws://alias/key?region=us-east-1",
            "gcp://projects/p/locations/global/keyRings/r/cryptoKeys/k",
        ] {
            let err = load_master_key_material(&KeySource::External(source.to_string()))
                .await
                .unwrap_err();
            assert!(matches!(err, EncryptionError::KeyManagement(_)), "{err}");
        }
    }

    #[tokio::test]
    async fn test_vault_master_key_invalid_config_fails() {
        // Secret path without a mount
        let err = load_master_key_from_vault("vault://key-only?addr=http://127.0.0.1:1")
            .await
            .unwrap_err();
        assert!(
            matches!(err, EncryptionError::InvalidConfiguration(_)),
            "{err}"
        );
    }

    #[cfg(not(feature = "vault-kms"))]
    #[tokio::test]
    async fn test_vault_master_key_without_feature_fails() {
        let err = load_master_key_from_vault("vault://secret/hammerwork?addr=http://127.0.0.1:1")
            .await
            .unwrap_err();
        assert!(
            matches!(err, EncryptionError::InvalidConfiguration(_)),
            "{err}"
        );
        assert!(err.to_string().contains("vault-kms"), "{err}");
    }

    #[cfg(feature = "vault-kms")]
    #[tokio::test]
    async fn test_vault_master_key_unavailable_fails() {
        // Without VAULT_TOKEN the loader fails before connecting; with one, the
        // unreachable address fails the read.
        let err = load_master_key_from_vault("vault://secret/hammerwork?addr=http://127.0.0.1:1")
            .await
            .unwrap_err();
        assert!(matches!(err, EncryptionError::KeyManagement(_)), "{err}");
    }

    #[tokio::test]
    async fn test_azure_master_key_invalid_config_fails() {
        let err = load_master_key_from_azure("azure://").await.unwrap_err();
        assert!(
            matches!(err, EncryptionError::InvalidConfiguration(_)),
            "{err}"
        );
    }

    #[cfg(not(feature = "azure-kv"))]
    #[tokio::test]
    async fn test_azure_master_key_without_feature_fails() {
        let err = load_master_key_from_azure("azure://my-vault.vault.azure.net/keys/master-key")
            .await
            .unwrap_err();
        assert!(
            matches!(err, EncryptionError::InvalidConfiguration(_)),
            "{err}"
        );
        assert!(err.to_string().contains("azure-kv"), "{err}");
    }

    /// `KeyManager::new` must fail when the master key cannot be loaded, before it
    /// touches the database (the lazy pool here never connects).
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_key_manager_new_fails_closed() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://hammerwork:unused@127.0.0.1:1/unused")
            .unwrap();
        for source in [
            KeySource::External("vault://key-only?addr=http://127.0.0.1:1".to_string()),
            KeySource::External("gcp://not-a-resource".to_string()),
            KeySource::External("aws://?region=us-east-1".to_string()),
            KeySource::External("azure://".to_string()),
            KeySource::External("unknown://key".to_string()),
            KeySource::Environment("HAMMERWORK_TEST_UNSET_MASTER_KEY_VAR".to_string()),
        ] {
            let config = KeyManagerConfig::new().with_master_key_source(source.clone());
            let result = KeyManager::new(config, pool.clone()).await;
            assert!(result.is_err(), "KeyManager::new succeeded with {source:?}");
        }
    }

    #[test]
    fn test_key_source_label_never_contains_key() {
        let label = key_source_label(&KeySource::Static("c2VjcmV0".to_string()));
        assert_eq!(label, "Static");
        for source in [
            KeySource::Environment("VAR".to_string()),
            KeySource::External("aws://k".to_string()),
            KeySource::Generated("x".to_string()),
        ] {
            let label = key_source_label(&source);
            assert!(!label.contains('('));
            assert!(parse_key_source(label).is_ok());
        }
    }

    #[test]
    fn test_wrap_unwrap_key_material() {
        let wrapping_key = [7u8; 32];
        let material = [9u8; 32];
        let wrapped = wrap_key_material(&wrapping_key, &material).unwrap();
        assert_eq!(wrapped.len(), 12 + 32 + 16);
        assert!(!wrapped.windows(32).any(|w| w == material));
        assert_eq!(
            unwrap_key_material(&wrapping_key, &wrapped).unwrap(),
            material
        );
        assert!(unwrap_key_material(&[8u8; 32], &wrapped).is_err());
    }

    /// Keys are stored encrypted, survive a restart (new `KeyManager`), keep old versions
    /// after rotation, and stay readable after a new master key is generated.
    async fn persistence_roundtrip<DB: KeyManagerBackend>(pool: Pool<DB>)
    where
        for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
        for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
        for<'r> Vec<u8>: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
        for<'n> &'n str: sqlx::ColumnIndex<DB::Row>,
    {
        let mut manager = KeyManager::new(test_config(), pool.clone()).await.unwrap();
        manager
            .generate_key("roundtrip-key", EncryptionAlgorithm::AES256GCM)
            .await
            .unwrap();
        let v1 = manager.get_key("roundtrip-key").await.unwrap();
        assert_eq!(v1.len(), 32);

        // Generating the same key ID again is an error, not an overwrite
        assert!(
            manager
                .generate_key("roundtrip-key", EncryptionAlgorithm::AES256GCM)
                .await
                .is_err()
        );

        // Stored material is encrypted and the source column holds only a label
        let row = sqlx::query(
            "SELECT key_material, key_source FROM hammerwork_encryption_keys \
             WHERE key_id = 'roundtrip-key'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let stored: Vec<u8> = row.get("key_material");
        let source: String = row.get("key_source");
        assert_eq!(source, "Generated");
        assert_eq!(stored.len(), 12 + 32 + 16);
        assert!(!stored.windows(32).any(|w| w == v1.as_slice()));

        // Rotation keeps the old version for decryption
        assert_eq!(manager.rotate_key("roundtrip-key").await.unwrap(), 2);
        let v2 = manager.get_key("roundtrip-key").await.unwrap();
        assert_ne!(v1, v2);
        assert_eq!(
            manager.get_key_version("roundtrip-key", 1).await.unwrap(),
            v1
        );
        let old = manager.load_key("roundtrip-key").await.unwrap();
        assert_eq!(old.version, 2);

        // A new master key wraps new keys; keys wrapped by the old one stay readable
        manager.generate_master_key().await.unwrap();
        manager
            .generate_key("kek-wrapped-key", EncryptionAlgorithm::ChaCha20Poly1305)
            .await
            .unwrap();
        let wrapped_by_kek = manager.get_key("kek-wrapped-key").await.unwrap();

        // "Restart": a fresh manager with the same configured master key
        let mut restarted = KeyManager::new(test_config(), pool.clone()).await.unwrap();
        assert_eq!(restarted.get_key("roundtrip-key").await.unwrap(), v2);
        assert_eq!(
            restarted.get_key_version("roundtrip-key", 1).await.unwrap(),
            v1
        );
        assert_eq!(
            restarted.get_key("kek-wrapped-key").await.unwrap(),
            wrapped_by_kek
        );

        // A different configured master key cannot unlock the stored key-encryption key
        let wrong = KeyManagerConfig::default().with_master_key_source(KeySource::Static(
            base64::engine::general_purpose::STANDARD.encode([0x24u8; 32]),
        ));
        assert!(KeyManager::new(wrong, pool.clone()).await.is_err());

        // Audit records were written for create, access and rotate
        let audit_rows = sqlx::query(
            "SELECT operation FROM hammerwork_key_audit_log WHERE key_id = 'roundtrip-key'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let operations: Vec<String> = audit_rows.iter().map(|r| r.get("operation")).collect();
        for op in ["Create", "Access", "Rotate"] {
            assert!(
                operations.iter().any(|o| o == op),
                "missing {op} audit record"
            );
        }
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_persistence_roundtrip_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        persistence_roundtrip(pool).await;
    }

    #[cfg(feature = "mysql")]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_persistence_roundtrip_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        persistence_roundtrip(pool).await;
    }

    async fn version_cleanup<DB: KeyManagerBackend>(pool: Pool<DB>) {
        let config = test_config().with_max_key_versions(2);
        let mut manager = KeyManager::new(config, pool).await.unwrap();
        manager
            .generate_key("cleanup-key", EncryptionAlgorithm::AES256GCM)
            .await
            .unwrap();
        for expected in 2..=4 {
            assert_eq!(manager.rotate_key("cleanup-key").await.unwrap(), expected);
        }
        assert!(manager.get_key_version("cleanup-key", 4).await.is_ok());
        assert!(manager.get_key_version("cleanup-key", 3).await.is_ok());
        assert!(manager.get_key_version("cleanup-key", 2).await.is_err());
        assert!(manager.get_key_version("cleanup-key", 1).await.is_err());
        let stats = manager.query_database_statistics().await.unwrap();
        assert_eq!(stats.total_keys, 2);
        assert_eq!(stats.active_keys, 1);
        assert_eq!(stats.retired_keys, 1);
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_version_cleanup_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        version_cleanup(pool).await;
    }

    #[cfg(feature = "mysql")]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_version_cleanup_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        version_cleanup(pool).await;
    }

    // Test-only struct for unit testing crypto functions without database
    #[cfg(feature = "encryption")]
    struct TestKeyManager {
        config: KeyManagerConfig,
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_database_statistics_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create test keys with different statuses
        let active_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "test-active-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(10),
            created_by: Some("test".to_string()),
            expires_at: Some(Utc::now() + Duration::days(30)),
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(90)),
            next_rotation_at: Some(Utc::now() - Duration::hours(1)), // Due for rotation
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now() - Duration::hours(1)),
            usage_count: 5,
        };

        let retired_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "test-retired-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(20),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: Some(Utc::now() - Duration::days(5)),
            retired_at: Some(Utc::now() - Duration::days(5)),
            status: KeyStatus::Retired,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now() - Duration::days(6)),
            usage_count: 10,
        };

        let expiring_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "test-expiring-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(5),
            created_by: Some("test".to_string()),
            expires_at: Some(Utc::now() + Duration::days(3)), // Expires soon
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now() - Duration::hours(2)),
            usage_count: 2,
        };

        // Store test keys
        key_manager.store_key(&active_key).await.unwrap();
        key_manager.store_key(&retired_key).await.unwrap();
        key_manager.store_key(&expiring_key).await.unwrap();

        // Query and verify statistics
        let stats = key_manager.query_database_statistics().await.unwrap();

        assert_eq!(stats.total_keys, 3, "Should have 3 total keys");
        assert_eq!(stats.active_keys, 2, "Should have 2 active keys");
        assert_eq!(stats.retired_keys, 1, "Should have 1 retired key");
        assert_eq!(stats.revoked_keys, 0, "Should have 0 revoked keys");
        assert_eq!(stats.expired_keys, 0, "Should have 0 expired keys");

        // Average age should be between 5 and 20 days (approximate check)
        assert!(
            stats.average_key_age_days > 5.0 && stats.average_key_age_days < 20.0,
            "Average key age should be reasonable: {}",
            stats.average_key_age_days
        );

        assert_eq!(
            stats.keys_expiring_soon, 1,
            "Should have 1 key expiring soon"
        );
        assert_eq!(
            stats.keys_due_for_rotation, 1,
            "Should have 1 key due for rotation"
        );
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_database_statistics_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create test keys with different statuses (same as PostgreSQL test)
        let active_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "test-active-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(10),
            created_by: Some("test".to_string()),
            expires_at: Some(Utc::now() + Duration::days(30)),
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(90)),
            next_rotation_at: Some(Utc::now() - Duration::hours(1)), // Due for rotation
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now() - Duration::hours(1)),
            usage_count: 5,
        };

        let retired_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "test-retired-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(20),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: Some(Utc::now() - Duration::days(5)),
            retired_at: Some(Utc::now() - Duration::days(5)),
            status: KeyStatus::Retired,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now() - Duration::days(6)),
            usage_count: 10,
        };

        let expiring_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "test-expiring-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(5),
            created_by: Some("test".to_string()),
            expires_at: Some(Utc::now() + Duration::days(3)), // Expires soon
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now() - Duration::hours(2)),
            usage_count: 2,
        };

        // Store test keys
        key_manager.store_key(&active_key).await.unwrap();
        key_manager.store_key(&retired_key).await.unwrap();
        key_manager.store_key(&expiring_key).await.unwrap();

        // Query and verify statistics
        let stats = key_manager.query_database_statistics().await.unwrap();

        assert_eq!(stats.total_keys, 3, "Should have 3 total keys");
        assert_eq!(stats.active_keys, 2, "Should have 2 active keys");
        assert_eq!(stats.retired_keys, 1, "Should have 1 retired key");
        assert_eq!(stats.revoked_keys, 0, "Should have 0 revoked keys");
        assert_eq!(stats.expired_keys, 0, "Should have 0 expired keys");

        // Average age should be between 5 and 20 days (approximate check)
        assert!(
            stats.average_key_age_days > 5.0 && stats.average_key_age_days < 20.0,
            "Average key age should be reasonable: {}",
            stats.average_key_age_days
        );

        assert_eq!(
            stats.keys_expiring_soon, 1,
            "Should have 1 key expiring soon"
        );
        assert_eq!(
            stats.keys_due_for_rotation, 1,
            "Should have 1 key due for rotation"
        );
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_refresh_stats_integration_postgres() {
        let (_db_guard, pool) = postgres_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Initially stats should be mostly zeros
        let initial_stats = key_manager.get_stats().await;
        assert_eq!(initial_stats.total_keys, 0);

        // Add a test key
        let test_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "refresh-test-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(7),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now()),
            usage_count: 1,
        };

        key_manager.store_key(&test_key).await.unwrap();

        // Refresh stats and verify they're updated
        key_manager.refresh_stats().await.unwrap();
        let updated_stats = key_manager.get_stats().await;

        assert_eq!(
            updated_stats.total_keys, 1,
            "Stats should reflect the added key"
        );
        assert_eq!(updated_stats.active_keys, 1, "Should have 1 active key");
        assert!(
            updated_stats.average_key_age_days > 6.0 && updated_stats.average_key_age_days < 8.0,
            "Average age should be around 7 days: {}",
            updated_stats.average_key_age_days
        );
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_refresh_stats_integration_mysql() {
        let (_db_guard, pool) = mysql_test_pool().await;
        let config = test_config();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Initially stats should be mostly zeros
        let initial_stats = key_manager.get_stats().await;
        assert_eq!(initial_stats.total_keys, 0);

        // Add a test key
        let test_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "refresh-test-key".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4],
            derivation_salt: Some(vec![5, 6, 7, 8]),
            source: KeySource::Generated("test_key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now() - Duration::days(7),
            created_by: Some("test".to_string()),
            expires_at: None,
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: None,
            next_rotation_at: None,
            key_strength: 256,
            master_key_id: None,
            last_used_at: Some(Utc::now()),
            usage_count: 1,
        };

        key_manager.store_key(&test_key).await.unwrap();

        // Refresh stats and verify they're updated
        key_manager.refresh_stats().await.unwrap();
        let updated_stats = key_manager.get_stats().await;

        assert_eq!(
            updated_stats.total_keys, 1,
            "Stats should reflect the added key"
        );
        assert_eq!(updated_stats.active_keys, 1, "Should have 1 active key");
        assert!(
            updated_stats.average_key_age_days > 6.0 && updated_stats.average_key_age_days < 8.0,
            "Average age should be around 7 days: {}",
            updated_stats.average_key_age_days
        );
    }

    #[cfg(feature = "encryption")]
    impl TestKeyManager {
        #[allow(dead_code)]
        fn derive_system_encryption_key(&self, salt: &[u8]) -> Result<Vec<u8>, EncryptionError> {
            use argon2::{
                Argon2,
                password_hash::{PasswordHasher, SaltString},
            };

            // Use a combination of system properties and configuration for key derivation
            let mut input = Vec::new();
            input.extend_from_slice(b"hammerwork-system-key-v1");

            // Add configuration-based entropy
            if let Some(ref external_config) = self.config.external_kms_config {
                input.extend_from_slice(external_config.service_type.as_bytes());
                input.extend_from_slice(external_config.endpoint.as_bytes());
                if let Some(ref region) = external_config.region {
                    input.extend_from_slice(region.as_bytes());
                }
            }

            // Add system-specific entropy (hostname, etc.)
            if let Ok(hostname) = std::env::var("HOSTNAME") {
                input.extend_from_slice(hostname.as_bytes());
            }

            // Use Argon2 for secure key derivation
            let argon2 = Argon2::default();
            let salt_string = SaltString::encode_b64(salt).map_err(|e| {
                EncryptionError::KeyManagement(format!("Failed to encode salt: {}", e))
            })?;

            let password_hash = argon2.hash_password(&input, &salt_string).map_err(|e| {
                EncryptionError::KeyManagement(format!("Key derivation failed: {}", e))
            })?;

            // Extract the raw hash bytes
            let hash = password_hash.hash.ok_or_else(|| {
                EncryptionError::KeyManagement("No hash in password result".to_string())
            })?;
            let hash_bytes = hash.as_bytes();

            // Return first 32 bytes for AES-256
            Ok(hash_bytes[0..32].to_vec())
        }

        #[allow(dead_code)]
        fn encrypt_with_system_key(
            &self,
            system_key: &[u8],
            plaintext: &[u8],
        ) -> Result<Vec<u8>, EncryptionError> {
            use aes_gcm::{
                Aes256Gcm, Nonce,
                aead::{Aead, KeyInit},
            };

            let cipher = Aes256Gcm::new_from_slice(system_key).map_err(|e| {
                EncryptionError::KeyManagement(format!("Failed to create cipher: {}", e))
            })?;

            // Generate random nonce
            let mut nonce_bytes = [0u8; 12];
            use rand::RngCore;
            rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
            let nonce = Nonce::from_slice(&nonce_bytes);

            // Encrypt the data
            let ciphertext = cipher
                .encrypt(nonce, plaintext)
                .map_err(|e| EncryptionError::KeyManagement(format!("Encryption failed: {}", e)))?;

            // Prepend nonce to ciphertext for storage
            let mut result = nonce_bytes.to_vec();
            result.extend_from_slice(&ciphertext);

            Ok(result)
        }
    }

    // ---- KMS-wrapped master keys (#26), with a local mock KMS ----

    use crate::encryption::envelope::test_support::MockKms;

    /// A new client for the same mock KMS key, as another process would have.
    fn same_kms(kms: &MockKms) -> Arc<dyn KmsKeyWrapper> {
        Arc::new(MockKms::new(kms.kms_key_id(), [0x5a; 32]))
    }

    /// The KMS-wrapped master key survives restarts, and keys encrypted under every
    /// rotated version (directly, or through a key-encryption key) stay readable.
    async fn kms_master_key_roundtrip<DB: KeyManagerBackend>(pool: Pool<DB>) {
        let kms = MockKms::unique();

        // rotate_kms_master_key needs a KMS source
        let mut static_manager = KeyManager::new(test_config(), pool.clone()).await.unwrap();
        assert!(matches!(
            static_manager.rotate_kms_master_key().await,
            Err(EncryptionError::InvalidConfiguration(_))
        ));

        let mut first = KeyManager::with_kms(test_config(), pool.clone(), Some(same_kms(&kms)))
            .await
            .unwrap();
        let root_v1 = first.get_master_key_id().await.unwrap();
        first
            .generate_key("kms-a", EncryptionAlgorithm::AES256GCM)
            .await
            .unwrap();
        let a = first.get_key("kms-a").await.unwrap();

        // Restart: the same master key comes back from the stored KMS-wrapped blob
        let mut second = KeyManager::with_kms(test_config(), pool.clone(), Some(same_kms(&kms)))
            .await
            .unwrap();
        assert_eq!(second.get_master_key_id().await, Some(root_v1));
        assert_eq!(second.get_key("kms-a").await.unwrap(), a);
        let stored = DB::load_kms_wrapped_keys(&pool, KMS_MASTER_KEY_NAME, "aws", kms.kms_key_id())
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert!(!stored[0].wrapped_key.windows(32).any(|w| w == a.as_slice()));

        // Rotate the KMS-wrapped master key: new keys use version 2
        assert_eq!(second.rotate_kms_master_key().await.unwrap(), 2);
        let root_v2 = second.get_master_key_id().await.unwrap();
        assert_ne!(root_v1, root_v2);
        second
            .generate_key("kms-b", EncryptionAlgorithm::AES256GCM)
            .await
            .unwrap();
        let b = second.get_key("kms-b").await.unwrap();

        // A fresh instance loads version 2 and still decrypts keys wrapped by version 1
        let mut third = KeyManager::with_kms(test_config(), pool.clone(), Some(same_kms(&kms)))
            .await
            .unwrap();
        assert_eq!(third.get_master_key_id().await, Some(root_v2));
        assert_eq!(third.get_key("kms-a").await.unwrap(), a);
        assert_eq!(third.get_key("kms-b").await.unwrap(), b);

        // A key-encryption key under version 2, then another rotation
        third.generate_master_key().await.unwrap();
        third
            .generate_key("kms-c", EncryptionAlgorithm::AES256GCM)
            .await
            .unwrap();
        let c = third.get_key("kms-c").await.unwrap();
        assert_eq!(third.rotate_kms_master_key().await.unwrap(), 3);

        // The active key-encryption key is wrapped by retired version 2
        let mut fourth = KeyManager::with_kms(test_config(), pool.clone(), Some(same_kms(&kms)))
            .await
            .unwrap();
        assert_eq!(fourth.get_key("kms-a").await.unwrap(), a);
        assert_eq!(fourth.get_key("kms-b").await.unwrap(), b);
        assert_eq!(fourth.get_key("kms-c").await.unwrap(), c);

        let stored = DB::load_kms_wrapped_keys(&pool, KMS_MASTER_KEY_NAME, "aws", kms.kms_key_id())
            .await
            .unwrap();
        let versions: Vec<_> = stored.iter().map(|k| (k.version, k.active)).collect();
        assert_eq!(versions, vec![(3, true), (2, false), (1, false)]);
        assert!(stored[1].retired_at.is_some());

        // A different KMS key cannot be used to read the stored master key
        let wrong: Arc<dyn KmsKeyWrapper> = Arc::new(MockKms::new(kms.kms_key_id(), [0x11; 32]));
        assert!(
            KeyManager::with_kms(test_config(), pool.clone(), Some(wrong))
                .await
                .is_err()
        );
    }

    /// Key managers starting at the same time converge on one stored master key.
    async fn kms_concurrent_first_load<DB: KeyManagerBackend>(pool: Pool<DB>) {
        let kms = MockKms::unique();
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let pool = pool.clone();
                let kms = same_kms(&kms);
                tokio::spawn(async move {
                    KeyManager::with_kms(test_config(), pool, Some(kms))
                        .await
                        .unwrap()
                        .get_master_key_id()
                        .await
                        .unwrap()
                })
            })
            .collect();
        let mut ids = Vec::new();
        for task in tasks {
            ids.push(task.await.unwrap());
        }
        assert!(ids.windows(2).all(|w| w[0] == w[1]), "{ids:?}");
        let stored = DB::load_kms_wrapped_keys(&pool, KMS_MASTER_KEY_NAME, "aws", kms.kms_key_id())
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_kms_master_key_roundtrip_postgres() {
        let (_guard, pool) = postgres_test_pool().await;
        kms_master_key_roundtrip(pool).await;
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_kms_master_key_roundtrip_mysql() {
        let (_guard, pool) = mysql_test_pool().await;
        kms_master_key_roundtrip(pool).await;
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[tokio::test]
    #[ignore = "requires PostgreSQL: DATABASE_URL"]
    async fn test_kms_concurrent_first_load_postgres() {
        let (_guard, pool) = postgres_test_pool().await;
        kms_concurrent_first_load(pool).await;
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[tokio::test]
    #[ignore = "requires MySQL: MYSQL_DATABASE_URL"]
    async fn test_kms_concurrent_first_load_mysql() {
        let (_guard, pool) = mysql_test_pool().await;
        kms_concurrent_first_load(pool).await;
    }
}
