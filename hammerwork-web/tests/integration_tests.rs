//! Integration tests for the Hammerwork Web Dashboard
//!
//! These tests validate the complete web dashboard functionality including:
//! - Server startup and configuration
//! - Authentication system
//! - API endpoints
//! - WebSocket connections
//! - Database integration

use hammerwork_web::{AuthConfig, DashboardConfig, WebDashboard};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::tempdir;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_dashboard_startup_with_postgres() {
        let temp_dir = tempdir().unwrap();

        let config = DashboardConfig {
            bind_address: "127.0.0.1".to_string(),
            port: 0, // Use random port
            database_url: std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgresql://postgres:hammerwork@localhost:5433/hammerwork".to_string()
            }),
            pool_size: 2,
            static_dir: temp_dir.path().to_path_buf(),
            auth: AuthConfig {
                enabled: false,
                ..Default::default()
            },
            enable_cors: true,
            request_timeout: Duration::from_secs(30),
            ..Default::default()
        };

        // Create minimal static files
        std::fs::create_dir_all(temp_dir.path()).unwrap();
        std::fs::write(
            temp_dir.path().join("index.html"),
            "<html><body>Test Dashboard</body></html>",
        )
        .unwrap();

        let dashboard = WebDashboard::new(config).await;
        assert!(
            dashboard.is_ok(),
            "Dashboard should be created successfully"
        );
    }

    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_dashboard_startup_with_mysql() {
        let temp_dir = tempdir().unwrap();

        let config = DashboardConfig {
            bind_address: "127.0.0.1".to_string(),
            port: 0, // Use random port
            database_url: std::env::var("MYSQL_DATABASE_URL").unwrap_or_else(|_| {
                "mysql://root:hammerwork@localhost:3307/hammerwork".to_string()
            }),
            pool_size: 2,
            static_dir: temp_dir.path().to_path_buf(),
            auth: AuthConfig {
                enabled: false,
                ..Default::default()
            },
            enable_cors: false,
            request_timeout: Duration::from_secs(30),
            ..Default::default()
        };

        // Create minimal static files
        std::fs::create_dir_all(temp_dir.path()).unwrap();
        std::fs::write(
            temp_dir.path().join("index.html"),
            "<html><body>Test Dashboard</body></html>",
        )
        .unwrap();

        let dashboard = WebDashboard::new(config).await;
        assert!(
            dashboard.is_ok(),
            "Dashboard should be created successfully"
        );
    }

    #[test]
    fn test_config_validation() {
        let temp_dir = tempdir().unwrap();

        let config = DashboardConfig::new()
            .with_bind_address("0.0.0.0", 8080)
            .with_database_url("postgresql://localhost/hammerwork")
            .with_static_dir(temp_dir.path().to_path_buf())
            .with_cors(true);

        assert_eq!(config.bind_addr(), "0.0.0.0:8080");
        assert_eq!(config.database_url, "postgresql://localhost/hammerwork");
        assert!(config.enable_cors);
        assert!(config.static_dir.exists());
    }

    #[test]
    fn test_auth_config_validation() {
        let config = DashboardConfig::new().with_auth("testuser", "testhash");

        assert!(config.auth.enabled);
        assert_eq!(config.auth.username, "testuser");
        assert_eq!(config.auth.password_hash, "testhash");
    }

    #[tokio::test]
    async fn test_config_file_operations() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("dashboard.toml");

        let original_config = DashboardConfig::new()
            .with_bind_address("192.168.1.100", 9090)
            .with_database_url("postgresql://test/database")
            .with_cors(true);

        // Save configuration
        original_config
            .save_to_file(config_path.to_str().unwrap())
            .unwrap();

        // Load configuration
        let loaded_config = DashboardConfig::from_file(config_path.to_str().unwrap()).unwrap();

        assert_eq!(loaded_config.bind_address, "192.168.1.100");
        assert_eq!(loaded_config.port, 9090);
        assert_eq!(loaded_config.database_url, "postgresql://test/database");
        assert!(loaded_config.enable_cors);
    }

    #[test]
    fn test_invalid_database_url() {
        let temp_dir = tempdir().unwrap();

        let config = DashboardConfig {
            database_url: "invalid://url".to_string(),
            static_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // This should be caught during dashboard creation
        let result = tokio_test::block_on(WebDashboard::new(config));
        assert!(result.is_err(), "Should fail with invalid database URL");
    }

    #[test]
    fn test_static_directory_validation() {
        let non_existent_dir = PathBuf::from("/non/existent/directory");

        let config = DashboardConfig {
            static_dir: non_existent_dir,
            ..Default::default()
        };

        // This should be handled gracefully or produce appropriate error
        assert!(!config.static_dir.exists());
    }

    #[test]
    fn test_auth_security_defaults() {
        let auth_config = AuthConfig::default();

        // Authentication should be enabled by default for security
        assert!(auth_config.enabled);
        assert_eq!(auth_config.username, "admin");
        assert_eq!(auth_config.max_failed_attempts, 5);
        assert_eq!(auth_config.lockout_duration, Duration::from_secs(15 * 60));
        assert_eq!(
            auth_config.session_timeout,
            Duration::from_secs(8 * 60 * 60)
        );
    }

    #[tokio::test]
    async fn test_websocket_config_validation() {
        use hammerwork_web::config::WebSocketConfig;

        let ws_config = WebSocketConfig {
            ping_interval: Duration::from_secs(10),
            max_connections: 50,
            message_buffer_size: 512,
            max_message_size: 32 * 1024,
        };

        assert_eq!(ws_config.ping_interval, Duration::from_secs(10));
        assert_eq!(ws_config.max_connections, 50);
        assert_eq!(ws_config.message_buffer_size, 512);
        assert_eq!(ws_config.max_message_size, 32 * 1024);
    }

    #[test]
    fn test_environment_variable_precedence() {
        // Test that environment variables can be used for sensitive configuration
        unsafe {
            std::env::set_var("HAMMERWORK_DATABASE_URL", "postgresql://test/env");
        }

        let database_url = std::env::var("HAMMERWORK_DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://localhost/hammerwork".to_string());

        assert_eq!(database_url, "postgresql://test/env");

        // Cleanup
        unsafe {
            std::env::remove_var("HAMMERWORK_DATABASE_URL");
        }
    }

    #[test]
    fn test_cors_configuration() {
        let config_with_cors = DashboardConfig::new().with_cors(true);
        let config_without_cors = DashboardConfig::new().with_cors(false);

        assert!(config_with_cors.enable_cors);
        assert!(!config_without_cors.enable_cors);
    }

    #[test]
    fn test_port_range_validation() {
        // Test various port configurations
        let config_low = DashboardConfig::new().with_bind_address("127.0.0.1", 1024);
        let config_high = DashboardConfig::new().with_bind_address("127.0.0.1", 65535);
        let config_standard = DashboardConfig::new().with_bind_address("127.0.0.1", 8080);

        assert_eq!(config_low.port, 1024);
        assert_eq!(config_high.port, 65535);
        assert_eq!(config_standard.port, 8080);
    }
}

/// Helper functions for integration tests
#[cfg(test)]
mod test_helpers {
    use super::*;
    use std::process::Command;

    /// Check if PostgreSQL test database is available
    #[allow(dead_code)]
    pub fn postgres_available() -> bool {
        let output = Command::new("pg_isready")
            .arg("-h")
            .arg("localhost")
            .arg("-p")
            .arg("5433")
            .arg("-U")
            .arg("postgres")
            .output();

        match output {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }

    /// Check if MySQL test database is available
    #[allow(dead_code)]
    pub fn mysql_available() -> bool {
        let output = Command::new("mysql")
            .arg("--host=127.0.0.1")
            .arg("--port=3307")
            .arg("--user=root")
            .arg("--password=hammerwork")
            .arg("--execute=SELECT 1")
            .output();

        match output {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }

    /// Create a test configuration with minimal setup
    #[allow(dead_code)]
    pub fn create_test_config() -> DashboardConfig {
        let temp_dir = tempdir().expect("Failed to create temp directory");

        // Create minimal static files
        std::fs::create_dir_all(temp_dir.path()).unwrap();
        std::fs::write(
            temp_dir.path().join("index.html"),
            include_str!("../assets/index.html"),
        )
        .unwrap_or_else(|_| {
            // Fallback if assets don't exist
            std::fs::write(
                temp_dir.path().join("index.html"),
                "<html><body>Test Dashboard</body></html>",
            )
            .unwrap();
        });

        DashboardConfig {
            bind_address: "127.0.0.1".to_string(),
            port: 0,                                     // Random port for testing
            database_url: "sqlite::memory:".to_string(), // Use in-memory SQLite for basic tests
            pool_size: 1,
            static_dir: temp_dir.path().to_path_buf(),
            auth: AuthConfig {
                enabled: false, // Disable auth for testing
                ..Default::default()
            },
            enable_cors: true,
            request_timeout: Duration::from_secs(5),
            ..Default::default()
        }
    }
}

// WebSocket Archive Event Tests
#[cfg(test)]
mod websocket_archive_tests {
    use super::*;
    use chrono::Utc;
    use hammerwork::archive::{ArchivalReason, ArchivalStats, ArchiveEvent};
    use hammerwork_web::config::WebSocketConfig;
    use hammerwork_web::websocket::{BroadcastMessage, WebSocketState};
    use uuid::Uuid;

    #[tokio::test]
    async fn test_websocket_publish_archive_event() {
        let config = WebSocketConfig::default();
        let mut ws_state = WebSocketState::new(config);

        // Test each type of archive event
        let test_events = vec![
            ArchiveEvent::JobArchived {
                job_id: Uuid::new_v4(),
                queue: "test_queue".to_string(),
                reason: ArchivalReason::Manual,
            },
            ArchiveEvent::JobRestored {
                job_id: Uuid::new_v4(),
                queue: "restore_queue".to_string(),
                restored_by: Some("admin".to_string()),
            },
            ArchiveEvent::BulkArchiveStarted {
                operation_id: "test_op_123".to_string(),
                estimated_jobs: 1000,
            },
            ArchiveEvent::BulkArchiveProgress {
                operation_id: "test_op_123".to_string(),
                jobs_processed: 500,
                total: 1000,
            },
            ArchiveEvent::BulkArchiveCompleted {
                operation_id: "test_op_123".to_string(),
                stats: ArchivalStats {
                    jobs_archived: 1000,
                    jobs_purged: 0,
                    bytes_archived: 50000,
                    compression_ratio: 0.8,
                    operation_duration: std::time::Duration::from_secs(30),
                    last_run_at: Utc::now(),
                },
            },
            ArchiveEvent::JobsPurged {
                count: 100,
                older_than: Utc::now(),
            },
        ];

        // Test that each event can be published without errors
        for event in test_events {
            let result = ws_state.publish_archive_event(event).await;
            assert!(result.is_ok(), "Failed to publish archive event");
        }
    }

    #[test]
    fn test_websocket_broadcast_message_conversion() {
        let job_id = Uuid::new_v4();
        let operation_id = "test_op_456".to_string();
        let queue_name = "test_queue".to_string();

        // Test JobArchived event conversion
        let job_archived = ArchiveEvent::JobArchived {
            job_id,
            queue: queue_name.clone(),
            reason: ArchivalReason::Automatic,
        };

        // Manually convert to verify the logic (similar to what happens in publish_archive_event)
        match job_archived {
            ArchiveEvent::JobArchived {
                job_id,
                queue,
                reason,
            } => {
                let broadcast_msg = BroadcastMessage::JobArchived {
                    job_id: job_id.to_string(),
                    queue,
                    reason,
                };

                // Test that the conversion produces valid data
                assert!(matches!(
                    broadcast_msg,
                    BroadcastMessage::JobArchived { .. }
                ));
            }
            _ => panic!("Expected JobArchived event"),
        }

        // Test BulkArchiveStarted event conversion
        let bulk_started = ArchiveEvent::BulkArchiveStarted {
            operation_id: operation_id.clone(),
            estimated_jobs: 500,
        };

        match bulk_started {
            ArchiveEvent::BulkArchiveStarted {
                operation_id,
                estimated_jobs,
            } => {
                let broadcast_msg = BroadcastMessage::BulkArchiveStarted {
                    operation_id,
                    estimated_jobs,
                };

                assert!(matches!(
                    broadcast_msg,
                    BroadcastMessage::BulkArchiveStarted { .. }
                ));
            }
            _ => panic!("Expected BulkArchiveStarted event"),
        }
    }

    #[test]
    fn test_websocket_config_defaults() {
        let config = WebSocketConfig::default();

        // Verify default WebSocket configuration supports archive events
        assert!(config.max_connections > 0);
        assert!(config.ping_interval.as_secs() > 0);
        assert!(config.buffer_size > 0);
    }

    #[tokio::test]
    async fn test_websocket_state_connection_management() {
        let config = WebSocketConfig {
            max_connections: 5,
            ping_interval: Duration::from_secs(30),
            buffer_size: 1024,
        };

        let ws_state = WebSocketState::new(config);

        // Test initial state
        assert_eq!(ws_state.connection_count(), 0);

        // Test that archive events can be published even with no connections
        let test_event = ArchiveEvent::JobArchived {
            job_id: Uuid::new_v4(),
            queue: "test_queue".to_string(),
            reason: ArchivalReason::Manual,
        };

        let result = ws_state.publish_archive_event(test_event).await;
        assert!(
            result.is_ok(),
            "Should handle publishing to empty connection list"
        );
    }

    #[test]
    fn test_archive_event_serialization_for_websocket() {
        let events = vec![
            ArchiveEvent::JobArchived {
                job_id: Uuid::new_v4(),
                queue: "serialize_test".to_string(),
                reason: ArchivalReason::Compliance,
            },
            ArchiveEvent::BulkArchiveProgress {
                operation_id: "progress_op".to_string(),
                jobs_processed: 75,
                total: 100,
            },
        ];

        // Test that all archive events can be serialized for WebSocket transmission
        for event in events {
            let serialized = serde_json::to_string(&event);
            assert!(serialized.is_ok(), "Archive event should be serializable");

            let json_value = serialized.unwrap();
            assert!(!json_value.is_empty());

            // Verify it can be deserialized back
            let deserialized: Result<ArchiveEvent, _> = serde_json::from_str(&json_value);
            assert!(
                deserialized.is_ok(),
                "Serialized event should be deserializable"
            );
        }
    }
}
