//! # Hammerwork Web Dashboard
//!
//! A web-based admin dashboard for monitoring and managing Hammerwork job queues.
//!
//! This crate provides a comprehensive web interface for:
//! - Real-time queue monitoring and statistics
//! - Job management (retry, cancel, inspect)
//! - Worker status and utilization
//! - Dead job analysis and bulk operations
//! - System health monitoring
//!
//! ## Usage
//!
//! ### As a Binary
//!
//! ```bash
//! # Install the web dashboard
//! cargo install hammerwork-web --features postgres
//!
//! # Start the dashboard
//! hammerwork-web --database-url postgresql://localhost/hammerwork --bind 0.0.0.0:8080
//! ```
//!
//! ### As a Library
//!
//! #### Basic Usage
//!
//! ```rust,no_run
//! use hammerwork_web::{WebDashboard, DashboardConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = DashboardConfig::new()
//!         .with_bind_address("127.0.0.1", 8080)
//!         .with_database_url("postgresql://localhost/hammerwork");
//!
//!     let dashboard = WebDashboard::new(config).await?;
//!     dashboard.start().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! #### Builder Pattern Configuration
//!
//! ```rust
//! use hammerwork_web::DashboardConfig;
//! use std::time::Duration;
//!
//! let config = DashboardConfig::new()
//!     .with_bind_address("0.0.0.0", 9090)
//!     .with_database_url("postgresql://localhost/hammerwork")
//!     .with_auth("admin", "bcrypt_hash_here")
//!     .with_cors(true)
//!     .with_allowed_origin("https://ops.example.com");
//!
//! assert_eq!(config.bind_addr(), "0.0.0.0:9090");
//! assert_eq!(config.database_url, "postgresql://localhost/hammerwork");
//! assert!(config.auth.enabled);
//! assert!(config.enable_cors);
//! ```
//!
//! #### Configuration from File
//!
//! ```rust,no_run
//! use hammerwork_web::DashboardConfig;
//!
//! // Load from TOML file
//! let config = DashboardConfig::from_file("dashboard.toml")?;
//!
//! // Save configuration
//! config.save_to_file("dashboard.toml")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! #### Authentication Configuration
//!
//! ```rust
//! use hammerwork_web::AuthConfig;
//! use std::time::Duration;
//!
//! let auth_config = AuthConfig::default()
//!     .with_enabled(true)
//!     .with_username("admin".to_string())
//!     .with_password_hash("$2b$12$hash...".to_string())
//!     .with_session_timeout(Duration::from_secs(8 * 60 * 60)) // 8 hours
//!     .with_max_failed_attempts(5)
//!     .with_lockout_duration(Duration::from_secs(15 * 60)); // 15 minutes
//!
//! assert!(auth_config.enabled);
//! assert_eq!(auth_config.max_failed_attempts, 5);
//! ```

// The dashboard needs a database backend to serve anything.
#[cfg(not(any(feature = "postgres", feature = "mysql")))]
compile_error!(
    "hammerwork-web needs a database backend: enable the `postgres` (default) and/or `mysql` feature"
);

mod api;
#[cfg(test)]
mod api_tests;
mod auth;
mod config;
mod live;
mod security;
mod server;
mod websocket;
#[cfg(test)]
mod websocket_archive_tests;

pub use config::{AuthConfig, DashboardConfig, WebSocketConfig};
pub use server::WebDashboard;

/// Result type alias for consistent error handling
pub type Result<T> = std::result::Result<T, anyhow::Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_dashboard_config_creation() {
        let config = DashboardConfig::new()
            .with_bind_address("0.0.0.0", 3000)
            .with_database_url("postgresql://localhost/test")
            .with_cors(true);

        assert_eq!(config.bind_addr(), "0.0.0.0:3000");
        assert_eq!(config.database_url, "postgresql://localhost/test");
        assert!(config.enable_cors);
    }

    #[test]
    fn test_auth_config_security_defaults() {
        let auth_config = AuthConfig::default();

        // Ensure secure defaults
        assert!(
            auth_config.enabled,
            "Authentication should be enabled by default"
        );
        assert_eq!(auth_config.username, "admin");
        assert_eq!(auth_config.max_failed_attempts, 5);
        assert!(auth_config.lockout_duration.as_secs() > 0);
    }

    #[tokio::test]
    async fn test_dashboard_creation_with_invalid_config() {
        let temp_dir = tempdir().unwrap();

        let config = DashboardConfig {
            database_url: "invalid://url".to_string(),
            static_dir: temp_dir.path().to_path_buf(),
            auth: crate::AuthConfig {
                enabled: cfg!(feature = "auth"),
                ..Default::default()
            },
            ..Default::default()
        };

        // Dashboard creation should succeed, but starting would fail with invalid URL
        let result = WebDashboard::new(config).await;
        assert!(
            result.is_ok(),
            "Dashboard creation should succeed, connection validation happens later"
        );
    }

    #[test]
    fn test_config_builder_pattern() {
        let temp_dir = tempdir().unwrap();

        let config = DashboardConfig::new()
            .with_bind_address("192.168.1.1", 8888)
            .with_database_url("mysql://root:pass@localhost/db")
            .with_static_dir(temp_dir.path().to_path_buf())
            .with_auth("user", "hash")
            .with_cors(false);

        assert_eq!(config.bind_address, "192.168.1.1");
        assert_eq!(config.port, 8888);
        assert_eq!(config.database_url, "mysql://root:pass@localhost/db");
        assert!(config.auth.enabled);
        assert_eq!(config.auth.username, "user");
        assert!(!config.enable_cors);
    }
}
