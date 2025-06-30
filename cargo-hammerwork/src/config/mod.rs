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
//! - `HAMMERWORK_DEFAULT_QUEUE` - Default queue name
//! - `HAMMERWORK_DEFAULT_LIMIT` - Default limit for list operations
//! - `HAMMERWORK_LOG_LEVEL` - Logging level (error, warn, info, debug, trace)
//! - `HAMMERWORK_POOL_SIZE` - Database connection pool size

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: None,
            default_queue: None,
            default_limit: Some(50),
            log_level: Some("info".to_string()),
            connection_pool_size: Some(5),
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
        let mut config = Self::default();

        // Try to load from config file
        if let Ok(config_content) = Self::load_from_file() {
            config = config_content;
        }

        // Override with environment variables
        if let Ok(db_url) = env::var("DATABASE_URL") {
            config.database_url = Some(db_url);
        }

        if let Ok(queue) = env::var("HAMMERWORK_DEFAULT_QUEUE") {
            config.default_queue = Some(queue);
        }

        if let Ok(limit) = env::var("HAMMERWORK_DEFAULT_LIMIT") {
            if let Ok(limit_num) = limit.parse() {
                config.default_limit = Some(limit_num);
            }
        }

        if let Ok(log_level) = env::var("HAMMERWORK_LOG_LEVEL") {
            config.log_level = Some(log_level);
        }

        if let Ok(pool_size) = env::var("HAMMERWORK_POOL_SIZE") {
            if let Ok(size_num) = pool_size.parse() {
                config.connection_pool_size = Some(size_num);
            }
        }

        Ok(config)
    }

    fn load_from_file() -> Result<Self> {
        let config_path = Self::config_file_path()?;
        let content = fs::read_to_string(config_path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    /// Save configuration to file.
    ///
    /// The configuration is saved to the platform-specific location:
    /// - Linux/Mac: `~/.config/hammerwork/config.toml`
    /// - Windows: `%APPDATA%\hammerwork\config.toml`
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
        let config_path = Self::config_file_path()?;

        // Create parent directory if it doesn't exist
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        fs::write(config_path, content)?;
        Ok(())
    }

    fn config_file_path() -> Result<PathBuf> {
        let mut path = dirs::config_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| anyhow::anyhow!("Cannot find config directory"))?;

        path.push("hammerwork");
        path.push("config.toml");
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
}
