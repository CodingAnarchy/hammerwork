//! Configuration for the Hammerwork web dashboard.
//!
//! This module provides comprehensive configuration options for the web dashboard,
//! including server settings, authentication, WebSocket configuration, and more.
//!
//! # Examples
//!
//! ## Basic Configuration
//!
//! ```rust
//! use hammerwork_web::config::DashboardConfig;
//!
//! let config = DashboardConfig::new()
//!     .with_bind_address("127.0.0.1", 8080)
//!     .with_database_url("postgresql://localhost/hammerwork");
//!
//! assert_eq!(config.bind_addr(), "127.0.0.1:8080");
//! ```
//!
//! ## Configuration with Authentication
//!
//! ```rust
//! use hammerwork_web::config::{DashboardConfig, AuthConfig};
//! use std::time::Duration;
//!
//! let config = DashboardConfig::new()
//!     .with_auth("admin", "$2b$12$hash...")
//!     .with_cors(true);
//!
//! assert!(config.auth.enabled);
//! assert_eq!(config.auth.username, "admin");
//! assert!(config.enable_cors);
//! ```
//!
//! ## Loading from File
//!
//! ```rust,no_run
//! use hammerwork_web::config::DashboardConfig;
//!
//! // Create a configuration file (dashboard.toml)
//! let config_content = r#"
//! bind_address = "0.0.0.0"
//! port = 9090
//! database_url = "postgresql://localhost/hammerwork"
//! enable_cors = true
//!
//! [auth]
//! enabled = true
//! username = "admin"
//! "#;
//!
//! std::fs::write("dashboard.toml", config_content)?;
//!
//! // Load the configuration
//! let config = DashboardConfig::from_file("dashboard.toml")?;
//! assert_eq!(config.port, 9090);
//! assert!(config.enable_cors);
//!
//! // Clean up
//! std::fs::remove_file("dashboard.toml")?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Main configuration for the web dashboard.
///
/// This struct contains all configuration options for the Hammerwork web dashboard,
/// including server settings, database connection, authentication, and WebSocket options.
///
/// # Examples
///
/// ```rust
/// use hammerwork_web::config::DashboardConfig;
/// use std::path::PathBuf;
///
/// // Create with defaults
/// let config = DashboardConfig::default();
/// assert_eq!(config.bind_address, "127.0.0.1");
/// assert_eq!(config.port, 8080);
///
/// // Use builder pattern
/// let config = DashboardConfig::new()
///     .with_bind_address("0.0.0.0", 9090)
///     .with_database_url("postgresql://localhost/hammerwork")
///     .with_cors(true);
///
/// assert_eq!(config.bind_addr(), "0.0.0.0:9090");
/// assert!(config.enable_cors);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    /// Server bind address
    pub bind_address: String,

    /// Server port
    pub port: u16,

    /// Database connection URL
    pub database_url: String,

    /// Database connection pool size
    pub pool_size: u32,

    /// Directory containing static assets (HTML, CSS, JS)
    pub static_dir: PathBuf,

    /// Authentication configuration
    pub auth: AuthConfig,

    /// WebSocket configuration
    pub websocket: WebSocketConfig,

    /// Enable CORS for cross-origin requests
    pub enable_cors: bool,

    /// Request timeout duration
    pub request_timeout: Duration,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1".to_string(),
            port: 8080,
            database_url: "postgresql://localhost/hammerwork".to_string(),
            pool_size: 5,
            static_dir: PathBuf::from("./assets"),
            auth: AuthConfig::default(),
            websocket: WebSocketConfig::default(),
            enable_cors: false,
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl DashboardConfig {
    /// Create a new configuration with defaults.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    ///
    /// let config = DashboardConfig::new();
    /// assert_eq!(config.bind_address, "127.0.0.1");
    /// assert_eq!(config.port, 8080);
    /// assert_eq!(config.database_url, "postgresql://localhost/hammerwork");
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the server bind address and port.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    ///
    /// let config = DashboardConfig::new()
    ///     .with_bind_address("0.0.0.0", 9090);
    ///
    /// assert_eq!(config.bind_address, "0.0.0.0");
    /// assert_eq!(config.port, 9090);
    /// assert_eq!(config.bind_addr(), "0.0.0.0:9090");
    /// ```
    pub fn with_bind_address(mut self, address: &str, port: u16) -> Self {
        self.bind_address = address.to_string();
        self.port = port;
        self
    }

    /// Set the database URL.
    ///
    /// Supports both PostgreSQL and MySQL database URLs.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    ///
    /// // PostgreSQL
    /// let pg_config = DashboardConfig::new()
    ///     .with_database_url("postgresql://user:pass@localhost/hammerwork");
    /// assert_eq!(pg_config.database_url, "postgresql://user:pass@localhost/hammerwork");
    ///
    /// // MySQL
    /// let mysql_config = DashboardConfig::new()
    ///     .with_database_url("mysql://root:password@localhost/hammerwork");
    /// assert_eq!(mysql_config.database_url, "mysql://root:password@localhost/hammerwork");
    /// ```
    pub fn with_database_url(mut self, url: &str) -> Self {
        self.database_url = url.to_string();
        self
    }

    /// Set the static assets directory.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    /// use std::path::PathBuf;
    ///
    /// let config = DashboardConfig::new()
    ///     .with_static_dir(PathBuf::from("/var/www/dashboard"));
    ///
    /// assert_eq!(config.static_dir, PathBuf::from("/var/www/dashboard"));
    /// ```
    pub fn with_static_dir(mut self, dir: PathBuf) -> Self {
        self.static_dir = dir;
        self
    }

    /// Enable authentication with username and password hash.
    ///
    /// The password should be a bcrypt hash for security. When authentication is enabled,
    /// all API endpoints and WebSocket connections will require basic authentication.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    ///
    /// let config = DashboardConfig::new()
    ///     .with_auth("admin", "$2b$12$hash...");
    ///
    /// assert!(config.auth.enabled);
    /// assert_eq!(config.auth.username, "admin");
    /// assert_eq!(config.auth.password_hash, "$2b$12$hash...");
    /// ```
    pub fn with_auth(mut self, username: &str, password_hash: &str) -> Self {
        self.auth.enabled = true;
        self.auth.username = username.to_string();
        self.auth.password_hash = password_hash.to_string();
        self
    }

    /// Enable or disable CORS support.
    ///
    /// When enabled, the server will accept cross-origin requests from any domain.
    /// This is useful for development or when the dashboard is accessed from different domains.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    ///
    /// let config = DashboardConfig::new()
    ///     .with_cors(true);
    ///
    /// assert!(config.enable_cors);
    ///
    /// let config = DashboardConfig::new()
    ///     .with_cors(false);
    ///
    /// assert!(!config.enable_cors);
    /// ```
    pub fn with_cors(mut self, enabled: bool) -> Self {
        self.enable_cors = enabled;
        self
    }

    /// Load configuration from a TOML file
    pub fn from_file(path: &str) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to a TOML file
    pub fn save_to_file(&self, path: &str) -> crate::Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get the full bind address (address:port)
    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.bind_address, self.port)
    }
}

/// Authentication configuration for the web dashboard.
///
/// Controls authentication behavior including credentials, session management,
/// and security policies like rate limiting and account lockout.
///
/// # Examples
///
/// ```rust
/// use hammerwork_web::config::AuthConfig;
/// use std::time::Duration;
///
/// // Default configuration (authentication enabled)
/// let auth_config = AuthConfig::default();
/// assert!(auth_config.enabled);
/// assert_eq!(auth_config.username, "admin");
/// assert_eq!(auth_config.max_failed_attempts, 5);
///
/// // Custom configuration
/// let auth_config = AuthConfig {
///     enabled: true,
///     username: "dashboard_admin".to_string(),
///     password_hash: "$2b$12$hash...".to_string(),
///     session_timeout: Duration::from_secs(4 * 60 * 60), // 4 hours
///     max_failed_attempts: 3,
///     lockout_duration: Duration::from_secs(30 * 60), // 30 minutes
/// };
///
/// assert_eq!(auth_config.username, "dashboard_admin");
/// assert_eq!(auth_config.max_failed_attempts, 3);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Whether authentication is enabled
    pub enabled: bool,

    /// Username for basic authentication
    pub username: String,

    /// Bcrypt hash of the password
    pub password_hash: String,

    /// Session timeout duration
    pub session_timeout: Duration,

    /// Maximum number of failed login attempts
    pub max_failed_attempts: u32,

    /// Lockout duration after max failed attempts
    pub lockout_duration: Duration,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Enable auth by default for security
            username: "admin".to_string(),
            password_hash: String::new(),
            session_timeout: Duration::from_secs(8 * 60 * 60), // 8 hours
            max_failed_attempts: 5,
            lockout_duration: Duration::from_secs(15 * 60), // 15 minutes
        }
    }
}

/// WebSocket configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketConfig {
    /// Ping interval to keep connections alive
    pub ping_interval: Duration,

    /// Maximum number of concurrent WebSocket connections
    pub max_connections: usize,

    /// Buffer size for WebSocket messages
    pub message_buffer_size: usize,

    /// Maximum message size in bytes
    pub max_message_size: usize,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            max_connections: 100,
            message_buffer_size: 1024,
            max_message_size: 64 * 1024, // 64KB
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_creation() {
        let config = DashboardConfig::new()
            .with_bind_address("0.0.0.0", 9090)
            .with_database_url("mysql://localhost/test")
            .with_cors(true);

        assert_eq!(config.bind_address, "0.0.0.0");
        assert_eq!(config.port, 9090);
        assert_eq!(config.database_url, "mysql://localhost/test");
        assert!(config.enable_cors);
        assert_eq!(config.bind_addr(), "0.0.0.0:9090");
    }

    #[test]
    fn test_config_file_operations() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = DashboardConfig::new()
            .with_bind_address("192.168.1.100", 8888)
            .with_database_url("postgresql://test/db");

        // Save config
        config.save_to_file(config_path.to_str().unwrap()).unwrap();

        // Load config
        let loaded_config = DashboardConfig::from_file(config_path.to_str().unwrap()).unwrap();

        assert_eq!(loaded_config.bind_address, "192.168.1.100");
        assert_eq!(loaded_config.port, 8888);
        assert_eq!(loaded_config.database_url, "postgresql://test/db");
    }

    #[test]
    fn test_auth_config_defaults() {
        let auth = AuthConfig::default();
        assert!(auth.enabled); // Auth is enabled by default for security
        assert_eq!(auth.username, "admin");
        assert_eq!(auth.max_failed_attempts, 5);
        assert_eq!(auth.lockout_duration.as_secs(), 15 * 60); // 15 minutes
        assert_eq!(auth.session_timeout.as_secs(), 8 * 60 * 60); // 8 hours
    }

    #[test]
    fn test_websocket_config_defaults() {
        let ws_config = WebSocketConfig::default();
        assert_eq!(ws_config.ping_interval, Duration::from_secs(30));
        assert_eq!(ws_config.max_connections, 100);
        assert_eq!(ws_config.max_message_size, 64 * 1024);
    }
}
