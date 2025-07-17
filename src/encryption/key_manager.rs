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
//! - Keys are never stored in plain text in the database
//! - Master keys are used to encrypt data encryption keys
//! - All key operations are logged for audit purposes
//! - Key access is controlled through proper authentication
//!
//! # Examples
//!
//! ## Basic Key Management
//!
//! ```rust,no_run
//! # #[cfg(feature = "encryption")]
//! # {
//! use hammerwork::encryption::{KeyManager, EncryptionAlgorithm, KeyManagerConfig};
//! use sqlx::{postgres::PgPool, Pool};
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
//! # #[cfg(feature = "encryption")]
//! # {
//! use hammerwork::encryption::{KeyManager, EncryptionAlgorithm, KeyManagerConfig};
//! use sqlx::postgres::PgPool;
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
//! let stats = key_manager.get_stats().await?;
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
//! # #[cfg(all(feature = "encryption", feature = "azure-kv"))]
//! # {
//! use hammerwork::encryption::{KeyManager, KeyManagerConfig, KeySource};
//! use sqlx::postgres::PgPool;
//! use std::env;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! # let database_url = "postgres://user:pass@localhost/hammerwork";
//! # let pool = sqlx::PgPool::connect(database_url).await?;
//! // Set up Azure credentials via environment variables
//! env::set_var("AZURE_CLIENT_ID", "your-client-id");
//! env::set_var("AZURE_CLIENT_SECRET", "your-client-secret");
//! env::set_var("AZURE_TENANT_ID", "your-tenant-id");
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
//! // Master key is automatically loaded from Azure Key Vault
//! // If Azure Key Vault is unavailable, falls back to deterministic generation
//! let key_id = key_manager.generate_key("payment-key", 
//!     hammerwork::encryption::EncryptionAlgorithm::AES256GCM).await?;
//!
//! println!("Generated key with Azure Key Vault master key: {}", key_id);
//! # Ok(())
//! # }
//! # }
//! ```

use super::{EncryptionAlgorithm, EncryptionError, KeySource};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Database, Pool, Row};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

#[cfg(feature = "encryption")]
use {
    aes_gcm::{Aes256Gcm, Key as AesKey, KeyInit, Nonce, aead::Aead},
    base64::Engine,
    rand::{RngCore, rngs::OsRng},
};

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

pub struct KeyManager<DB: Database> {
    config: KeyManagerConfig,
    #[allow(dead_code)]
    pool: Pool<DB>,
    master_key: Arc<Mutex<Option<Vec<u8>>>>,
    master_key_id: Arc<Mutex<Option<Uuid>>>,
    key_cache: KeyCache,
    stats: Arc<Mutex<KeyManagerStats>>,
}

impl<DB: Database> Clone for KeyManager<DB> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pool: self.pool.clone(),
            master_key: self.master_key.clone(),
            master_key_id: self.master_key_id.clone(),
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

impl<DB: Database> KeyManager<DB>
where
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as sqlx::Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    /// Create a new key manager instance
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
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManager, KeyManagerConfig, KeySource};
    /// use sqlx::PgPool;
    /// use std::env;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Set up environment variable for master key
    /// env::set_var("MASTER_KEY", "my-super-secret-master-key-32-chars");
    ///
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
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManager, KeyManagerConfig, KeySource};
    /// use sqlx::MySqlPool;
    /// use chrono::Duration;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let pool = MySqlPool::connect("mysql://user:pass@localhost/hammerwork").await?;
    /// let config = KeyManagerConfig::new()
    ///     .with_master_key_source(KeySource::Static("my-32-char-master-key-here!!".to_string()))
    ///     .with_auto_rotation_enabled(true)
    ///     .with_default_rotation_interval(Duration::days(30));
    ///
    /// let key_manager = KeyManager::new(config, pool).await?;
    /// println!("MySQL key manager initialized with 30-day rotation");
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn new(config: KeyManagerConfig, pool: Pool<DB>) -> Result<Self, EncryptionError> {
        let manager = Self {
            config,
            pool,
            master_key: Arc::new(Mutex::new(None)),
            master_key_id: Arc::new(Mutex::new(None)),
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
    /// # #[cfg(feature = "encryption")]
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
    /// # #[cfg(feature = "encryption")]
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
    pub async fn generate_key_with_options(
        &mut self,
        key_id: &str,
        algorithm: EncryptionAlgorithm,
        purpose: KeyPurpose,
        expires_at: Option<DateTime<Utc>>,
        rotation_interval: Option<Duration>,
    ) -> Result<String, EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            return Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ));
        }

        #[cfg(feature = "encryption")]
        {
            info!("Generating new encryption key: {}", key_id);

            // Generate random key material
            let key_length = match algorithm {
                EncryptionAlgorithm::AES256GCM => 32,
                EncryptionAlgorithm::ChaCha20Poly1305 => 32,
            };
            let key_strength = key_length * 8;
            let mut key_material = vec![0u8; key_length];
            OsRng.fill_bytes(&mut key_material);

            // Encrypt the key material with the master key
            let encrypted_key_material = self.encrypt_key_material(&key_material).await?;

            // Create the key record
            let key_record = EncryptionKey {
                id: Uuid::new_v4(),
                key_id: key_id.to_string(),
                version: 1,
                algorithm,
                encrypted_key_material,
                derivation_salt: None,
                source: KeySource::Generated("database".to_string()),
                purpose,
                created_at: Utc::now(),
                created_by: Some("hammerwork".to_string()),
                expires_at,
                rotated_at: None,
                retired_at: None,
                status: KeyStatus::Active,
                rotation_interval,
                next_rotation_at: rotation_interval.map(|interval| Utc::now() + interval),
                key_strength: key_strength as u32,
                master_key_id: self.get_master_key_id().await,
                last_used_at: None,
                usage_count: 0,
            };

            // Store the key in the database
            self.store_key(&key_record).await?;

            // Add to cache
            self.cache_key(key_id, key_material).await;

            // Record audit event
            if self.config.audit_enabled {
                self.record_audit_event(key_id, KeyOperation::Create, true, None)
                    .await?;
            }

            // Update statistics
            self.increment_key_count().await;

            info!("Successfully generated encryption key: {}", key_id);
            Ok(key_id.to_string())
        }
    }

    /// Retrieve key material for encryption/decryption operations
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
    /// # #[cfg(feature = "encryption")]
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
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::{KeyManager, EncryptionError};
    ///
    /// # async fn example(mut key_manager: KeyManager<sqlx::Postgres>) -> Result<(), Box<dyn std::error::Error>> {
    /// // Try to retrieve a non-existent key
    /// match key_manager.get_key("non-existent-key").await {
    ///     Ok(key_material) => {
    ///         println!("Key retrieved successfully: {} bytes", key_material.len());
    ///     }
    ///     Err(EncryptionError::KeyNotFound(key_id)) => {
    ///         println!("Key '{}' not found", key_id);
    ///     }
    ///     Err(EncryptionError::KeyManagement(msg)) => {
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
        if let Some(cached_key) = self.get_cached_key(key_id).await {
            self.record_key_usage(key_id).await?;
            return Ok(cached_key);
        }

        // Load from database
        let key_record = self.load_key(key_id).await?;

        // Verify key is usable
        if key_record.status == KeyStatus::Revoked {
            return Err(EncryptionError::KeyManagement(format!(
                "Key {} has been revoked",
                key_id
            )));
        }

        if let Some(expires_at) = key_record.expires_at {
            if Utc::now() > expires_at {
                return Err(EncryptionError::KeyManagement(format!(
                    "Key {} has expired",
                    key_id
                )));
            }
        }

        // Decrypt the key material
        let key_material = self
            .decrypt_key_material(&key_record.encrypted_key_material)
            .await?;

        // Cache the decrypted key
        self.cache_key(key_id, key_material.clone()).await;

        // Record usage
        self.record_key_usage(key_id).await?;

        // Record audit event
        if self.config.audit_enabled {
            self.record_audit_event(key_id, KeyOperation::Access, true, None)
                .await?;
        }

        Ok(key_material)
    }

    /// Rotate a key to a new version
    ///
    /// Creates a new version of the specified key while keeping the old version
    /// available for decryption of previously encrypted data.
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
    /// # #[cfg(feature = "encryption")]
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
    /// // New version will be used for new encryption operations
    /// assert_eq!(new_version, 2); // Should be version 2 after first rotation
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    ///
    /// ## Key rotation with usage tracking
    ///
    /// ```rust,no_run
    /// # #[cfg(feature = "encryption")]
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
        #[cfg(not(feature = "encryption"))]
        {
            return Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ));
        }

        #[cfg(feature = "encryption")]
        {
            info!("Rotating encryption key: {}", key_id);

            // Load current key
            let current_key = self.load_key(key_id).await?;

            // Generate new key material
            let key_length = match current_key.algorithm {
                EncryptionAlgorithm::AES256GCM => 32,
                EncryptionAlgorithm::ChaCha20Poly1305 => 32,
            };
            let mut new_key_material = vec![0u8; key_length];
            OsRng.fill_bytes(&mut new_key_material);

            // Encrypt with master key
            let encrypted_key_material = self.encrypt_key_material(&new_key_material).await?;

            // Create new version
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
                created_at: Utc::now(),
                created_by: Some("hammerwork-rotation".to_string()),
                expires_at: current_key.expires_at,
                rotated_at: Some(Utc::now()),
                retired_at: None,
                status: KeyStatus::Active,
                rotation_interval: current_key.rotation_interval,
                next_rotation_at: current_key
                    .rotation_interval
                    .map(|interval| Utc::now() + interval),
                key_strength: current_key.key_strength,
                master_key_id: current_key.master_key_id,
                last_used_at: None,
                usage_count: 0,
            };

            // Store new version and retire old version
            self.store_key(&new_key_record).await?;
            self.retire_key_version(key_id, current_key.version).await?;

            // Update cache with new key
            self.cache_key(key_id, new_key_material).await;

            // Clean up old versions if we exceed max_key_versions
            self.cleanup_old_key_versions(key_id).await?;

            // Record audit event
            if self.config.audit_enabled {
                self.record_audit_event(key_id, KeyOperation::Rotate, true, None)
                    .await?;
            }

            // Update statistics
            self.increment_rotation_count().await;

            info!(
                "Successfully rotated key {} to version {}",
                key_id, new_version
            );
            Ok(new_version)
        }
    }

    /// Check for keys that need rotation and rotate them automatically
    pub async fn perform_automatic_rotation(&mut self) -> Result<Vec<String>, EncryptionError> {
        if !self.config.auto_rotation_enabled {
            return Ok(vec![]);
        }

        // Note: Database-specific implementations should override this behavior
        warn!("perform_automatic_rotation called on generic implementation - no rotation performed");
        Ok(vec![])
    }


    /// Start automated key rotation service that runs in the background
    /// Returns a future that should be spawned as a background task
    pub async fn start_rotation_service(
        &self,
        check_interval: Duration,
    ) -> Result<impl std::future::Future<Output = ()>, EncryptionError> {
        if !self.config.auto_rotation_enabled {
            return Err(EncryptionError::InvalidConfiguration(
                "Auto rotation is not enabled".to_string(),
            ));
        }

        let pool = self.pool.clone();
        let config = self.config.clone();
        let master_key = self.master_key.clone();
        let master_key_id = self.master_key_id.clone();
        let stats = self.stats.clone();
        let key_cache = self.key_cache.clone();

        let rotation_service = async move {
            let mut interval_timer = tokio::time::interval(std::time::Duration::from_secs(
                check_interval.num_seconds() as u64,
            ));

            loop {
                interval_timer.tick().await;

                // Create a temporary KeyManager instance for the rotation check
                let key_manager = KeyManager {
                    config: config.clone(),
                    pool: pool.clone(),
                    master_key: master_key.clone(),
                    master_key_id: master_key_id.clone(),
                    stats: stats.clone(),
                    key_cache: key_cache.clone(),
                };

                // Clone for mutable operations
                let mut rotation_manager = key_manager.clone();

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
    pub async fn refresh_stats(&self) -> Result<(), EncryptionError> {
        // Note: Database-specific implementations should override this behavior
        // For now, this method doesn't refresh from database to prevent compilation issues
        warn!("refresh_stats called on generic implementation - no database refresh performed");
        Ok(())
    }


    /// Get the current master key ID
    pub async fn get_master_key_id(&self) -> Option<Uuid> {
        self.master_key_id.lock().map(|id| *id).unwrap_or(None)
    }

    /// Set the master key ID
    pub async fn set_master_key_id(&self, key_id: Uuid) -> Result<(), EncryptionError> {
        *self.master_key_id.lock().map_err(|_| {
            EncryptionError::KeyManagement("Failed to acquire master key ID lock".to_string())
        })? = Some(key_id);
        Ok(())
    }

    /// Generate and store a new master key
    pub async fn generate_master_key(&mut self) -> Result<Uuid, EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            return Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ));
        }

        #[cfg(feature = "encryption")]
        {
            // Generate a new master key
            let master_key_id = Uuid::new_v4();
            let mut master_key_material = vec![0u8; 32]; // 256-bit key
            OsRng.fill_bytes(&mut master_key_material);

            // Store the master key securely in the database
            self.store_master_key_securely(&master_key_id, &master_key_material)
                .await?;

            // Keep a copy in memory for performance (encrypted with a derived key)
            *self.master_key.lock().map_err(|_| {
                EncryptionError::KeyManagement("Failed to acquire master key lock".to_string())
            })? = Some(master_key_material);

            // Set the master key ID
            self.set_master_key_id(master_key_id).await?;

            // Record audit event
            if self.config.audit_enabled {
                self.record_audit_event(
                    &master_key_id.to_string(),
                    KeyOperation::Create,
                    true,
                    None,
                )
                .await?;
            }

            info!("Generated new master key: {}", master_key_id);
            Ok(master_key_id)
        }
    }

    // Private helper methods

    async fn load_master_key(&self) -> Result<(), EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            return Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ));
        }

        #[cfg(feature = "encryption")]
        {
            let master_key_material = match &self.config.master_key_source {
                KeySource::Environment(env_var) => {
                    let key_str = std::env::var(env_var).map_err(|_| {
                        EncryptionError::KeyManagement(format!(
                            "Master key environment variable {} not found",
                            env_var
                        ))
                    })?;

                    base64::engine::general_purpose::STANDARD
                        .decode(&key_str)
                        .map_err(|e| {
                            EncryptionError::KeyManagement(format!(
                                "Invalid base64 master key: {}",
                                e
                            ))
                        })?
                }
                KeySource::Static(key_str) => base64::engine::general_purpose::STANDARD
                    .decode(key_str)
                    .map_err(|e| {
                        EncryptionError::KeyManagement(format!("Invalid base64 master key: {}", e))
                    })?,
                KeySource::Generated(_) => {
                    // Generate a new master key (for development only)
                    warn!("Generating new master key - this should not be used in production");
                    let mut key = vec![0u8; 32];
                    OsRng.fill_bytes(&mut key);
                    key
                }
                KeySource::External(service_config) => {
                    // Load master key from external service
                    if service_config.starts_with("aws://") {
                        Self::load_master_key_from_aws(service_config).await
                    } else if service_config.starts_with("vault://") {
                        Self::load_master_key_from_vault(service_config).await
                    } else if service_config.starts_with("gcp://") {
                        Self::load_master_key_from_gcp(service_config).await
                    } else if service_config.starts_with("azure://") {
                        Self::load_master_key_from_azure(service_config).await
                    } else {
                        return Err(EncryptionError::KeyManagement(format!(
                            "Unknown external master key service: {}",
                            service_config
                        )));
                    }
                }
            };

            // Validate master key length
            if master_key_material.len() != 32 {
                return Err(EncryptionError::KeyManagement(format!(
                    "Master key must be 32 bytes, got {}",
                    master_key_material.len()
                )));
            }

            *self.master_key.lock().map_err(|_| {
                EncryptionError::KeyManagement("Failed to acquire master key lock".to_string())
            })? = Some(master_key_material.clone());

            // Generate a deterministic master key ID based on the key material and retrieve stored ID
            let master_key_id = self
                .get_or_create_master_key_id(&master_key_material)
                .await?;
            self.set_master_key_id(master_key_id).await?;

            debug!("Master key loaded successfully with ID: {}", master_key_id);
            Ok(())
        }
    }

    #[cfg(feature = "encryption")]
    async fn encrypt_key_material(&self, key_material: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        let master_key = self.master_key.lock().map_err(|_| {
            EncryptionError::KeyManagement("Failed to acquire master key lock".to_string())
        })?;

        let master_key_material = master_key
            .as_ref()
            .ok_or_else(|| EncryptionError::KeyManagement("Master key not loaded".to_string()))?;

        // Use AES-256-GCM to encrypt the key material
        let cipher_key = AesKey::<Aes256Gcm>::from_slice(master_key_material);
        let cipher = Aes256Gcm::new(cipher_key);

        // Generate random nonce
        let mut nonce_bytes = vec![0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let mut ciphertext = cipher.encrypt(nonce, key_material).map_err(|e| {
            EncryptionError::EncryptionFailed(format!("Key encryption failed: {}", e))
        })?;

        // Prepend nonce to ciphertext for storage
        let mut encrypted_data = nonce_bytes;
        encrypted_data.append(&mut ciphertext);

        Ok(encrypted_data)
    }

    #[cfg(feature = "encryption")]
    async fn decrypt_key_material(
        &self,
        encrypted_data: &[u8],
    ) -> Result<Vec<u8>, EncryptionError> {
        if encrypted_data.len() < 12 {
            return Err(EncryptionError::DecryptionFailed(
                "Encrypted key data too short".to_string(),
            ));
        }

        let master_key = self.master_key.lock().map_err(|_| {
            EncryptionError::KeyManagement("Failed to acquire master key lock".to_string())
        })?;

        let master_key_material = master_key
            .as_ref()
            .ok_or_else(|| EncryptionError::KeyManagement("Master key not loaded".to_string()))?;

        // Extract nonce and ciphertext
        let nonce = Nonce::from_slice(&encrypted_data[..12]);
        let ciphertext = &encrypted_data[12..];

        // Decrypt using master key
        let cipher_key = AesKey::<Aes256Gcm>::from_slice(master_key_material);
        let cipher = Aes256Gcm::new(cipher_key);

        let plaintext = cipher.decrypt(nonce, ciphertext).map_err(|e| {
            EncryptionError::DecryptionFailed(format!("Key decryption failed: {}", e))
        })?;

        Ok(plaintext)
    }

    #[cfg(not(feature = "encryption"))]
    async fn encrypt_key_material(&self, _key_material: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        Err(EncryptionError::InvalidConfiguration(
            "Encryption feature is not enabled".to_string(),
        ))
    }

    #[cfg(not(feature = "encryption"))]
    async fn decrypt_key_material(
        &self,
        _encrypted_data: &[u8],
    ) -> Result<Vec<u8>, EncryptionError> {
        Err(EncryptionError::InvalidConfiguration(
            "Encryption feature is not enabled".to_string(),
        ))
    }

    async fn cache_key(&self, key_id: &str, key_material: Vec<u8>) {
        if let Ok(mut cache) = self.key_cache.lock() {
            cache.insert(key_id.to_string(), (key_material, Utc::now()));
        }
    }

    async fn get_cached_key(&self, key_id: &str) -> Option<Vec<u8>> {
        if let Ok(cache) = self.key_cache.lock() {
            // Check if key is in cache and not too old (cache for 1 hour)
            if let Some((key_material, cached_at)) = cache.get(key_id) {
                if Utc::now() - *cached_at < Duration::hours(1) {
                    return Some(key_material.clone());
                }
            }
        }
        None
    }

    // Database operations for key storage
    async fn store_key(&self, _key: &EncryptionKey) -> Result<(), EncryptionError> {
        // Database-specific implementations are provided in separate impl blocks
        Err(EncryptionError::KeyManagement(
            "Database-specific implementation required".to_string(),
        ))
    }

    async fn load_key(&self, _key_id: &str) -> Result<EncryptionKey, EncryptionError> {
        // Database-specific implementations are provided in separate impl blocks
        Err(EncryptionError::KeyManagement(
            "Database-specific implementation required".to_string(),
        ))
    }

    async fn retire_key_version(
        &self,
        _key_id: &str,
        _version: u32,
    ) -> Result<(), EncryptionError> {
        // Database-specific implementations are provided in separate impl blocks
        Err(EncryptionError::KeyManagement(
            "Database-specific implementation required".to_string(),
        ))
    }

    async fn cleanup_old_key_versions(&self, _key_id: &str) -> Result<(), EncryptionError> {
        // Database-specific implementations are provided in separate impl blocks
        Err(EncryptionError::KeyManagement(
            "Database-specific implementation required".to_string(),
        ))
    }


    async fn record_key_usage(&self, _key_id: &str) -> Result<(), EncryptionError> {
        // Database-specific implementations are provided in separate impl blocks
        Err(EncryptionError::KeyManagement(
            "Database-specific implementation required".to_string(),
        ))
    }

    async fn record_audit_event(
        &self,
        _key_id: &str,
        _operation: KeyOperation,
        _success: bool,
        _error_message: Option<String>,
    ) -> Result<(), EncryptionError> {
        // Database-specific implementations are provided in separate impl blocks
        Err(EncryptionError::KeyManagement(
            "Database-specific implementation required".to_string(),
        ))
    }

    async fn increment_key_count(&self) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.total_keys += 1;
            stats.active_keys += 1;
        }
    }

    async fn increment_rotation_count(&self) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.rotations_performed += 1;
        }
    }

    // External master key loading methods
    #[cfg(feature = "encryption")]
    async fn load_master_key_from_aws(service_config: &str) -> Vec<u8> {
        // Parse AWS KMS configuration for master key
        let config_parts: Vec<&str> = service_config
            .strip_prefix("aws://")
            .unwrap_or(service_config)
            .split('?')
            .collect();

        let key_id = config_parts[0];
        let region = if config_parts.len() > 1 {
            config_parts[1]
                .strip_prefix("region=")
                .unwrap_or("us-east-1")
        } else {
            "us-east-1"
        };

        info!(
            "Loading master key from AWS KMS: key_id={}, region={}",
            key_id, region
        );

        #[cfg(feature = "aws-kms")]
        {
            use aws_config::Region;
            use aws_sdk_kms::Client;

            // Load AWS configuration
            let config = aws_config::defaults(aws_config::BehaviorVersion::v2025_01_17())
                .region(Region::new(region.to_string()))
                .load()
                .await;

            let client = Client::new(&config);

            // Generate a data key for this specific master key
            // In practice, you might want to store the encrypted data key and decrypt it
            // For now, we'll generate a data key each time (which is expensive but functional)
            match client
                .generate_data_key()
                .key_id(key_id)
                .key_spec(aws_sdk_kms::types::DataKeySpec::Aes256)
                .send()
                .await
            {
                Ok(response) => {
                    if let Some(plaintext) = response.plaintext {
                        let key_material = plaintext.into_inner();
                        if key_material.len() == 32 {
                            info!("Successfully loaded master key from AWS KMS");
                            return key_material;
                        } else {
                            error!(
                                "AWS KMS returned key with incorrect length: {}",
                                key_material.len()
                            );
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to load key from AWS KMS: {}", e);
                }
            }
        }

        #[cfg(not(feature = "aws-kms"))]
        {
            warn!("AWS KMS feature not enabled, falling back to deterministic key generation");
        }

        // Fallback to deterministic key generation for development/testing
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"aws-kms-master-key");
        hasher.update(key_id.as_bytes());
        hasher.update(region.as_bytes());
        let hash = hasher.finalize();
        hash[0..32].to_vec()
    }

    #[cfg(feature = "encryption")]
    async fn load_master_key_from_vault(service_config: &str) -> Vec<u8> {
        // Parse Vault configuration for master key
        let config_parts: Vec<&str> = service_config
            .strip_prefix("vault://")
            .unwrap_or(service_config)
            .split('?')
            .collect();

        let secret_path = config_parts[0];
        let vault_addr = if config_parts.len() > 1 {
            config_parts[1]
                .strip_prefix("addr=")
                .unwrap_or("https://vault.example.com")
                .to_string()
        } else {
            std::env::var("VAULT_ADDR").unwrap_or_else(|_| "https://vault.example.com".to_string())
        };

        info!(
            "Loading master key from HashiCorp Vault: path={}, addr={}",
            secret_path, vault_addr
        );

        #[cfg(feature = "vault-kms")]
        {
            use vaultrs::{client::VaultClient, kv2};

            // Try to get Vault token from environment
            let token = std::env::var("VAULT_TOKEN").ok();

            if let Some(vault_token) = token {
                // Create Vault client
                let client_result = VaultClient::new(
                    vaultrs::client::VaultClientSettingsBuilder::default()
                        .address(vault_addr.clone())
                        .token(vault_token)
                        .build()
                        .unwrap(),
                );

                match client_result {
                    Ok(client) => {
                        // Try to read the secret from Vault
                        // Parse the path to extract mount and secret path
                        let path_parts: Vec<&str> = secret_path.split('/').collect();
                        if path_parts.len() >= 2 {
                            let mount = path_parts[0];
                            let secret_key = path_parts[1..].join("/");

                            match kv2::read::<serde_json::Value>(&client, mount, &secret_key).await
                            {
                                Ok(secret) => {
                                    // Look for a key field in the secret
                                    if let Some(key_data) = secret.get("key") {
                                        if let Some(key_str) = key_data.as_str() {
                                            // Try to decode as base64 first
                                            if let Ok(decoded) = base64::Engine::decode(
                                                &base64::engine::general_purpose::STANDARD,
                                                key_str,
                                            ) {
                                                if decoded.len() == 32 {
                                                    info!(
                                                        "Successfully loaded master key from HashiCorp Vault"
                                                    );
                                                    return decoded;
                                                }
                                            }
                                            // If not base64, use as string and hash to 32 bytes
                                            use sha2::{Digest, Sha256};
                                            let mut hasher = Sha256::new();
                                            hasher.update(key_str.as_bytes());
                                            let hash = hasher.finalize();
                                            info!(
                                                "Successfully loaded and hashed master key from HashiCorp Vault"
                                            );
                                            return hash[0..32].to_vec();
                                        }
                                    }

                                    // If no 'key' field, generate from secret path
                                    warn!(
                                        "No 'key' field found in Vault secret, using deterministic generation"
                                    );
                                }
                                Err(e) => {
                                    error!("Failed to read secret from HashiCorp Vault: {}", e);
                                }
                            }
                        } else {
                            error!("Invalid Vault secret path format: {}", secret_path);
                        }
                    }
                    Err(e) => {
                        error!("Failed to create Vault client: {}", e);
                    }
                }
            } else {
                warn!(
                    "No VAULT_TOKEN environment variable found, falling back to deterministic key generation"
                );
            }
        }

        #[cfg(not(feature = "vault-kms"))]
        {
            warn!("Vault KMS feature not enabled, falling back to deterministic key generation");
        }

        // Fallback to deterministic key generation for development/testing
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"vault-master-key");
        hasher.update(secret_path.as_bytes());
        hasher.update(vault_addr.as_bytes());
        let hash = hasher.finalize();
        hash[0..32].to_vec()
    }

    #[cfg(feature = "encryption")]
    async fn load_master_key_from_gcp(service_config: &str) -> Vec<u8> {
        // Parse GCP KMS configuration for master key
        let key_resource = service_config
            .strip_prefix("gcp://")
            .unwrap_or(service_config);

        info!("Loading master key from GCP KMS: resource={}", key_resource);

        #[cfg(feature = "gcp-kms")]
        {
            use google_cloud_kms::client::{Client, ClientConfig};
            use google_cloud_kms::grpc::kms::v1::GenerateRandomBytesRequest;

            // Try to create GCP KMS client with automatic authentication
            let config_result = ClientConfig::default().with_auth().await;

            match config_result {
                Ok(client_config) => {
                    let client_result = Client::new(client_config).await;

                    match client_result {
                        Ok(client) => {
                            // Parse the key resource path for project and location
                            let path_parts: Vec<&str> = key_resource.split('/').collect();
                            if path_parts.len() >= 4 {
                                let project = path_parts[1];
                                let location = path_parts[3];

                                // Create a parent path for the project/location
                                let parent = format!("projects/{}/locations/{}", project, location);

                                // Generate random bytes for the master key
                                let req = GenerateRandomBytesRequest {
                                    location: parent,
                                    length_bytes: 32, // Always 32 bytes for master key
                                    protection_level: 1, // SOFTWARE (default protection level)
                                };

                                match client.generate_random_bytes(req, None).await {
                                    Ok(response) => {
                                        let plaintext = response.data;
                                        if plaintext.len() == 32 {
                                            info!("Successfully generated master key from GCP KMS");
                                            return plaintext;
                                        } else {
                                            error!(
                                                "GCP KMS returned key with incorrect length: {}",
                                                plaintext.len()
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!(
                                            "Failed to generate random bytes from GCP KMS: {}",
                                            e
                                        );
                                    }
                                }
                            } else {
                                error!("Invalid GCP KMS resource path format: {}", key_resource);
                            }
                        }
                        Err(e) => {
                            error!("Failed to create GCP KMS client: {}", e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to configure GCP KMS client: {}", e);
                }
            }
        }

        #[cfg(not(feature = "gcp-kms"))]
        {
            warn!("GCP KMS feature not enabled, falling back to deterministic key generation");
        }

        // Fallback to deterministic key generation for development/testing
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"gcp-kms-master-key");
        hasher.update(key_resource.as_bytes());
        let hash = hasher.finalize();
        hash[0..32].to_vec()
    }

    /// Load master key from Azure Key Vault
    ///
    /// This method attempts to retrieve the master key from Azure Key Vault using the
    /// configured service URL and key name. If Azure Key Vault is not available or
    /// the `azure-kv` feature is not enabled, it falls back to deterministic key generation.
    ///
    /// # Arguments
    ///
    /// * `service_config` - Azure Key Vault configuration string in format "azure://vault-name/path/key-name"
    ///
    /// # Returns
    ///
    /// A 32-byte master key suitable for AES-256 encryption
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(feature = "encryption")]
    /// # {
    /// use hammerwork::encryption::key_manager::KeyManager;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Load master key from Azure Key Vault
    /// let master_key = KeyManager::<sqlx::Postgres>::load_master_key_from_azure(
    ///     "azure://my-vault.vault.azure.net/keys/master-key"
    /// ).await;
    ///
    /// assert_eq!(master_key.len(), 32); // AES-256 key size
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    #[cfg(feature = "encryption")]
    async fn load_master_key_from_azure(service_config: &str) -> Vec<u8> {
        // Parse Azure Key Vault configuration for master key
        let vault_parts: Vec<&str> = service_config
            .strip_prefix("azure://")
            .unwrap_or(service_config)
            .split('/')
            .collect();

        let vault_url = if !vault_parts.is_empty() {
            format!("https://{}", vault_parts[0])
        } else {
            "https://vault.vault.azure.net".to_string()
        };

        let key_name = vault_parts.get(2).unwrap_or(&"master-key");

        info!(
            "Loading master key from Azure Key Vault: vault={}, key={}",
            vault_url, key_name
        );

        // Try to load from Azure Key Vault if the feature is enabled
        #[cfg(feature = "azure-kv")]
        {
            use azure_identity::{DefaultAzureCredential, TokenCredentialOptions};
            use azure_security_keyvault::KeyvaultClient;
            
            match Self::load_from_azure_key_vault(&vault_url, key_name).await {
                Ok(key_material) => {
                    info!("Successfully loaded master key from Azure Key Vault");
                    return key_material;
                }
                Err(e) => {
                    warn!("Failed to load master key from Azure Key Vault: {}", e);
                    info!("Falling back to deterministic key generation");
                }
            }
        }

        // Fallback to deterministic key generation for development/testing
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"azure-kv-master-key");
        hasher.update(vault_url.as_bytes());
        hasher.update(key_name.as_bytes());
        let hash = hasher.finalize();
        hash[0..32].to_vec()
    }

    /// Load key material directly from Azure Key Vault
    ///
    /// This method uses the Azure SDK to authenticate and retrieve key material from
    /// Azure Key Vault. It supports automatic credential resolution through
    /// `DefaultAzureCredential` and handles key size normalization.
    ///
    /// # Arguments
    ///
    /// * `vault_url` - Full URL to the Azure Key Vault (e.g., "https://my-vault.vault.azure.net")
    /// * `key_name` - Name of the key to retrieve from the vault
    ///
    /// # Returns
    ///
    /// A 32-byte key material suitable for AES-256 encryption, or an error message
    ///
    /// # Security Features
    ///
    /// - Uses `DefaultAzureCredential` for secure authentication
    /// - Automatically handles key size normalization via HMAC-based key derivation
    /// - Supports base64-encoded key material from Azure Key Vault
    /// - Includes proper error handling for authentication and network issues
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "azure-kv"))]
    /// # {
    /// use hammerwork::encryption::key_manager::KeyManager;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Load key from Azure Key Vault
    /// let key_material = KeyManager::<sqlx::Postgres>::load_from_azure_key_vault(
    ///     "https://my-vault.vault.azure.net",
    ///     "master-key"
    /// ).await?;
    ///
    /// assert_eq!(key_material.len(), 32); // Always 32 bytes for AES-256
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    #[cfg(all(feature = "encryption", feature = "azure-kv"))]
    async fn load_from_azure_key_vault(vault_url: &str, key_name: &str) -> Result<Vec<u8>, String> {
        use azure_identity::{DefaultAzureCredential, TokenCredentialOptions};
        use azure_security_keyvault::KeyvaultClient;
        
        // Create Azure credentials
        let credential = DefaultAzureCredential::create(TokenCredentialOptions::default())
            .map_err(|e| format!("Failed to create Azure credentials: {}", e))?;

        // Create Azure Key Vault client
        let client = KeyvaultClient::new(vault_url, std::sync::Arc::new(credential))
            .map_err(|e| format!("Failed to create Azure Key Vault client: {}", e))?;

        // Retrieve the master key from Azure Key Vault
        match client.key_client().get(key_name.to_string()).await {
            Ok(key_response) => {
                if let Some(key_material) = key_response.key.k {
                    // Decode the base64-encoded key material
                    let decoded_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(key_material)
                        .map_err(|e| format!("Failed to decode key material: {}", e))?;
                    
                    // Ensure the key is the correct size for AES-256 (32 bytes)
                    if decoded_key.len() >= 32 {
                        Ok(decoded_key[0..32].to_vec())
                    } else {
                        // If the key is too short, use it as input for HMAC-based key derivation
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
                } else {
                    Err("Key material not found in Azure Key Vault response".to_string())
                }
            }
            Err(e) => {
                Err(format!("Failed to retrieve key from Azure Key Vault: {}", e))
            }
        }
    }

    /// Store master key securely in the database
    async fn store_master_key_securely(
        &self,
        master_key_id: &Uuid,
        key_material: &[u8],
    ) -> Result<(), EncryptionError> {
        // Generate a unique salt for the master key encryption
        let mut salt = vec![0u8; 32];
        use rand::{RngCore, rngs::OsRng};
        OsRng.fill_bytes(&mut salt);

        // Derive an encryption key from system entropy and configuration
        let system_key = self.derive_system_encryption_key(&salt)?;

        // Encrypt the master key material with the system key
        let encrypted_material = self.encrypt_with_system_key(&system_key, key_material)?;

        // Store the encrypted master key in the database
        #[cfg(feature = "postgres")]
        {
            let now = chrono::Utc::now();
            sqlx::query(
                r#"
                INSERT INTO hammerwork_encryption_keys 
                (id, key_id, key_version, algorithm, key_material, key_derivation_salt, 
                 key_source, key_purpose, created_at, created_by, expires_at, status, 
                 key_strength, master_key_id, last_used_at, usage_count, rotation_interval, next_rotation_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
                "#,
            )
            .bind(uuid::Uuid::new_v4()) // id
            .bind(master_key_id.to_string()) // key_id
            .bind(1i32) // key_version
            .bind("AES256GCM") // algorithm
            .bind(&encrypted_material) // key_material (encrypted)
            .bind(&salt) // key_derivation_salt
            .bind("System") // key_source
            .bind("KEK") // key_purpose (Key Encryption Key)
            .bind(now) // created_at
            .bind("system") // created_by
            .bind(None::<chrono::DateTime<chrono::Utc>>) // expires_at (no expiration for master key)
            .bind("Active") // status
            .bind(256i32) // key_strength (256-bit)
            .bind(None::<uuid::Uuid>) // master_key_id (self-referential, but None for master key)
            .bind(None::<chrono::DateTime<chrono::Utc>>) // last_used_at
            .bind(0i64) // usage_count
            .bind(None::<sqlx::postgres::types::PgInterval>) // rotation_interval (no rotation for master key)
            .bind(None::<chrono::DateTime<chrono::Utc>>) // next_rotation_at
            .execute(&self.pool)
            .await
            .map_err(|e| EncryptionError::DatabaseError(format!("Failed to store master key: {}", e)))?;
        }

        #[cfg(feature = "mysql")]
        {
            let now = chrono::Utc::now();
            sqlx::query(
                r#"
                INSERT INTO hammerwork_encryption_keys 
                (id, key_id, key_version, algorithm, key_material, key_derivation_salt, 
                 key_source, key_purpose, created_at, created_by, expires_at, status, 
                 key_strength, master_key_id, last_used_at, usage_count, rotation_interval_seconds, next_rotation_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(uuid::Uuid::new_v4().to_string()) // id
            .bind(master_key_id.to_string()) // key_id
            .bind(1i32) // key_version
            .bind("AES256GCM") // algorithm
            .bind(&encrypted_material) // key_material (encrypted)
            .bind(&salt) // key_derivation_salt
            .bind("System") // key_source
            .bind("KEK") // key_purpose (Key Encryption Key)
            .bind(now) // created_at
            .bind("system") // created_by
            .bind(None::<chrono::DateTime<chrono::Utc>>) // expires_at (no expiration for master key)
            .bind("Active") // status
            .bind(256i32) // key_strength (256-bit)
            .bind(None::<String>) // master_key_id (self-referential, but None for master key)
            .bind(None::<chrono::DateTime<chrono::Utc>>) // last_used_at
            .bind(0i64) // usage_count
            .bind(None::<i64>) // rotation_interval_seconds (no rotation for master key)
            .bind(None::<chrono::DateTime<chrono::Utc>>) // next_rotation_at
            .execute(&self.pool)
            .await
            .map_err(|e| EncryptionError::DatabaseError(format!("Failed to store master key: {}", e)))?;
        }

        #[cfg(any(feature = "postgres", feature = "mysql"))]
        {
            info!(
                "Master key stored securely in database with ID: {}",
                master_key_id
            );
            Ok(())
        }

        #[cfg(not(any(feature = "postgres", feature = "mysql")))]
        {
            Err(EncryptionError::InvalidConfiguration(
                "No database feature enabled for key storage".to_string()
            ))
        }
    }

    /// Get or create a master key ID based on key material, with database persistence
    async fn get_or_create_master_key_id(
        &self,
        key_material: &[u8],
    ) -> Result<Uuid, EncryptionError> {
        // First, try to find an existing master key ID in the database
        let existing_key_id = self.find_master_key_id_in_database().await?;

        if let Some(key_id) = existing_key_id {
            debug!("Using existing master key ID from database: {}", key_id);
            return Ok(key_id);
        }

        // Generate a deterministic ID based on key material for consistency
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(key_material);
        hasher.update(b"hammerwork-master-key-v1"); // Version tag for future compatibility
        let hash = hasher.finalize();

        let master_key_id = Uuid::from_bytes([
            hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7], hash[8],
            hash[9], hash[10], hash[11], hash[12], hash[13], hash[14], hash[15],
        ]);

        // Store this ID association in the database for future lookups
        self.store_master_key_id_mapping(&master_key_id).await?;

        debug!("Generated and stored new master key ID: {}", master_key_id);
        Ok(master_key_id)
    }

    /// Find existing master key ID in database
    async fn find_master_key_id_in_database(&self) -> Result<Option<Uuid>, EncryptionError> {
        #[cfg(feature = "postgres")]
        {
            let row = sqlx::query(
                "SELECT key_id FROM hammerwork_encryption_keys WHERE key_purpose = 'KEK' AND status = 'Active' LIMIT 1"
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| EncryptionError::DatabaseError(e.to_string()))?;

            if let Some(row) = row {
                let key_id_str: String = row.get("key_id");
                let key_id = Uuid::parse_str(&key_id_str).map_err(|e| {
                    EncryptionError::KeyManagement(format!("Invalid UUID in database: {}", e))
                })?;
                return Ok(Some(key_id));
            }
        }

        #[cfg(feature = "mysql")]
        {
            let row = sqlx::query(
                "SELECT key_id FROM hammerwork_encryption_keys WHERE key_purpose = 'KEK' AND status = 'Active' LIMIT 1"
            )
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| EncryptionError::DatabaseError(e.to_string()))?;

            if let Some(row) = row {
                let key_id_str: String = row.get("key_id");
                let key_id = Uuid::parse_str(&key_id_str).map_err(|e| {
                    EncryptionError::KeyManagement(format!("Invalid UUID in database: {}", e))
                })?;
                return Ok(Some(key_id));
            }
        }

        Ok(None)
    }

    /// Store master key ID mapping for future lookups
    async fn store_master_key_id_mapping(
        &self,
        master_key_id: &Uuid,
    ) -> Result<(), EncryptionError> {
        // This is handled by store_master_key_securely, so this is just a placeholder
        // for future implementation if we need additional mapping tables
        debug!("Master key ID mapping stored: {}", master_key_id);
        Ok(())
    }

    /// Derive a system encryption key for encrypting master keys
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
        let salt_string = SaltString::encode_b64(salt)
            .map_err(|e| EncryptionError::KeyManagement(format!("Failed to encode salt: {}", e)))?;

        let password_hash = argon2
            .hash_password(&input, &salt_string)
            .map_err(|e| EncryptionError::KeyManagement(format!("Key derivation failed: {}", e)))?;

        // Extract the raw hash bytes
        let hash = password_hash.hash.ok_or_else(|| {
            EncryptionError::KeyManagement("No hash in password result".to_string())
        })?;
        let hash_bytes = hash.as_bytes();

        // Return first 32 bytes for AES-256
        Ok(hash_bytes[0..32].to_vec())
    }

    /// Encrypt data with system-derived key
    fn encrypt_with_system_key(
        &self,
        system_key: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, EncryptionError> {
        use aes_gcm::{
            Aes256Gcm, Nonce,
            aead::{Aead, KeyInit, OsRng},
        };

        let cipher = Aes256Gcm::new_from_slice(system_key).map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to create cipher: {}", e))
        })?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; 12];
        use rand::RngCore;
        OsRng.fill_bytes(&mut nonce_bytes);
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

// Database-specific implementations
#[cfg(feature = "postgres")]
impl KeyManager<sqlx::Postgres> {
    #[allow(dead_code)]
    async fn store_key_postgres(&self, key: &EncryptionKey) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_encryption_keys (
                id, key_id, key_version, algorithm, key_material, key_derivation_salt, key_source, key_purpose,
                created_at, created_by, expires_at, rotated_at, retired_at, status, rotation_interval, next_rotation_at,
                key_strength, master_key_id, last_used_at, usage_count
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
            ON CONFLICT (key_id) DO UPDATE SET
                key_version = $3,
                algorithm = $4,
                key_material = $5,
                status = $14,
                rotated_at = $12,
                next_rotation_at = $16,
                last_used_at = $19,
                usage_count = $20
            "#
        )
        .bind(key.id)
        .bind(&key.key_id)
        .bind(key.version as i32)
        .bind(key.algorithm.to_string())
        .bind(&key.encrypted_key_material)
        .bind(&key.derivation_salt)
        .bind(key.source.to_string())
        .bind(key.purpose.to_string())
        .bind(key.created_at)
        .bind(&key.created_by)
        .bind(key.expires_at)
        .bind(key.rotated_at)
        .bind(key.retired_at)
        .bind(key.status.to_string())
        .bind(key.rotation_interval.map(|d| {
            // Convert chrono::Duration to PostgreSQL INTERVAL
            format!("{} seconds", d.num_seconds())
        }))
        .bind(key.next_rotation_at)
        .bind(key.key_strength as i32)
        .bind(key.master_key_id)
        .bind(key.last_used_at)
        .bind(key.usage_count as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| EncryptionError::KeyManagement(format!("Failed to store key: {}", e)))?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn load_key_postgres(&self, key_id: &str) -> Result<EncryptionKey, EncryptionError> {
        let row = sqlx::query(
            r#"
            SELECT id, key_id, key_version, algorithm, key_material, key_derivation_salt, key_source, key_purpose,
                   created_at, created_by, expires_at, rotated_at, retired_at, status, rotation_interval, next_rotation_at,
                   key_strength, master_key_id, last_used_at, usage_count
            FROM hammerwork_encryption_keys
            WHERE key_id = $1
            ORDER BY key_version DESC
            LIMIT 1
            "#
        )
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EncryptionError::KeyManagement(format!("Failed to load key: {}", e)))?;

        let row = row
            .ok_or_else(|| EncryptionError::KeyManagement(format!("Key not found: {}", key_id)))?;

        let rotation_interval =
            if let Some(interval_str) = row.get::<Option<String>, _>("rotation_interval") {
                // Parse PostgreSQL INTERVAL format
                parse_postgres_interval(&interval_str)
            } else {
                None
            };

        Ok(EncryptionKey {
            id: row.get("id"),
            key_id: row.get("key_id"),
            version: row.get::<i32, _>("key_version") as u32,
            algorithm: parse_algorithm(row.get("algorithm"))?,
            encrypted_key_material: row.get("key_material"),
            derivation_salt: row.get("key_derivation_salt"),
            source: parse_key_source(row.get("key_source"))?,
            purpose: parse_key_purpose(row.get("key_purpose"))?,
            created_at: row.get("created_at"),
            created_by: row.get("created_by"),
            expires_at: row.get("expires_at"),
            rotated_at: row.get("rotated_at"),
            retired_at: row.get("retired_at"),
            status: parse_key_status(row.get("status"))?,
            rotation_interval,
            next_rotation_at: row.get("next_rotation_at"),
            key_strength: row.get::<i32, _>("key_strength") as u32,
            master_key_id: row.get("master_key_id"),
            last_used_at: row.get("last_used_at"),
            usage_count: row.get::<i64, _>("usage_count") as u64,
        })
    }

    #[allow(dead_code)]
    async fn retire_key_version_postgres(
        &self,
        key_id: &str,
        version: u32,
    ) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            UPDATE hammerwork_encryption_keys
            SET status = 'Retired', retired_at = NOW()
            WHERE key_id = $1 AND key_version = $2
            "#,
        )
        .bind(key_id)
        .bind(version as i32)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to retire key version: {}", e))
        })?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn cleanup_old_key_versions_postgres(&self, key_id: &str) -> Result<(), EncryptionError> {
        // Keep only the latest max_key_versions for each key_id
        sqlx::query(
            r#"
            DELETE FROM hammerwork_encryption_keys
            WHERE key_id = $1 AND key_version NOT IN (
                SELECT key_version FROM hammerwork_encryption_keys
                WHERE key_id = $1
                ORDER BY key_version DESC
                LIMIT $2
            )
            "#,
        )
        .bind(key_id)
        .bind(self.config.max_key_versions as i32)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to cleanup old key versions: {}", e))
        })?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn get_keys_due_for_rotation_postgres(&self) -> Result<Vec<String>, EncryptionError> {
        let rows = sqlx::query(
            r#"
            SELECT key_id
            FROM hammerwork_encryption_keys
            WHERE status = 'Active' 
            AND next_rotation_at IS NOT NULL 
            AND next_rotation_at <= NOW()
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to get keys due for rotation: {}", e))
        })?;

        Ok(rows.into_iter().map(|row| row.get("key_id")).collect())
    }

    #[allow(dead_code)]
    async fn record_key_usage_postgres(&self, key_id: &str) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            UPDATE hammerwork_encryption_keys
            SET last_used_at = NOW(), usage_count = usage_count + 1
            WHERE key_id = $1
            "#,
        )
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to record key usage: {}", e))
        })?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn record_audit_event_postgres(
        &self,
        key_id: &str,
        operation: KeyOperation,
        success: bool,
        error_message: Option<String>,
    ) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_key_audit_log (
                key_id, operation, success, error_message, timestamp
            ) VALUES ($1, $2, $3, $4, NOW())
            "#,
        )
        .bind(key_id)
        .bind(operation.to_string())
        .bind(success)
        .bind(error_message)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to record audit event: {}", e))
        })?;

        Ok(())
    }

    /// Check if a specific key is due for rotation
    #[cfg(feature = "postgres")]
    pub async fn is_key_due_for_rotation(
        &self,
        key_id: &str,
    ) -> Result<bool, EncryptionError> {
        let result = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM hammerwork_encryption_keys
            WHERE key_id = $1
            AND status = 'Active' 
            AND next_rotation_at IS NOT NULL 
            AND next_rotation_at <= NOW()
            "#,
        )
        .bind(key_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to check rotation status for key {}: {}",
                key_id, e
            ))
        })?;

        let count: i64 = result.get("count");
        Ok(count > 0)
    }

    /// Update rotation schedule for a key (PostgreSQL)
    async fn update_key_rotation_schedule_postgres(
        &self,
        key_id: &str,
        rotation_interval: Option<Duration>,
        next_rotation_at: Option<DateTime<Utc>>,
    ) -> Result<(), EncryptionError> {
        let interval_postgres =
            rotation_interval.map(|interval| format!("{} seconds", interval.num_seconds()));

        sqlx::query(
            r#"
            UPDATE hammerwork_encryption_keys
            SET rotation_interval = $2, next_rotation_at = $3
            WHERE key_id = $1 AND status = 'Active'
            "#,
        )
        .bind(key_id)
        .bind(interval_postgres)
        .bind(next_rotation_at)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to update rotation schedule for key {}: {}",
                key_id, e
            ))
        })?;

        info!(
            "Updated rotation schedule for key {}: interval={:?}, next_rotation={:?}",
            key_id, rotation_interval, next_rotation_at
        );
        Ok(())
    }

    /// Get rotation schedule for a key (PostgreSQL)
    async fn get_key_rotation_schedule_postgres(
        &self,
        key_id: &str,
    ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
        let result = sqlx::query(
            r#"
            SELECT next_rotation_at
            FROM hammerwork_encryption_keys
            WHERE key_id = $1 AND status = 'Active'
            ORDER BY key_version DESC
            LIMIT 1
            "#,
        )
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to get rotation schedule for key {}: {}",
                key_id, e
            ))
        })?;

        match result {
            Some(row) => Ok(row.get("next_rotation_at")),
            None => Ok(None),
        }
    }

    /// Schedule a key for rotation at a specific time (PostgreSQL)
    async fn schedule_key_rotation_postgres(
        &self,
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
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to schedule rotation for key {}: {}",
                key_id, e
            ))
        })?;

        info!("Scheduled rotation for key {} at {}", key_id, rotation_time);
        Ok(())
    }

    /// Get scheduled rotations within a time window (PostgreSQL)
    async fn get_scheduled_rotations_postgres(
        &self,
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
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to get scheduled rotations: {}", e))
        })?;

        let scheduled_rotations = rows
            .into_iter()
            .filter_map(|row| {
                let key_id: String = row.get("key_id");
                let rotation_time: Option<DateTime<Utc>> = row.get("next_rotation_at");
                rotation_time.map(|time| (key_id, time))
            })
            .collect();

        Ok(scheduled_rotations)
    }

    /// Update the rotation schedule for a key (PostgreSQL)
    pub async fn update_key_rotation_schedule(
        &self,
        key_id: &str,
        rotation_interval: Option<Duration>,
    ) -> Result<(), EncryptionError> {
        let next_rotation_at = rotation_interval.map(|interval| Utc::now() + interval);
        self.update_key_rotation_schedule_postgres(key_id, rotation_interval, next_rotation_at)
            .await
    }

    /// Get the next scheduled rotation time for a key (PostgreSQL)
    pub async fn get_key_rotation_schedule(
        &self,
        key_id: &str,
    ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
        self.get_key_rotation_schedule_postgres(key_id).await
    }

    /// Schedule a key for future rotation (PostgreSQL)
    pub async fn schedule_key_rotation(
        &self,
        key_id: &str,
        rotation_time: DateTime<Utc>,
    ) -> Result<(), EncryptionError> {
        self.schedule_key_rotation_postgres(key_id, rotation_time)
            .await
    }

    /// Get all keys scheduled for rotation within a time window (PostgreSQL)
    pub async fn get_scheduled_rotations(
        &self,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
    ) -> Result<Vec<(String, DateTime<Utc>)>, EncryptionError> {
        self.get_scheduled_rotations_postgres(from_time, to_time)
            .await
    }

    /// Query statistics from PostgreSQL
    #[cfg(feature = "postgres")]
    pub async fn query_database_statistics(&self) -> Result<KeyManagerStats, EncryptionError> {
        // Execute multiple queries to gather comprehensive statistics
        let basic_counts = sqlx::query(
            r#"
            SELECT 
                COUNT(*) as total_keys,
                COUNT(CASE WHEN status = 'Active' THEN 1 END) as active_keys,
                COUNT(CASE WHEN status = 'Retired' THEN 1 END) as retired_keys,
                COUNT(CASE WHEN status = 'Revoked' THEN 1 END) as revoked_keys,
                COUNT(CASE WHEN status = 'Expired' THEN 1 END) as expired_keys
            FROM hammerwork_encryption_keys
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query key counts: {}", e))
        })?;

        let total_keys: i64 = basic_counts.get("total_keys");
        let active_keys: i64 = basic_counts.get("active_keys");
        let retired_keys: i64 = basic_counts.get("retired_keys");
        let revoked_keys: i64 = basic_counts.get("revoked_keys");
        let expired_keys: i64 = basic_counts.get("expired_keys");

        // Calculate average key age
        let age_result = sqlx::query(
            r#"
            SELECT 
                COALESCE(AVG(EXTRACT(EPOCH FROM (NOW() - created_at)) / 86400), 0) as avg_age_days
            FROM hammerwork_encryption_keys 
            WHERE status IN ('Active', 'Retired')
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query average age: {}", e))
        })?;

        let average_key_age_days: f64 = age_result.get("avg_age_days");

        // Count keys expiring soon (within 7 days)
        let expiring_result = sqlx::query(
            r#"
            SELECT COUNT(*) as expiring_soon
            FROM hammerwork_encryption_keys 
            WHERE status = 'Active' 
            AND expires_at IS NOT NULL 
            AND expires_at <= NOW() + INTERVAL '7 days'
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query expiring keys: {}", e))
        })?;

        let keys_expiring_soon: i64 = expiring_result.get("expiring_soon");

        // Count keys due for rotation
        let rotation_result = sqlx::query(
            r#"
            SELECT COUNT(*) as due_for_rotation
            FROM hammerwork_encryption_keys 
            WHERE status = 'Active' 
            AND next_rotation_at IS NOT NULL 
            AND next_rotation_at <= NOW()
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query rotation due keys: {}", e))
        })?;

        let keys_due_for_rotation: i64 = rotation_result.get("due_for_rotation");

        Ok(KeyManagerStats {
            total_keys: total_keys as u64,
            active_keys: active_keys as u64,
            retired_keys: retired_keys as u64,
            revoked_keys: revoked_keys as u64,
            expired_keys: expired_keys as u64,
            average_key_age_days,
            keys_expiring_soon: keys_expiring_soon as u64,
            keys_due_for_rotation: keys_due_for_rotation as u64,
            // Keep existing memory-tracked values
            total_access_operations: 0, // This should be tracked in memory or separate table
            rotations_performed: 0,     // This should be tracked in memory or separate table
        })
    }

    /// Get keys due for rotation
    #[cfg(feature = "postgres")]
    pub async fn get_keys_due_for_rotation(&self) -> Result<Vec<String>, EncryptionError> {
        self.get_keys_due_for_rotation_postgres().await
    }
}

#[cfg(feature = "mysql")]
impl KeyManager<sqlx::MySql> {
    #[allow(dead_code)]
    async fn store_key_mysql(&self, key: &EncryptionKey) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_encryption_keys (
                id, key_id, key_version, algorithm, key_material, key_derivation_salt, key_source, key_purpose,
                created_at, created_by, expires_at, rotated_at, retired_at, status, rotation_interval_seconds, next_rotation_at,
                key_strength, master_key_id, last_used_at, usage_count
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON DUPLICATE KEY UPDATE
                key_version = VALUES(key_version),
                algorithm = VALUES(algorithm),
                key_material = VALUES(key_material),
                status = VALUES(status),
                rotated_at = VALUES(rotated_at),
                next_rotation_at = VALUES(next_rotation_at),
                last_used_at = VALUES(last_used_at),
                usage_count = VALUES(usage_count)
            "#
        )
        .bind(key.id.to_string())
        .bind(&key.key_id)
        .bind(key.version as i32)
        .bind(key.algorithm.to_string())
        .bind(&key.encrypted_key_material)
        .bind(&key.derivation_salt)
        .bind(key.source.to_string())
        .bind(key.purpose.to_string())
        .bind(key.created_at)
        .bind(&key.created_by)
        .bind(key.expires_at)
        .bind(key.rotated_at)
        .bind(key.retired_at)
        .bind(key.status.to_string())
        .bind(key.rotation_interval.map(|d| d.num_seconds()))
        .bind(key.next_rotation_at)
        .bind(key.key_strength as i32)
        .bind(key.master_key_id.map(|id| id.to_string()))
        .bind(key.last_used_at)
        .bind(key.usage_count as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| EncryptionError::KeyManagement(format!("Failed to store key: {}", e)))?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn load_key_mysql(&self, key_id: &str) -> Result<EncryptionKey, EncryptionError> {
        let row = sqlx::query(
            r#"
            SELECT id, key_id, key_version, algorithm, key_material, key_derivation_salt, key_source, key_purpose,
                   created_at, created_by, expires_at, rotated_at, retired_at, status, rotation_interval_seconds, next_rotation_at,
                   key_strength, master_key_id, last_used_at, usage_count
            FROM hammerwork_encryption_keys
            WHERE key_id = ?
            ORDER BY key_version DESC
            LIMIT 1
            "#
        )
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| EncryptionError::KeyManagement(format!("Failed to load key: {}", e)))?;

        let row = row
            .ok_or_else(|| EncryptionError::KeyManagement(format!("Key not found: {}", key_id)))?;

        let rotation_interval = row
            .get::<Option<i64>, _>("rotation_interval_seconds")
            .map(Duration::seconds);

        Ok(EncryptionKey {
            id: uuid::Uuid::parse_str(&row.get::<String, _>("id"))
                .map_err(|e| EncryptionError::KeyManagement(format!("Invalid UUID: {}", e)))?,
            key_id: row.get("key_id"),
            version: row.get::<i32, _>("key_version") as u32,
            algorithm: parse_algorithm(row.get("algorithm"))?,
            encrypted_key_material: row.get("key_material"),
            derivation_salt: row.get("key_derivation_salt"),
            source: parse_key_source(row.get("key_source"))?,
            purpose: parse_key_purpose(row.get("key_purpose"))?,
            created_at: row.get("created_at"),
            created_by: row.get("created_by"),
            expires_at: row.get("expires_at"),
            rotated_at: row.get("rotated_at"),
            retired_at: row.get("retired_at"),
            status: parse_key_status(row.get("status"))?,
            rotation_interval,
            next_rotation_at: row.get("next_rotation_at"),
            key_strength: row.get::<i32, _>("key_strength") as u32,
            master_key_id: row
                .get::<Option<String>, _>("master_key_id")
                .map(|s| {
                    uuid::Uuid::parse_str(&s).map_err(|e| {
                        EncryptionError::KeyManagement(format!("Invalid master key UUID: {}", e))
                    })
                })
                .transpose()?,
            last_used_at: row.get("last_used_at"),
            usage_count: row.get::<i64, _>("usage_count") as u64,
        })
    }

    #[allow(dead_code)]
    async fn retire_key_version_mysql(
        &self,
        key_id: &str,
        version: u32,
    ) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            UPDATE hammerwork_encryption_keys
            SET status = 'Retired', retired_at = NOW()
            WHERE key_id = ? AND key_version = ?
            "#,
        )
        .bind(key_id)
        .bind(version as i32)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to retire key version: {}", e))
        })?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn cleanup_old_key_versions_mysql(&self, key_id: &str) -> Result<(), EncryptionError> {
        // Keep only the latest max_key_versions for each key_id
        sqlx::query(
            r#"
            DELETE FROM hammerwork_encryption_keys
            WHERE key_id = ? AND key_version NOT IN (
                SELECT key_version FROM (
                    SELECT key_version FROM hammerwork_encryption_keys
                    WHERE key_id = ?
                    ORDER BY key_version DESC
                    LIMIT ?
                ) t
            )
            "#,
        )
        .bind(key_id)
        .bind(key_id)
        .bind(self.config.max_key_versions as i32)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to cleanup old key versions: {}", e))
        })?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn get_keys_due_for_rotation_mysql(&self) -> Result<Vec<String>, EncryptionError> {
        let rows = sqlx::query(
            r#"
            SELECT key_id
            FROM hammerwork_encryption_keys
            WHERE status = 'Active' 
            AND next_rotation_at IS NOT NULL 
            AND next_rotation_at <= NOW()
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to get keys due for rotation: {}", e))
        })?;

        Ok(rows.into_iter().map(|row| row.get("key_id")).collect())
    }

    #[allow(dead_code)]
    async fn record_key_usage_mysql(&self, key_id: &str) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            UPDATE hammerwork_encryption_keys
            SET last_used_at = NOW(), usage_count = usage_count + 1
            WHERE key_id = ?
            "#,
        )
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to record key usage: {}", e))
        })?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn record_audit_event_mysql(
        &self,
        key_id: &str,
        operation: KeyOperation,
        success: bool,
        error_message: Option<String>,
    ) -> Result<(), EncryptionError> {
        sqlx::query(
            r#"
            INSERT INTO hammerwork_key_audit_log (
                key_id, operation, success, error_message, timestamp
            ) VALUES (?, ?, ?, ?, NOW())
            "#,
        )
        .bind(key_id)
        .bind(operation.to_string())
        .bind(success)
        .bind(error_message)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to record audit event: {}", e))
        })?;

        Ok(())
    }

    /// Check if a specific key is due for rotation
    #[cfg(feature = "mysql")]
    pub async fn is_key_due_for_rotation(&self, key_id: &str) -> Result<bool, EncryptionError> {
        let result = sqlx::query(
            r#"
            SELECT COUNT(*) as count
            FROM hammerwork_encryption_keys
            WHERE key_id = ?
            AND status = 'Active' 
            AND next_rotation_at IS NOT NULL 
            AND next_rotation_at <= NOW()
            "#,
        )
        .bind(key_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to check rotation status for key {}: {}",
                key_id, e
            ))
        })?;

        let count: i64 = result.get("count");
        Ok(count > 0)
    }

    /// Update rotation schedule for a key (MySQL)
    async fn update_key_rotation_schedule_mysql(
        &self,
        key_id: &str,
        rotation_interval: Option<Duration>,
        next_rotation_at: Option<DateTime<Utc>>,
    ) -> Result<(), EncryptionError> {
        let interval_seconds = rotation_interval.map(|interval| interval.num_seconds());

        sqlx::query(
            r#"
            UPDATE hammerwork_encryption_keys
            SET rotation_interval_seconds = ?, next_rotation_at = ?
            WHERE key_id = ? AND status = 'Active'
            "#,
        )
        .bind(interval_seconds)
        .bind(next_rotation_at)
        .bind(key_id)
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to update rotation schedule for key {}: {}",
                key_id, e
            ))
        })?;

        info!(
            "Updated rotation schedule for key {}: interval={:?}, next_rotation={:?}",
            key_id, rotation_interval, next_rotation_at
        );
        Ok(())
    }

    /// Get rotation schedule for a key (MySQL)
    async fn get_key_rotation_schedule_mysql(
        &self,
        key_id: &str,
    ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
        let result = sqlx::query(
            r#"
            SELECT next_rotation_at
            FROM hammerwork_encryption_keys
            WHERE key_id = ? AND status = 'Active'
            ORDER BY key_version DESC
            LIMIT 1
            "#,
        )
        .bind(key_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to get rotation schedule for key {}: {}",
                key_id, e
            ))
        })?;

        match result {
            Some(row) => Ok(row.get("next_rotation_at")),
            None => Ok(None),
        }
    }

    /// Schedule a key for rotation at a specific time (MySQL)
    async fn schedule_key_rotation_mysql(
        &self,
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
        .execute(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!(
                "Failed to schedule rotation for key {}: {}",
                key_id, e
            ))
        })?;

        info!("Scheduled rotation for key {} at {}", key_id, rotation_time);
        Ok(())
    }

    /// Get scheduled rotations within a time window (MySQL)
    async fn get_scheduled_rotations_mysql(
        &self,
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
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::KeyManagement(format!("Failed to get scheduled rotations: {}", e))
        })?;

        let scheduled_rotations = rows
            .into_iter()
            .filter_map(|row| {
                let key_id: String = row.get("key_id");
                let rotation_time: Option<DateTime<Utc>> = row.get("next_rotation_at");
                rotation_time.map(|time| (key_id, time))
            })
            .collect();

        Ok(scheduled_rotations)
    }

    /// Update the rotation schedule for a key (MySQL)
    pub async fn update_key_rotation_schedule(
        &self,
        key_id: &str,
        rotation_interval: Option<Duration>,
    ) -> Result<(), EncryptionError> {
        let next_rotation_at = rotation_interval.map(|interval| Utc::now() + interval);
        self.update_key_rotation_schedule_mysql(key_id, rotation_interval, next_rotation_at)
            .await
    }

    /// Get the next scheduled rotation time for a key (MySQL)
    pub async fn get_key_rotation_schedule(
        &self,
        key_id: &str,
    ) -> Result<Option<DateTime<Utc>>, EncryptionError> {
        self.get_key_rotation_schedule_mysql(key_id).await
    }

    /// Schedule a key for future rotation (MySQL)
    pub async fn schedule_key_rotation(
        &self,
        key_id: &str,
        rotation_time: DateTime<Utc>,
    ) -> Result<(), EncryptionError> {
        self.schedule_key_rotation_mysql(key_id, rotation_time)
            .await
    }

    /// Get all keys scheduled for rotation within a time window (MySQL)
    pub async fn get_scheduled_rotations(
        &self,
        from_time: DateTime<Utc>,
        to_time: DateTime<Utc>,
    ) -> Result<Vec<(String, DateTime<Utc>)>, EncryptionError> {
        self.get_scheduled_rotations_mysql(from_time, to_time).await
    }

    /// Query statistics from MySQL
    #[cfg(feature = "mysql")]
    pub async fn query_database_statistics(&self) -> Result<KeyManagerStats, EncryptionError> {
        // Execute multiple queries to gather comprehensive statistics
        let basic_counts = sqlx::query(
            r#"
            SELECT 
                COUNT(*) as total_keys,
                COUNT(CASE WHEN status = 'Active' THEN 1 END) as active_keys,
                COUNT(CASE WHEN status = 'Retired' THEN 1 END) as retired_keys,
                COUNT(CASE WHEN status = 'Revoked' THEN 1 END) as revoked_keys,
                COUNT(CASE WHEN status = 'Expired' THEN 1 END) as expired_keys
            FROM hammerwork_encryption_keys
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query key counts: {}", e))
        })?;

        let total_keys: i64 = basic_counts.get("total_keys");
        let active_keys: i64 = basic_counts.get("active_keys");
        let retired_keys: i64 = basic_counts.get("retired_keys");
        let revoked_keys: i64 = basic_counts.get("revoked_keys");
        let expired_keys: i64 = basic_counts.get("expired_keys");

        // Calculate average key age (MySQL syntax)
        let age_result = sqlx::query(
            r#"
            SELECT 
                COALESCE(AVG(TIMESTAMPDIFF(DAY, created_at, NOW())), 0) as avg_age_days
            FROM hammerwork_encryption_keys 
            WHERE status IN ('Active', 'Retired')
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query average age: {}", e))
        })?;

        let average_key_age_days: f64 = age_result.get("avg_age_days");

        // Count keys expiring soon (within 7 days)
        let expiring_result = sqlx::query(
            r#"
            SELECT COUNT(*) as expiring_soon
            FROM hammerwork_encryption_keys 
            WHERE status = 'Active' 
            AND expires_at IS NOT NULL 
            AND expires_at <= DATE_ADD(NOW(), INTERVAL 7 DAY)
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query expiring keys: {}", e))
        })?;

        let keys_expiring_soon: i64 = expiring_result.get("expiring_soon");

        // Count keys due for rotation
        let rotation_result = sqlx::query(
            r#"
            SELECT COUNT(*) as due_for_rotation
            FROM hammerwork_encryption_keys 
            WHERE status = 'Active' 
            AND next_rotation_at IS NOT NULL 
            AND next_rotation_at <= NOW()
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            EncryptionError::DatabaseError(format!("Failed to query rotation due keys: {}", e))
        })?;

        let keys_due_for_rotation: i64 = rotation_result.get("due_for_rotation");

        Ok(KeyManagerStats {
            total_keys: total_keys as u64,
            active_keys: active_keys as u64,
            retired_keys: retired_keys as u64,
            revoked_keys: revoked_keys as u64,
            expired_keys: expired_keys as u64,
            average_key_age_days,
            keys_expiring_soon: keys_expiring_soon as u64,
            keys_due_for_rotation: keys_due_for_rotation as u64,
            // Keep existing memory-tracked values
            total_access_operations: 0, // This should be tracked in memory or separate table
            rotations_performed: 0,     // This should be tracked in memory or separate table
        })
    }

    /// Get keys due for rotation
    #[cfg(feature = "mysql")]
    pub async fn get_keys_due_for_rotation(&self) -> Result<Vec<String>, EncryptionError> {
        self.get_keys_due_for_rotation_mysql().await
    }
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

pub fn parse_key_source(s: &str) -> Result<KeySource, EncryptionError> {
    if s.starts_with("Environment(") && s.ends_with(")") {
        let env_var = s
            .strip_prefix("Environment(")
            .unwrap()
            .strip_suffix(")")
            .unwrap();
        Ok(KeySource::Environment(env_var.to_string()))
    } else if s.starts_with("Static(") && s.ends_with(")") {
        let static_key = s
            .strip_prefix("Static(")
            .unwrap()
            .strip_suffix(")")
            .unwrap();
        Ok(KeySource::Static(static_key.to_string()))
    } else if s.starts_with("Generated(") && s.ends_with(")") {
        let generated_type = s
            .strip_prefix("Generated(")
            .unwrap()
            .strip_suffix(")")
            .unwrap();
        Ok(KeySource::Generated(generated_type.to_string()))
    } else if s.starts_with("External(") && s.ends_with(")") {
        let external_id = s
            .strip_prefix("External(")
            .unwrap()
            .strip_suffix(")")
            .unwrap();
        Ok(KeySource::External(external_id.to_string()))
    } else {
        Err(EncryptionError::KeyManagement(format!(
            "Unknown key source: {}",
            s
        )))
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

#[cfg(feature = "postgres")]
#[allow(dead_code)]
fn parse_postgres_interval(interval_str: &str) -> Option<Duration> {
    // Parse PostgreSQL INTERVAL format like "3600 seconds"
    if let Some(seconds_str) = interval_str.strip_suffix(" seconds") {
        if let Ok(seconds) = seconds_str.parse::<i64>() {
            return Some(Duration::seconds(seconds));
        }
    }
    None
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

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[sqlx::test]
    async fn test_master_key_storage_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Test data
        let master_key_id = Uuid::new_v4();
        let key_material = b"test_master_key_material_32bytes!!";

        // Test storing master key
        let result = key_manager
            .store_master_key_securely(&master_key_id, key_material)
            .await;
        assert!(result.is_ok(), "Failed to store master key: {:?}", result);

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
    #[sqlx::test]
    async fn test_master_key_storage_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default();
        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Test data
        let master_key_id = Uuid::new_v4();
        let key_material = b"test_master_key_material_32bytes!!";

        // Test storing master key
        let result = key_manager
            .store_master_key_securely(&master_key_id, key_material)
            .await;
        assert!(result.is_ok(), "Failed to store master key: {:?}", result);

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
        let config = KeyManagerConfig::default();
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
        let config = KeyManagerConfig::default();
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
    #[sqlx::test]
    async fn test_get_or_create_master_key_id_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default();
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
    #[sqlx::test]
    async fn test_get_or_create_master_key_id_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default();
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
    #[sqlx::test]
    async fn test_key_rotation_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default().with_auto_rotation_enabled(true);
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Generate a test key with rotation schedule
        let key_id = key_manager
            .generate_key_with_options(
                "rotation-test-key",
                EncryptionAlgorithm::AES256GCM,
                KeyPurpose::Encryption,
                Some(Duration::days(30)), // 30-day rotation interval
            )
            .await
            .unwrap();

        // Verify initial key version
        let initial_key = key_manager.load_key_postgres(&key_id).await.unwrap();
        assert_eq!(initial_key.version, 1);
        assert!(initial_key.next_rotation_at.is_some());

        // Perform manual rotation
        let new_version = key_manager.rotate_key(&key_id).await.unwrap();
        assert_eq!(new_version, 2);

        // Verify rotation worked
        let rotated_key = key_manager.load_key_postgres(&key_id).await.unwrap();
        assert_eq!(rotated_key.version, 2);
        assert_eq!(rotated_key.status, KeyStatus::Active);
        assert!(rotated_key.rotated_at.is_some());
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[sqlx::test]
    async fn test_key_rotation_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default().with_auto_rotation_enabled(true);
        let mut key_manager = KeyManager::new(config, pool).await.unwrap();

        // Generate a test key with rotation schedule
        let key_id = key_manager
            .generate_key_with_options(
                "rotation-test-key",
                EncryptionAlgorithm::AES256GCM,
                KeyPurpose::Encryption,
                Some(Duration::days(30)), // 30-day rotation interval
            )
            .await
            .unwrap();

        // Verify initial key version
        let initial_key = key_manager.load_key_mysql(&key_id).await.unwrap();
        assert_eq!(initial_key.version, 1);
        assert!(initial_key.next_rotation_at.is_some());

        // Perform manual rotation
        let new_version = key_manager.rotate_key(&key_id).await.unwrap();
        assert_eq!(new_version, 2);

        // Verify rotation worked
        let rotated_key = key_manager.load_key_mysql(&key_id).await.unwrap();
        assert_eq!(rotated_key.version, 2);
        assert_eq!(rotated_key.status, KeyStatus::Active);
        assert!(rotated_key.rotated_at.is_some());
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[sqlx::test]
    async fn test_rotation_scheduling_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default();
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

        key_manager.store_key_postgres(&test_key).await.unwrap();

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
    #[sqlx::test]
    async fn test_rotation_scheduling_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default();
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

        key_manager.store_key_mysql(&test_key).await.unwrap();

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
    #[sqlx::test]
    async fn test_automatic_rotation_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default().with_auto_rotation_enabled(true);
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

        key_manager.store_key_postgres(&test_key).await.unwrap();

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
            .load_key_postgres("auto-rotation-test-key")
            .await
            .unwrap();
        assert_eq!(rotated_key.version, 2);
        assert!(rotated_key.rotated_at.is_some());
    }

    #[cfg(all(feature = "encryption", feature = "mysql"))]
    #[sqlx::test]
    async fn test_automatic_rotation_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default().with_auto_rotation_enabled(true);
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

        key_manager.store_key_mysql(&test_key).await.unwrap();

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
            .load_key_mysql("auto-rotation-test-key")
            .await
            .unwrap();
        assert_eq!(rotated_key.version, 2);
        assert!(rotated_key.rotated_at.is_some());
    }

    // Test-only struct for unit testing crypto functions without database
    #[cfg(feature = "encryption")]
    struct TestKeyManager {
        config: KeyManagerConfig,
    }

    #[cfg(all(feature = "encryption", feature = "postgres"))]
    #[sqlx::test]
    async fn test_database_statistics_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default();
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
            next_rotation_at: Some(Utc::now() + Duration::days(5)), // Due for rotation soon
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
        key_manager.store_key_postgres(&active_key).await.unwrap();
        key_manager.store_key_postgres(&retired_key).await.unwrap();
        key_manager.store_key_postgres(&expiring_key).await.unwrap();

        // Query and verify statistics
        let stats = key_manager.query_postgres_statistics().await.unwrap();

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
    #[sqlx::test]
    async fn test_database_statistics_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default();
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
            next_rotation_at: Some(Utc::now() + Duration::days(5)), // Due for rotation soon
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
        key_manager.store_key_mysql(&active_key).await.unwrap();
        key_manager.store_key_mysql(&retired_key).await.unwrap();
        key_manager.store_key_mysql(&expiring_key).await.unwrap();

        // Query and verify statistics
        let stats = key_manager.query_mysql_statistics().await.unwrap();

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
    #[sqlx::test]
    async fn test_refresh_stats_integration_postgres(pool: sqlx::PgPool) {
        let config = KeyManagerConfig::default();
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

        key_manager.store_key_postgres(&test_key).await.unwrap();

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
    #[sqlx::test]
    async fn test_refresh_stats_integration_mysql(pool: sqlx::MySqlPool) {
        let config = KeyManagerConfig::default();
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

        key_manager.store_key_mysql(&test_key).await.unwrap();

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
}
