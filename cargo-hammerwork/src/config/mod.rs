//! Configuration management for the Hammerwork CLI.
//!
//! This module handles loading, saving, and managing configuration for the CLI tool.
//! Configuration can be loaded from multiple sources with the following precedence:
//!
//! 1. Environment variables (highest priority)
//! 2. Configuration file
//! 3. Default values (lowest priority)
//!
//! # Configuration File Location
//!
//! The configuration file is stored at platform-specific locations:
//!
//! - **Linux/Mac**: `~/.config/hammerwork/config.toml`
//! - **Windows**: `%APPDATA%\hammerwork\config.toml`
//!
//! # Examples
//!
//! ## Loading Configuration
//!
//! ```rust,no_run
//! use cargo_hammerwork::config::Config;
//!
//! // Load configuration from file and environment
//! let config = Config::load().expect("Failed to load config");
//!
//! // Access configuration values
//! if let Some(db_url) = config.get_database_url() {
//!     println!("Database URL: {}", db_url);
//! }
//!
//! println!("Default queue: {:?}", config.get_default_queue());
//! println!("Default limit: {}", config.get_default_limit());
//! ```
//!
//! ## Creating and Saving Configuration
//!
//! ```rust,no_run
//! use cargo_hammerwork::config::Config;
//!
//! // Create a new configuration with custom values
//! let mut config = Config::default();
//! config.database_url = Some("postgresql://localhost/hammerwork".to_string());
//! config.default_queue = Some("default".to_string());
//! config.default_limit = Some(100);
//! config.log_level = Some("debug".to_string());
//!
//! // Save configuration to file
//! config.save().expect("Failed to save config");
//! ```
//!
//! ## Environment Variable Override
//!
//! ```rust,no_run
//! use std::env;
//! use cargo_hammerwork::config::Config;
//!
//! // Set environment variables (unsafe in real code, safe for docs)
//! unsafe {
//!     env::set_var("DATABASE_URL", "postgresql://prod-server/hammerwork");
//!     env::set_var("HAMMERWORK_DEFAULT_QUEUE", "high-priority");
//!     env::set_var("HAMMERWORK_LOG_LEVEL", "warn");
//! }
//!
//! // Load config - environment variables take precedence
//! let config = Config::load().expect("Failed to load config");
//!
//! assert_eq!(config.get_database_url(), Some("postgresql://prod-server/hammerwork"));
//! assert_eq!(config.get_default_queue(), Some("high-priority"));
//! assert_eq!(config.get_log_level(), "warn");
//! ```
//!
//! ## Configuration File Format
//!
//! The configuration file uses TOML format:
//!
//! ```toml
//! database_url = "postgresql://localhost/hammerwork"
//! default_queue = "default"
//! default_limit = 50
//! log_level = "info"
//! connection_pool_size = 5
//! ```
//!
//! # Environment Variables
//!
//! The following environment variables are supported:
//!
//! - `DATABASE_URL` - Database connection URL
//! - `HAMMERWORK_CONFIG` - Path of the config file (default `hammerwork/config.toml` in the platform config directory)
//! - `HAMMERWORK_DEFAULT_QUEUE` - Default queue name
//! - `HAMMERWORK_DEFAULT_LIMIT` - Default limit for list operations
//! - `HAMMERWORK_LOG_LEVEL` - Logging level (error, warn, info, debug, trace)
//! - `HAMMERWORK_POOL_SIZE` - Database connection pool size
//! - `HAMMERWORK_WEBHOOKS_FILE` - Path of the webhook registry (default `webhooks.json` beside `config.toml`)

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// CLI configuration structure.
///
/// This struct holds all configuration options for the Hammerwork CLI.
/// Fields are optional to allow partial configuration and merging from multiple sources.
///
/// # Examples
///
/// ```rust
/// use cargo_hammerwork::config::Config;
///
/// // Create configuration with defaults
/// let config = Config::default();
/// assert_eq!(config.get_default_limit(), 50);
/// assert_eq!(config.get_log_level(), "info");
/// assert_eq!(config.get_connection_pool_size(), 5);
/// ```
///
/// `Debug` shows `database_url` with its password replaced by `***`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// Database connection URL (e.g., "postgresql://localhost/hammerwork")
    pub database_url: Option<String>,

    /// Default queue name for operations
    pub default_queue: Option<String>,

    /// Default limit for list operations
    pub default_limit: Option<u32>,

    /// Log level (error, warn, info, debug, trace)
    pub log_level: Option<String>,

    /// Database connection pool size
    pub connection_pool_size: Option<u32>,

    /// Seconds to wait for a database connection before failing (default 10)
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,

    /// Path of the application's `hammerwork.toml`, whose `[encryption]` section commands
    /// that write jobs (`job enqueue`, `batch enqueue`, `cron create`,
    /// `workflow create`) use, so they encrypt exactly like the application. See
    /// [`Config::encryption_settings`].
    #[serde(default)]
    pub encryption_config: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field(
                "database_url",
                &self
                    .database_url
                    .as_deref()
                    .map(crate::utils::validation::redact_url),
            )
            .field("default_queue", &self.default_queue)
            .field("default_limit", &self.default_limit)
            .field("log_level", &self.log_level)
            .field("connection_pool_size", &self.connection_pool_size)
            .field("connect_timeout_secs", &self.connect_timeout_secs)
            .field("encryption_config", &self.encryption_config)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: None,
            default_queue: None,
            default_limit: Some(50),
            log_level: Some("info".to_string()),
            connection_pool_size: Some(5),
            connect_timeout_secs: None,
            encryption_config: None,
        }
    }
}

impl Config {
    /// Load configuration from file and environment variables.
    ///
    /// Configuration is loaded with the following precedence:
    /// 1. Environment variables (highest priority)
    /// 2. Configuration file at `~/.config/hammerwork/config.toml`
    /// 3. Default values (lowest priority)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use cargo_hammerwork::config::Config;
    ///
    /// let config = Config::load().expect("Failed to load configuration");
    /// println!("Database URL: {:?}", config.database_url);
    /// ```
    pub fn load() -> Result<Self> {
        Self::load_with(|key| env::var(key).ok())
    }

    /// [`Config::load`] reading environment variables through `get`.
    ///
    /// A missing config file means defaults; a file that exists but cannot be read or parsed
    /// is an error, so a typo in `config.toml` is reported instead of silently ignored.
    pub fn load_with(get: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let path = Self::config_file_path_with(&get)?;
        let mut config = Self::load_from_path(&path)?;
        config.apply_env(&get);
        Ok(config)
    }

    /// Read the config file at `path`; defaults if it does not exist.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(content) => toml::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Invalid config file {}: {}", path.display(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(anyhow::anyhow!(
                "Cannot read config file {}: {}",
                path.display(),
                e
            )),
        }
    }

    /// Override fields with the `DATABASE_URL` / `HAMMERWORK_*` environment variables.
    /// Unparseable numbers are ignored.
    fn apply_env(&mut self, get: &impl Fn(&str) -> Option<String>) {
        if let Some(db_url) = get("DATABASE_URL") {
            self.database_url = Some(db_url);
        }
        if let Some(queue) = get("HAMMERWORK_DEFAULT_QUEUE") {
            self.default_queue = Some(queue);
        }
        if let Some(limit) = get("HAMMERWORK_DEFAULT_LIMIT").and_then(|v| v.parse().ok()) {
            self.default_limit = Some(limit);
        }
        if let Some(log_level) = get("HAMMERWORK_LOG_LEVEL") {
            self.log_level = Some(log_level);
        }
        if let Some(size) = get("HAMMERWORK_POOL_SIZE").and_then(|v| v.parse().ok()) {
            self.connection_pool_size = Some(size);
        }
        if let Some(secs) = get("HAMMERWORK_CONNECT_TIMEOUT").and_then(|v| v.parse().ok()) {
            self.connect_timeout_secs = Some(secs);
        }
        if let Some(path) = get("HAMMERWORK_ENCRYPTION_CONFIG").filter(|p| !p.is_empty()) {
            self.encryption_config = Some(path);
        }
    }

    /// The application's payload encryption settings, for commands that write jobs.
    ///
    /// The `[encryption]` section of [`Config::encryption_config`] (the application's
    /// `hammerwork.toml`; `HAMMERWORK_ENCRYPTION_CONFIG` overrides it), with the
    /// `HAMMERWORK_ENCRYPTION_*` environment variables applied on top, exactly as
    /// `HammerworkConfig::from_env` does for the application. Without either, encryption
    /// is disabled: the CLI then still refuses to write plaintext jobs to queues that hold
    /// encrypted jobs (see `DatabasePool::create_enqueue_queue`).
    pub fn encryption_settings(&self) -> Result<hammerwork::config::PayloadEncryptionConfig> {
        hammerwork::config::PayloadEncryptionConfig::load(
            self.encryption_config.as_deref().map(Path::new),
        )
        .map_err(|e| anyhow::anyhow!("Invalid encryption settings: {}", e))
    }

    /// Save configuration to file.
    ///
    /// The configuration is saved to [`Config::config_file_path`]:
    /// - `HAMMERWORK_CONFIG` if set
    /// - otherwise Linux/Mac: `~/.config/hammerwork/config.toml`, Windows:
    ///   `%APPDATA%\hammerwork\config.toml`
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use cargo_hammerwork::config::Config;
    ///
    /// let mut config = Config::default();
    /// config.database_url = Some("postgresql://localhost/hammerwork".to_string());
    /// config.save().expect("Failed to save configuration");
    /// ```
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::config_file_path()?)
    }

    /// Save configuration to `path`, creating parent directories.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Where the config file lives: `HAMMERWORK_CONFIG` if set, otherwise
    /// `hammerwork/config.toml` in the platform config directory.
    pub fn config_file_path() -> Result<PathBuf> {
        Self::config_file_path_with(&|key| env::var(key).ok())
    }

    fn config_file_path_with(get: &impl Fn(&str) -> Option<String>) -> Result<PathBuf> {
        if let Some(path) = get("HAMMERWORK_CONFIG").filter(|p| !p.is_empty()) {
            return Ok(PathBuf::from(path));
        }
        let mut path = dirs::config_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?;

        path.push("hammerwork");
        path.push("config.toml");
        Ok(path)
    }

    /// Path of the JSON file holding webhooks managed by `cargo hammerwork webhook`.
    ///
    /// Defaults to `webhooks.json` beside `config.toml`; `HAMMERWORK_WEBHOOKS_FILE`
    /// overrides it.
    pub fn webhooks_file_path(&self) -> Result<PathBuf> {
        if let Ok(path) = env::var("HAMMERWORK_WEBHOOKS_FILE") {
            return Ok(PathBuf::from(path));
        }
        let mut path = Self::config_file_path()?;
        path.set_file_name("webhooks.json");
        Ok(path)
    }

    pub fn get_database_url(&self) -> Option<&str> {
        self.database_url.as_deref()
    }

    pub fn get_default_queue(&self) -> Option<&str> {
        self.default_queue.as_deref()
    }

    pub fn get_default_limit(&self) -> u32 {
        self.default_limit.unwrap_or(50)
    }

    pub fn get_log_level(&self) -> &str {
        self.log_level.as_deref().unwrap_or("info")
    }

    pub fn get_connection_pool_size(&self) -> u32 {
        self.connection_pool_size.unwrap_or(5)
    }

    /// Seconds to wait when connecting to the database (default 10, never 0).
    pub fn get_connect_timeout_secs(&self) -> u64 {
        self.connect_timeout_secs
            .unwrap_or(crate::utils::database::DEFAULT_CONNECT_TIMEOUT_SECS)
            .max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn connect_timeout_defaults_to_ten_seconds_and_can_be_overridden() {
        let mut config = Config::default();
        assert_eq!(config.get_connect_timeout_secs(), 10);
        config.apply_env(&env_of(&[("HAMMERWORK_CONNECT_TIMEOUT", "3")]));
        assert_eq!(config.get_connect_timeout_secs(), 3);
        config.apply_env(&env_of(&[("HAMMERWORK_CONNECT_TIMEOUT", "soon")]));
        assert_eq!(config.get_connect_timeout_secs(), 3);
        config.connect_timeout_secs = Some(0);
        assert_eq!(
            config.get_connect_timeout_secs(),
            1,
            "0 would never connect"
        );
    }

    #[test]
    fn defaults_and_getters() {
        let config = Config::default();
        assert_eq!(config.get_database_url(), None);
        assert_eq!(config.get_default_queue(), None);
        assert_eq!(config.get_default_limit(), 50);
        assert_eq!(config.get_log_level(), "info");
        assert_eq!(config.get_connection_pool_size(), 5);

        // Unset optional fields fall back to the same defaults.
        let bare = Config {
            database_url: None,
            default_queue: None,
            default_limit: None,
            log_level: None,
            connection_pool_size: None,
            connect_timeout_secs: None,
            encryption_config: None,
        };
        assert_eq!(bare.get_default_limit(), 50);
        assert_eq!(bare.get_log_level(), "info");
        assert_eq!(bare.get_connection_pool_size(), 5);
    }

    #[test]
    fn environment_overrides_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "database_url = \"postgres://file/db\"\ndefault_queue = \"from_file\"\ndefault_limit = 7\n",
        )
        .unwrap();

        let from_file = Config::load_from_path(&path).unwrap();
        assert_eq!(from_file.get_database_url(), Some("postgres://file/db"));
        assert_eq!(from_file.get_default_queue(), Some("from_file"));
        assert_eq!(from_file.get_default_limit(), 7);

        let env = env_of(&[
            ("HAMMERWORK_CONFIG", path.to_str().unwrap()),
            ("DATABASE_URL", "mysql://env/db"),
            ("HAMMERWORK_DEFAULT_LIMIT", "99"),
            ("HAMMERWORK_LOG_LEVEL", "trace"),
            ("HAMMERWORK_POOL_SIZE", "12"),
        ]);
        let config = Config::load_with(env).unwrap();
        assert_eq!(config.get_database_url(), Some("mysql://env/db"));
        assert_eq!(config.get_default_queue(), Some("from_file"));
        assert_eq!(config.get_default_limit(), 99);
        assert_eq!(config.get_log_level(), "trace");
        assert_eq!(config.get_connection_pool_size(), 12);

        let env = env_of(&[
            ("HAMMERWORK_CONFIG", path.to_str().unwrap()),
            ("HAMMERWORK_DEFAULT_QUEUE", "from_env"),
        ]);
        let config = Config::load_with(env).unwrap();
        assert_eq!(config.get_default_queue(), Some("from_env"));
        assert_eq!(config.get_database_url(), Some("postgres://file/db"));
    }

    #[test]
    fn unparseable_numeric_environment_values_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_of(&[
            (
                "HAMMERWORK_CONFIG",
                dir.path().join("missing.toml").to_str().unwrap(),
            ),
            ("HAMMERWORK_DEFAULT_LIMIT", "lots"),
            ("HAMMERWORK_POOL_SIZE", "-3"),
        ]);
        let config = Config::load_with(env).unwrap();
        assert_eq!(config.get_default_limit(), 50);
        assert_eq!(config.get_connection_pool_size(), 5);
    }

    #[test]
    fn missing_file_is_defaults_but_corrupt_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let config = Config::load_from_path(&missing).unwrap();
        assert_eq!(config.get_default_limit(), 50);

        let corrupt = dir.path().join("config.toml");
        std::fs::write(&corrupt, "default_limit = \"many\"").unwrap();
        let err = Config::load_from_path(&corrupt).unwrap_err().to_string();
        assert!(err.contains("Invalid config file"), "{err}");
        assert!(err.contains("config.toml"), "{err}");

        // A directory where the file should be is an I/O error, not "no config".
        let err = Config::load_from_path(dir.path()).unwrap_err().to_string();
        assert!(err.contains("Cannot read config file"), "{err}");
    }

    #[test]
    fn save_creates_parent_directories_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deeper").join("config.toml");
        let config = Config {
            database_url: Some("postgres://u@h/d".into()),
            default_queue: Some("emails".into()),
            default_limit: Some(11),
            log_level: Some("warn".into()),
            connection_pool_size: Some(3),
            connect_timeout_secs: Some(7),
            encryption_config: None,
        };
        config.save_to(&path).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.get_database_url(), Some("postgres://u@h/d"));
        assert_eq!(loaded.get_default_queue(), Some("emails"));
        assert_eq!(loaded.get_default_limit(), 11);
        assert_eq!(loaded.get_log_level(), "warn");
        assert_eq!(loaded.get_connection_pool_size(), 3);
    }

    #[test]
    fn config_path_honours_hammerwork_config_and_webhooks_sit_beside_it() {
        let env = env_of(&[("HAMMERWORK_CONFIG", "/tmp/custom/cfg.toml")]);
        assert_eq!(
            Config::config_file_path_with(&env).unwrap(),
            PathBuf::from("/tmp/custom/cfg.toml")
        );

        // Empty means unset.
        let env = env_of(&[("HAMMERWORK_CONFIG", "")]);
        let default = Config::config_file_path_with(&env).unwrap();
        assert!(default.ends_with("hammerwork/config.toml"), "{default:?}");
    }

    #[test]
    fn real_environment_selects_the_config_file() {
        let _guard = crate::utils::test_support::serial_blocking();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        let webhooks = dir.path().join("hooks.json");
        // SAFETY: serialized with every other test that reads these variables.
        unsafe {
            env::set_var("HAMMERWORK_CONFIG", &path);
            env::set_var("HAMMERWORK_WEBHOOKS_FILE", &webhooks);
        }
        assert_eq!(Config::config_file_path().unwrap(), path);
        assert_eq!(Config::default().webhooks_file_path().unwrap(), webhooks);

        let config = Config {
            default_queue: Some("via_env_path".into()),
            ..Config::default()
        };
        config.save().unwrap();
        assert!(path.exists());

        unsafe {
            env::remove_var("HAMMERWORK_WEBHOOKS_FILE");
        }
        assert_eq!(
            Config::default().webhooks_file_path().unwrap(),
            dir.path().join("webhooks.json")
        );
        let loaded = Config::load().unwrap();
        assert_eq!(loaded.get_default_queue(), Some("via_env_path"));
        unsafe {
            env::remove_var("HAMMERWORK_CONFIG");
        }
    }
}
