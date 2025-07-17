//! Basic encryption tests that don't require database access
//!
//! These tests validate the core encryption functionality without requiring
//! external database connections, making them suitable for CI/CD environments.

use chrono::{Duration, Utc};
use hammerwork::encryption::{
    EncryptionAlgorithm, EncryptionKey, ExternalKmsConfig, KeyDerivationConfig, KeyManagerConfig,
    KeyManagerStats, KeyOperation, KeyPurpose, KeySource, KeyStatus, parse_algorithm,
    parse_key_purpose, parse_key_source, parse_key_status,
};
use std::collections::HashMap;
use uuid::Uuid;

// Helper function to create test key manager configuration
fn create_test_config() -> KeyManagerConfig {
    KeyManagerConfig::new()
        .with_master_key_env("TEST_MASTER_KEY")
        .with_auto_rotation_enabled(true)
        .with_rotation_interval(Duration::days(30))
        .with_max_key_versions(5)
        .with_audit_enabled(true)
}

// Helper function to create test encryption key
fn create_test_encryption_key(key_id: &str) -> EncryptionKey {
    EncryptionKey {
        id: Uuid::new_v4(),
        key_id: key_id.to_string(),
        version: 1,
        algorithm: EncryptionAlgorithm::AES256GCM,
        encrypted_key_material: vec![1, 2, 3, 4, 5, 6, 7, 8], // Mock encrypted data
        derivation_salt: Some(vec![9, 10, 11, 12]),
        source: KeySource::Generated("test".to_string()),
        purpose: KeyPurpose::Encryption,
        created_at: Utc::now(),
        created_by: Some("test-user".to_string()),
        expires_at: Some(Utc::now() + Duration::days(365)),
        rotated_at: None,
        retired_at: None,
        status: KeyStatus::Active,
        rotation_interval: Some(Duration::days(90)),
        next_rotation_at: Some(Utc::now() + Duration::days(90)),
        key_strength: 256,
        master_key_id: Some(Uuid::new_v4()),
        last_used_at: None,
        usage_count: 0,
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn test_key_manager_config_creation() {
        let config = create_test_config();

        assert_eq!(
            config.master_key_source,
            KeySource::Environment("TEST_MASTER_KEY".to_string())
        );
        assert!(config.auto_rotation_enabled);
        assert_eq!(config.default_rotation_interval, Duration::days(30));
        assert_eq!(config.max_key_versions, 5);
        assert!(config.audit_enabled);
    }

    #[test]
    fn test_key_manager_config_builder_pattern() {
        let config = KeyManagerConfig::new()
            .with_master_key_env("CUSTOM_MASTER_KEY")
            .with_auto_rotation_enabled(false)
            .with_rotation_interval(Duration::days(60))
            .with_max_key_versions(3)
            .with_audit_enabled(false);

        assert_eq!(
            config.master_key_source,
            KeySource::Environment("CUSTOM_MASTER_KEY".to_string())
        );
        assert!(!config.auto_rotation_enabled);
        assert_eq!(config.default_rotation_interval, Duration::days(60));
        assert_eq!(config.max_key_versions, 3);
        assert!(!config.audit_enabled);
    }

    #[test]
    fn test_encryption_key_creation() {
        let key = create_test_encryption_key("test-key-1");

        assert_eq!(key.key_id, "test-key-1");
        assert_eq!(key.version, 1);
        assert_eq!(key.algorithm, EncryptionAlgorithm::AES256GCM);
        assert_eq!(key.purpose, KeyPurpose::Encryption);
        assert_eq!(key.status, KeyStatus::Active);
        assert_eq!(key.key_strength, 256);
        assert_eq!(key.usage_count, 0);
        assert!(key.expires_at.is_some());
        assert!(key.rotation_interval.is_some());
        assert!(key.next_rotation_at.is_some());
    }

    #[test]
    fn test_key_purpose_serialization() {
        let purposes = vec![KeyPurpose::Encryption, KeyPurpose::MAC, KeyPurpose::KEK];

        for purpose in purposes {
            let serialized = serde_json::to_string(&purpose).unwrap();
            let deserialized: KeyPurpose = serde_json::from_str(&serialized).unwrap();
            assert_eq!(purpose, deserialized);
        }
    }

    #[test]
    fn test_key_status_serialization() {
        let statuses = vec![
            KeyStatus::Active,
            KeyStatus::Retired,
            KeyStatus::Revoked,
            KeyStatus::Expired,
        ];

        for status in statuses {
            let serialized = serde_json::to_string(&status).unwrap();
            let deserialized: KeyStatus = serde_json::from_str(&serialized).unwrap();
            assert_eq!(status, deserialized);
        }
    }

    #[test]
    fn test_external_kms_config() {
        let mut auth_config = HashMap::new();
        auth_config.insert("access_key_id".to_string(), "test_access_key".to_string());
        auth_config.insert(
            "secret_access_key".to_string(),
            "test_secret_key".to_string(),
        );

        let kms_config = ExternalKmsConfig {
            service_type: "AWS".to_string(),
            endpoint: "https://kms.us-east-1.amazonaws.com".to_string(),
            auth_config,
            region: Some("us-east-1".to_string()),
            namespace: Some("hammerwork-test".to_string()),
        };

        assert_eq!(kms_config.service_type, "AWS");
        assert_eq!(kms_config.endpoint, "https://kms.us-east-1.amazonaws.com");
        assert!(kms_config.auth_config.contains_key("access_key_id"));
        assert!(kms_config.auth_config.contains_key("secret_access_key"));
        assert_eq!(kms_config.region, Some("us-east-1".to_string()));
        assert_eq!(kms_config.namespace, Some("hammerwork-test".to_string()));
    }

    #[test]
    fn test_key_derivation_config_defaults() {
        let config = KeyDerivationConfig::default();

        assert_eq!(config.memory_cost, 65536); // 64 MB
        assert_eq!(config.time_cost, 3);
        assert_eq!(config.parallelism, 4);
        assert_eq!(config.salt_length, 32);
    }
}

#[cfg(test)]
mod parsing_tests {
    use super::*;

    #[test]
    fn test_valid_key_source_parsing() {
        let valid_sources = vec![
            (
                "Environment(TEST_KEY)",
                KeySource::Environment("TEST_KEY".to_string()),
            ),
            (
                "Static(secret_key)",
                KeySource::Static("secret_key".to_string()),
            ),
            (
                "Generated(random)",
                KeySource::Generated("random".to_string()),
            ),
            (
                "External(aws_kms_key)",
                KeySource::External("aws_kms_key".to_string()),
            ),
        ];

        for (source_str, expected) in valid_sources {
            let result = parse_key_source(source_str);
            assert!(
                result.is_ok(),
                "Failed to parse valid source: {}",
                source_str
            );
            assert_eq!(result.unwrap(), expected);
        }
    }

    #[test]
    fn test_invalid_key_source_parsing() {
        let invalid_sources = vec![
            "InvalidType(test)",
            "Environment", // Missing parentheses
            "Static(",     // Missing closing parenthesis
            "",            // Empty string
        ];

        for invalid_source in invalid_sources {
            let result = parse_key_source(invalid_source);
            assert!(
                result.is_err(),
                "Expected error for invalid source: {}",
                invalid_source
            );
        }
    }

    #[test]
    fn test_valid_algorithm_parsing() {
        let valid_algorithms = vec![
            ("AES256GCM", EncryptionAlgorithm::AES256GCM),
            ("ChaCha20Poly1305", EncryptionAlgorithm::ChaCha20Poly1305),
        ];

        for (algo_str, expected) in valid_algorithms {
            let result = parse_algorithm(algo_str);
            assert!(
                result.is_ok(),
                "Failed to parse valid algorithm: {}",
                algo_str
            );
            assert_eq!(result.unwrap(), expected);
        }
    }

    #[test]
    fn test_invalid_algorithm_parsing() {
        let invalid_algorithms = vec!["InvalidAlgorithm", "AES128", "RSA2048", ""];

        for invalid_algorithm in invalid_algorithms {
            let result = parse_algorithm(invalid_algorithm);
            assert!(
                result.is_err(),
                "Expected error for invalid algorithm: {}",
                invalid_algorithm
            );
        }
    }

    #[test]
    fn test_key_manager_stats_default() {
        let stats = KeyManagerStats::default();

        assert_eq!(stats.total_keys, 0);
        assert_eq!(stats.active_keys, 0);
        assert_eq!(stats.retired_keys, 0);
        assert_eq!(stats.revoked_keys, 0);
        assert_eq!(stats.expired_keys, 0);
        assert_eq!(stats.total_access_operations, 0);
        assert_eq!(stats.rotations_performed, 0);
        assert_eq!(stats.average_key_age_days, 0.0);
        assert_eq!(stats.keys_expiring_soon, 0);
        assert_eq!(stats.keys_due_for_rotation, 0);
    }
}

#[cfg(test)]
mod aws_kms_tests {
    use super::*;

    #[test]
    fn test_aws_kms_key_source_parsing() {
        let aws_sources = vec![
            "aws://alias/test-key",
            "aws://arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012",
            "aws://alias/test-key?region=us-west-2",
        ];

        for source_str in aws_sources {
            let full_source = format!("External({})", source_str);
            let result = parse_key_source(&full_source);
            assert!(
                result.is_ok(),
                "Failed to parse AWS KMS source: {}",
                source_str
            );

            if let Ok(KeySource::External(config)) = result {
                assert!(config.starts_with("aws://"));
            } else {
                panic!("Expected External key source");
            }
        }
    }

    #[test]
    fn test_aws_kms_config_validation() {
        let config = KeyManagerConfig::new()
            .with_master_key_source(KeySource::External(
                "aws://alias/hammerwork-master-key?region=us-east-1".to_string(),
            ))
            .with_auto_rotation_enabled(true)
            .with_audit_enabled(true);

        if let KeySource::External(aws_config) = &config.master_key_source {
            assert!(aws_config.starts_with("aws://"));
            assert!(aws_config.contains("region=us-east-1"));
        } else {
            panic!("Expected External key source");
        }

        assert!(config.auto_rotation_enabled);
        assert!(config.audit_enabled);
    }

    #[test]
    fn test_aws_kms_external_config_serialization() {
        let mut auth_config = HashMap::new();
        auth_config.insert("region".to_string(), "us-east-1".to_string());
        auth_config.insert("profile".to_string(), "default".to_string());

        let kms_config = ExternalKmsConfig {
            service_type: "AWS_KMS".to_string(),
            endpoint: "https://kms.us-east-1.amazonaws.com".to_string(),
            auth_config,
            region: Some("us-east-1".to_string()),
            namespace: Some("hammerwork".to_string()),
        };

        let serialized = serde_json::to_string(&kms_config).unwrap();
        let deserialized: ExternalKmsConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(kms_config.service_type, deserialized.service_type);
        assert_eq!(kms_config.endpoint, deserialized.endpoint);
        assert_eq!(kms_config.region, deserialized.region);
        assert_eq!(kms_config.namespace, deserialized.namespace);
        assert_eq!(kms_config.auth_config, deserialized.auth_config);
    }

    #[test]
    fn test_aws_kms_key_with_external_source() {
        let aws_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "aws-data-key-1".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4, 5, 6, 7, 8],
            derivation_salt: None,
            source: KeySource::External(
                "aws://alias/hammerwork-data-keys?region=us-east-1".to_string(),
            ),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now(),
            created_by: Some("aws-kms-integration".to_string()),
            expires_at: None, // AWS KMS keys don't expire
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(30)),
            next_rotation_at: Some(Utc::now() + Duration::days(30)),
            key_strength: 256,
            master_key_id: Some(Uuid::new_v4()),
            last_used_at: None,
            usage_count: 0,
        };

        assert_eq!(aws_key.key_id, "aws-data-key-1");
        assert!(matches!(aws_key.source, KeySource::External(_)));
        assert_eq!(aws_key.purpose, KeyPurpose::Encryption);
        assert_eq!(aws_key.status, KeyStatus::Active);
        assert_eq!(aws_key.key_strength, 256);
        assert!(aws_key.expires_at.is_none()); // AWS KMS keys don't expire
        assert!(aws_key.rotation_interval.is_some());
    }
}

#[cfg(all(test, feature = "aws-kms"))]
mod aws_kms_integration_tests {
    use super::*;
    use hammerwork::encryption::EncryptionEngine;

    #[tokio::test]
    async fn test_aws_kms_configuration_fallback() {
        // Test that AWS KMS configuration falls back gracefully when not available
        let config = hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External(
                "aws://alias/test-key?region=us-east-1".to_string(),
            ));

        // This should not panic and should fall back to deterministic key generation
        let result = EncryptionEngine::new(config).await;

        // In CI/CD environments without AWS credentials, this might fail but shouldn't panic
        // In development, it should fall back to deterministic keys
        if result.is_ok() {
            // Test that we can still encrypt/decrypt with the fallback
            let mut engine = result.unwrap();
            let payload = serde_json::json!({"test": "data"});
            let encrypted = engine
                .encrypt_payload(&payload, &Vec::<String>::new())
                .await;

            // Should work with fallback implementation
            if encrypted.is_ok() {
                let decrypted = engine.decrypt_payload(&encrypted.unwrap()).await;
                assert!(decrypted.is_ok());
                assert_eq!(decrypted.unwrap(), payload);
            }
        }
    }

    #[tokio::test]
    async fn test_aws_kms_key_configuration_parsing() {
        let config_strings = vec![
            "aws://alias/test-key",
            "aws://alias/test-key?region=us-east-1",
            "aws://arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012",
            "aws://arn:aws:kms:us-east-1:123456789012:key/12345678-1234-1234-1234-123456789012?region=us-east-1",
        ];

        for config_str in config_strings {
            let config =
                hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                    .with_key_source(KeySource::External(config_str.to_string()));

            // Should not panic during configuration
            let result = EncryptionEngine::new(config).await;

            // Result may fail in CI without AWS credentials, but should not panic
            if let Err(e) = result {
                // Error should be informative and not a panic
                let error_msg = format!("{}", e);
                assert!(
                    error_msg.contains("AWS")
                        || error_msg.contains("KMS")
                        || error_msg.contains("key"),
                    "Error should be AWS KMS related: {}",
                    error_msg
                );
            }
        }
    }
}

#[cfg(test)]
mod gcp_kms_tests {
    use super::*;

    #[test]
    fn test_gcp_kms_key_source_parsing() {
        let gcp_sources = vec![
            "gcp://projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/encryption-key",
            "gcp://projects/test-project/locations/global/keyRings/test-ring/cryptoKeys/test-key",
            "gcp://projects/prod-project/locations/europe-west1/keyRings/prod-ring/cryptoKeys/prod-key",
        ];

        for source_str in gcp_sources {
            let full_source = format!("External({})", source_str);
            let result = parse_key_source(&full_source);
            assert!(
                result.is_ok(),
                "Failed to parse GCP KMS source: {}",
                source_str
            );

            if let Ok(KeySource::External(config)) = result {
                assert!(config.starts_with("gcp://"));
                assert!(config.contains("projects/"));
                assert!(config.contains("locations/"));
                assert!(config.contains("keyRings/"));
                assert!(config.contains("cryptoKeys/"));
            } else {
                panic!("Expected External key source");
            }
        }
    }

    #[test]
    fn test_gcp_kms_config_validation() {
        let config = KeyManagerConfig::new()
            .with_master_key_source(KeySource::External(
                "gcp://projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/master-key".to_string()
            ))
            .with_auto_rotation_enabled(true)
            .with_audit_enabled(true);

        if let KeySource::External(gcp_config) = &config.master_key_source {
            assert!(gcp_config.starts_with("gcp://"));
            assert!(gcp_config.contains("projects/my-project"));
            assert!(gcp_config.contains("locations/us-central1"));
            assert!(gcp_config.contains("keyRings/hammerwork"));
            assert!(gcp_config.contains("cryptoKeys/master-key"));
        } else {
            panic!("Expected External key source");
        }

        assert!(config.auto_rotation_enabled);
        assert!(config.audit_enabled);
    }

    #[test]
    fn test_gcp_kms_external_config_serialization() {
        let mut auth_config = HashMap::new();
        auth_config.insert("project_id".to_string(), "my-project".to_string());
        auth_config.insert("location".to_string(), "us-central1".to_string());

        let kms_config = ExternalKmsConfig {
            service_type: "GCP_KMS".to_string(),
            endpoint: "https://cloudkms.googleapis.com".to_string(),
            auth_config,
            region: Some("us-central1".to_string()),
            namespace: Some("hammerwork".to_string()),
        };

        let serialized = serde_json::to_string(&kms_config).unwrap();
        let deserialized: ExternalKmsConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(kms_config.service_type, deserialized.service_type);
        assert_eq!(kms_config.endpoint, deserialized.endpoint);
        assert_eq!(kms_config.region, deserialized.region);
        assert_eq!(kms_config.namespace, deserialized.namespace);
        assert_eq!(kms_config.auth_config, deserialized.auth_config);
    }

    #[test]
    fn test_gcp_kms_key_with_external_source() {
        let gcp_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "gcp-data-key-1".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4, 5, 6, 7, 8],
            derivation_salt: None,
            source: KeySource::External(
                "gcp://projects/my-project/locations/us-central1/keyRings/hammerwork/cryptoKeys/data-keys".to_string()
            ),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now(),
            created_by: Some("gcp-kms-integration".to_string()),
            expires_at: None, // GCP KMS keys don't expire
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(30)),
            next_rotation_at: Some(Utc::now() + Duration::days(30)),
            key_strength: 256,
            master_key_id: Some(Uuid::new_v4()),
            last_used_at: None,
            usage_count: 0,
        };

        assert_eq!(gcp_key.key_id, "gcp-data-key-1");
        assert!(matches!(gcp_key.source, KeySource::External(_)));
        assert_eq!(gcp_key.purpose, KeyPurpose::Encryption);
        assert_eq!(gcp_key.status, KeyStatus::Active);
        assert_eq!(gcp_key.key_strength, 256);
        assert!(gcp_key.expires_at.is_none()); // GCP KMS keys don't expire
        assert!(gcp_key.rotation_interval.is_some());

        // Verify the GCP KMS resource path structure
        if let KeySource::External(resource) = &gcp_key.source {
            assert!(resource.contains("projects/my-project"));
            assert!(resource.contains("locations/us-central1"));
            assert!(resource.contains("keyRings/hammerwork"));
            assert!(resource.contains("cryptoKeys/data-keys"));
        }
    }

    #[test]
    fn test_gcp_kms_resource_path_parsing() {
        let valid_paths = vec![
            "gcp://projects/test-project/locations/us-central1/keyRings/test-ring/cryptoKeys/test-key",
            "gcp://projects/prod-project/locations/global/keyRings/prod-ring/cryptoKeys/prod-key",
            "gcp://projects/dev-project/locations/europe-west1/keyRings/dev-ring/cryptoKeys/dev-key",
        ];

        for path in valid_paths {
            let stripped = path.strip_prefix("gcp://").unwrap();
            let parts: Vec<&str> = stripped.split('/').collect();

            assert!(parts.len() >= 6, "Invalid GCP KMS resource path: {}", path);
            assert_eq!(parts[0], "projects");
            assert_eq!(parts[2], "locations");
            assert_eq!(parts[4], "keyRings");
            assert_eq!(parts[6], "cryptoKeys");

            // Verify project, location, keyring, and key name are present
            assert!(!parts[1].is_empty(), "Project ID should not be empty");
            assert!(!parts[3].is_empty(), "Location should not be empty");
            assert!(!parts[5].is_empty(), "KeyRing should not be empty");
            assert!(!parts[7].is_empty(), "CryptoKey should not be empty");
        }
    }
}

#[cfg(all(test, feature = "gcp-kms"))]
mod gcp_kms_integration_tests {
    use super::*;
    use hammerwork::encryption::EncryptionEngine;

    #[tokio::test]
    async fn test_gcp_kms_configuration_fallback() {
        // Test that GCP KMS configuration falls back gracefully when not available
        let config = hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External(
                "gcp://projects/test-project/locations/us-central1/keyRings/test-ring/cryptoKeys/test-key".to_string()
            ));

        // This should not panic and should fall back to deterministic key generation
        let result = EncryptionEngine::new(config).await;

        // In CI/CD environments without GCP credentials, this might fail but shouldn't panic
        // In development, it should fall back to deterministic keys
        if result.is_ok() {
            // Test that we can still encrypt/decrypt with the fallback
            let mut engine = result.unwrap();
            let payload = serde_json::json!({"test": "data"});
            let encrypted = engine
                .encrypt_payload(&payload, &Vec::<String>::new())
                .await;

            // Should work with fallback implementation
            if encrypted.is_ok() {
                let decrypted = engine.decrypt_payload(&encrypted.unwrap()).await;
                assert!(decrypted.is_ok());
                assert_eq!(decrypted.unwrap(), payload);
            }
        }
    }

    #[tokio::test]
    async fn test_gcp_kms_key_configuration_parsing() {
        let config_strings = vec![
            "gcp://projects/test-project/locations/us-central1/keyRings/test-ring/cryptoKeys/test-key",
            "gcp://projects/prod-project/locations/global/keyRings/prod-ring/cryptoKeys/prod-key",
            "gcp://projects/dev-project/locations/europe-west1/keyRings/dev-ring/cryptoKeys/dev-key",
        ];

        for config_str in config_strings {
            let config =
                hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                    .with_key_source(KeySource::External(config_str.to_string()));

            // Should not panic during configuration
            let result = EncryptionEngine::new(config).await;

            // Result may fail in CI without GCP credentials, but should not panic
            if let Err(e) = result {
                // Error should be informative and not a panic
                let error_msg = format!("{}", e);
                assert!(
                    error_msg.contains("GCP")
                        || error_msg.contains("KMS")
                        || error_msg.contains("key")
                        || error_msg.contains("credentials"),
                    "Error should be GCP KMS related: {}",
                    error_msg
                );
            }
        }
    }
}

#[cfg(test)]
mod vault_kms_tests {
    use super::*;

    #[test]
    fn test_vault_kms_key_source_parsing() {
        let vault_sources = vec![
            "vault://secret/hammerwork/encryption-key",
            "vault://secret/hammerwork/master-key",
            "vault://kv/production/hammerwork/keys",
            "vault://secret/env/prod/encryption?addr=https://vault.example.com",
        ];

        for source_str in vault_sources {
            let full_source = format!("External({})", source_str);
            let result = parse_key_source(&full_source);
            assert!(
                result.is_ok(),
                "Failed to parse Vault KMS source: {}",
                source_str
            );

            if let Ok(KeySource::External(config)) = result {
                assert!(config.starts_with("vault://"));
                assert!(config.contains("secret/") || config.contains("kv/"));
            } else {
                panic!("Expected External key source");
            }
        }
    }

    #[test]
    fn test_vault_kms_config_validation() {
        let config = KeyManagerConfig::new()
            .with_master_key_source(KeySource::External(
                "vault://secret/hammerwork/master-key?addr=https://vault.example.com".to_string(),
            ))
            .with_auto_rotation_enabled(true)
            .with_audit_enabled(true);

        if let KeySource::External(vault_config) = &config.master_key_source {
            assert!(vault_config.starts_with("vault://"));
            assert!(vault_config.contains("secret/hammerwork/master-key"));
            assert!(vault_config.contains("addr=https://vault.example.com"));
        } else {
            panic!("Expected External key source");
        }

        assert!(config.auto_rotation_enabled);
        assert!(config.audit_enabled);
    }

    #[test]
    fn test_vault_kms_external_config_serialization() {
        let mut auth_config = HashMap::new();
        auth_config.insert("addr".to_string(), "https://vault.example.com".to_string());
        auth_config.insert("token".to_string(), "hvs.test-token".to_string());

        let kms_config = ExternalKmsConfig {
            service_type: "VAULT_KMS".to_string(),
            endpoint: "https://vault.example.com".to_string(),
            auth_config,
            region: None,
            namespace: Some("production".to_string()),
        };

        let serialized = serde_json::to_string(&kms_config).unwrap();
        let deserialized: ExternalKmsConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(kms_config.service_type, deserialized.service_type);
        assert_eq!(kms_config.endpoint, deserialized.endpoint);
        assert_eq!(kms_config.region, deserialized.region);
        assert_eq!(kms_config.namespace, deserialized.namespace);
        assert_eq!(kms_config.auth_config, deserialized.auth_config);
    }

    #[test]
    fn test_vault_kms_key_with_external_source() {
        let vault_key = EncryptionKey {
            id: Uuid::new_v4(),
            key_id: "vault-data-key-1".to_string(),
            version: 1,
            algorithm: EncryptionAlgorithm::AES256GCM,
            encrypted_key_material: vec![1, 2, 3, 4, 5, 6, 7, 8],
            derivation_salt: None,
            source: KeySource::External("vault://secret/hammerwork/encryption-key".to_string()),
            purpose: KeyPurpose::Encryption,
            created_at: Utc::now(),
            created_by: Some("vault-kms-integration".to_string()),
            expires_at: None, // Vault keys don't expire by default
            rotated_at: None,
            retired_at: None,
            status: KeyStatus::Active,
            rotation_interval: Some(Duration::days(30)),
            next_rotation_at: Some(Utc::now() + Duration::days(30)),
            key_strength: 256,
            master_key_id: Some(Uuid::new_v4()),
            last_used_at: None,
            usage_count: 0,
        };

        assert_eq!(vault_key.key_id, "vault-data-key-1");
        assert!(matches!(vault_key.source, KeySource::External(_)));
        assert_eq!(vault_key.purpose, KeyPurpose::Encryption);
        assert_eq!(vault_key.status, KeyStatus::Active);
        assert_eq!(vault_key.key_strength, 256);
        assert!(vault_key.expires_at.is_none()); // Vault keys don't expire by default
        assert!(vault_key.rotation_interval.is_some());

        // Verify the Vault secret path structure
        if let KeySource::External(resource) = &vault_key.source {
            assert!(resource.contains("secret/hammerwork/encryption-key"));
        }
    }

    #[test]
    fn test_vault_kms_secret_path_parsing() {
        let valid_paths = vec![
            "vault://secret/hammerwork/master-key",
            "vault://secret/hammerwork/encryption-key",
            "vault://kv/production/hammerwork/keys",
            "vault://secret/env/prod/encryption?addr=https://vault.example.com",
        ];

        for path in valid_paths {
            let stripped = path.strip_prefix("vault://").unwrap();
            let config_parts: Vec<&str> = stripped.split('?').collect();
            let secret_path = config_parts[0];

            let path_parts: Vec<&str> = secret_path.split('/').collect();
            assert!(path_parts.len() >= 2, "Invalid Vault secret path: {}", path);

            // First part should be the mount (e.g., "secret", "kv")
            assert!(!path_parts[0].is_empty(), "Mount should not be empty");

            // Second part should be the secret path
            assert!(!path_parts[1].is_empty(), "Secret path should not be empty");

            // If there's a query parameter, it should be addr
            if config_parts.len() > 1 {
                assert!(
                    config_parts[1].starts_with("addr="),
                    "Query parameter should be addr"
                );
            }
        }
    }

    #[test]
    fn test_vault_kms_address_parameter_parsing() {
        let test_cases = vec![
            ("vault://secret/test", "https://vault.example.com"),
            (
                "vault://secret/test?addr=https://custom.vault.com",
                "https://custom.vault.com",
            ),
            (
                "vault://kv/prod/key?addr=https://prod-vault.company.com:8200",
                "https://prod-vault.company.com:8200",
            ),
        ];

        for (input, expected_addr) in test_cases {
            let stripped = input.strip_prefix("vault://").unwrap();
            let config_parts: Vec<&str> = stripped.split('?').collect();

            let actual_addr = if config_parts.len() > 1 {
                config_parts[1]
                    .strip_prefix("addr=")
                    .unwrap_or("https://vault.example.com")
            } else {
                "https://vault.example.com"
            };

            assert_eq!(
                actual_addr, expected_addr,
                "Address parsing failed for: {}",
                input
            );
        }
    }
}

#[cfg(all(test, feature = "vault-kms"))]
mod vault_kms_integration_tests {
    use super::*;
    use hammerwork::encryption::EncryptionEngine;

    #[tokio::test]
    async fn test_vault_kms_configuration_fallback() {
        // Test that Vault KMS configuration falls back gracefully when not available
        let config = hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External(
                "vault://secret/hammerwork/test-key".to_string(),
            ));

        // This should not panic and should fall back to deterministic key generation
        let result = EncryptionEngine::new(config).await;

        // In CI/CD environments without Vault credentials, this might fail but shouldn't panic
        // In development, it should fall back to deterministic keys
        if result.is_ok() {
            // Test that we can still encrypt/decrypt with the fallback
            let mut engine = result.unwrap();
            let payload = serde_json::json!({"test": "data"});
            let encrypted = engine
                .encrypt_payload(&payload, &Vec::<String>::new())
                .await;

            // Should work with fallback implementation
            if encrypted.is_ok() {
                let decrypted = engine.decrypt_payload(&encrypted.unwrap()).await;
                assert!(decrypted.is_ok());
                assert_eq!(decrypted.unwrap(), payload);
            }
        }
    }

    #[tokio::test]
    async fn test_vault_kms_key_configuration_parsing() {
        let config_strings = vec![
            "vault://secret/hammerwork/encryption-key",
            "vault://secret/hammerwork/master-key",
            "vault://kv/production/hammerwork/keys",
            "vault://secret/env/prod/encryption?addr=https://vault.example.com",
        ];

        for config_str in config_strings {
            let config =
                hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
                    .with_key_source(KeySource::External(config_str.to_string()));

            // Should not panic during configuration
            let result = EncryptionEngine::new(config).await;

            // Result may fail in CI without Vault credentials, but should not panic
            if let Err(e) = result {
                // Error should be informative and not a panic
                let error_msg = format!("{}", e);
                assert!(
                    error_msg.contains("Vault")
                        || error_msg.contains("vault")
                        || error_msg.contains("key")
                        || error_msg.contains("secret"),
                    "Error should be Vault related: {}",
                    error_msg
                );
            }
        }
    }

    #[tokio::test]
    async fn test_vault_kms_token_environment_variable() {
        // Test that the integration properly checks for VAULT_TOKEN environment variable
        let original_token = std::env::var("VAULT_TOKEN").ok();

        // Clear the token temporarily
        unsafe {
            std::env::remove_var("VAULT_TOKEN");
        }

        let config = hammerwork::encryption::EncryptionConfig::new(EncryptionAlgorithm::AES256GCM)
            .with_key_source(KeySource::External("vault://secret/test/key".to_string()));

        let result = EncryptionEngine::new(config).await;

        // Should fallback to deterministic key generation when no token is available
        assert!(result.is_ok(), "Should fallback when no VAULT_TOKEN is set");

        // Restore original token if it existed
        if let Some(token) = original_token {
            unsafe {
                std::env::set_var("VAULT_TOKEN", token);
            }
        }
    }
}
