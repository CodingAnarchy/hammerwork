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
///
/// `Debug` shows `database_url` with its password replaced by `***`.
#[derive(Clone, Serialize, Deserialize)]
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

    /// Enable CORS for cross-origin requests from [`allowed_origins`](Self::allowed_origins).
    ///
    /// CORS is only ever granted to the listed origins, never to any origin, so enabling it
    /// requires at least one entry there.
    pub enable_cors: bool,

    /// Origins other than the dashboard's own (`scheme://host[:port]`) that may send
    /// state-changing requests and WebSocket connections, and, with
    /// [`enable_cors`](Self::enable_cors), read API responses. Browsers on any other origin
    /// are refused (see [`crate::security`]).
    #[serde(default)]
    pub allowed_origins: Vec<String>,
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
            allowed_origins: Vec::new(),
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
    /// When enabled, browsers on the origins in [`allowed_origins`](Self::allowed_origins)
    /// may call the API; [`validate`](Self::validate) requires at least one of them.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork_web::config::DashboardConfig;
    ///
    /// let config = DashboardConfig::new()
    ///     .with_cors(true)
    ///     .with_allowed_origin("https://ops.example.com");
    ///
    /// assert!(config.enable_cors);
    /// assert_eq!(config.allowed_origins, ["https://ops.example.com"]);
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

    /// Allow requests from another origin (`scheme://host[:port]`); see
    /// [`allowed_origins`](Self::allowed_origins).
    pub fn with_allowed_origin(mut self, origin: &str) -> Self {
        self.allowed_origins.push(origin.to_string());
        self
    }

    /// Load configuration from a TOML file
    pub fn from_file(path: &str) -> crate::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Check settings that would make the server unsafe or make it fail at runtime:
    ///
    /// - authentication enabled in a build without the `auth` feature, which cannot verify
    ///   bcrypt password hashes;
    /// - an entry of `allowed_origins` that is not an origin, or `enable_cors` without any;
    /// - a zero `websocket.ping_interval` (`tokio::time::interval` panics on a zero period),
    ///   `websocket.message_buffer_size` or `websocket.max_message_size`;
    /// - a zero `websocket.live_update_max_jobs` while live updates are enabled.
    pub fn validate(&self) -> crate::Result<()> {
        if self.auth.enabled && !cfg!(feature = "auth") {
            anyhow::bail!(
                "Authentication is enabled, but hammerwork-web was built without the `auth` \
                 feature and cannot verify bcrypt password hashes. Rebuild with \
                 `--features auth` (a default feature), or disable authentication"
            );
        }
        crate::security::AllowedOrigins::new(&self.allowed_origins)?;
        if self.enable_cors && self.allowed_origins.is_empty() {
            anyhow::bail!(
                "enable_cors requires at least one entry in allowed_origins: CORS is never \
                 granted to every origin"
            );
        }
        if self.websocket.ping_interval.is_zero() {
            anyhow::bail!("websocket.ping_interval must be greater than zero");
        }
        if self.websocket.message_buffer_size == 0 {
            anyhow::bail!("websocket.message_buffer_size must be greater than zero");
        }
        if self.websocket.max_message_size == 0 {
            anyhow::bail!("websocket.max_message_size must be greater than zero");
        }
        if !self.websocket.live_update_interval.is_zero()
            && self.websocket.live_update_max_jobs == 0
        {
            anyhow::bail!(
                "websocket.live_update_max_jobs must be greater than zero (set \
                 websocket.live_update_interval to zero to disable live updates)"
            );
        }
        Ok(())
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
///
/// `Debug` never shows `password_hash`.
#[derive(Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Whether authentication is enabled
    pub enabled: bool,

    /// Username for basic authentication
    pub username: String,

    /// Bcrypt hash of the password. It is only ever verified with bcrypt (the `auth`
    /// feature), never compared to the password as text.
    pub password_hash: String,

    /// Upper bound on how long a successful credential check is remembered (at most
    /// [`crate::auth::VERIFIED_CREDENTIALS_TTL`]); zero re-verifies every request.
    #[serde(with = "hammerwork::config::serde_duration")]
    pub session_timeout: Duration,

    /// Failed attempts from one client address before it is locked out
    pub max_failed_attempts: u32,

    /// How long a locked-out client is refused. A failure older than this no longer counts.
    #[serde(with = "hammerwork::config::serde_duration")]
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
    #[serde(with = "hammerwork::config::serde_duration")]
    pub ping_interval: Duration,

    /// Maximum number of concurrent WebSocket connections
    pub max_connections: usize,

    /// Outgoing messages queued per connection; further messages for a client that does
    /// not keep up are dropped
    pub message_buffer_size: usize,

    /// Maximum size in bytes of a message (and of a frame) received from a client
    pub max_message_size: usize,

    /// How often the dashboard polls the database for job state changes and queue
    /// statistics to push to connected clients (`JobUpdate` and `QueueUpdate` messages).
    /// Polling only happens while at least one client is connected. Zero disables live
    /// updates. Optional in configuration files (default 2 seconds).
    #[serde(
        default = "default_live_update_interval",
        with = "hammerwork::config::serde_duration"
    )]
    pub live_update_interval: Duration,

    /// The most changed jobs one poll reads and pushes; further changes in the same
    /// interval are not pushed individually (the queue statistics still reflect them).
    /// Optional in configuration files (default 100).
    #[serde(default = "default_live_update_max_jobs")]
    pub live_update_max_jobs: u32,
}

fn default_live_update_interval() -> Duration {
    Duration::from_secs(2)
}

fn default_live_update_max_jobs() -> u32 {
    100
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(30),
            max_connections: 100,
            message_buffer_size: 1024,
            max_message_size: 64 * 1024, // 64KB
            live_update_interval: default_live_update_interval(),
            live_update_max_jobs: default_live_update_max_jobs(),
        }
    }
}

impl std::fmt::Debug for DashboardConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashboardConfig")
            .field("bind_address", &self.bind_address)
            .field("port", &self.port)
            .field(
                "database_url",
                &hammerwork::config::redact_url(&self.database_url),
            )
            .field("pool_size", &self.pool_size)
            .field("static_dir", &self.static_dir)
            .field("auth", &self.auth)
            .field("websocket", &self.websocket)
            .field("enable_cors", &self.enable_cors)
            .finish()
    }
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Fields added later are left out rather than risk printing a secret.
        f.debug_struct("AuthConfig")
            .field("enabled", &self.enabled)
            .field("username", &self.username)
            .field("password_hash", &"[REDACTED]")
            .field("session_timeout", &self.session_timeout)
            .field("max_failed_attempts", &self.max_failed_attempts)
            .field("lockout_duration", &self.lockout_duration)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod debug_redaction_tests {
    use super::*;

    #[test]
    fn debug_does_not_print_secrets() {
        let mut config = DashboardConfig {
            database_url: "postgres://app:hunter2-db@db.internal/jobs".to_string(),
            ..Default::default()
        };
        config.auth.password_hash = "$2b$12$hunter2hashhunter2hashhu".to_string();
        let debug = format!("{config:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(
            debug.contains("postgres://app:***@db.internal/jobs"),
            "{debug}"
        );
        assert!(debug.contains("[REDACTED]"), "{debug}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// The configuration file example in the README must load as written.
    #[test]
    fn readme_configuration_example_loads() {
        let readme = include_str!("../README.md");
        let start = readme
            .find("```toml\nbind_address")
            .expect("README has a configuration example");
        let body = &readme[start + "```toml\n".len()..];
        let example = &body[..body.find("```").unwrap()];
        let config: DashboardConfig = toml::from_str(example).unwrap();
        assert_eq!(config.auth.session_timeout, Duration::from_secs(8 * 3600));
        assert_eq!(config.auth.lockout_duration, Duration::from_secs(15 * 60));
        assert_eq!(config.websocket.ping_interval, Duration::from_secs(30));
        assert_eq!(
            config.websocket.live_update_interval,
            Duration::from_secs(2)
        );
    }

    /// Files written before durations became strings (serde's `{ secs, nanos }` tables)
    /// still load, and saved files use the readable form.
    #[test]
    fn durations_accept_the_old_table_form_and_save_as_strings() {
        let mut value = toml::Value::try_from(DashboardConfig::new()).unwrap();
        value["websocket"]["ping_interval"] =
            toml::from_str::<toml::Table>("v = { secs = 45, nanos = 0 }").unwrap()["v"].clone();
        let config: DashboardConfig = toml::from_str(&toml::to_string(&value).unwrap()).unwrap();
        assert_eq!(config.websocket.ping_interval, Duration::from_secs(45));

        let saved = toml::to_string(&DashboardConfig::new()).unwrap();
        assert!(saved.contains("ping_interval = \"30s\""), "{saved}");
        let reloaded: DashboardConfig = toml::from_str(&saved).unwrap();
        assert_eq!(reloaded.auth.session_timeout, Duration::from_secs(8 * 3600));
    }

    /// The defaults, with authentication only where this build can verify passwords.
    fn defaults() -> DashboardConfig {
        let mut config = DashboardConfig::new();
        config.auth.enabled = cfg!(feature = "auth");
        config
    }

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
    fn test_validate_rejects_zero_ping_interval() {
        let mut config = defaults();
        assert!(config.validate().is_ok());
        config.websocket.ping_interval = Duration::ZERO;
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("ping_interval"), "{err}");
    }

    #[test]
    fn test_from_file_rejects_zero_ping_interval() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        let mut config = defaults();
        config.websocket.ping_interval = Duration::ZERO;
        config.save_to_file(path.to_str().unwrap()).unwrap();
        assert!(DashboardConfig::from_file(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn test_config_file_operations() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.toml");

        let config = defaults()
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
    fn test_validate_rejects_unusable_websocket_limits() {
        let mut config = defaults();
        config.websocket.message_buffer_size = 0;
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("message_buffer_size"), "{err}");
        let mut config = defaults();
        config.websocket.max_message_size = 0;
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("max_message_size"), "{err}");
    }

    #[test]
    fn cors_is_only_granted_to_listed_origins() {
        // M16: CORS used to allow any origin.
        let err = defaults()
            .with_cors(true)
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("allowed_origins"), "{err}");
        let config = defaults()
            .with_cors(true)
            .with_allowed_origin("https://ops.example.com");
        assert!(config.validate().is_ok());
        let err = defaults()
            .with_allowed_origin("*")
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid origin"), "{err}");
    }

    #[test]
    fn config_files_without_allowed_origins_still_load() {
        let text = toml::to_string(&DashboardConfig::new()).unwrap();
        let text: String = text
            .lines()
            .filter(|line| !line.starts_with("allowed_origins"))
            .collect::<Vec<_>>()
            .join("\n");
        let config: DashboardConfig = toml::from_str(&text).unwrap();
        assert!(config.allowed_origins.is_empty());
    }

    /// `request_timeout` was removed in 2.0; files that still set it (as 1.x wrote it)
    /// load and ignore it.
    #[test]
    fn config_files_with_removed_request_timeout_still_load() {
        let text = format!(
            "{}\n[request_timeout]\nsecs = 30\nnanos = 0\n",
            toml::to_string(&DashboardConfig::new()).unwrap()
        );
        let config: DashboardConfig = toml::from_str(&text).unwrap();
        assert_eq!(config.port, DashboardConfig::new().port);
    }

    #[cfg(feature = "auth")]
    #[test]
    fn auth_builds_accept_authentication() {
        assert!(DashboardConfig::new().validate().is_ok());
    }

    #[cfg(not(feature = "auth"))]
    #[test]
    fn builds_without_bcrypt_refuse_to_enable_authentication() {
        // H5: without bcrypt the stored hash must never be compared as a plaintext password.
        let err = DashboardConfig::new().validate().unwrap_err().to_string();
        assert!(err.contains("`auth`"), "{err}");
        let mut config = DashboardConfig::new();
        config.auth.enabled = false;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_websocket_config_defaults() {
        let ws_config = WebSocketConfig::default();
        assert_eq!(ws_config.ping_interval, Duration::from_secs(30));
        assert_eq!(ws_config.max_connections, 100);
        assert_eq!(ws_config.max_message_size, 64 * 1024);
        assert_eq!(ws_config.live_update_interval, Duration::from_secs(2));
        assert_eq!(ws_config.live_update_max_jobs, 100);
    }

    #[test]
    fn test_live_update_settings_are_optional_and_validated() {
        // A configuration written before live updates existed still loads.
        let toml = r#"
            ping_interval = { secs = 30, nanos = 0 }
            max_connections = 10
            message_buffer_size = 16
            max_message_size = 1024
        "#;
        let ws: WebSocketConfig = toml::from_str(toml).unwrap();
        assert_eq!(ws.live_update_interval, Duration::from_secs(2));
        assert_eq!(ws.live_update_max_jobs, 100);

        let mut config = DashboardConfig::new();
        config.auth.enabled = false;
        config.websocket.live_update_max_jobs = 0;
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("live_update_max_jobs"), "{err}");
        // Disabled live updates do not need a limit.
        config.websocket.live_update_interval = Duration::ZERO;
        assert!(config.validate().is_ok());
    }
}
