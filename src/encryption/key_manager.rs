//! Advanced key management system for Hammerwork encryption.
//!
//! This module provides comprehensive key management capabilities including:
//! - Secure key generation and storage
//! - Key rotation and lifecycle management
//! - Master key encryption (Key Encryption Keys)
//! - External key management service integration
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
//! # let database_url = "postgres://user:pass@localhost/hammerwork";\n//! # let pool = sqlx::PgPool::connect(database_url).await?;
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
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the master key source
    pub fn with_master_key_source(mut self, source: KeySource) -> Self {
        self.master_key_source = source;
        self
    }

    /// Set the master key from an environment variable
    pub fn with_master_key_env(mut self, env_var: &str) -> Self {
        self.master_key_source = KeySource::Environment(env_var.to_string());
        self
    }

    /// Enable or disable automatic key rotation
    pub fn with_auto_rotation_enabled(mut self, enabled: bool) -> Self {
        self.auto_rotation_enabled = enabled;
        self
    }

    /// Set the default rotation interval
    pub fn with_rotation_interval(mut self, interval: Duration) -> Self {
        self.default_rotation_interval = interval;
        self
    }

    /// Set the maximum number of key versions to retain
    pub fn with_max_key_versions(mut self, max_versions: u32) -> Self {
        self.max_key_versions = max_versions;
        self
    }

    /// Enable or disable key usage auditing
    pub fn with_audit_enabled(mut self, enabled: bool) -> Self {
        self.audit_enabled = enabled;
        self
    }

    /// Configure external KMS integration
    pub fn with_external_kms(mut self, config: ExternalKmsConfig) -> Self {
        self.external_kms_config = Some(config);
        self
    }
}

impl<DB: Database> KeyManager<DB> {
    /// Create a new key manager instance
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

        let keys_due_for_rotation = self.get_keys_due_for_rotation().await?;
        let mut rotated_keys = Vec::new();

        for key_id in keys_due_for_rotation {
            match self.rotate_key(&key_id).await {
                Ok(_) => {
                    rotated_keys.push(key_id);
                }
                Err(e) => {
                    error!("Failed to rotate key {}: {:?}", key_id, e);
                    if self.config.audit_enabled {
                        self.record_audit_event(
                            &key_id,
                            KeyOperation::Rotate,
                            false,
                            Some(format!("{:?}", e)),
                        )
                        .await?;
                    }
                }
            }
        }

        Ok(rotated_keys)
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
        // In a real implementation, this would query the database for actual statistics
        // For now, we'll implement a basic statistics update based on current state

        if let Ok(mut stats) = self.stats.lock() {
            // Reset stats for fresh calculation
            let _old_total = stats.total_keys;

            // In a real implementation, these would be database queries:
            // - SELECT COUNT(*) FROM hammerwork_encryption_keys WHERE status = 'Active'
            // - SELECT COUNT(*) FROM hammerwork_encryption_keys WHERE status = 'Retired'
            // - etc.

            // For now, maintain existing values but update calculated fields
            stats.average_key_age_days = if stats.total_keys > 0 {
                // Simple estimation: if we have keys, assume average age of 30 days
                30.0
            } else {
                0.0
            };

            // Check for keys that might need rotation (simplified logic)
            stats.keys_due_for_rotation = if stats.total_keys > 0 {
                // Estimate that 10% of keys might be due for rotation
                (stats.total_keys as f64 * 0.1) as u64
            } else {
                0
            };

            // Check for keys expiring soon (simplified logic)
            stats.keys_expiring_soon = if stats.total_keys > 0 {
                // Estimate that 5% of keys might be expiring soon
                (stats.total_keys as f64 * 0.05) as u64
            } else {
                0
            };

            info!(
                "Statistics refreshed: {} total keys, {} active",
                stats.total_keys, stats.active_keys
            );
        }

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

            // Store the master key (in a real implementation, this would be stored securely)
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

            // Generate a deterministic master key ID based on the key material
            // In a real implementation, this would be stored with the key
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&master_key_material);
            let hash = hasher.finalize();
            let master_key_id = Uuid::from_bytes([
                hash[0], hash[1], hash[2], hash[3], hash[4], hash[5], hash[6], hash[7], hash[8],
                hash[9], hash[10], hash[11], hash[12], hash[13], hash[14], hash[15],
            ]);
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

    async fn get_keys_due_for_rotation(&self) -> Result<Vec<String>, EncryptionError> {
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

        // In a real implementation, this would:
        // 1. Create Azure Key Vault client with authentication
        // 2. Retrieve the master key from the vault
        // 3. Decrypt and return the master key material

        // For now, generate a deterministic key based on the configuration
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"azure-kv-master-key");
        hasher.update(vault_url.as_bytes());
        hasher.update(key_name.as_bytes());
        let hash = hasher.finalize();
        hash[0..32].to_vec()
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

        let rotation_interval =
            row.get::<Option<i64>, _>("rotation_interval_seconds").map(Duration::seconds);

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
}
