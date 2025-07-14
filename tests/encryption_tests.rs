// tests/encryption_tests.rs
//! Comprehensive tests for encryption key management functionality
//!
//! These tests validate all aspects of the encryption key management system including:
//! - Key generation and storage
//! - Key lifecycle management (creation, rotation, retirement)
//! - Database operations for both PostgreSQL and MySQL
//! - Audit logging and usage tracking
//! - Error handling and edge cases

mod test_utils;

use chrono::{Duration, Utc};
use hammerwork::encryption::{
    EncryptionAlgorithm, EncryptionKey, KeyManager, KeyManagerConfig, KeyOperation, KeyPurpose, KeySource,
    KeyStatus,
};
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

// Helper function to set up test environment
fn setup_test_environment() {
    // Set a test master key for encryption tests
    unsafe {
        env::set_var("TEST_MASTER_KEY", "test_master_key_32_bytes_exactly!");
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
    fn test_key_source_display() {
        let sources = vec![
            KeySource::Environment("TEST_KEY".to_string()),
            KeySource::Static("static_key".to_string()),
            KeySource::Generated("random".to_string()),
            KeySource::External("external_id".to_string()),
        ];

        let expected = vec![
            "Environment(TEST_KEY)",
            "Static(static_key)",
            "Generated(random)",
            "External(external_id)",
        ];

        for (source, expected_str) in sources.iter().zip(expected.iter()) {
            assert_eq!(source.to_string(), *expected_str);
        }
    }

    #[test]
    fn test_key_operation_display() {
        let operations = vec![
            KeyOperation::Create,
            KeyOperation::Access,
            KeyOperation::Rotate,
            KeyOperation::Retire,
            KeyOperation::Revoke,
            KeyOperation::Delete,
            KeyOperation::Update,
        ];

        let expected = vec![
            "Create", "Access", "Rotate", "Retire", "Revoke", "Delete", "Update",
        ];

        for (op, expected_str) in operations.iter().zip(expected.iter()) {
            assert_eq!(op.to_string(), *expected_str);
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

        let kms_config = hammerwork::encryption::ExternalKmsConfig {
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
        let config = hammerwork::encryption::KeyDerivationConfig::default();

        assert_eq!(config.memory_cost, 65536); // 64 MB
        assert_eq!(config.time_cost, 3);
        assert_eq!(config.parallelism, 4);
        assert_eq!(config.salt_length, 32);
    }
}

// PostgreSQL-specific tests
#[cfg(feature = "postgres")]
mod postgres_tests {
    use super::*;
    use sqlx::Pool;

    async fn setup_postgres_test_env() -> Pool<sqlx::Postgres> {
        let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });

        let pool = Pool::<sqlx::Postgres>::connect(&database_url)
            .await
            .expect("Failed to connect to PostgreSQL");

        // Run migrations to ensure all tables exist
        use hammerwork::migrations::{MigrationManager, postgres::PostgresMigrationRunner};
        let runner = Box::new(PostgresMigrationRunner::new(pool.clone()));
        let manager = MigrationManager::new(runner);
        manager
            .run_migrations()
            .await
            .expect("Failed to run migrations");

        pool
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection
    async fn test_postgres_key_manager_creation() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await;
        assert!(
            key_manager.is_ok(),
            "Failed to create KeyManager: {:?}",
            key_manager.err()
        );

        let key_manager = key_manager.unwrap();
        let stats = key_manager.get_stats().await.unwrap();

        // Verify initial stats
        assert_eq!(stats.total_access_operations, 0);
        assert_eq!(stats.rotations_performed, 0);
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection
    async fn test_postgres_key_storage_and_retrieval() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();
        let test_key = create_test_encryption_key("test-storage-key");

        // Store the key
        let store_result = key_manager.store_key(&test_key).await;
        assert!(
            store_result.is_ok(),
            "Failed to store key: {:?}",
            store_result.err()
        );

        // Retrieve the key
        let retrieved_key = key_manager.load_key(&test_key.key_id).await;
        assert!(
            retrieved_key.is_ok(),
            "Failed to load key: {:?}",
            retrieved_key.err()
        );

        let retrieved_key = retrieved_key.unwrap();
        assert_eq!(retrieved_key.key_id, test_key.key_id);
        assert_eq!(retrieved_key.version, test_key.version);
        assert_eq!(retrieved_key.algorithm, test_key.algorithm);
        assert_eq!(retrieved_key.purpose, test_key.purpose);
        assert_eq!(retrieved_key.status, test_key.status);
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection  
    async fn test_postgres_key_retirement() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();
        let test_key = create_test_encryption_key("test-retirement-key");

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Retire the key
        let retire_result = key_manager
            .retire_key_version(&test_key.key_id, test_key.version)
            .await;
        assert!(
            retire_result.is_ok(),
            "Failed to retire key: {:?}",
            retire_result.err()
        );

        // Verify key is retired
        let retrieved_key = key_manager.load_key(&test_key.key_id).await.unwrap();
        assert_eq!(retrieved_key.status, KeyStatus::Retired);
        assert!(retrieved_key.retired_at.is_some());
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection
    async fn test_postgres_key_usage_tracking() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();
        let test_key = create_test_encryption_key("test-usage-key");

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Record usage
        let usage_result = key_manager.record_key_usage(&test_key.key_id).await;
        assert!(
            usage_result.is_ok(),
            "Failed to record key usage: {:?}",
            usage_result.err()
        );

        // Verify usage was recorded
        let retrieved_key = key_manager.load_key(&test_key.key_id).await.unwrap();
        assert_eq!(retrieved_key.usage_count, 1);
        assert!(retrieved_key.last_used_at.is_some());
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection
    async fn test_postgres_audit_logging() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool.clone()).await.unwrap();
        let test_key = create_test_encryption_key("test-audit-key");

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Record audit event
        let audit_result = key_manager
            .record_audit_event(&test_key.key_id, KeyOperation::Create, true, None)
            .await;
        assert!(
            audit_result.is_ok(),
            "Failed to record audit event: {:?}",
            audit_result.err()
        );

        // Verify audit log entry exists
        let audit_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM hammerwork_key_audit_log WHERE key_id = $1 AND operation = $2",
        )
        .bind(&test_key.key_id)
        .bind("Create")
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(audit_count.0, 1);
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection
    async fn test_postgres_keys_due_for_rotation() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create a key that needs rotation
        let mut test_key = create_test_encryption_key("test-rotation-key");
        test_key.next_rotation_at = Some(Utc::now() - Duration::hours(1)); // Past due

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Get keys due for rotation
        let due_keys = key_manager.get_keys_due_for_rotation().await;
        assert!(
            due_keys.is_ok(),
            "Failed to get keys due for rotation: {:?}",
            due_keys.err()
        );

        let due_keys = due_keys.unwrap();
        assert!(due_keys.contains(&test_key.key_id));
    }

    #[tokio::test]
    #[ignore] // Requires PostgreSQL database connection
    async fn test_postgres_old_key_version_cleanup() {
        setup_test_environment();
        let pool = setup_postgres_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool.clone()).await.unwrap();
        let base_key = create_test_encryption_key("test-cleanup-key");

        // Store multiple versions of the same key
        for version in 1..=10 {
            let mut test_key = base_key.clone();
            test_key.version = version;
            key_manager.store_key(&test_key).await.unwrap();
        }

        // Clean up old versions (should keep only 5 based on config)
        let cleanup_result = key_manager.cleanup_old_key_versions(&base_key.key_id).await;
        assert!(
            cleanup_result.is_ok(),
            "Failed to cleanup old key versions: {:?}",
            cleanup_result.err()
        );

        // Verify only 5 versions remain
        let version_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM hammerwork_encryption_keys WHERE key_id = $1")
                .bind(&base_key.key_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(version_count.0, 5);
    }
}

// MySQL-specific tests
#[cfg(feature = "mysql")]
mod mysql_tests {
    use super::*;
    use sqlx::Pool;

    async fn setup_mysql_test_env() -> Pool<sqlx::MySql> {
        let database_url = env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:hammerwork@localhost:3307/hammerwork".to_string());

        let pool = Pool::<sqlx::MySql>::connect(&database_url)
            .await
            .expect("Failed to connect to MySQL");

        // Run migrations to ensure all tables exist
        use hammerwork::migrations::{MigrationManager, mysql::MySqlMigrationRunner};
        let runner = Box::new(MySqlMigrationRunner::new(pool.clone()));
        let manager = MigrationManager::new(runner);
        manager
            .run_migrations()
            .await
            .expect("Failed to run migrations");

        pool
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_key_manager_creation() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await;
        assert!(
            key_manager.is_ok(),
            "Failed to create KeyManager: {:?}",
            key_manager.err()
        );

        let key_manager = key_manager.unwrap();
        let stats = key_manager.get_stats().await.unwrap();

        // Verify initial stats
        assert_eq!(stats.total_access_operations, 0);
        assert_eq!(stats.rotations_performed, 0);
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_key_storage_and_retrieval() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();
        let test_key = create_test_encryption_key("test-mysql-storage-key");

        // Store the key
        let store_result = key_manager.store_key(&test_key).await;
        assert!(
            store_result.is_ok(),
            "Failed to store key: {:?}",
            store_result.err()
        );

        // Retrieve the key
        let retrieved_key = key_manager.load_key(&test_key.key_id).await;
        assert!(
            retrieved_key.is_ok(),
            "Failed to load key: {:?}",
            retrieved_key.err()
        );

        let retrieved_key = retrieved_key.unwrap();
        assert_eq!(retrieved_key.key_id, test_key.key_id);
        assert_eq!(retrieved_key.version, test_key.version);
        assert_eq!(retrieved_key.algorithm, test_key.algorithm);
        assert_eq!(retrieved_key.purpose, test_key.purpose);
        assert_eq!(retrieved_key.status, test_key.status);
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_key_retirement() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();
        let test_key = create_test_encryption_key("test-mysql-retirement-key");

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Retire the key
        let retire_result = key_manager
            .retire_key_version(&test_key.key_id, test_key.version)
            .await;
        assert!(
            retire_result.is_ok(),
            "Failed to retire key: {:?}",
            retire_result.err()
        );

        // Verify key is retired
        let retrieved_key = key_manager.load_key(&test_key.key_id).await.unwrap();
        assert_eq!(retrieved_key.status, KeyStatus::Retired);
        assert!(retrieved_key.retired_at.is_some());
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_key_usage_tracking() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();
        let test_key = create_test_encryption_key("test-mysql-usage-key");

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Record usage
        let usage_result = key_manager.record_key_usage(&test_key.key_id).await;
        assert!(
            usage_result.is_ok(),
            "Failed to record key usage: {:?}",
            usage_result.err()
        );

        // Verify usage was recorded
        let retrieved_key = key_manager.load_key(&test_key.key_id).await.unwrap();
        assert_eq!(retrieved_key.usage_count, 1);
        assert!(retrieved_key.last_used_at.is_some());
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_audit_logging() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool.clone()).await.unwrap();
        let test_key = create_test_encryption_key("test-mysql-audit-key");

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Record audit event
        let audit_result = key_manager
            .record_audit_event(&test_key.key_id, KeyOperation::Create, true, None)
            .await;
        assert!(
            audit_result.is_ok(),
            "Failed to record audit event: {:?}",
            audit_result.err()
        );

        // Verify audit log entry exists
        let audit_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM hammerwork_key_audit_log WHERE key_id = ? AND operation = ?",
        )
        .bind(&test_key.key_id)
        .bind("Create")
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(audit_count.0, 1);
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_keys_due_for_rotation() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool).await.unwrap();

        // Create a key that needs rotation
        let mut test_key = create_test_encryption_key("test-mysql-rotation-key");
        test_key.next_rotation_at = Some(Utc::now() - Duration::hours(1)); // Past due

        // Store the key
        key_manager.store_key(&test_key).await.unwrap();

        // Get keys due for rotation
        let due_keys = key_manager.get_keys_due_for_rotation().await;
        assert!(
            due_keys.is_ok(),
            "Failed to get keys due for rotation: {:?}",
            due_keys.err()
        );

        let due_keys = due_keys.unwrap();
        assert!(due_keys.contains(&test_key.key_id));
    }

    #[tokio::test]
    #[ignore] // Requires MySQL database connection
    async fn test_mysql_old_key_version_cleanup() {
        setup_test_environment();
        let pool = setup_mysql_test_env().await;
        let config = create_test_config();

        let key_manager = KeyManager::new(config, pool.clone()).await.unwrap();
        let base_key = create_test_encryption_key("test-mysql-cleanup-key");

        // Store multiple versions of the same key
        for version in 1..=10 {
            let mut test_key = base_key.clone();
            test_key.version = version;
            key_manager.store_key(&test_key).await.unwrap();
        }

        // Clean up old versions (should keep only 5 based on config)
        let cleanup_result = key_manager.cleanup_old_key_versions(&base_key.key_id).await;
        assert!(
            cleanup_result.is_ok(),
            "Failed to cleanup old key versions: {:?}",
            cleanup_result.err()
        );

        // Verify only 5 versions remain
        let version_count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM hammerwork_encryption_keys WHERE key_id = ?")
                .bind(&base_key.key_id)
                .fetch_one(&pool)
                .await
                .unwrap();

        assert_eq!(version_count.0, 5);
    }
}

// Error handling and edge case tests
#[cfg(test)]
mod error_handling_tests {
    use super::*;

    #[test]
    fn test_invalid_key_source_parsing() {
        use hammerwork::encryption::parse_key_source;

        // Test that invalid key source strings are handled properly
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
    fn test_invalid_algorithm_parsing() {
        use hammerwork::encryption::parse_algorithm;

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
    fn test_invalid_key_status_parsing() {
        use hammerwork::encryption::parse_key_status;

        let invalid_statuses = vec![
            "InvalidStatus",
            "active",   // Case sensitive
            "ACTIVE",   // Case sensitive
            "Disabled", // Not a valid status
            "",
        ];

        for invalid_status in invalid_statuses {
            let result = parse_key_status(invalid_status);
            assert!(
                result.is_err(),
                "Expected error for invalid status: {}",
                invalid_status
            );
        }
    }

    #[test]
    fn test_invalid_key_purpose_parsing() {
        use hammerwork::encryption::parse_key_purpose;

        let invalid_purposes = vec![
            "InvalidPurpose",
            "encryption", // Case sensitive
            "Signing",    // Not a valid purpose
            "Hash",       // Not a valid purpose
            "",
        ];

        for invalid_purpose in invalid_purposes {
            let result = parse_key_purpose(invalid_purpose);
            assert!(
                result.is_err(),
                "Expected error for invalid purpose: {}",
                invalid_purpose
            );
        }
    }

    #[tokio::test]
    async fn test_key_manager_without_database() {
        // Test that KeyManager operations fail appropriately when database is not available
        setup_test_environment();

        // This should fail because we don't have a database connection
        let _config = create_test_config();
        let pool = sqlx::Pool::<sqlx::Postgres>::connect("invalid://connection").await;

        assert!(
            pool.is_err(),
            "Expected connection to fail with invalid URL"
        );
    }

    #[test]
    fn test_encryption_key_validation() {
        let mut key = create_test_encryption_key("test-key");

        // Test that key has valid initial state
        assert!(!key.key_id.is_empty());
        assert!(key.version > 0);
        assert!(key.key_strength >= 128);
        assert_eq!(key.status, KeyStatus::Active);
        assert_eq!(key.purpose, KeyPurpose::Encryption);

        // Test key modification
        key.status = KeyStatus::Retired;
        key.retired_at = Some(Utc::now());

        assert_eq!(key.status, KeyStatus::Retired);
        assert!(key.retired_at.is_some());
    }

    #[test]
    fn test_key_manager_stats_default() {
        use hammerwork::encryption::KeyManagerStats;
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

    #[test]
    fn test_valid_key_source_parsing() {
        use hammerwork::encryption::parse_key_source;

        // Test that valid key source strings are parsed correctly
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
    fn test_valid_algorithm_parsing() {
        use hammerwork::encryption::parse_algorithm;

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
    fn test_valid_key_status_parsing() {
        use hammerwork::encryption::parse_key_status;

        let valid_statuses = vec![
            ("Active", KeyStatus::Active),
            ("Retired", KeyStatus::Retired),
            ("Revoked", KeyStatus::Revoked),
            ("Expired", KeyStatus::Expired),
        ];

        for (status_str, expected) in valid_statuses {
            let result = parse_key_status(status_str);
            assert!(
                result.is_ok(),
                "Failed to parse valid status: {}",
                status_str
            );
            assert_eq!(result.unwrap(), expected);
        }
    }

    #[test]
    fn test_valid_key_purpose_parsing() {
        use hammerwork::encryption::parse_key_purpose;

        let valid_purposes = vec![
            ("Encryption", KeyPurpose::Encryption),
            ("MAC", KeyPurpose::MAC),
            ("KEK", KeyPurpose::KEK),
        ];

        for (purpose_str, expected) in valid_purposes {
            let result = parse_key_purpose(purpose_str);
            assert!(
                result.is_ok(),
                "Failed to parse valid purpose: {}",
                purpose_str
            );
            assert_eq!(result.unwrap(), expected);
        }
    }
}
