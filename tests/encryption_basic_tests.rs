//! Basic encryption tests that don't require database access
//! 
//! These tests validate the core encryption functionality without requiring
//! external database connections, making them suitable for CI/CD environments.

use hammerwork::encryption::{
    EncryptionAlgorithm, EncryptionKey, KeyManagerConfig, KeyOperation, KeyPurpose, KeySource,
    KeyStatus, parse_algorithm, parse_key_source, parse_key_purpose, parse_key_status,
    ExternalKmsConfig, KeyDerivationConfig, KeyManagerStats,
};
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::env;
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
        let purposes = vec![
            KeyPurpose::Encryption,
            KeyPurpose::MAC,
            KeyPurpose::KEK,
        ];
        
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
        auth_config.insert("secret_access_key".to_string(), "test_secret_key".to_string());
        
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
            ("Environment(TEST_KEY)", KeySource::Environment("TEST_KEY".to_string())),
            ("Static(secret_key)", KeySource::Static("secret_key".to_string())),
            ("Generated(random)", KeySource::Generated("random".to_string())),
            ("External(aws_kms_key)", KeySource::External("aws_kms_key".to_string())),
        ];
        
        for (source_str, expected) in valid_sources {
            let result = parse_key_source(source_str);
            assert!(result.is_ok(), "Failed to parse valid source: {}", source_str);
            assert_eq!(result.unwrap(), expected);
        }
    }

    #[test]
    fn test_invalid_key_source_parsing() {
        let invalid_sources = vec![
            "InvalidType(test)",
            "Environment",  // Missing parentheses
            "Static(",      // Missing closing parenthesis
            "",             // Empty string
        ];
        
        for invalid_source in invalid_sources {
            let result = parse_key_source(invalid_source);
            assert!(result.is_err(), "Expected error for invalid source: {}", invalid_source);
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
            assert!(result.is_ok(), "Failed to parse valid algorithm: {}", algo_str);
            assert_eq!(result.unwrap(), expected);
        }
    }

    #[test]
    fn test_invalid_algorithm_parsing() {
        let invalid_algorithms = vec![
            "InvalidAlgorithm",
            "AES128",
            "RSA2048",
            "",
        ];
        
        for invalid_algorithm in invalid_algorithms {
            let result = parse_algorithm(invalid_algorithm);
            assert!(result.is_err(), "Expected error for invalid algorithm: {}", invalid_algorithm);
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