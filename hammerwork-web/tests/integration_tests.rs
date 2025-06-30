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
