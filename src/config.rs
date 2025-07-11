//! Configuration management for Hammerwork job queue.
//!
//! This module provides comprehensive configuration options for the Hammerwork job queue,
//! including database settings, worker configuration, webhook settings, streaming configuration,
//! and monitoring options.

use crate::{
    events::EventConfig, priority::PriorityWeights, rate_limit::ThrottleConfig,
    retry::RetryStrategy,
};

#[cfg(feature = "webhooks")]
use crate::{
    events::EventFilter,
    streaming::{
        BufferConfig, PartitioningStrategy, SerializationFormat, StreamBackend, StreamRetryPolicy,
    },
    webhooks::{HttpMethod, RetryPolicy as WebhookRetryPolicy, WebhookAuth},
};

#[cfg(feature = "alerting")]
use crate::alerting::AlertingConfig;

#[cfg(feature = "metrics")]
use crate::metrics::MetricsConfig;

use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf, time::Duration as StdDuration};

/// Module for serializing std::time::Duration as seconds
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_secs())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(deserializer)?;
        Ok(Duration::from_secs(secs))
    }
}
use uuid::Uuid;

/// Main configuration for the Hammerwork job queue system.
///
/// This struct contains all configuration options for the Hammerwork job queue,
/// including database connection, worker settings, webhook configuration,
/// streaming settings, and monitoring options.
///
/// # Examples
///
/// ```rust
/// use hammerwork::config::HammerworkConfig;
///
/// // Create with defaults
/// let config = HammerworkConfig::default();
///
/// // Use builder pattern
/// let config = HammerworkConfig::new()
///     .with_database_url("postgresql://localhost/hammerwork")
///     .with_worker_pool_size(5)
///     .with_job_timeout(std::time::Duration::from_secs(300));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HammerworkConfig {
    /// Database configuration
    pub database: DatabaseConfig,

    /// Worker configuration
    pub worker: WorkerConfig,

    /// Event system configuration
    pub events: EventConfig,

    /// Webhook configurations
    #[cfg(feature = "webhooks")]
    pub webhooks: WebhookConfigs,

    /// Streaming configurations
    pub streaming: StreamingConfigs,

    /// Alerting configuration
    #[cfg(feature = "alerting")]
    pub alerting: AlertingConfig,

    /// Metrics configuration
    #[cfg(feature = "metrics")]
    pub metrics: MetricsConfig,

    /// Archive configuration
    pub archive: ArchiveConfig,

    /// Rate limiting configuration
    pub rate_limiting: RateLimitingConfig,

    /// Logging and tracing configuration
    pub logging: LoggingConfig,
}

impl HammerworkConfig {
    /// Create a new configuration with defaults
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the database URL
    pub fn with_database_url(mut self, url: &str) -> Self {
        self.database.url = url.to_string();
        self
    }

    /// Set the database pool size
    pub fn with_database_pool_size(mut self, size: u32) -> Self {
        self.database.pool_size = size;
        self
    }

    /// Set the worker pool size
    pub fn with_worker_pool_size(mut self, size: usize) -> Self {
        self.worker.pool_size = size;
        self
    }

    /// Set job timeout duration
    pub fn with_job_timeout(mut self, timeout: StdDuration) -> Self {
        self.worker.job_timeout = timeout;
        self
    }

    /// Enable or disable event publishing
    pub fn with_events_enabled(mut self, enabled: bool) -> Self {
        if enabled {
            self.events.max_buffer_size = 10_000;
        } else {
            self.events.max_buffer_size = 0;
        }
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

    /// Load configuration from environment variables
    pub fn from_env() -> crate::Result<Self> {
        let mut config = Self::default();

        // Database configuration
        if let Ok(url) = std::env::var("HAMMERWORK_DATABASE_URL") {
            config.database.url = url;
        }
        if let Ok(pool_size) = std::env::var("HAMMERWORK_DATABASE_POOL_SIZE") {
            config.database.pool_size = pool_size.parse().unwrap_or(config.database.pool_size);
        }

        // Worker configuration
        if let Ok(pool_size) = std::env::var("HAMMERWORK_WORKER_POOL_SIZE") {
            config.worker.pool_size = pool_size.parse().unwrap_or(config.worker.pool_size);
        }
        if let Ok(timeout) = std::env::var("HAMMERWORK_JOB_TIMEOUT_SECONDS") {
            if let Ok(seconds) = timeout.parse::<u64>() {
                config.worker.job_timeout = StdDuration::from_secs(seconds);
            }
        }

        // Event configuration
        if let Ok(buffer_size) = std::env::var("HAMMERWORK_EVENT_BUFFER_SIZE") {
            config.events.max_buffer_size =
                buffer_size.parse().unwrap_or(config.events.max_buffer_size);
        }

        Ok(config)
    }
}

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Database connection URL
    pub url: String,

    /// Connection pool size
    pub pool_size: u32,

    /// Connection timeout in seconds
    pub connection_timeout_secs: u64,

    /// Whether to run migrations automatically
    pub auto_migrate: bool,

    /// Whether to create tables if they don't exist
    pub create_tables: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgresql://localhost/hammerwork".to_string(),
            pool_size: 10,
            connection_timeout_secs: 30,
            auto_migrate: false,
            create_tables: true,
        }
    }
}

/// Worker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Number of workers in the pool
    pub pool_size: usize,

    /// Polling interval for checking new jobs
    #[serde(with = "duration_secs")]
    pub polling_interval: StdDuration,

    /// Default job timeout
    #[serde(with = "duration_secs")]
    pub job_timeout: StdDuration,

    /// Priority weights for job selection
    pub priority_weights: PriorityWeights,

    /// Retry strategy for failed jobs
    pub retry_strategy: RetryStrategy,

    /// Whether to enable autoscaling
    pub autoscaling_enabled: bool,

    /// Minimum number of workers (for autoscaling)
    pub min_workers: usize,

    /// Maximum number of workers (for autoscaling)
    pub max_workers: usize,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            pool_size: 4,
            polling_interval: StdDuration::from_millis(500),
            job_timeout: StdDuration::from_secs(300), // 5 minutes
            priority_weights: PriorityWeights::default(),
            retry_strategy: RetryStrategy::exponential(
                StdDuration::from_secs(1),
                2.0,
                Some(StdDuration::from_secs(300)),
            ),
            autoscaling_enabled: false,
            min_workers: 1,
            max_workers: 16,
        }
    }
}

/// Webhook configurations container
#[cfg(feature = "webhooks")]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebhookConfigs {
    /// List of configured webhooks
    pub webhooks: Vec<WebhookConfig>,

    /// Global webhook settings
    pub global_settings: WebhookGlobalSettings,
}

/// Individual webhook configuration
#[cfg(feature = "webhooks")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Unique identifier
    pub id: Uuid,

    /// Human-readable name
    pub name: String,

    /// Webhook URL
    pub url: String,

    /// HTTP method
    pub method: HttpMethod,

    /// Custom headers
    pub headers: HashMap<String, String>,

    /// Event filter
    pub filter: EventFilter,

    /// Retry policy
    pub retry_policy: WebhookRetryPolicy,

    /// Authentication
    pub auth: Option<WebhookAuth>,

    /// Request timeout in seconds
    pub timeout_secs: u64,

    /// Whether enabled
    pub enabled: bool,

    /// Secret for HMAC signatures
    pub secret: Option<String>,
}

#[cfg(feature = "webhooks")]
impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Default Webhook".to_string(),
            url: "https://example.com/webhook".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            filter: EventFilter::default(),
            retry_policy: WebhookRetryPolicy::default(),
            auth: None,
            timeout_secs: 30,
            enabled: true,
            secret: None,
        }
    }
}

/// Global webhook settings
#[cfg(feature = "webhooks")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookGlobalSettings {
    /// Maximum concurrent webhook deliveries
    pub max_concurrent_deliveries: usize,

    /// Maximum response body size to store
    pub max_response_body_size: usize,

    /// Whether to log webhook deliveries
    pub log_deliveries: bool,

    /// User agent string for requests
    pub user_agent: String,
}

#[cfg(feature = "webhooks")]
impl Default for WebhookGlobalSettings {
    fn default() -> Self {
        Self {
            max_concurrent_deliveries: 100,
            max_response_body_size: 64 * 1024, // 64KB
            log_deliveries: true,
            user_agent: format!("hammerwork-webhooks/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

/// Streaming configurations container
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamingConfigs {
    /// List of configured streams
    pub streams: Vec<StreamConfig>,

    /// Global streaming settings
    pub global_settings: StreamingGlobalSettings,
}

/// Individual stream configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Unique identifier
    pub id: Uuid,

    /// Human-readable name
    pub name: String,

    /// Streaming backend
    pub backend: StreamBackend,

    /// Event filter
    #[cfg(feature = "webhooks")]
    pub filter: EventFilter,

    #[cfg(not(feature = "webhooks"))]
    /// Simple event filter when webhooks feature is disabled
    pub filter: SimpleEventFilter,

    /// Partitioning strategy
    pub partitioning: PartitioningStrategy,

    /// Serialization format
    pub serialization: SerializationFormat,

    /// Retry policy
    pub retry_policy: StreamRetryPolicy,

    /// Whether enabled
    pub enabled: bool,

    /// Buffer configuration
    pub buffer_config: BufferConfig,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Default Stream".to_string(),
            backend: StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "hammerwork-events".to_string(),
                config: HashMap::new(),
            },
            #[cfg(feature = "webhooks")]
            filter: EventFilter::default(),
            #[cfg(not(feature = "webhooks"))]
            filter: SimpleEventFilter::default(),
            partitioning: PartitioningStrategy::QueueName,
            serialization: SerializationFormat::Json,
            retry_policy: StreamRetryPolicy::default(),
            enabled: true,
            buffer_config: BufferConfig::default(),
        }
    }
}

/// Simple event filter for when webhooks feature is disabled
#[cfg(not(feature = "webhooks"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleEventFilter {
    /// Event types to include
    pub event_types: Vec<String>,

    /// Queue names to include
    pub queue_names: Vec<String>,

    /// Whether to include payload data
    pub include_payload: bool,
}

#[cfg(not(feature = "webhooks"))]
impl Default for SimpleEventFilter {
    fn default() -> Self {
        Self {
            event_types: vec!["completed".to_string(), "failed".to_string()],
            queue_names: Vec::new(),
            include_payload: false,
        }
    }
}

/// Global streaming settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingGlobalSettings {
    /// Maximum concurrent stream processors
    pub max_concurrent_processors: usize,

    /// Whether to log stream operations
    pub log_operations: bool,

    /// Global buffer flush interval in seconds
    pub global_flush_interval_secs: u64,
}

impl Default for StreamingGlobalSettings {
    fn default() -> Self {
        Self {
            max_concurrent_processors: 50,
            log_operations: true,
            global_flush_interval_secs: 10,
        }
    }
}

/// Archive configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveConfig {
    /// Whether archiving is enabled
    pub enabled: bool,

    /// Directory for storing archived jobs
    pub archive_directory: PathBuf,

    /// Compression level (0-9, 0=no compression)
    pub compression_level: u32,

    /// Archive jobs older than this duration
    pub archive_after: Duration,

    /// Delete archived files older than this duration
    pub delete_after: Option<Duration>,

    /// Maximum archive file size in bytes
    pub max_file_size_bytes: u64,

    /// Whether to include job payloads in archives
    pub include_payloads: bool,
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            archive_directory: PathBuf::from("./archives"),
            compression_level: 6,
            archive_after: Duration::days(30),
            delete_after: Some(Duration::days(365)),
            max_file_size_bytes: 100 * 1024 * 1024, // 100MB
            include_payloads: true,
        }
    }
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RateLimitingConfig {
    /// Whether rate limiting is enabled
    pub enabled: bool,

    /// Default throttle configuration
    pub default_throttle: ThrottleConfig,

    /// Per-queue throttle configurations
    pub queue_throttles: HashMap<String, ThrottleConfig>,
}

/// Logging and tracing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    pub level: String,

    /// Whether to enable structured JSON logging
    pub json_format: bool,

    /// Whether to include file and line information
    pub include_location: bool,

    /// Whether to enable OpenTelemetry tracing
    pub enable_tracing: bool,

    /// OpenTelemetry endpoint URL
    pub tracing_endpoint: Option<String>,

    /// Service name for tracing
    pub service_name: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            json_format: false,
            include_location: false,
            enable_tracing: false,
            tracing_endpoint: None,
            service_name: "hammerwork".to_string(),
        }
    }
}

/// Helper functions for creating configurations
impl HammerworkConfig {
    /// Create a configuration for development use
    pub fn development() -> Self {
        Self {
            database: DatabaseConfig {
                url: "postgresql://localhost/hammerwork_dev".to_string(),
                pool_size: 5,
                auto_migrate: true,
                ..Default::default()
            },
            worker: WorkerConfig {
                pool_size: 2,
                polling_interval: StdDuration::from_millis(100),
                ..Default::default()
            },
            events: EventConfig {
                max_buffer_size: 1000,
                log_events: true,
                ..Default::default()
            },
            logging: LoggingConfig {
                level: "debug".to_string(),
                include_location: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Create a configuration for production use
    pub fn production() -> Self {
        Self {
            database: DatabaseConfig {
                pool_size: 20,
                connection_timeout_secs: 60,
                auto_migrate: false,
                ..Default::default()
            },
            worker: WorkerConfig {
                pool_size: 8,
                autoscaling_enabled: true,
                min_workers: 4,
                max_workers: 32,
                ..Default::default()
            },
            events: EventConfig {
                max_buffer_size: 50_000,
                log_events: false,
                ..Default::default()
            },
            archive: ArchiveConfig {
                enabled: true,
                compression_level: 9,
                ..Default::default()
            },
            rate_limiting: RateLimitingConfig {
                enabled: true,
                ..Default::default()
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                json_format: true,
                enable_tracing: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Add a webhook configuration
    #[cfg(feature = "webhooks")]
    pub fn add_webhook(mut self, webhook: WebhookConfig) -> Self {
        self.webhooks.webhooks.push(webhook);
        self
    }

    /// Add a stream configuration
    pub fn add_stream(mut self, stream: StreamConfig) -> Self {
        self.streaming.streams.push(stream);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_creation() {
        let config = HammerworkConfig::new()
            .with_database_url("postgresql://localhost/test")
            .with_worker_pool_size(8)
            .with_job_timeout(StdDuration::from_secs(600));

        assert_eq!(config.database.url, "postgresql://localhost/test");
        assert_eq!(config.worker.pool_size, 8);
        assert_eq!(config.worker.job_timeout, StdDuration::from_secs(600));
    }

    #[test]
    fn test_development_config() {
        let config = HammerworkConfig::development();
        assert_eq!(config.database.url, "postgresql://localhost/hammerwork_dev");
        assert_eq!(config.worker.pool_size, 2);
        assert!(config.database.auto_migrate);
        assert_eq!(config.logging.level, "debug");
    }

    #[test]
    fn test_production_config() {
        let config = HammerworkConfig::production();
        assert_eq!(config.database.pool_size, 20);
        assert_eq!(config.worker.pool_size, 8);
        assert!(config.worker.autoscaling_enabled);
        assert!(config.archive.enabled);
        assert!(config.rate_limiting.enabled);
        assert!(config.logging.json_format);
    }

    #[test]
    #[ignore] // TODO: Fix Duration serialization in TOML
    fn test_config_file_operations() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("hammerwork.toml");

        let config = HammerworkConfig::new()
            .with_database_url("mysql://localhost/test")
            .with_worker_pool_size(6);

        // Save config
        config.save_to_file(config_path.to_str().unwrap()).unwrap();

        // Load config
        let loaded_config = HammerworkConfig::from_file(config_path.to_str().unwrap()).unwrap();

        assert_eq!(loaded_config.database.url, "mysql://localhost/test");
        assert_eq!(loaded_config.worker.pool_size, 6);
    }

    #[test]
    fn test_env_config() {
        unsafe {
            std::env::set_var("HAMMERWORK_DATABASE_URL", "postgresql://env/test");
            std::env::set_var("HAMMERWORK_WORKER_POOL_SIZE", "12");
            std::env::set_var("HAMMERWORK_JOB_TIMEOUT_SECONDS", "900");
        }

        let config = HammerworkConfig::from_env().unwrap();

        assert_eq!(config.database.url, "postgresql://env/test");
        assert_eq!(config.worker.pool_size, 12);
        assert_eq!(config.worker.job_timeout, StdDuration::from_secs(900));

        // Clean up
        unsafe {
            std::env::remove_var("HAMMERWORK_DATABASE_URL");
            std::env::remove_var("HAMMERWORK_WORKER_POOL_SIZE");
            std::env::remove_var("HAMMERWORK_JOB_TIMEOUT_SECONDS");
        }
    }

    #[cfg(feature = "webhooks")]
    #[test]
    fn test_webhook_config() {
        let webhook = WebhookConfig {
            name: "Test Webhook".to_string(),
            url: "https://api.example.com/webhook".to_string(),
            ..Default::default()
        };

        let config = HammerworkConfig::new().add_webhook(webhook);
        assert_eq!(config.webhooks.webhooks.len(), 1);
        assert_eq!(config.webhooks.webhooks[0].name, "Test Webhook");
    }

    #[test]
    fn test_stream_config() {
        let stream = StreamConfig {
            name: "Test Stream".to_string(),
            backend: StreamBackend::PubSub {
                project_id: "test-project".to_string(),
                topic_name: "test-topic".to_string(),
                service_account_key: None,
                config: HashMap::new(),
            },
            ..Default::default()
        };

        let config = HammerworkConfig::new().add_stream(stream);
        assert_eq!(config.streaming.streams.len(), 1);
        assert_eq!(config.streaming.streams[0].name, "Test Stream");
    }

    #[test]
    fn test_default_configs() {
        let database_config = DatabaseConfig::default();
        assert_eq!(database_config.url, "postgresql://localhost/hammerwork");
        assert_eq!(database_config.pool_size, 10);

        let worker_config = WorkerConfig::default();
        assert_eq!(worker_config.pool_size, 4);
        assert_eq!(
            worker_config.polling_interval,
            StdDuration::from_millis(500)
        );

        let archive_config = ArchiveConfig::default();
        assert!(!archive_config.enabled);
        assert_eq!(archive_config.compression_level, 6);

        let logging_config = LoggingConfig::default();
        assert_eq!(logging_config.level, "info");
        assert!(!logging_config.json_format);
    }
}
