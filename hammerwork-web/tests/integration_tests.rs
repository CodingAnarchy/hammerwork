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

/// Start the dashboard and assert it is still serving after a grace period.
///
/// `WebDashboard::start` only returns on failure (e.g. it cannot connect to the
/// database), so a start that is still running after the grace period succeeded.
#[allow(dead_code)] // unused when neither database feature is enabled
async fn assert_dashboard_starts(config: DashboardConfig) {
    let dashboard = WebDashboard::new(config)
        .await
        .expect("Dashboard should be created successfully");

    match tokio::time::timeout(Duration::from_secs(3), dashboard.start()).await {
        Err(_elapsed) => {} // still serving: startup succeeded
        Ok(Ok(())) => panic!("Dashboard server exited unexpectedly"),
        Ok(Err(e)) => panic!("Dashboard failed to start: {e:#}"),
    }
}

/// Write a minimal `index.html` so the static file routes have something to serve.
fn write_index(dir: &std::path::Path) {
    std::fs::write(
        dir.join("index.html"),
        "<html><body>Test Dashboard</body></html>",
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_dashboard_startup_with_postgres() {
        let temp_dir = tempdir().unwrap();

        let mut config = DashboardConfig::default();
        config.bind_address = "127.0.0.1".to_string();
        config.port = 0;
        config.database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgresql://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });
        config.pool_size = 2;
        config.static_dir = temp_dir.path().to_path_buf();
        config.auth = AuthConfig::default().with_enabled(false);
        config.enable_cors = true;
        config.allowed_origins = vec!["https://ops.example.com".to_string()];

        // Create minimal static files
        write_index(temp_dir.path());

        assert_dashboard_starts(config).await;
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_dashboard_startup_without_cors() {
        let temp_dir = tempdir().unwrap();
        write_index(temp_dir.path());

        let mut config = DashboardConfig::default();
        config.bind_address = "127.0.0.1".to_string();
        config.port = 0;
        config.database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
            "postgres://postgres:hammerwork@localhost:5433/hammerwork".to_string()
        });
        config.static_dir = temp_dir.path().to_path_buf();
        config.auth = AuthConfig::default().with_enabled(false);
        config.enable_cors = false;

        assert_dashboard_starts(config).await;
    }

    #[cfg(feature = "mysql")]
    #[tokio::test]
    #[ignore] // Requires database connection
    async fn test_dashboard_startup_with_mysql() {
        let temp_dir = tempdir().unwrap();

        let mut config = DashboardConfig::default();
        config.bind_address = "127.0.0.1".to_string();
        config.port = 0;
        config.database_url = std::env::var("MYSQL_DATABASE_URL")
            .unwrap_or_else(|_| "mysql://root:hammerwork@localhost:3307/hammerwork".to_string());
        config.pool_size = 2;
        config.static_dir = temp_dir.path().to_path_buf();
        config.auth = AuthConfig::default().with_enabled(false);
        config.enable_cors = false;

        // Create minimal static files
        write_index(temp_dir.path());

        assert_dashboard_starts(config).await;
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
            .with_cors(true)
            .with_allowed_origin("https://ops.example.com");

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

    #[tokio::test]
    async fn test_invalid_database_url() {
        let temp_dir = tempdir().unwrap();
        write_index(temp_dir.path());

        let mut config = DashboardConfig::default();
        config.bind_address = "127.0.0.1".to_string();
        config.port = 0;
        config.database_url = "invalid://url".to_string();
        config.static_dir = temp_dir.path().to_path_buf();

        // The database URL is only used when the server starts, which must fail fast.
        let dashboard = WebDashboard::new(config).await.unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), dashboard.start())
            .await
            .expect("start() should fail immediately for an unsupported database URL");
        let err = result.expect_err("Should fail with invalid database URL");
        assert!(
            err.to_string().contains("Unsupported database URL"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn test_static_directory_validation() {
        let non_existent_dir = PathBuf::from("/non/existent/directory");

        let mut config = DashboardConfig::default();
        config.static_dir = non_existent_dir;

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
        use hammerwork_web::WebSocketConfig;

        let ws_config = WebSocketConfig::default()
            .with_ping_interval(Duration::from_secs(10))
            .with_max_connections(50)
            .with_message_buffer_size(512)
            .with_max_message_size(32 * 1024);

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
