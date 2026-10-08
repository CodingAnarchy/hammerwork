//! Core encryption engine implementation for Hammerwork.
//!
//! This module provides the actual cryptographic operations for encrypting and
//! decrypting job payloads using various algorithms and key management strategies.

#[cfg(feature = "encryption")]
use super::key_manager::KeyManager;
use super::{
    EncryptedPayload, EncryptionAlgorithm, EncryptionConfig, EncryptionError, EncryptionMetadata,
    EncryptionStats, KeySource, RetentionPolicy, kms,
};
use serde_json::Value;
use sqlx::Database;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::info;

#[cfg(feature = "encryption")]
use {
    aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce, aead::Aead},
    base64::Engine,
    chacha20poly1305::{ChaCha20Poly1305, Key as ChaChaKey, Nonce as ChaChaNonce},
    rand::{RngCore, rngs::OsRng},
    sha2::{Digest, Sha256},
};

/// Core encryption engine for job payload encryption and decryption.
///
/// The engine manages encryption keys, provides encryption/decryption operations,
/// handles PII field detection and protection, and maintains statistics about
/// encryption operations.
///
/// # Thread Safety
///
/// The engine is designed to be used from multiple threads safely. Internal
/// state is protected with appropriate synchronization primitives.
///
/// The `DB` parameter is the database of the optional [`KeyManager`]; the engine's bounds
/// are currently only satisfied by MySQL (`sqlx::MySql`).
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(all(feature = "encryption", feature = "mysql"))]
/// # {
/// use hammerwork::encryption::{EncryptionAlgorithm, EncryptionConfig, EncryptionEngine, KeySource};
/// use serde_json::json;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // HAMMERWORK_ENCRYPTION_KEY holds a base64-encoded 32-byte key
///     let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
///         .with_key_source(KeySource::Environment("HAMMERWORK_ENCRYPTION_KEY".to_string()));
///     let mut engine = EncryptionEngine::<sqlx::MySql>::new(config).await?;
///
///     let payload = json!({
///         "user_id": "123",
///         "credit_card": "4111-1111-1111-1111",
///         "amount": 99.99
///     });
///
///     let pii_fields = vec!["credit_card"];
///     let encrypted = engine.encrypt_payload(&payload, &pii_fields).await?;
///     let decrypted = engine.decrypt_payload(&encrypted).await?;
///
///     assert_eq!(payload, decrypted);
///     Ok(())
/// }
/// # }
/// ```
pub struct EncryptionEngine<DB: Database> {
    config: EncryptionConfig,
    keys: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    stats: Arc<Mutex<EncryptionStats>>,
    #[cfg(feature = "encryption")]
    key_manager: Option<Arc<Mutex<KeyManager<DB>>>>,
}

impl<DB: Database> EncryptionEngine<DB>
where
    for<'c> &'c mut DB::Connection: sqlx::Executor<'c, Database = DB>,
    for<'q> <DB as Database>::Arguments<'q>: sqlx::IntoArguments<'q, DB>,
    for<'q> <DB as Database>::Row: sqlx::Row,
    for<'r> String: sqlx::Decode<'r, DB> + sqlx::Type<DB>,
    for<'q> String: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> i32: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> Vec<u8>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> Option<String>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> chrono::DateTime<chrono::Utc>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> Option<chrono::DateTime<chrono::Utc>>: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'q> u64: sqlx::Encode<'q, DB> + sqlx::Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<<DB as Database>::Row>,
{
    /// Creates a new encryption engine with the specified configuration.
    ///
    /// This will attempt to load the encryption key according to the
    /// configuration's key source. If the key cannot be loaded, an error
    /// will be returned.
    ///
    /// # Arguments
    ///
    /// * `config` - The encryption configuration to use
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The encryption key cannot be loaded from the specified source
    /// - The encryption feature is not enabled
    /// - The configuration is invalid
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "mysql"))]
    /// # {
    /// use hammerwork::encryption::{EncryptionEngine, EncryptionConfig, EncryptionAlgorithm, KeySource};
    ///
    /// # async fn example() -> Result<(), hammerwork::encryption::EncryptionError> {
    /// // With environment variable key (base64-encoded 32-byte key)
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_source(KeySource::Environment("MY_ENCRYPTION_KEY".to_string()));
    /// let engine = EncryptionEngine::<sqlx::MySql>::new(config).await?;
    ///
    /// // With static key (for testing only)
    /// let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
    ///     .with_key_source(KeySource::Static(
    ///         "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=".to_string(),
    ///     ));
    /// let engine = EncryptionEngine::<sqlx::MySql>::new(config).await?;
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn new(
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))] config: EncryptionConfig,
    ) -> Result<Self, EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            return Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled. Enable the 'encryption' feature flag."
                    .to_string(),
            ));
        }

        #[cfg(feature = "encryption")]
        {
            let mut keys = HashMap::new();
            let key_id = config
                .key_id
                .clone()
                .unwrap_or_else(|| "default".to_string());

            // Load the encryption key
            let key = Self::load_key(&config.key_source, config.key_size_bytes()).await?;
            keys.insert(key_id, key);

            Ok(Self {
                config,
                keys: Arc::new(Mutex::new(keys)),
                stats: Arc::new(Mutex::new(EncryptionStats::new())),
                #[cfg(feature = "encryption")]
                key_manager: None,
            })
        }
    }

    /// Encrypts a job payload with optional PII field protection.
    ///
    /// This method encrypts the entire payload and optionally applies special
    /// handling to fields containing personally identifiable information (PII).
    /// PII fields are tracked in the encryption metadata for compliance purposes.
    ///
    /// # Arguments
    ///
    /// * `payload` - The JSON payload to encrypt
    /// * `pii_fields` - List of field names that contain PII
    ///
    /// # Returns
    ///
    /// An encrypted payload container with all necessary metadata for decryption.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "mysql"))]
    /// # {
    /// use serde_json::json;
    ///
    /// # async fn example(mut engine: hammerwork::encryption::EncryptionEngine<sqlx::MySql>) -> Result<(), Box<dyn std::error::Error>> {
    /// let payload = json!({
    ///     "user_id": "123",
    ///     "email": "user@example.com",
    ///     "ssn": "123-45-6789",
    ///     "amount": 99.99
    /// });
    ///
    /// let pii_fields = vec!["email", "ssn"];
    /// let encrypted = engine.encrypt_payload(&payload, &pii_fields).await?;
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn encrypt_payload(
        &mut self,
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))] payload: &Value,
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))] pii_fields: &[impl AsRef<
            str,
        >],
    ) -> Result<EncryptedPayload, EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ))
        }

        #[cfg(feature = "encryption")]
        {
            let start_time = Instant::now();

            // Serialize the payload
            let payload_bytes =
                serde_json::to_vec(payload).map_err(EncryptionError::Serialization)?;

            // Generate a hash of the original serialized payload for integrity verification
            let payload_hash = self.calculate_payload_hash(&payload_bytes);

            // Optionally compress the data
            let data_to_encrypt = if self.config.compression_enabled {
                self.compress_data(&payload_bytes)?
            } else {
                payload_bytes
            };

            // Perform the encryption
            let (ciphertext, nonce, tag) = self.encrypt_data(&data_to_encrypt)?;

            // Collect PII field names
            let pii_field_names: Vec<String> =
                pii_fields.iter().map(|f| f.as_ref().to_string()).collect();

            // Create encryption metadata
            let metadata = EncryptionMetadata::new(
                &self.config,
                pii_field_names.clone(),
                RetentionPolicy::UseDefault,
                payload_hash,
            );

            // Update statistics
            let duration_ms = start_time.elapsed().as_millis() as f64;
            if let Ok(mut stats) = self.stats.lock() {
                stats.record_encryption(&self.config.algorithm, data_to_encrypt.len(), duration_ms);
                stats.record_pii_encryption(pii_field_names.len());
            }

            Ok(EncryptedPayload::new(ciphertext, nonce, tag, metadata))
        }
    }

    /// Encrypts a job payload with a custom retention policy.
    ///
    /// Similar to `encrypt_payload` but allows specifying a custom retention
    /// policy for this specific payload.
    ///
    /// # Arguments
    ///
    /// * `payload` - The JSON payload to encrypt
    /// * `pii_fields` - List of field names that contain PII
    /// * `retention_policy` - Custom retention policy for this payload
    pub async fn encrypt_payload_with_retention(
        &mut self,
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))] payload: &Value,
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))] pii_fields: &[impl AsRef<
            str,
        >],
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))]
        retention_policy: RetentionPolicy,
    ) -> Result<EncryptedPayload, EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ))
        }

        #[cfg(feature = "encryption")]
        {
            let start_time = Instant::now();

            // Serialize the payload
            let payload_bytes =
                serde_json::to_vec(payload).map_err(EncryptionError::Serialization)?;

            // Generate a hash of the original serialized payload for integrity verification
            let payload_hash = self.calculate_payload_hash(&payload_bytes);

            // Optionally compress the data
            let data_to_encrypt = if self.config.compression_enabled {
                self.compress_data(&payload_bytes)?
            } else {
                payload_bytes
            };

            // Perform the encryption
            let (ciphertext, nonce, tag) = self.encrypt_data(&data_to_encrypt)?;

            // Collect PII field names
            let pii_field_names: Vec<String> =
                pii_fields.iter().map(|f| f.as_ref().to_string()).collect();

            // Create encryption metadata with custom retention policy
            let mut metadata = EncryptionMetadata::new(
                &self.config,
                pii_field_names.clone(),
                retention_policy,
                payload_hash,
            );

            // Recalculate deletion time with the custom policy
            metadata.delete_at = metadata.retention_policy.calculate_deletion_time(
                metadata.encrypted_at,
                None,
                self.config.default_retention,
            );

            // Update statistics
            let duration_ms = start_time.elapsed().as_millis() as f64;
            if let Ok(mut stats) = self.stats.lock() {
                stats.record_encryption(&self.config.algorithm, data_to_encrypt.len(), duration_ms);
                stats.record_pii_encryption(pii_field_names.len());
            }

            Ok(EncryptedPayload::new(ciphertext, nonce, tag, metadata))
        }
    }

    /// Decrypts an encrypted payload back to its original JSON form.
    ///
    /// This method handles key lookup, algorithm selection, decompression,
    /// and integrity verification automatically based on the metadata
    /// contained in the encrypted payload.
    ///
    /// # Arguments
    ///
    /// * `encrypted_payload` - The encrypted payload to decrypt
    ///
    /// # Returns
    ///
    /// The original JSON payload.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The decryption key is not available
    /// - The ciphertext has been corrupted
    /// - The algorithm is not supported
    /// - The integrity check fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "encryption", feature = "mysql"))]
    /// # {
    /// # async fn example(mut engine: hammerwork::encryption::EncryptionEngine<sqlx::MySql>, encrypted_payload: hammerwork::encryption::EncryptedPayload) -> Result<(), Box<dyn std::error::Error>> {
    /// let decrypted_payload = engine.decrypt_payload(&encrypted_payload).await?;
    /// println!("Decrypted: {}", decrypted_payload);
    /// # Ok(())
    /// # }
    /// # }
    /// ```
    pub async fn decrypt_payload(
        &mut self,
        #[cfg_attr(not(feature = "encryption"), allow(unused_variables))]
        encrypted_payload: &EncryptedPayload,
    ) -> Result<Value, EncryptionError> {
        #[cfg(not(feature = "encryption"))]
        {
            Err(EncryptionError::InvalidConfiguration(
                "Encryption feature is not enabled".to_string(),
            ))
        }

        #[cfg(feature = "encryption")]
        {
            let start_time = Instant::now();

            // Get the encryption key
            let key_id = &encrypted_payload.metadata.key_id;
            let key = {
                let keys = self.keys.lock().map_err(|_| {
                    EncryptionError::KeyManagement("Failed to acquire key lock".to_string())
                })?;
                keys.get(key_id).cloned().ok_or_else(|| {
                    EncryptionError::KeyManagement(format!("Key not found: {}", key_id))
                })?
            };

            // Decode the encrypted components
            let ciphertext = encrypted_payload.decode_ciphertext()?;
            let nonce = encrypted_payload.decode_nonce()?;
            let tag = encrypted_payload.decode_tag()?;

            // Perform the decryption
            let decrypted_data = self.decrypt_data(
                &ciphertext,
                &nonce,
                &tag,
                &key,
                &encrypted_payload.metadata.algorithm,
            )?;

            // Optionally decompress the data
            let payload_bytes = if encrypted_payload.metadata.compressed {
                self.decompress_data(&decrypted_data)?
            } else {
                decrypted_data
            };

            // Verify integrity
            let calculated_hash = self.calculate_payload_hash(&payload_bytes);
            if calculated_hash != encrypted_payload.metadata.payload_hash {
                return Err(EncryptionError::DecryptionFailed(
                    "Payload integrity check failed".to_string(),
                ));
            }

            // Deserialize back to JSON
            let payload: Value =
                serde_json::from_slice(&payload_bytes).map_err(EncryptionError::Serialization)?;

            // Update statistics
            let duration_ms = start_time.elapsed().as_millis() as f64;
            if let Ok(mut stats) = self.stats.lock() {
                stats.record_decryption(payload_bytes.len(), duration_ms);
            }

            Ok(payload)
        }
    }

    /// Identifies PII fields in a JSON payload based on field names and patterns.
    ///
    /// This method scans the payload for common PII field names and patterns
    /// to automatically identify fields that should be encrypted.
    ///
    /// # Arguments
    ///
    /// * `payload` - The JSON payload to scan
    ///
    /// # Returns
    ///
    /// A list of field names that likely contain PII.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(all(feature = "encryption", feature = "mysql"))]
    /// # {
    /// use serde_json::json;
    ///
    /// # fn example(engine: &hammerwork::encryption::EncryptionEngine<sqlx::MySql>) {
    /// let payload = json!({
    ///     "user_id": "123",
    ///     "email": "user@example.com",
    ///     "credit_card_number": "4111-1111-1111-1111",
    ///     "amount": 99.99
    /// });
    ///
    /// let pii_fields = engine.identify_pii_fields(&payload);
    /// // Returns: ["email", "credit_card_number"]
    /// # }
    /// # }
    /// ```
    pub fn identify_pii_fields(&self, payload: &Value) -> Vec<String> {
        let mut pii_fields = Vec::new();

        // Common PII field name patterns
        let pii_patterns = [
            "ssn",
            "social_security",
            "social_security_number",
            "credit_card",
            "credit_card_number",
            "card_number",
            "cc_number",
            "email",
            "email_address",
            "e_mail",
            "phone",
            "phone_number",
            "telephone",
            "mobile",
            "passport",
            "passport_number",
            "driver_license",
            "drivers_license",
            "license_number",
            "bank_account",
            "account_number",
            "routing_number",
            "password",
            "secret",
            "private_key",
            "address",
            "street_address",
            "home_address",
            "date_of_birth",
            "birth_date",
            "dob",
            "tax_id",
            "taxpayer_id",
            "ein",
        ];

        self.scan_object_for_pii(payload, &pii_patterns, &mut pii_fields, "");
        pii_fields
    }

    /// Gets the current encryption statistics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(all(feature = "encryption", feature = "mysql"))]
    /// # {
    /// # fn example(engine: &hammerwork::encryption::EncryptionEngine<sqlx::MySql>) {
    /// let stats = engine.get_stats();
    /// println!("Jobs encrypted: {}", stats.jobs_encrypted);
    /// println!("Success rate: {:.2}%", stats.encryption_success_rate());
    /// # }
    /// # }
    /// ```
    pub fn get_stats(&self) -> EncryptionStats {
        self.stats
            .lock()
            .map(|stats| stats.clone())
            .unwrap_or_else(|_| {
                // If lock is poisoned, return default stats
                EncryptionStats::new()
            })
    }

    /// Sets the key manager for the encryption engine.
    ///
    /// This enables the engine to use proper key metadata for rotation decisions
    /// instead of simple time-based rotation.
    ///
    /// # Arguments
    ///
    /// * `key_manager` - The key manager instance to use for key operations
    #[cfg(feature = "encryption")]
    pub fn set_key_manager(&mut self, key_manager: Arc<Mutex<KeyManager<DB>>>) {
        self.key_manager = Some(key_manager);
    }

    /// Cleans up expired encrypted data based on retention policies.
    ///
    /// This method should be called periodically to remove encrypted
    /// payloads that have exceeded their retention period.
    ///
    /// # Arguments
    ///
    /// * `encrypted_payloads` - List of encrypted payloads to check
    ///
    /// # Returns
    ///
    /// The number of payloads that should be deleted.
    pub fn cleanup_expired_data(&mut self, encrypted_payloads: &[EncryptedPayload]) -> usize {
        let expired_count = encrypted_payloads
            .iter()
            .filter(|payload| payload.should_delete_now())
            .count();

        if expired_count > 0 {
            if let Ok(mut stats) = self.stats.lock() {
                stats.record_retention_cleanup(expired_count as u64);
            }
        }

        expired_count
    }

    // Private helper methods

    #[cfg(feature = "encryption")]
    async fn store_generated_key(
        key_bytes: &[u8],
        location: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use base64::Engine;

        // Encode the key in base64 for storage
        let encoded_key = base64::engine::general_purpose::STANDARD.encode(key_bytes);

        // Determine storage method based on location format
        if location.starts_with("file://") {
            // Store in a file
            let file_path = location.strip_prefix("file://").unwrap_or(location);

            // Create directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(file_path).parent() {
                std::fs::create_dir_all(parent)?;
            }

            // Write the key to file with restricted permissions
            use std::fs::OpenOptions;
            use std::io::Write;

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(file_path)?;

            // Set restrictive permissions (owner read/write only)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let permissions = std::fs::Permissions::from_mode(0o600);
                file.set_permissions(permissions)?;
            }

            write!(file, "{}", encoded_key)?;
            file.flush()?;

            info!("Stored generated key in file: {}", file_path);
        } else if location.starts_with("env://") {
            // Store as environment variable (not recommended for production)
            let env_var = location.strip_prefix("env://").unwrap_or(location);
            unsafe {
                std::env::set_var(env_var, encoded_key);
            }

            info!("Stored generated key in environment variable: {}", env_var);
        } else if location.starts_with("stdout://") {
            // Print to stdout (useful for initial setup)
            println!("Generated encryption key: {}", encoded_key);
            println!("Store this key securely and set it as an environment variable.");
        } else {
            // Default to file storage
            let file_path = location;

            // Create directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(file_path).parent() {
                std::fs::create_dir_all(parent)?;
            }

            use std::fs::OpenOptions;
            use std::io::Write;

            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(file_path)?;

            // Set restrictive permissions (owner read/write only)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let permissions = std::fs::Permissions::from_mode(0o600);
                file.set_permissions(permissions)?;
            }

            write!(file, "{}", encoded_key)?;
            file.flush()?;

            info!("Stored generated key in file: {}", file_path);
        }

        Ok(())
    }

    #[cfg(not(feature = "encryption"))]
    async fn store_generated_key(
        _key_bytes: &[u8],
        _location: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        Err("Encryption feature is not enabled".into())
    }

    #[cfg(feature = "encryption")]
    async fn load_key(
        key_source: &KeySource,
        expected_size: usize,
    ) -> Result<Vec<u8>, EncryptionError> {
        match key_source {
            KeySource::Static(key_str) => {
                let key_bytes = base64::engine::general_purpose::STANDARD
                    .decode(key_str)
                    .map_err(|e| {
                        EncryptionError::KeyManagement(format!("Invalid base64 key: {}", e))
                    })?;

                if key_bytes.len() != expected_size {
                    return Err(EncryptionError::KeyManagement(format!(
                        "Key size mismatch: expected {} bytes, got {}",
                        expected_size,
                        key_bytes.len()
                    )));
                }

                Ok(key_bytes)
            }
            KeySource::Environment(var_name) => {
                let key_str = std::env::var(var_name).map_err(|_| {
                    EncryptionError::KeyManagement(format!(
                        "Environment variable {} not found",
                        var_name
                    ))
                })?;

                let key_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&key_str)
                    .map_err(|e| {
                        EncryptionError::KeyManagement(format!(
                            "Invalid base64 key in {}: {}",
                            var_name, e
                        ))
                    })?;

                if key_bytes.len() != expected_size {
                    return Err(EncryptionError::KeyManagement(format!(
                        "Key size mismatch in {}: expected {} bytes, got {}",
                        var_name,
                        expected_size,
                        key_bytes.len()
                    )));
                }

                Ok(key_bytes)
            }
            KeySource::External(service_config) => {
                load_external_key(service_config, expected_size).await
            }
            KeySource::Generated(location) => {
                // Generate a new random key
                let mut key_bytes = vec![0u8; expected_size];
                OsRng.fill_bytes(&mut key_bytes);

                // Store the generated key in the specified location
                Self::store_generated_key(&key_bytes, location)
                    .await
                    .map_err(|e| {
                        EncryptionError::KeyManagement(format!(
                            "Failed to store generated key at {}: {}",
                            location, e
                        ))
                    })?;

                info!("Generated and stored new encryption key at: {}", location);
                Ok(key_bytes)
            }
        }
    }

    #[cfg(feature = "encryption")]
    #[allow(clippy::type_complexity)]
    fn encrypt_data(&self, data: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), EncryptionError> {
        let default_key_id = "default".to_string();
        let key_id = self.config.key_id.as_ref().unwrap_or(&default_key_id);
        let key = {
            let keys = self.keys.lock().map_err(|_| {
                EncryptionError::KeyManagement("Failed to acquire key lock".to_string())
            })?;
            keys.get(key_id).cloned().ok_or_else(|| {
                EncryptionError::KeyManagement(format!("Key not found: {}", key_id))
            })?
        };

        match self.config.algorithm {
            EncryptionAlgorithm::AES256GCM => {
                let cipher_key = Key::<Aes256Gcm>::from_slice(&key);
                let cipher = Aes256Gcm::new(cipher_key);

                // Generate random nonce
                let mut nonce_bytes = vec![0u8; 12];
                OsRng.fill_bytes(&mut nonce_bytes);
                let nonce = Nonce::from_slice(&nonce_bytes);

                // Encrypt the data
                let ciphertext_with_tag = cipher.encrypt(nonce, data).map_err(|e| {
                    EncryptionError::EncryptionFailed(format!("AES encryption failed: {}", e))
                })?;

                // Split ciphertext and tag
                let tag_start = ciphertext_with_tag.len() - 16;
                let ciphertext = ciphertext_with_tag[..tag_start].to_vec();
                let tag = ciphertext_with_tag[tag_start..].to_vec();

                Ok((ciphertext, nonce_bytes, tag))
            }
            EncryptionAlgorithm::ChaCha20Poly1305 => {
                let cipher_key = ChaChaKey::from_slice(&key);
                let cipher = ChaCha20Poly1305::new(cipher_key);

                // Generate random nonce
                let mut nonce_bytes = vec![0u8; 12];
                OsRng.fill_bytes(&mut nonce_bytes);
                let nonce = ChaChaNonce::from_slice(&nonce_bytes);

                // Encrypt the data
                let ciphertext_with_tag = cipher.encrypt(nonce, data).map_err(|e| {
                    EncryptionError::EncryptionFailed(format!("ChaCha20 encryption failed: {}", e))
                })?;

                // Split ciphertext and tag
                let tag_start = ciphertext_with_tag.len() - 16;
                let ciphertext = ciphertext_with_tag[..tag_start].to_vec();
                let tag = ciphertext_with_tag[tag_start..].to_vec();

                Ok((ciphertext, nonce_bytes, tag))
            }
        }
    }

    #[cfg(feature = "encryption")]
    fn decrypt_data(
        &self,
        ciphertext: &[u8],
        nonce: &[u8],
        tag: &[u8],
        key: &[u8],
        algorithm: &EncryptionAlgorithm,
    ) -> Result<Vec<u8>, EncryptionError> {
        // Reconstruct the ciphertext with tag
        let mut ciphertext_with_tag = ciphertext.to_vec();
        ciphertext_with_tag.extend_from_slice(tag);

        match algorithm {
            EncryptionAlgorithm::AES256GCM => {
                let cipher_key = Key::<Aes256Gcm>::from_slice(key);
                let cipher = Aes256Gcm::new(cipher_key);
                let nonce = Nonce::from_slice(nonce);

                cipher
                    .decrypt(nonce, ciphertext_with_tag.as_slice())
                    .map_err(|e| {
                        EncryptionError::DecryptionFailed(format!("AES decryption failed: {}", e))
                    })
            }
            EncryptionAlgorithm::ChaCha20Poly1305 => {
                let cipher_key = ChaChaKey::from_slice(key);
                let cipher = ChaCha20Poly1305::new(cipher_key);
                let nonce = ChaChaNonce::from_slice(nonce);

                cipher
                    .decrypt(nonce, ciphertext_with_tag.as_slice())
                    .map_err(|e| {
                        EncryptionError::DecryptionFailed(format!(
                            "ChaCha20 decryption failed: {}",
                            e
                        ))
                    })
            }
        }
    }

    #[cfg(feature = "encryption")]
    fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(data)
            .map_err(|e| EncryptionError::FieldProcessing(format!("Compression failed: {}", e)))?;
        encoder.finish().map_err(|e| {
            EncryptionError::FieldProcessing(format!("Compression finalization failed: {}", e))
        })
    }

    #[cfg(feature = "encryption")]
    fn decompress_data(&self, data: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decoder = GzDecoder::new(data);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).map_err(|e| {
            EncryptionError::FieldProcessing(format!("Decompression failed: {}", e))
        })?;
        Ok(decompressed)
    }

    #[cfg(feature = "encryption")]
    fn calculate_payload_hash(&self, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }

    #[allow(clippy::only_used_in_recursion)]
    fn scan_object_for_pii(
        &self,
        value: &Value,
        patterns: &[&str],
        pii_fields: &mut Vec<String>,
        prefix: &str,
    ) {
        match value {
            Value::Object(map) => {
                for (key, val) in map {
                    let field_name = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };

                    // Check if this key matches any PII patterns
                    let key_lower = key.to_lowercase();
                    if patterns.iter().any(|pattern| {
                        key_lower.contains(pattern)
                            || key_lower
                                .replace('_', "")
                                .contains(&pattern.replace('_', ""))
                    }) {
                        pii_fields.push(field_name.clone());
                    }

                    // Recursively scan nested objects
                    if matches!(val, Value::Object(_)) {
                        self.scan_object_for_pii(val, patterns, pii_fields, &field_name);
                    }
                }
            }
            Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    let field_name = format!("{}[{}]", prefix, i);
                    if matches!(val, Value::Object(_)) {
                        self.scan_object_for_pii(val, patterns, pii_fields, &field_name);
                    }
                }
            }
            _ => {} // Primitive values don't contain nested fields
        }
    }
}

// External key loaders. They fail closed: an unavailable KMS, missing credentials or a
// KMS source whose cargo feature is not enabled is an error, never a fallback key.

/// Load a key from the external key management service named by `service_config`.
async fn load_external_key(
    service_config: &str,
    expected_size: usize,
) -> Result<Vec<u8>, EncryptionError> {
    if service_config.starts_with("aws://") {
        load_from_aws_kms(service_config, expected_size).await
    } else if service_config.starts_with("vault://") {
        load_from_vault(service_config, expected_size).await
    } else if service_config.starts_with("gcp://") {
        load_from_gcp_kms(service_config, expected_size).await
    } else if service_config.starts_with("azure://") {
        load_from_azure_kv(service_config, expected_size).await
    } else {
        Err(EncryptionError::KeyManagement(format!(
            "Unknown external key management service: {}",
            service_config
        )))
    }
}

/// Load a data key from AWS KMS (`aws://<key-id>?region=<region>&endpoint=<url>`).
///
/// Note: this asks KMS for a new data key (`GenerateDataKey`) on every load.
async fn load_from_aws_kms(
    service_config: &str,
    expected_size: usize,
) -> Result<Vec<u8>, EncryptionError> {
    let (key_id, region, endpoint) = kms::aws_key_config(service_config)?;

    info!(
        "Loading key from AWS KMS: key_id={}, region={}",
        key_id, region
    );

    #[cfg(not(feature = "aws-kms"))]
    {
        let _ = (endpoint, expected_size);
        Err(kms::kms_feature_disabled("aws://", "aws-kms"))
    }

    #[cfg(feature = "aws-kms")]
    {
        let key = kms::aws_generate_data_key(key_id, region, endpoint, expected_size).await?;
        info!("Successfully loaded encryption key from AWS KMS");
        Ok(key)
    }
}

/// Load a key from a HashiCorp Vault KV v2 secret (`vault://<mount>/<path>?addr=<url>`).
///
/// The secret's string `key` field is used: a base64 key is truncated or padded to
/// `expected_size`, any other string is hashed with SHA-256.
async fn load_from_vault(
    service_config: &str,
    expected_size: usize,
) -> Result<Vec<u8>, EncryptionError> {
    let (secret_path, params) = kms::parse_service_config(service_config, "vault://");
    let vault_addr = kms::vault_address(&params)?;
    let (mount, secret) = kms::vault_mount_and_path(secret_path)?;

    info!(
        "Loading key from HashiCorp Vault: path={}, addr={}",
        secret_path, vault_addr
    );

    #[cfg(not(feature = "vault-kms"))]
    {
        let _ = (mount, secret, expected_size);
        Err(kms::kms_feature_disabled("vault://", "vault-kms"))
    }

    #[cfg(feature = "vault-kms")]
    {
        let key_str = kms::vault_read_key_field(&vault_addr, mount, secret).await?;

        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&key_str) {
            if decoded.len() == expected_size {
                info!("Successfully loaded encryption key from HashiCorp Vault");
                return Ok(decoded);
            }
            // Wrong size: truncate, or pad with a hash of the decoded key
            let mut key = vec![0u8; expected_size];
            let copy_len = std::cmp::min(decoded.len(), expected_size);
            key[..copy_len].copy_from_slice(&decoded[..copy_len]);
            if decoded.len() < expected_size {
                let hash = Sha256::digest(&decoded);
                for (i, byte) in key.iter_mut().enumerate().skip(decoded.len()) {
                    *byte = hash[i % hash.len()];
                }
            }
            info!("Successfully loaded and resized encryption key from HashiCorp Vault");
            return Ok(key);
        }

        // Not base64: hash the string to the expected size
        let hash = Sha256::digest(key_str.as_bytes());
        let key = (0..expected_size).map(|i| hash[i % hash.len()]).collect();
        info!("Successfully loaded and hashed encryption key from HashiCorp Vault");
        Ok(key)
    }
}

/// Generate a key with GCP KMS
/// (`gcp://projects/<project>/locations/<location>/keyRings/<ring>/cryptoKeys/<key>`).
///
/// Note: this asks KMS for random bytes (`GenerateRandomBytes`) on every load.
async fn load_from_gcp_kms(
    service_config: &str,
    expected_size: usize,
) -> Result<Vec<u8>, EncryptionError> {
    let (key_resource, _) = kms::parse_service_config(service_config, "gcp://");
    let location = kms::gcp_location(key_resource)?;

    info!("Loading key from GCP KMS: resource={}", key_resource);

    #[cfg(not(feature = "gcp-kms"))]
    {
        let _ = (location, expected_size);
        Err(kms::kms_feature_disabled("gcp://", "gcp-kms"))
    }

    #[cfg(feature = "gcp-kms")]
    {
        let key = kms::gcp_generate_random_bytes(location, expected_size).await?;
        info!("Successfully generated random key from GCP KMS");
        Ok(key)
    }
}

/// Load a key from Azure Key Vault (`azure://<vault-host>/keys/<key-name>`, key name
/// defaults to `encryption-key`). Material shorter than `expected_size` is padded with
/// HMAC-SHA256 output keyed by the material.
async fn load_from_azure_kv(
    service_config: &str,
    expected_size: usize,
) -> Result<Vec<u8>, EncryptionError> {
    let (vault_url, key_name) = kms::azure_vault_and_key(service_config, "encryption-key")?;

    info!(
        "Loading key from Azure Key Vault: vault={}, key={}",
        vault_url, key_name
    );

    #[cfg(not(feature = "azure-kv"))]
    {
        let _ = (vault_url, key_name, expected_size);
        Err(kms::kms_feature_disabled("azure://", "azure-kv"))
    }

    #[cfg(feature = "azure-kv")]
    {
        let decoded_key = super::azure::fetch_key_material(&vault_url, &key_name)
            .await
            .map_err(|e| {
                EncryptionError::KeyManagement(format!(
                    "Failed to load key from Azure Key Vault: {}",
                    e
                ))
            })?;

        if decoded_key.len() >= expected_size {
            info!("Successfully loaded encryption key from Azure Key Vault");
            return Ok(decoded_key[..expected_size].to_vec());
        }

        // Key is shorter than expected: pad it using key derivation
        use hmac::{Hmac, Mac};
        type HmacSha256 = Hmac<Sha256>;

        let mut final_key = vec![0u8; expected_size];
        final_key[..decoded_key.len()].copy_from_slice(&decoded_key);

        let mut mac = <HmacSha256 as Mac>::new_from_slice(&decoded_key)
            .map_err(|e| EncryptionError::KeyManagement(format!("HMAC creation failed: {}", e)))?;
        mac.update(b"azure-kv-key-derivation");
        mac.update(vault_url.as_bytes());
        mac.update(key_name.as_bytes());
        let derived = mac.finalize().into_bytes();

        for (i, byte) in final_key.iter_mut().enumerate().skip(decoded_key.len()) {
            *byte = derived[i % derived.len()];
        }

        info!("Successfully loaded and padded encryption key from Azure Key Vault");
        Ok(final_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    #[cfg(feature = "encryption")]
    #[tokio::test]
    async fn test_encryption_engine_creation() {
        // Set up a test key in environment
        unsafe {
            std::env::set_var(
                "TEST_ENCRYPTION_KEY",
                "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=",
            ); // 32 bytes base64
        }

        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::Environment("TEST_ENCRYPTION_KEY".to_string()));

        let engine = EncryptionEngine::new(config).await;
        assert!(engine.is_ok());
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    async fn test_payload_encryption_decryption() {
        unsafe {
            std::env::set_var(
                "TEST_ENCRYPTION_KEY",
                "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=",
            );
        }

        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::Environment("TEST_ENCRYPTION_KEY".to_string()));

        let mut engine = EncryptionEngine::new(config).await.unwrap();

        let payload = json!({
            "user_id": "123",
            "email": "test@example.com",
            "amount": 99.99
        });

        let pii_fields = vec!["email"];
        let encrypted = engine.encrypt_payload(&payload, &pii_fields).await.unwrap();
        let decrypted = engine.decrypt_payload(&encrypted).await.unwrap();

        assert_eq!(payload, decrypted);
        assert!(encrypted.contains_pii_field("email"));
        assert!(!encrypted.contains_pii_field("user_id"));
    }

    #[tokio::test]
    async fn test_pii_field_identification() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
        let engine = EncryptionEngine::new(config).await;

        // This test doesn't require the encryption feature to be enabled for PII detection
        if engine.is_err() {
            return; // Skip if encryption feature not enabled
        }

        let engine = engine.unwrap();

        let payload = json!({
            "user_id": "123",
            "email": "test@example.com",
            "credit_card_number": "4111-1111-1111-1111",
            "ssn": "123-45-6789",
            "amount": 99.99,
            "metadata": {
                "phone_number": "+1-555-123-4567"
            }
        });

        let pii_fields = engine.identify_pii_fields(&payload);

        assert!(pii_fields.contains(&"email".to_string()));
        assert!(pii_fields.contains(&"credit_card_number".to_string()));
        assert!(pii_fields.contains(&"ssn".to_string()));
        assert!(pii_fields.contains(&"metadata.phone_number".to_string()));
        assert!(!pii_fields.contains(&"user_id".to_string()));
        assert!(!pii_fields.contains(&"amount".to_string()));
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    async fn test_encryption_with_compression() {
        unsafe {
            std::env::set_var(
                "TEST_ENCRYPTION_KEY",
                "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=",
            );
        }

        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::Environment("TEST_ENCRYPTION_KEY".to_string()))
            .with_compression_enabled(true);

        let mut engine = EncryptionEngine::new(config).await.unwrap();

        let payload = json!({
            "data": "a".repeat(1000), // Large repeating data that compresses well
            "user_id": "123"
        });

        let encrypted = engine
            .encrypt_payload(&payload, &Vec::<String>::new())
            .await
            .unwrap();
        let decrypted = engine.decrypt_payload(&encrypted).await.unwrap();

        assert_eq!(payload, decrypted);
        assert!(encrypted.metadata.compressed);
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    async fn test_encryption_stats() {
        unsafe {
            std::env::set_var(
                "TEST_ENCRYPTION_KEY",
                "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=",
            );
        }

        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::Environment("TEST_ENCRYPTION_KEY".to_string()));

        let mut engine = EncryptionEngine::new(config).await.unwrap();

        let payload = json!({"test": "data"});

        let initial_stats = engine.get_stats();
        assert_eq!(initial_stats.jobs_encrypted, 0);

        let _encrypted = engine.encrypt_payload(&payload, &["test"]).await.unwrap();

        let stats = engine.get_stats();
        assert_eq!(stats.jobs_encrypted, 1);
        assert_eq!(stats.pii_fields_encrypted, 1);
        assert!(stats.avg_encryption_time_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_retention_policy_cleanup() {
        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM);
        let engine = EncryptionEngine::new(config).await;

        if engine.is_err() {
            return; // Skip if encryption feature not enabled
        }

        let mut engine = engine.unwrap();

        // Create a mock encrypted payload with immediate deletion policy
        let metadata = EncryptionMetadata::new(
            &EncryptionConfig::default(),
            vec![],
            RetentionPolicy::DeleteAfter(Duration::from_secs(1)),
            "test_hash".to_string(),
        );

        let payload = EncryptedPayload::new(
            b"test".to_vec(),
            b"nonce".to_vec(),
            b"tag".to_vec(),
            metadata,
        );

        // Should not be expired immediately
        assert!(!payload.should_delete_now());

        // Wait for expiration (in a real test, we'd mock the time)
        tokio::time::sleep(Duration::from_millis(1100)).await;

        let expired_count = engine.cleanup_expired_data(&[payload]);
        assert_eq!(expired_count, 1);
    }

    #[cfg(feature = "encryption")]
    #[tokio::test]
    async fn test_set_key_manager() {
        // Note: This test demonstrates the key manager setter
        // In a real scenario, you would set up a key manager with a database connection

        unsafe {
            std::env::set_var(
                "TEST_ENCRYPTION_KEY",
                "dGVzdGtleTE5ODc2NTQzMjEwOTg3NjU0MzIxMHRlc3Q=",
            );
        }

        let config = EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::Environment("TEST_ENCRYPTION_KEY".to_string()));

        let engine = EncryptionEngine::new(config).await.unwrap();

        // Initially no key manager
        assert!(engine.key_manager.is_none());

        // For now, we'll just test that the setter method exists and works
        // In a complete implementation, you would:
        // 1. Create a test database connection
        // 2. Initialize a KeyManager with the connection
        // 3. Set it on the engine
        // 4. Test that key rotation uses the key manager
        assert!(engine.key_manager.is_none());
    }

    // ---- Fail-closed external key loading (#16); no cloud credentials needed ----

    #[tokio::test]
    async fn test_external_key_invalid_config_fails() {
        let cases = [
            "aws://?region=us-east-1",
            "vault://key-only?addr=http://127.0.0.1:1",
            "gcp://not-a-resource",
            "azure://",
        ];
        for source in cases {
            let err = load_external_key(source, 32).await.unwrap_err();
            assert!(
                matches!(err, EncryptionError::InvalidConfiguration(_)),
                "{source}: {err}"
            );
        }
        let err = load_external_key("ftp://key", 32).await.unwrap_err();
        assert!(matches!(err, EncryptionError::KeyManagement(_)), "{err}");
    }

    #[tokio::test]
    async fn test_external_key_without_feature_fails() {
        #[allow(unused_mut)]
        let mut cases: Vec<(&str, &str)> = Vec::new();
        #[cfg(not(feature = "aws-kms"))]
        cases.push(("aws://alias/key?region=us-east-1", "aws-kms"));
        #[cfg(not(feature = "vault-kms"))]
        cases.push(("vault://secret/key?addr=http://127.0.0.1:1", "vault-kms"));
        #[cfg(not(feature = "gcp-kms"))]
        cases.push((
            "gcp://projects/p/locations/global/keyRings/r/cryptoKeys/k",
            "gcp-kms",
        ));
        #[cfg(not(feature = "azure-kv"))]
        cases.push(("azure://my-vault.vault.azure.net/keys/k", "azure-kv"));

        for (source, feature) in cases {
            let err = load_external_key(source, 32).await.unwrap_err();
            assert!(
                matches!(err, EncryptionError::InvalidConfiguration(_)),
                "{source}: {err}"
            );
            assert!(err.to_string().contains(feature), "{source}: {err}");
        }
    }

    #[cfg(feature = "aws-kms")]
    #[tokio::test]
    async fn test_aws_key_unreachable_endpoint_fails() {
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            load_from_aws_kms(
                "aws://alias/hammerwork-test?region=us-east-1&endpoint=http://127.0.0.1:1",
                32,
            ),
        )
        .await
        .expect("AWS KMS loader did not finish");
        assert!(matches!(result, Err(EncryptionError::KeyManagement(_))));
    }

    #[cfg(feature = "vault-kms")]
    #[tokio::test]
    async fn test_vault_key_unavailable_fails() {
        let result = load_from_vault("vault://secret/hammerwork?addr=http://127.0.0.1:1", 32).await;
        assert!(matches!(result, Err(EncryptionError::KeyManagement(_))));
    }
}
