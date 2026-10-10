//! Configuration management for Hammerwork job queue.
//!
//! This module provides comprehensive configuration options for the Hammerwork job queue,
//! including database settings, worker configuration, webhook settings, streaming configuration,
//! and monitoring options.

use crate::{
    archive::{ArchivalConfig, ArchivalPolicy},
    events::EventConfig,
    priority::PriorityWeights,
    rate_limit::ThrottleConfig,
    retry::RetryStrategy,
    worker::AutoscaleConfig,
};

#[cfg(feature = "webhooks")]
use crate::webhooks::WebhookConfig;

#[cfg(feature = "alerting")]
use crate::alerting::AlertingConfig;

#[cfg(feature = "metrics")]
use crate::metrics::MetricsConfig;

#[cfg(any(
    feature = "streaming",
    feature = "kafka",
    feature = "google-pubsub",
    feature = "kinesis"
))]
use crate::streaming::StreamConfig;
use chrono::Duration;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration as StdDuration};

/// Serde support for [`std::time::Duration`] as human-readable strings, for use with
/// `#[serde(with = "hammerwork::config::serde_duration")]`.
///
/// Durations are written as `"500ms"`, `"30s"`, `"5m"` or `"1h"`. Reading accepts those
/// forms plus `"2d"`, a bare number of seconds (`90` or `"90"`), and serde's default
/// `{ secs = 30, nanos = 0 }` table, so files written before a field switched to this
/// format still load.
pub mod serde_duration {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let secs = duration.as_secs();
        if duration.subsec_nanos() != 0 {
            // Sub-second precision (e.g. the default 500ms polling interval) is kept in
            // milliseconds; whole seconds would round it down, to zero for short ones.
            let millis = duration.as_millis().max(1);
            serializer.serialize_str(&format!("{millis}ms"))
        } else if secs == 0 {
            serializer.serialize_str("0s")
        } else if secs.is_multiple_of(3600) {
            serializer.serialize_str(&format!("{}h", secs / 3600))
        } else if secs.is_multiple_of(60) {
            serializer.serialize_str(&format!("{}m", secs / 60))
        } else {
            serializer.serialize_str(&format!("{}s", secs))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Text(String),
            Seconds(u64),
            Table { secs: u64, nanos: u32 },
        }

        match Repr::deserialize(deserializer)? {
            Repr::Text(s) => parse_duration(&s).map_err(D::Error::custom),
            Repr::Seconds(secs) => Ok(Duration::from_secs(secs)),
            Repr::Table { secs, nanos } => {
                if nanos >= 1_000_000_000 {
                    return Err(D::Error::custom("nanos must be below 1000000000"));
                }
                Ok(Duration::new(secs, nanos))
            }
        }
    }

    fn checked_secs(num: u64, unit: u64) -> Result<Duration, String> {
        num.checked_mul(unit)
            .map(Duration::from_secs)
            .ok_or_else(|| format!("Duration too large: {num} x {unit}s"))
    }

    /// Parse a duration string like "500ms", "30s", "5m", "1h", "90", etc.
    fn parse_duration(s: &str) -> Result<Duration, String> {
        let s = s.trim();

        // Handle just numbers (assume seconds)
        if let Ok(secs) = s.parse::<u64>() {
            return Ok(Duration::from_secs(secs));
        }

        if let Some(millis) = s.strip_suffix("ms") {
            let millis: u64 = millis
                .trim()
                .parse()
                .map_err(|_| format!("Invalid number in duration: {}", millis))?;
            return Ok(Duration::from_millis(millis));
        }

        // Handle suffixed durations
        if s.len() < 2 {
            return Err(format!("Invalid duration format: {}", s));
        }

        let (num_str, suffix) = s.split_at(s.len() - 1);
        let num: u64 = num_str
            .parse()
            .map_err(|_| format!("Invalid number in duration: {}", num_str))?;

        match suffix {
            "s" => Ok(Duration::from_secs(num)),
            "m" => checked_secs(num, 60),
            "h" => checked_secs(num, 3600),
            "d" => checked_secs(num, 86400),
            _ => Err(format!(
                "Invalid duration suffix: {}. Use s, m, h, or d",
                suffix
            )),
        }
    }
}

/// Module for serializing chrono::Duration as human-readable strings (in days)
mod chrono_duration_days {
    use chrono::Duration;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let days = duration.num_days();
        if days == 0 {
            serializer.serialize_str("0d")
        } else {
            serializer.serialize_str(&format!("{}d", days))
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        let s = String::deserialize(deserializer)?;
        parse_chrono_duration(&s).map_err(D::Error::custom)
    }

    /// Parse a duration string like "30d", "7d", etc.
    pub fn parse_chrono_duration(s: &str) -> Result<Duration, String> {
        let s = s.trim();

        // Handle just numbers (assume days)
        if let Ok(days) = s.parse::<i64>() {
            return Ok(Duration::days(days));
        }

        // Handle suffixed durations
        if s.len() < 2 {
            return Err(format!("Invalid duration format: {}", s));
        }

        let (num_str, suffix) = s.split_at(s.len() - 1);
        let num: i64 = num_str
            .parse()
            .map_err(|_| format!("Invalid number in duration: {}", num_str))?;

        match suffix {
            "d" => Ok(Duration::days(num)),
            _ => Err(format!(
                "Invalid duration suffix: {}. Use d for days",
                suffix
            )),
        }
    }
}

/// Module for serializing Option<chrono::Duration> as human-readable strings (in days)
mod chrono_duration_days_option {
    use chrono::Duration;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => super::chrono_duration_days::serialize(d, serializer),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(s) => super::chrono_duration_days::parse_chrono_duration(&s)
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

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
    #[cfg(any(
        feature = "streaming",
        feature = "kafka",
        feature = "google-pubsub",
        feature = "kinesis"
    ))]
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

    /// Job payload encryption (`[encryption]`). Optional in TOML files; disabled by
    /// default.
    #[serde(default)]
    pub encryption: PayloadEncryptionConfig,
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
        config.worker.validate()?;
        config.archive.validate()?;
        config.validate_delivery_targets()?;
        Ok(config)
    }

    /// Reject alert and webhook settings that could never deliver anything (an email
    /// alert target without SMTP settings, an invalid webhook payload template), so
    /// they fail when the file is loaded instead of when the first alert or event fires.
    fn validate_delivery_targets(&self) -> crate::Result<()> {
        #[cfg(feature = "alerting")]
        self.alerting.validate()?;
        #[cfg(feature = "webhooks")]
        for webhook in &self.webhooks.webhooks {
            webhook.validate()?;
        }
        Ok(())
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
        if let Ok(timeout) = std::env::var("HAMMERWORK_JOB_TIMEOUT_SECONDS")
            && let Ok(seconds) = timeout.parse::<u64>()
        {
            config.worker.job_timeout = StdDuration::from_secs(seconds);
        }

        // Event configuration
        if let Ok(buffer_size) = std::env::var("HAMMERWORK_EVENT_BUFFER_SIZE") {
            config.events.max_buffer_size =
                buffer_size.parse().unwrap_or(config.events.max_buffer_size);
        }

        config.encryption.apply_env()?;
        config.worker.validate()?;

        Ok(config)
    }
}

/// Database configuration
///
/// `Debug` shows `url` with its password replaced by `***` ([`redact_url`]).
#[derive(Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Database connection URL
    pub url: String,

    /// Connection pool size
    pub pool_size: u32,

    /// Connection timeout in seconds
    pub connection_timeout_secs: u64,

    /// Whether to run migrations automatically when connecting with
    /// [`JobQueue::from_config`](crate::JobQueue)
    pub auto_migrate: bool,
}

impl std::fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("url", &redact_url(&self.url))
            .field("pool_size", &self.pool_size)
            .field("connection_timeout_secs", &self.connection_timeout_secs)
            .field("auto_migrate", &self.auto_migrate)
            .finish()
    }
}

/// `url` with the password in its user info replaced by `***`, safe to print or log:
/// `postgres://app:s3cret@db/jobs` becomes `postgres://app:***@db/jobs`. A URL without
/// a password is returned unchanged.
///
/// ```
/// use hammerwork::config::redact_url;
///
/// assert_eq!(
///     redact_url("postgres://app:s3cret@db:5432/jobs"),
///     "postgres://app:***@db:5432/jobs"
/// );
/// assert_eq!(redact_url("mysql://db/jobs"), "mysql://db/jobs");
/// ```
pub fn redact_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    // Credentials end at the last '@' before the first '/' of the path.
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let Some(at) = rest[..authority_end].rfind('@') else {
        return url.to_string();
    };
    let (userinfo, tail) = rest.split_at(at);
    match userinfo.split_once(':') {
        Some((user, _password)) => format!("{scheme}://{user}:***{tail}"),
        None => url.to_string(),
    }
}

/// Only the scheme and host of `url`, safe to print or log for URLs whose path or query
/// is itself a credential (Slack, Discord and Teams webhook URLs, signed URLs):
/// `https://hooks.slack.com/services/T0/B0/xyz` becomes `https://hooks.slack.com/***`.
/// User info is dropped. Something that is not a URL is replaced by `***`.
///
/// ```
/// use hammerwork::config::redact_url_path;
///
/// assert_eq!(
///     redact_url_path("https://hooks.slack.com/services/T0/B0/xyz"),
///     "https://hooks.slack.com/***"
/// );
/// assert_eq!(redact_url_path("https://example.com"), "https://example.com");
/// ```
pub fn redact_url_path(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "***".to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if authority_end == rest.len() || rest[authority_end..] == *"/" {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}/***")
    }
}

impl DatabaseConfig {
    /// Connection acquire timeout as a [`std::time::Duration`]
    pub fn connection_timeout(&self) -> StdDuration {
        StdDuration::from_secs(self.connection_timeout_secs)
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: "postgresql://localhost/hammerwork".to_string(),
            pool_size: 10,
            connection_timeout_secs: 30,
            auto_migrate: false,
        }
    }
}

/// Worker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Number of workers in the pool
    pub pool_size: usize,

    /// Polling interval for checking new jobs. Must be greater than zero (see
    /// [`WorkerConfig::validate`]).
    #[serde(with = "serde_duration")]
    pub polling_interval: StdDuration,

    /// Default job timeout, applied by [`Worker::with_config`](crate::Worker::with_config)
    /// to every job that has no timeout of its own (default 5 minutes). Must be greater
    /// than zero (see [`WorkerConfig::validate`]).
    #[serde(with = "serde_duration")]
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

impl WorkerConfig {
    /// Reject settings that cannot work: a zero `polling_interval` (idle workers would
    /// poll in a tight loop) or a zero `job_timeout` (every job would time out before
    /// its handler ran). Called when a configuration is loaded
    /// ([`HammerworkConfig::from_file`], [`HammerworkConfig::from_env`]) and by
    /// [`WorkerPool::from_hammerwork_config`](crate::WorkerPool::from_hammerwork_config).
    pub fn validate(&self) -> crate::Result<()> {
        if self.polling_interval.is_zero() {
            return Err(crate::HammerworkError::Config(
                "worker.polling_interval must be greater than zero".to_string(),
            ));
        }
        if self.job_timeout.is_zero() {
            return Err(crate::HammerworkError::Config(
                "worker.job_timeout must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }

    /// Build the [`AutoscaleConfig`] described by this configuration.
    ///
    /// Returns a disabled autoscale configuration unless `autoscaling_enabled` is set.
    pub fn autoscale_config(&self) -> AutoscaleConfig {
        if !self.autoscaling_enabled {
            return AutoscaleConfig::disabled();
        }
        AutoscaleConfig::default()
            .with_min_workers(self.min_workers)
            .with_max_workers(self.max_workers)
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

    /// Timeout in seconds for webhooks that do not set `timeout_secs` (or set it to 0).
    /// See [`WebhookManagerConfig::default_timeout_secs`](crate::webhooks::WebhookManagerConfig::default_timeout_secs).
    #[serde(default = "default_webhook_timeout_secs")]
    pub default_timeout_secs: u64,

    /// Maximum deliveries pending per webhook before its listener waits. See
    /// [`WebhookManagerConfig::max_pending_deliveries`](crate::webhooks::WebhookManagerConfig::max_pending_deliveries).
    #[serde(default = "default_max_pending_deliveries")]
    pub max_pending_deliveries: usize,
}

#[cfg(feature = "webhooks")]
fn default_webhook_timeout_secs() -> u64 {
    30
}

#[cfg(feature = "webhooks")]
fn default_max_pending_deliveries() -> usize {
    1_000
}

#[cfg(feature = "webhooks")]
impl Default for WebhookGlobalSettings {
    fn default() -> Self {
        Self {
            max_concurrent_deliveries: 100,
            max_response_body_size: 64 * 1024, // 64KB
            log_deliveries: true,
            user_agent: format!("hammerwork-webhooks/{}", env!("CARGO_PKG_VERSION")),
            default_timeout_secs: default_webhook_timeout_secs(),
            max_pending_deliveries: default_max_pending_deliveries(),
        }
    }
}

/// Streaming configurations container
#[cfg(any(
    feature = "streaming",
    feature = "kafka",
    feature = "google-pubsub",
    feature = "kinesis"
))]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StreamingConfigs {
    /// List of configured streams
    pub streams: Vec<StreamConfig>,

    /// Global streaming settings  
    pub global_settings: StreamingGlobalSettings,
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
#[cfg(any(
    feature = "streaming",
    feature = "kafka",
    feature = "google-pubsub",
    feature = "kinesis"
))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamingGlobalSettings {
    /// Maximum concurrent stream processors
    pub max_concurrent_processors: usize,

    /// Whether to log stream operations
    pub log_operations: bool,

    /// Global buffer flush interval in seconds
    pub global_flush_interval_secs: u64,
}

#[cfg(any(
    feature = "streaming",
    feature = "kafka",
    feature = "google-pubsub",
    feature = "kinesis"
))]
impl Default for StreamingGlobalSettings {
    fn default() -> Self {
        Self {
            max_concurrent_processors: 50,
            log_operations: true,
            global_flush_interval_secs: 10,
        }
    }
}

/// Default [`ArchiveConfig::check_interval`]: one hour.
pub const DEFAULT_ARCHIVE_CHECK_INTERVAL: StdDuration = StdDuration::from_secs(3600);

fn default_archive_check_interval() -> StdDuration {
    DEFAULT_ARCHIVE_CHECK_INTERVAL
}

/// Archive configuration (`[archive]`).
///
/// When `enabled`, a pool built with
/// [`WorkerPool::from_hammerwork_config`](crate::WorkerPool::from_hammerwork_config)
/// archives finished jobs older than `archive_after` and purges archived jobs older than
/// `delete_after`, every `check_interval` (see
/// [`WorkerPool::with_archival`](crate::WorkerPool::with_archival)).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveConfig {
    /// Whether automatic archiving is enabled
    pub enabled: bool,

    /// Compression level (0-9, 0=no compression)
    pub compression_level: u32,

    /// Archive completed, failed, dead and timed out jobs older than this duration
    #[serde(with = "chrono_duration_days")]
    pub archive_after: Duration,

    /// Purge archived jobs older than this duration (`None`: keep them)
    #[serde(with = "chrono_duration_days_option")]
    pub delete_after: Option<Duration>,

    /// How often the worker pool archives and purges ("30m", "1h", ...; default 1 hour).
    /// Must be greater than zero.
    #[serde(default = "default_archive_check_interval", with = "duration_secs")]
    pub check_interval: StdDuration,
}

impl ArchiveConfig {
    /// Reject a zero `check_interval` when archiving is enabled (the pool would archive
    /// in a tight loop). Called when a configuration is loaded and by
    /// [`WorkerPool::from_hammerwork_config`](crate::WorkerPool::from_hammerwork_config).
    pub fn validate(&self) -> crate::Result<()> {
        if self.enabled && self.check_interval.is_zero() {
            return Err(crate::HammerworkError::Config(
                "archive.check_interval must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }

    /// Build the [`ArchivalPolicy`] described by this configuration.
    ///
    /// `archive_after` applies to completed, failed, dead and timed out jobs, and
    /// `delete_after` becomes the purge delay for archived jobs. A `compression_level`
    /// of 0 disables payload compression.
    pub fn archival_policy(&self) -> ArchivalPolicy {
        ArchivalPolicy {
            archive_completed_after: Some(self.archive_after),
            archive_failed_after: Some(self.archive_after),
            archive_dead_after: Some(self.archive_after),
            archive_timed_out_after: Some(self.archive_after),
            purge_archived_after: self.delete_after,
            compress_payloads: self.compression_level > 0,
            enabled: self.enabled,
            ..ArchivalPolicy::default()
        }
    }

    /// Build the [`ArchivalConfig`] described by this configuration.
    pub fn archival_config(&self) -> ArchivalConfig {
        ArchivalConfig {
            compression_level: self.compression_level,
            ..ArchivalConfig::default()
        }
    }
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            compression_level: 6,
            archive_after: Duration::days(30),
            delete_after: Some(Duration::days(365)),
            check_interval: DEFAULT_ARCHIVE_CHECK_INTERVAL,
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

impl RateLimitingConfig {
    /// The throttle that applies to `queue_name`.
    ///
    /// Returns the queue-specific throttle if one is configured, otherwise the default
    /// throttle. Returns `None` when rate limiting or the selected throttle is disabled.
    pub fn throttle_for(&self, queue_name: &str) -> Option<ThrottleConfig> {
        if !self.enabled {
            return None;
        }
        let throttle = self
            .queue_throttles
            .get(queue_name)
            .unwrap_or(&self.default_throttle);
        throttle.enabled.then(|| throttle.clone())
    }
}

/// The subscriber [`LoggingConfig`] layers its outputs on: the registry with the level filter.
type LoggingSubscriber =
    tracing_subscriber::layer::Layered<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>;

/// Logging and tracing configuration (`[logging]`).
///
/// A library does not install a global logger on its own: call
/// [`LoggingConfig::try_init`] (for example `config.logging.try_init()?`) once at
/// startup to apply this section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log filter: a level (trace, debug, info, warn, error) or any
    /// [`EnvFilter`](tracing_subscriber::EnvFilter) directive, such as
    /// `"info,hammerwork=debug"`.
    pub level: String,

    /// Whether to enable structured JSON logging
    pub json_format: bool,

    /// Whether to include file and line information
    pub include_location: bool,

    /// Whether to export spans with OpenTelemetry (requires the `tracing` feature)
    pub enable_tracing: bool,

    /// OpenTelemetry (OTLP/gRPC) endpoint URL. Without one, spans are not exported.
    pub tracing_endpoint: Option<String>,

    /// Service name for tracing
    pub service_name: String,
}

impl LoggingConfig {
    /// Install a global `tracing` subscriber configured by this section: log lines on
    /// stdout filtered by `level`, as JSON when `json_format` is set and with file and
    /// line numbers when `include_location` is set. With `enable_tracing`, spans are
    /// also exported over OTLP to `tracing_endpoint` under `service_name` (call
    /// [`shutdown_tracing`](crate::tracing::shutdown_tracing) before exiting to flush
    /// them); this needs the `tracing` feature and must be called inside a Tokio runtime.
    ///
    /// Fails with a configuration error for an invalid `level`, or when
    /// `enable_tracing` is set in a build without the `tracing` feature, and with a
    /// tracing error when a global subscriber is already installed.
    pub fn try_init(&self) -> crate::Result<()> {
        use tracing_subscriber::util::SubscriberInitExt;
        self.subscriber()?
            .try_init()
            .map_err(|e| crate::HammerworkError::Tracing {
                message: format!("Failed to initialize tracing subscriber: {e}"),
            })
    }

    /// The subscriber [`try_init`](Self::try_init) installs.
    fn subscriber(&self) -> crate::Result<impl tracing::Subscriber + Send + Sync + 'static> {
        use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt};

        let filter = EnvFilter::try_new(&self.level).map_err(|e| {
            crate::HammerworkError::Config(format!("invalid logging.level '{}': {e}", self.level))
        })?;

        let mut layers: Vec<Box<dyn Layer<LoggingSubscriber> + Send + Sync>> = Vec::new();
        let fmt = tracing_subscriber::fmt::layer()
            .with_file(self.include_location)
            .with_line_number(self.include_location);
        if self.json_format {
            layers.push(fmt.json().boxed());
        } else {
            layers.push(fmt.boxed());
        }
        if self.enable_tracing {
            layers.push(self.telemetry_layer()?);
        }

        Ok(tracing_subscriber::registry().with(filter).with(layers))
    }

    #[cfg(feature = "tracing")]
    fn telemetry_layer(
        &self,
    ) -> crate::Result<Box<dyn tracing_subscriber::Layer<LoggingSubscriber> + Send + Sync>> {
        use opentelemetry::trace::TracerProvider;
        use tracing_subscriber::Layer;
        let config = crate::tracing::TracingConfig {
            service_name: self.service_name.clone(),
            otlp_endpoint: self.tracing_endpoint.clone(),
            ..crate::tracing::TracingConfig::new()
        };
        let provider = crate::tracing::install_tracer_provider(&config)?;
        Ok(tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("hammerwork"))
            .boxed())
    }

    #[cfg(not(feature = "tracing"))]
    fn telemetry_layer(
        &self,
    ) -> crate::Result<Box<dyn tracing_subscriber::Layer<LoggingSubscriber> + Send + Sync>> {
        Err(crate::HammerworkError::Config(
            "logging.enable_tracing requires the `tracing` feature".to_string(),
        ))
    }
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

/// Encryption algorithm named in [`PayloadEncryptionConfig`].
///
/// The same names as [`EncryptionAlgorithm`](crate::encryption::EncryptionAlgorithm),
/// which is only available with the `encryption` feature.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadEncryptionAlgorithm {
    /// AES-256-GCM
    #[default]
    AES256GCM,
    /// ChaCha20-Poly1305
    ChaCha20Poly1305,
}

impl std::str::FromStr for PayloadEncryptionAlgorithm {
    type Err = crate::HammerworkError;

    fn from_str(s: &str) -> crate::Result<Self> {
        match s {
            "AES256GCM" => Ok(Self::AES256GCM),
            "ChaCha20Poly1305" => Ok(Self::ChaCha20Poly1305),
            other => Err(crate::HammerworkError::Config(format!(
                "Unknown encryption algorithm '{other}' (expected AES256GCM or ChaCha20Poly1305)"
            ))),
        }
    }
}

/// Where an encryption key is loaded from, as a reference that never contains the key.
///
/// Accepted forms:
///
/// - `env://VAR`: a base64 key in environment variable `VAR`
///   ([`KeySource::Environment`](crate::encryption::KeySource::Environment))
/// - `aws://...`, `gcp://...`: a data key generated by AWS or GCP KMS and stored
///   KMS-encrypted in the database (`aws-kms` / `gcp-kms` features)
/// - `vault://...`: a HashiCorp Vault secret (`vault-kms` feature)
/// - `azure://...`: an Azure Key Vault secret (`azure-kv` feature)
///
/// Static keys cannot be configured: a key written in a configuration file would be
/// printed and saved with it. Anything else is rejected when the configuration is
/// loaded, without echoing the value (it may be a key pasted by mistake).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeySourceRef(String);

impl KeySourceRef {
    const EXTERNAL_SCHEMES: [&'static str; 4] = ["aws://", "gcp://", "vault://", "azure://"];

    /// Parses a key source reference (see the type documentation for the forms).
    pub fn parse(source: &str) -> crate::Result<Self> {
        if let Some(var) = source.strip_prefix("env://") {
            let valid =
                !var.is_empty() && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return Err(crate::HammerworkError::Config(
                    "Invalid encryption key source: env:// needs an environment variable name \
                     (letters, digits and underscores)"
                        .to_string(),
                ));
            }
            return Ok(Self(source.to_string()));
        }
        if let Some(scheme) = Self::EXTERNAL_SCHEMES
            .iter()
            .find(|scheme| source.starts_with(**scheme))
        {
            if source.len() == scheme.len() {
                return Err(crate::HammerworkError::Config(format!(
                    "Invalid encryption key source: {scheme} needs a key reference"
                )));
            }
            return Ok(Self(source.to_string()));
        }
        // Do not echo the value: it may be a key
        Err(crate::HammerworkError::Config(
            "Invalid encryption key source: expected env://VAR, aws://, gcp://, vault:// or \
             azure://; keys cannot be written in the configuration"
                .to_string(),
        ))
    }

    /// The reference, e.g. `env://HAMMERWORK_ENCRYPTION_KEY`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The [`KeySource`](crate::encryption::KeySource) this reference names.
    #[cfg(feature = "encryption")]
    pub fn key_source(&self) -> crate::encryption::KeySource {
        match self.0.strip_prefix("env://") {
            Some(var) => crate::encryption::KeySource::Environment(var.to_string()),
            None => crate::encryption::KeySource::External(self.0.clone()),
        }
    }
}

impl Default for KeySourceRef {
    fn default() -> Self {
        Self("env://HAMMERWORK_ENCRYPTION_KEY".to_string())
    }
}

impl TryFrom<String> for KeySourceRef {
    type Error = crate::HammerworkError;

    fn try_from(source: String) -> crate::Result<Self> {
        Self::parse(&source)
    }
}

impl From<KeySourceRef> for String {
    fn from(source: KeySourceRef) -> Self {
        source.0
    }
}

impl std::fmt::Debug for KeySourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A reference (variable name or KMS key URI), never key material
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for KeySourceRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Job payload encryption (`[encryption]` in `hammerwork.toml`).
///
/// When `enabled`, `JobQueue::from_config` builds an
/// [`EncryptionEngine`](crate::encryption::EncryptionEngine) from this section and sets
/// it on the queue (`JobQueue::with_encryption`), so jobs with an encryption config are
/// encrypted, and jobs on `encrypted_queues` are encrypted without one.
/// [`WorkerPool::from_hammerwork_config`](crate::worker::WorkerPool::from_hammerwork_config)
/// schedules the retention purge every `purge_interval_secs`. Needs the `encryption`
/// feature: `from_config` fails if the section is enabled without it.
///
/// Keys are referenced, never written here ([`KeySourceRef`]), so the section can be
/// printed and saved safely.
///
/// ```toml
/// [encryption]
/// enabled = true
/// algorithm = "AES256GCM"
/// key_source = "env://HAMMERWORK_ENCRYPTION_KEY"
/// key_id = "key-2026-10"
/// compression = false
/// default_retention_secs = 2592000   # 30 days
/// purge_interval_secs = 3600
/// encrypted_queues = ["payments"]
///
/// [encryption.decryption_keys]   # decrypt-only keys of earlier key ids
/// "key-2026-04" = "env://HAMMERWORK_ENCRYPTION_KEY_2026_04"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PayloadEncryptionConfig {
    /// Whether the queue encrypts payloads.
    pub enabled: bool,

    /// Algorithm used to encrypt.
    pub algorithm: PayloadEncryptionAlgorithm,

    /// Where the encryption key comes from.
    pub key_source: KeySourceRef,

    /// Id of the encryption key, recorded with each payload (default `"default"`).
    pub key_id: Option<String>,

    /// Whether payloads are compressed before they are encrypted.
    pub compression: bool,

    /// Retention of encrypted jobs whose retention policy is `UseDefault` (the default
    /// for jobs without one), in seconds. `None` keeps them indefinitely.
    pub default_retention_secs: Option<u64>,

    /// How often a [`WorkerPool`](crate::worker::WorkerPool) built with
    /// `from_hammerwork_config` deletes encrypted jobs whose retention ended, in seconds.
    /// `None` leaves the purge to `cargo hammerwork maintenance purge-encrypted`.
    pub purge_interval_secs: Option<u64>,

    /// Queues whose jobs are encrypted even without an encryption config (`"*"`: all).
    pub encrypted_queues: Vec<String>,

    /// Decrypt-only keys by key id, for payloads encrypted under earlier key ids.
    pub decryption_keys: std::collections::BTreeMap<String, KeySourceRef>,
}

impl PayloadEncryptionConfig {
    /// Checks the section for settings that cannot work. Called by `JobQueue::from_config`.
    /// A disabled section is only checked for settings that need it enabled.
    pub fn validate(&self) -> crate::Result<()> {
        let error = |message: &str| Err(crate::HammerworkError::Config(message.to_string()));
        // The purge also runs with encryption disabled (jobs encrypted earlier still
        // expire), and a zero interval would run it in a tight loop.
        if self.purge_interval_secs == Some(0) {
            return error("encryption.purge_interval_secs must be greater than zero");
        }
        if !self.enabled {
            if !self.encrypted_queues.is_empty() {
                return error(
                    "encryption.encrypted_queues is set but encryption.enabled is false; \
                     jobs on those queues would be stored in plaintext",
                );
            }
            return Ok(());
        }
        if self.key_id.as_deref().is_some_and(str::is_empty) {
            return error("encryption.key_id must not be empty");
        }
        let key_id = self.key_id.as_deref().unwrap_or("default");
        if self.decryption_keys.contains_key(key_id) {
            return error("encryption.decryption_keys must not contain the encryption key id");
        }
        if self.decryption_keys.keys().any(String::is_empty) {
            return error("encryption.decryption_keys has an empty key id");
        }
        if self.encrypted_queues.iter().any(String::is_empty) {
            return error("encryption.encrypted_queues has an empty queue name");
        }
        Ok(())
    }

    /// The encryption settings of an application, for tools that write jobs on its behalf
    /// (`cargo hammerwork`, the web dashboard).
    ///
    /// Reads the `[encryption]` section of the application's `hammerwork.toml` at `path`
    /// (other sections are ignored, so the file does not need to be complete; a file
    /// without the section means encryption is disabled), then applies the
    /// `HAMMERWORK_ENCRYPTION_*` environment variables ([`Self::apply_env`]). Without a
    /// path, only the environment is used. The result is validated.
    ///
    /// ```no_run
    /// use hammerwork::config::PayloadEncryptionConfig;
    ///
    /// let encryption = PayloadEncryptionConfig::load(Some("hammerwork.toml".as_ref()))?;
    /// if encryption.enabled {
    ///     println!("encrypting queues {:?}", encryption.encrypted_queues);
    /// }
    /// # Ok::<(), hammerwork::HammerworkError>(())
    /// ```
    pub fn load(path: Option<&std::path::Path>) -> crate::Result<Self> {
        let mut config = match path {
            Some(path) => {
                let content = std::fs::read_to_string(path).map_err(|e| {
                    crate::HammerworkError::Config(format!("Cannot read {}: {}", path.display(), e))
                })?;
                Self::from_toml_document(&content).map_err(|e| {
                    crate::HammerworkError::Config(format!("{}: {}", path.display(), e))
                })?
            }
            None => Self::default(),
        };
        config.apply_env()?;
        config.validate()?;
        Ok(config)
    }

    /// The `[encryption]` section of a `hammerwork.toml` document (the default if there
    /// is none).
    fn from_toml_document(content: &str) -> crate::Result<Self> {
        #[derive(Deserialize)]
        struct Document {
            #[serde(default)]
            encryption: PayloadEncryptionConfig,
        }
        let document: Document = toml::from_str(content)?;
        Ok(document.encryption)
    }

    /// Whether jobs on `queue_name` are encrypted without an encryption config of their
    /// own (`enabled` and listed in `encrypted_queues`, or `"*"` is listed).
    pub fn encrypts_queue(&self, queue_name: &str) -> bool {
        self.enabled
            && self
                .encrypted_queues
                .iter()
                .any(|queue| queue == "*" || queue == queue_name)
    }

    /// How often the retention purge runs, if configured.
    pub fn purge_interval(&self) -> Option<StdDuration> {
        self.purge_interval_secs.map(StdDuration::from_secs)
    }

    /// The [`EncryptionConfig`](crate::encryption::EncryptionConfig) for the engine.
    /// Validates the section first.
    #[cfg(feature = "encryption")]
    pub fn encryption_config(&self) -> crate::Result<crate::encryption::EncryptionConfig> {
        use crate::encryption::{EncryptionAlgorithm, EncryptionConfig};

        self.validate()?;
        let algorithm = match self.algorithm {
            PayloadEncryptionAlgorithm::AES256GCM => EncryptionAlgorithm::AES256GCM,
            PayloadEncryptionAlgorithm::ChaCha20Poly1305 => EncryptionAlgorithm::ChaCha20Poly1305,
        };
        let mut config = EncryptionConfig::new(algorithm)
            .with_key_source(self.key_source.key_source())
            .with_compression_enabled(self.compression);
        if let Some(key_id) = &self.key_id {
            config = config.with_key_id(key_id);
        }
        if let Some(secs) = self.default_retention_secs {
            config = config.with_default_retention(StdDuration::from_secs(secs));
        }
        Ok(config)
    }

    /// Overrides settings from `HAMMERWORK_ENCRYPTION_*` environment variables.
    ///
    /// `HAMMERWORK_ENCRYPTION_ENABLED`, `_ALGORITHM`, `_KEY_SOURCE`, `_KEY_ID`,
    /// `_COMPRESSION`, `_DEFAULT_RETENTION_SECS`, `_PURGE_INTERVAL_SECS`,
    /// `_ENCRYPTED_QUEUES` (comma-separated) and `_DECRYPTION_KEYS`
    /// (`id=source,id=source`). An invalid value is an error rather than ignored, so a
    /// typo cannot silently disable encryption. (`HAMMERWORK_ENCRYPTION_KEY` is the
    /// default key source, the key itself, not a setting.)
    pub fn apply_env(&mut self) -> crate::Result<()> {
        self.apply_vars(|name| std::env::var(name).ok())
    }

    /// [`PayloadEncryptionConfig::apply_env`] with variables looked up by `var`.
    fn apply_vars(&mut self, var: impl Fn(&str) -> Option<String>) -> crate::Result<()> {
        fn invalid(name: &str) -> crate::HammerworkError {
            crate::HammerworkError::Config(format!("Invalid value for {name}"))
        }
        fn parse_bool(name: &str, value: &str) -> crate::Result<bool> {
            match value.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                _ => Err(invalid(name)),
            }
        }
        fn parse_secs(name: &str, value: &str) -> crate::Result<u64> {
            value.trim().parse().map_err(|_| invalid(name))
        }

        const ENABLED: &str = "HAMMERWORK_ENCRYPTION_ENABLED";
        const COMPRESSION: &str = "HAMMERWORK_ENCRYPTION_COMPRESSION";
        const RETENTION: &str = "HAMMERWORK_ENCRYPTION_DEFAULT_RETENTION_SECS";
        const PURGE: &str = "HAMMERWORK_ENCRYPTION_PURGE_INTERVAL_SECS";
        const DECRYPTION_KEYS: &str = "HAMMERWORK_ENCRYPTION_DECRYPTION_KEYS";

        if let Some(value) = var(ENABLED) {
            self.enabled = parse_bool(ENABLED, &value)?;
        }
        if let Some(value) = var("HAMMERWORK_ENCRYPTION_ALGORITHM") {
            self.algorithm = value.trim().parse()?;
        }
        if let Some(value) = var("HAMMERWORK_ENCRYPTION_KEY_SOURCE") {
            self.key_source = KeySourceRef::parse(value.trim())?;
        }
        if let Some(value) = var("HAMMERWORK_ENCRYPTION_KEY_ID") {
            self.key_id = Some(value.trim().to_string());
        }
        if let Some(value) = var(COMPRESSION) {
            self.compression = parse_bool(COMPRESSION, &value)?;
        }
        if let Some(value) = var(RETENTION) {
            self.default_retention_secs = Some(parse_secs(RETENTION, &value)?);
        }
        if let Some(value) = var(PURGE) {
            self.purge_interval_secs = Some(parse_secs(PURGE, &value)?);
        }
        if let Some(value) = var("HAMMERWORK_ENCRYPTION_ENCRYPTED_QUEUES") {
            self.encrypted_queues = value
                .split(',')
                .map(str::trim)
                .filter(|queue| !queue.is_empty())
                .map(str::to_string)
                .collect();
        }
        if let Some(value) = var(DECRYPTION_KEYS) {
            let mut keys = std::collections::BTreeMap::new();
            for entry in value.split(',').map(str::trim).filter(|e| !e.is_empty()) {
                let (key_id, source) = entry
                    .split_once('=')
                    .ok_or_else(|| invalid(DECRYPTION_KEYS))?;
                keys.insert(
                    key_id.trim().to_string(),
                    KeySourceRef::parse(source.trim())?,
                );
            }
            self.decryption_keys = keys;
        }
        Ok(())
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
    // Which fields the final `..Default::default()` fills depends on the enabled features
    // (webhooks, streaming, alerting, metrics); with none of them it's a no-op.
    #[allow(clippy::needless_update)]
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
    #[cfg(any(
        feature = "streaming",
        feature = "kafka",
        feature = "google-pubsub",
        feature = "kinesis"
    ))]
    pub fn add_stream(mut self, stream: StreamConfig) -> Self {
        self.streaming.streams.push(stream);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(
        feature = "streaming",
        feature = "kafka",
        feature = "google-pubsub",
        feature = "kinesis"
    ))]
    use crate::streaming::StreamBackend;
    use tempfile::tempdir;

    #[test]
    fn serde_duration_reads_every_documented_form() {
        #[derive(Deserialize)]
        struct T {
            #[serde(with = "serde_duration")]
            d: StdDuration,
        }
        let read = |toml: &str| toml::from_str::<T>(toml).map(|t| t.d);
        assert_eq!(
            read(r#"d = "500ms""#).unwrap(),
            StdDuration::from_millis(500)
        );
        assert_eq!(read(r#"d = "30s""#).unwrap(), StdDuration::from_secs(30));
        assert_eq!(read(r#"d = "5m""#).unwrap(), StdDuration::from_secs(300));
        assert_eq!(read(r#"d = "1h""#).unwrap(), StdDuration::from_secs(3600));
        assert_eq!(
            read(r#"d = "2d""#).unwrap(),
            StdDuration::from_secs(172_800)
        );
        assert_eq!(read(r#"d = "90""#).unwrap(), StdDuration::from_secs(90));
        assert_eq!(read("d = 90").unwrap(), StdDuration::from_secs(90));
        assert_eq!(
            read("d = { secs = 1, nanos = 500000000 }").unwrap(),
            StdDuration::from_millis(1500)
        );
        assert!(read("d = { secs = 1, nanos = 1000000000 }").is_err());
        assert!(read(r#"d = "5x""#).is_err());
        assert!(read(&format!(r#"d = "{}d""#, u64::MAX)).is_err());
    }

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
    fn test_config_file_operations() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("hammerwork.toml");

        let config = HammerworkConfig::new()
            .with_database_url("mysql://localhost/test")
            .with_worker_pool_size(6);

        // Save config
        println!("Testing TOML serialization...");
        let toml_result = toml::to_string_pretty(&config);
        println!("TOML result: {:?}", toml_result);

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

    /// #64 M5: zero durations that would hot-loop or time every job out are rejected.
    #[test]
    fn test_zero_worker_durations_are_rejected() {
        assert!(WorkerConfig::default().validate().is_ok());
        let zero_poll = WorkerConfig {
            polling_interval: StdDuration::ZERO,
            ..WorkerConfig::default()
        };
        let err = zero_poll.validate().unwrap_err().to_string();
        assert!(err.contains("polling_interval"), "{err}");
        let zero_timeout = WorkerConfig {
            job_timeout: StdDuration::ZERO,
            ..WorkerConfig::default()
        };
        let err = zero_timeout.validate().unwrap_err().to_string();
        assert!(err.contains("job_timeout"), "{err}");

        // The default 500ms polling interval survives a save and load (it used to be
        // written as "0s", which loaded as a zero interval).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("default.toml");
        HammerworkConfig::new()
            .save_to_file(path.to_str().unwrap())
            .unwrap();
        let loaded = HammerworkConfig::from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(
            loaded.worker.polling_interval,
            StdDuration::from_millis(500)
        );

        // Loading a file with a zero interval fails.
        let path = dir.path().join("zero.toml");
        let mut config = HammerworkConfig::new();
        config.worker.polling_interval = StdDuration::ZERO;
        config.save_to_file(path.to_str().unwrap()).unwrap();
        assert!(matches!(
            HammerworkConfig::from_file(path.to_str().unwrap()),
            Err(crate::HammerworkError::Config(_))
        ));
    }

    #[test]
    fn test_duration_serialization() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let config_path = dir.path().join("duration_test.toml");

        // Create config with various durations
        let mut config = HammerworkConfig::new();
        config.worker.polling_interval = StdDuration::from_secs(30); // Should serialize as "30s"
        config.worker.job_timeout = StdDuration::from_secs(300); // Should serialize as "5m"

        // Save to TOML
        config.save_to_file(config_path.to_str().unwrap()).unwrap();

        // Read the TOML content to verify human-readable format
        let toml_content = std::fs::read_to_string(&config_path).unwrap();
        assert!(toml_content.contains("polling_interval = \"30s\""));
        assert!(toml_content.contains("job_timeout = \"5m\""));

        // Load back and verify values
        let loaded_config = HammerworkConfig::from_file(config_path.to_str().unwrap()).unwrap();
        assert_eq!(
            loaded_config.worker.polling_interval,
            StdDuration::from_secs(30)
        );
        assert_eq!(
            loaded_config.worker.job_timeout,
            StdDuration::from_secs(300)
        );

        // Test parsing various duration formats
        let test_durations = [
            ("30", StdDuration::from_secs(30)),
            ("30s", StdDuration::from_secs(30)),
            ("5m", StdDuration::from_secs(300)),
            ("2h", StdDuration::from_secs(7200)),
            ("1d", StdDuration::from_secs(86400)),
        ];

        for (duration_str, expected) in test_durations.iter() {
            let toml_content = format!(
                r#"
[database]
url = "postgresql://localhost/test"
pool_size = 10
connection_timeout_secs = 30
auto_migrate = false
create_tables = true

[worker]
pool_size = 4
polling_interval = "{}"
job_timeout = "5m"
autoscaling_enabled = false
min_workers = 1
max_workers = 10

[worker.priority_weights]
strict_priority = false
fairness_factor = 0.1

[worker.priority_weights.weights]
Background = 1
Low = 2
Normal = 5
High = 10
Critical = 20

[worker.retry_strategy]
type = "Exponential"
base_ms = 1000
multiplier = 2.0
max_delay_ms = 60000

[events]
max_buffer_size = 1000
include_payload_default = false
max_payload_size_bytes = 65536
log_events = false

[webhooks]
webhooks = []

[webhooks.global_settings]
max_concurrent_deliveries = 100
max_response_body_size = 65536
log_deliveries = true
user_agent = "hammerwork-webhooks/1.13.0"

[streaming]
streams = []

[streaming.global_settings]
max_concurrent_processors = 50
log_operations = true
global_flush_interval_secs = 10

[alerting]
targets = []
enabled = true

[alerting.cooldown_period]
secs = 300
nanos = 0

[alerting.custom_thresholds]

[metrics]
registry_name = "hammerwork"
collect_histograms = true
custom_gauges = []
custom_histograms = []
update_interval = 15

[metrics.custom_labels]

[archive]
enabled = false
compression_level = 6
archive_after = "30d"
delete_after = "365d"

[rate_limiting]
enabled = false

[rate_limiting.default_throttle]
enabled = true

[rate_limiting.queue_throttles]

[logging]
level = "info"
json_format = false
include_location = false
enable_tracing = false
service_name = "hammerwork"
"#,
                duration_str
            );

            let config: HammerworkConfig = toml::from_str(&toml_content).unwrap();
            assert_eq!(
                config.worker.polling_interval, *expected,
                "Failed to parse duration: {}",
                duration_str
            );
        }
    }

    /// Configuration files written for 1.x keep loading in 2.0: keys of removed settings
    /// (`database.create_tables`, `worker.priority_weights.fairness_factor`,
    /// `alerting.custom_thresholds`, `metrics.registry_name`) are ignored.
    #[test]
    fn test_config_file_with_removed_keys_still_loads() {
        let mut doc: toml::Table =
            toml::from_str(&toml::to_string(&HammerworkConfig::default()).unwrap()).unwrap();
        let table = |doc: &mut toml::Table, path: &[&str]| -> toml::Table {
            let mut current = doc.clone();
            for key in path {
                current = current[*key].as_table().unwrap().clone();
            }
            current
        };
        let mut database = table(&mut doc, &["database"]);
        database.insert("create_tables".into(), toml::Value::Boolean(true));
        doc.insert("database".into(), database.into());

        let mut worker = table(&mut doc, &["worker"]);
        let mut weights = table(&mut doc, &["worker", "priority_weights"]);
        weights.insert("fairness_factor".into(), toml::Value::Float(0.1));
        worker.insert("priority_weights".into(), weights.into());
        doc.insert("worker".into(), worker.into());

        if doc.contains_key("alerting") {
            let mut alerting = table(&mut doc, &["alerting"]);
            let mut thresholds = toml::Table::new();
            thresholds.insert("cpu_usage".into(), toml::Value::Float(90.0));
            alerting.insert("custom_thresholds".into(), thresholds.into());
            doc.insert("alerting".into(), alerting.into());
        }
        if doc.contains_key("metrics") {
            let mut metrics = table(&mut doc, &["metrics"]);
            metrics.insert(
                "registry_name".into(),
                toml::Value::String("hammerwork".into()),
            );
            doc.insert("metrics".into(), metrics.into());
        }

        let text = toml::to_string(&doc).unwrap();
        assert!(text.contains("create_tables") && text.contains("fairness_factor"));
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), &text).unwrap();
        let config = HammerworkConfig::from_file(file.path().to_str().unwrap()).unwrap();
        assert_eq!(
            config.database.pool_size,
            DatabaseConfig::default().pool_size
        );
        assert!(!config.worker.priority_weights.is_strict());
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
    #[cfg(any(
        feature = "streaming",
        feature = "kafka",
        feature = "google-pubsub",
        feature = "kinesis"
    ))]
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

    /// M4: the `[logging]` section builds a working subscriber (plain or JSON, with or
    /// without locations) and rejects what it cannot apply.
    #[test]
    fn test_logging_config_builds_its_subscriber() {
        for (json_format, include_location) in [(false, false), (true, true)] {
            let config = LoggingConfig {
                level: "warn,hammerwork=debug".to_string(),
                json_format,
                include_location,
                ..Default::default()
            };
            let subscriber = config.subscriber().unwrap();
            tracing::subscriber::with_default(subscriber, || {
                assert!(tracing::enabled!(target: "hammerwork::worker", tracing::Level::DEBUG));
                assert!(!tracing::enabled!(target: "other", tracing::Level::INFO));
                tracing::warn!("logging config test");
            });
        }

        let invalid = LoggingConfig {
            level: "hammerwork=loud".to_string(),
            ..Default::default()
        };
        let err = invalid.subscriber().err().expect("invalid level");
        assert!(err.to_string().contains("logging.level"), "{err}");

        #[cfg(not(feature = "tracing"))]
        {
            let tracing_without_feature = LoggingConfig {
                enable_tracing: true,
                ..Default::default()
            };
            let err = tracing_without_feature
                .subscriber()
                .err()
                .expect("no feature");
            assert!(err.to_string().contains("`tracing` feature"), "{err}");
        }
    }

    #[test]
    fn test_database_connection_timeout() {
        let config = DatabaseConfig {
            connection_timeout_secs: 45,
            ..Default::default()
        };
        assert_eq!(config.connection_timeout(), StdDuration::from_secs(45));
    }

    #[test]
    fn test_worker_autoscale_config() {
        let disabled = WorkerConfig::default().autoscale_config();
        assert!(!disabled.enabled);

        let worker = WorkerConfig {
            autoscaling_enabled: true,
            min_workers: 3,
            max_workers: 12,
            ..Default::default()
        };
        let autoscale = worker.autoscale_config();
        assert!(autoscale.enabled);
        assert_eq!(autoscale.min_workers, 3);
        assert_eq!(autoscale.max_workers, 12);
    }

    #[test]
    fn test_rate_limiting_throttle_for() {
        let mut config = RateLimitingConfig {
            enabled: false,
            default_throttle: ThrottleConfig::new().rate_per_minute(60),
            queue_throttles: HashMap::new(),
        };
        config.queue_throttles.insert(
            "email".to_string(),
            ThrottleConfig::new().rate_per_minute(10),
        );
        config.queue_throttles.insert(
            "reports".to_string(),
            ThrottleConfig::new().rate_per_minute(5).enabled(false),
        );

        assert!(config.throttle_for("email").is_none());

        config.enabled = true;
        assert_eq!(
            config.throttle_for("email").unwrap().rate_per_minute,
            Some(10)
        );
        assert_eq!(
            config.throttle_for("other").unwrap().rate_per_minute,
            Some(60)
        );
        assert!(config.throttle_for("reports").is_none());
    }

    #[test]
    fn test_archive_config_conversions() {
        let config = ArchiveConfig {
            enabled: true,
            compression_level: 0,
            archive_after: Duration::days(7),
            delete_after: None,
            check_interval: StdDuration::from_secs(60),
        };

        let policy = config.archival_policy();
        assert!(policy.enabled);
        assert!(!policy.compress_payloads);
        assert_eq!(policy.archive_completed_after, Some(Duration::days(7)));
        assert_eq!(policy.archive_failed_after, Some(Duration::days(7)));
        assert_eq!(policy.archive_dead_after, Some(Duration::days(7)));
        assert_eq!(policy.archive_timed_out_after, Some(Duration::days(7)));
        assert_eq!(policy.purge_archived_after, None);

        let archival = ArchiveConfig::default().archival_config();
        assert_eq!(archival.compression_level, 6);
    }

    #[test]
    fn test_archive_config_ignores_removed_fields() {
        let toml = r#"
            enabled = true
            compression_level = 3
            archive_after = "10d"
            delete_after = "100d"
            archive_directory = "./archives"
            max_file_size_bytes = 104857600
            include_payloads = true
        "#;
        let config: ArchiveConfig = toml::from_str(toml).unwrap();
        assert!(config.enabled);
        assert_eq!(config.compression_level, 3);
        assert_eq!(config.archive_after, Duration::days(10));
        // Files written before check_interval existed get the default.
        assert_eq!(config.check_interval, DEFAULT_ARCHIVE_CHECK_INTERVAL);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_archive_check_interval() {
        let config: ArchiveConfig = toml::from_str(
            r#"
            enabled = true
            compression_level = 6
            archive_after = "30d"
            delete_after = "90d"
            check_interval = "15m"
        "#,
        )
        .unwrap();
        assert_eq!(config.check_interval, StdDuration::from_secs(15 * 60));
        assert_eq!(config.delete_after, Some(Duration::days(90)));
        let round_trip: ArchiveConfig = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
        assert_eq!(round_trip.check_interval, config.check_interval);

        let zero = ArchiveConfig {
            check_interval: StdDuration::ZERO,
            ..config.clone()
        };
        let err = zero.validate().unwrap_err().to_string();
        assert!(err.contains("archive.check_interval"), "{err}");
        // A disabled section is not checked.
        let disabled = ArchiveConfig {
            enabled: false,
            ..zero
        };
        assert!(disabled.validate().is_ok());
    }

    fn encryption_toml() -> &'static str {
        r#"
            enabled = true
            algorithm = "ChaCha20Poly1305"
            key_source = "env://MY_HAMMERWORK_KEY"
            key_id = "key-2026-10"
            compression = true
            default_retention_secs = 2592000
            purge_interval_secs = 3600
            encrypted_queues = ["payments", "pii"]

            [decryption_keys]
            "key-2026-04" = "env://MY_OLD_KEY"
            "kms-key" = "aws://alias/hammerwork?region=us-east-1"
        "#
    }

    #[test]
    fn test_encryption_section_parses_and_round_trips() {
        let section: PayloadEncryptionConfig = toml::from_str(encryption_toml()).unwrap();
        assert!(section.enabled);
        assert_eq!(
            section.algorithm,
            PayloadEncryptionAlgorithm::ChaCha20Poly1305
        );
        assert_eq!(section.key_source.as_str(), "env://MY_HAMMERWORK_KEY");
        assert_eq!(section.key_id.as_deref(), Some("key-2026-10"));
        assert!(section.compression);
        assert_eq!(section.default_retention_secs, Some(2_592_000));
        assert_eq!(section.purge_interval(), Some(StdDuration::from_secs(3600)));
        assert_eq!(section.encrypted_queues, vec!["payments", "pii"]);
        assert_eq!(section.decryption_keys.len(), 2);
        section.validate().unwrap();

        let config = HammerworkConfig {
            encryption: section.clone(),
            ..HammerworkConfig::default()
        };
        let saved = toml::to_string_pretty(&config).unwrap();
        assert!(saved.contains("env://MY_HAMMERWORK_KEY"), "{saved}");
        let loaded: HammerworkConfig = toml::from_str(&saved).unwrap();
        assert_eq!(loaded.encryption, section);

        // The file operations round-trip it too
        let dir = tempdir().unwrap();
        let path = dir.path().join("hammerwork.toml");
        config.save_to_file(path.to_str().unwrap()).unwrap();
        let loaded = HammerworkConfig::from_file(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.encryption, section);
    }

    #[cfg(all(feature = "alerting", feature = "webhooks"))]
    #[test]
    fn test_from_file_rejects_targets_that_cannot_deliver() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("hammerwork.toml");
        let path = path.to_str().unwrap();

        // An email alert target without SMTP settings.
        let mut config = HammerworkConfig::default();
        config
            .alerting
            .targets
            .push(crate::alerting::AlertTarget::Email {
                recipient: "oncall@example.com".to_string(),
                smtp: None,
            });
        config.save_to_file(path).unwrap();
        let err = HammerworkConfig::from_file(path).unwrap_err();
        assert!(err.to_string().contains("no SMTP settings"), "{err}");

        // With SMTP settings it loads.
        config.alerting.targets = vec![crate::alerting::AlertTarget::Email {
            recipient: "oncall@example.com".to_string(),
            smtp: Some(crate::alerting::SmtpConfig::new(
                "smtp.example.com",
                "alerts@example.com",
            )),
        }];
        config.save_to_file(path).unwrap();
        HammerworkConfig::from_file(path).unwrap();

        // A webhook with an invalid payload template.
        config.webhooks.webhooks.push(
            crate::webhooks::WebhookConfig::new("hook".into(), "https://example.com".into())
                .with_payload_template(r#"{"x": "{{event.missing}}"}"#),
        );
        config.save_to_file(path).unwrap();
        let err = HammerworkConfig::from_file(path).unwrap_err();
        assert!(err.to_string().contains("unknown placeholder"), "{err}");
    }

    #[test]
    fn test_encryption_section_is_optional_and_disabled_by_default() {
        let config = HammerworkConfig::default();
        assert!(!config.encryption.enabled);
        assert_eq!(
            config.encryption.key_source.as_str(),
            "env://HAMMERWORK_ENCRYPTION_KEY"
        );
        config.encryption.validate().unwrap();

        // Configuration files written before the section existed still load
        let mut value = toml::Value::try_from(&config).unwrap();
        value.as_table_mut().unwrap().remove("encryption");
        let loaded: HammerworkConfig = toml::from_str(&toml::to_string(&value).unwrap()).unwrap();
        assert_eq!(loaded.encryption, PayloadEncryptionConfig::default());
    }

    #[test]
    fn test_encryption_section_rejects_keys_and_typos() {
        let key = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUE=";
        for source in [
            key.to_string(),
            format!("static://{key}"),
            "env://".to_string(),
            "env://NOT-A-VAR".to_string(),
            "aws://".to_string(),
            "file:///etc/key".to_string(),
        ] {
            let err = toml::from_str::<PayloadEncryptionConfig>(&format!(
                "enabled = true\nkey_source = \"{source}\""
            ))
            .unwrap_err()
            .to_string();
            assert!(err.contains("key source"), "{source}: {err}");
            // Our message never repeats the value (the TOML parser quotes the line)
            let err = KeySourceRef::parse(&source).unwrap_err().to_string();
            assert!(!err.contains(key), "the error echoes the key: {err}");
        }

        // Unknown fields (e.g. a key under a guessed name) and algorithms fail closed
        assert!(
            toml::from_str::<PayloadEncryptionConfig>(&format!("enabled = true\nkey = \"{key}\""))
                .is_err()
        );
        assert!(toml::from_str::<PayloadEncryptionConfig>("algorithm = \"AES128\"").is_err());
        assert!(
            toml::from_str::<PayloadEncryptionConfig>("[decryption_keys]\nold = \"QUFBQQ==\"")
                .is_err()
        );
    }

    #[test]
    fn test_encryption_section_validation() {
        let enabled = PayloadEncryptionConfig {
            enabled: true,
            ..PayloadEncryptionConfig::default()
        };
        enabled.validate().unwrap();

        let invalid = [
            PayloadEncryptionConfig {
                encrypted_queues: vec!["payments".into()],
                ..PayloadEncryptionConfig::default()
            },
            PayloadEncryptionConfig {
                key_id: Some(String::new()),
                ..enabled.clone()
            },
            PayloadEncryptionConfig {
                decryption_keys: [("default".to_string(), KeySourceRef::default())].into(),
                ..enabled.clone()
            },
            PayloadEncryptionConfig {
                purge_interval_secs: Some(0),
                ..enabled.clone()
            },
            PayloadEncryptionConfig {
                encrypted_queues: vec![String::new()],
                ..enabled.clone()
            },
        ];
        for config in invalid {
            assert!(
                matches!(config.validate(), Err(crate::HammerworkError::Config(_))),
                "{config:?}"
            );
        }
    }

    #[test]
    fn test_encryption_section_debug_shows_only_references() {
        let section: PayloadEncryptionConfig = toml::from_str(encryption_toml()).unwrap();
        let debug = format!("{section:?}");
        assert!(debug.contains("env://MY_HAMMERWORK_KEY"), "{debug}");
        assert!(debug.contains("aws://alias/hammerwork"), "{debug}");
    }

    #[test]
    fn test_encryption_section_from_env_vars() {
        let vars: HashMap<&str, &str> = [
            ("HAMMERWORK_ENCRYPTION_ENABLED", "true"),
            ("HAMMERWORK_ENCRYPTION_ALGORITHM", "ChaCha20Poly1305"),
            ("HAMMERWORK_ENCRYPTION_KEY_SOURCE", "env://PROD_KEY"),
            ("HAMMERWORK_ENCRYPTION_KEY_ID", "prod-1"),
            ("HAMMERWORK_ENCRYPTION_COMPRESSION", "1"),
            ("HAMMERWORK_ENCRYPTION_DEFAULT_RETENTION_SECS", "86400"),
            ("HAMMERWORK_ENCRYPTION_PURGE_INTERVAL_SECS", "600"),
            ("HAMMERWORK_ENCRYPTION_ENCRYPTED_QUEUES", "payments, pii,"),
            (
                "HAMMERWORK_ENCRYPTION_DECRYPTION_KEYS",
                "prod-0=env://OLD_KEY,kms=gcp://projects/p/locations/l/keyRings/r/cryptoKeys/k",
            ),
        ]
        .into();
        let lookup = |name: &str| vars.get(name).map(|v| v.to_string());

        let mut section = PayloadEncryptionConfig::default();
        section.apply_vars(lookup).unwrap();
        assert!(section.enabled);
        assert_eq!(
            section.algorithm,
            PayloadEncryptionAlgorithm::ChaCha20Poly1305
        );
        assert_eq!(section.key_source.as_str(), "env://PROD_KEY");
        assert_eq!(section.key_id.as_deref(), Some("prod-1"));
        assert!(section.compression);
        assert_eq!(section.default_retention_secs, Some(86_400));
        assert_eq!(section.purge_interval_secs, Some(600));
        assert_eq!(section.encrypted_queues, vec!["payments", "pii"]);
        assert_eq!(section.decryption_keys["prod-0"].as_str(), "env://OLD_KEY");
        section.validate().unwrap();

        // Invalid values are errors, not silently ignored
        for (name, value) in [
            ("HAMMERWORK_ENCRYPTION_ENABLED", "maybe"),
            ("HAMMERWORK_ENCRYPTION_ALGORITHM", "rot13"),
            ("HAMMERWORK_ENCRYPTION_KEY_SOURCE", "QUFBQQ=="),
            ("HAMMERWORK_ENCRYPTION_PURGE_INTERVAL_SECS", "hourly"),
            ("HAMMERWORK_ENCRYPTION_DECRYPTION_KEYS", "env://NO_ID"),
        ] {
            let mut section = PayloadEncryptionConfig::default();
            let result = section.apply_vars(|n| (n == name).then(|| value.to_string()));
            assert!(result.is_err(), "{name}={value}");
        }

        // Nothing set: unchanged
        let mut section = PayloadEncryptionConfig::default();
        section.apply_vars(|_| None).unwrap();
        assert_eq!(section, PayloadEncryptionConfig::default());
    }

    #[cfg(feature = "encryption")]
    #[test]
    fn test_encryption_section_engine_config() {
        use crate::encryption::{EncryptionAlgorithm, KeySource};

        let section: PayloadEncryptionConfig = toml::from_str(encryption_toml()).unwrap();
        let config = section.encryption_config().unwrap();
        assert_eq!(config.algorithm, EncryptionAlgorithm::ChaCha20Poly1305);
        assert_eq!(
            config.key_source,
            KeySource::Environment("MY_HAMMERWORK_KEY".to_string())
        );
        assert_eq!(config.key_id.as_deref(), Some("key-2026-10"));
        assert!(config.compression_enabled);
        assert_eq!(
            config.default_retention,
            Some(StdDuration::from_secs(2_592_000))
        );
        assert_eq!(
            section.decryption_keys["kms-key"].key_source(),
            KeySource::External("aws://alias/hammerwork?region=us-east-1".to_string())
        );

        let invalid = PayloadEncryptionConfig {
            purge_interval_secs: Some(0),
            ..section
        };
        assert!(invalid.encryption_config().is_err());
    }

    #[test]
    fn test_redact_url_hides_only_the_password() {
        assert_eq!(
            redact_url("postgres://admin:s3cret@db.internal:5432/app?sslmode=require"),
            "postgres://admin:***@db.internal:5432/app?sslmode=require"
        );
        // The password itself may contain ':' and '@'.
        assert_eq!(
            redact_url("mysql://root:p@ss:word@127.0.0.1:3306/db"),
            "mysql://root:***@127.0.0.1:3306/db"
        );
        assert_eq!(
            redact_url("postgres://user@host/db"),
            "postgres://user@host/db"
        );
        assert_eq!(redact_url("postgres://host/db"), "postgres://host/db");
        assert_eq!(redact_url("not a url"), "not a url");
        // An '@' after the path is not user info.
        assert_eq!(
            redact_url("postgres://host/db?options=a:b@c"),
            "postgres://host/db?options=a:b@c"
        );
    }

    #[test]
    fn test_redact_url_path_keeps_only_scheme_and_host() {
        for (url, expected) in [
            (
                "https://hooks.slack.com/services/T0/B0/secret",
                "https://hooks.slack.com/***",
            ),
            (
                "https://discord.com/api/webhooks/1/token",
                "https://discord.com/***",
            ),
            ("https://example.com?sig=secret", "https://example.com/***"),
            (
                "https://user:pw@example.com/hook",
                "https://example.com/***",
            ),
            ("https://example.com/", "https://example.com"),
            ("https://example.com", "https://example.com"),
            ("garbage-secret", "***"),
        ] {
            assert_eq!(redact_url_path(url), expected, "{url}");
        }
    }

    #[test]
    fn test_debug_does_not_print_the_database_password() {
        let config = HammerworkConfig::new()
            .with_database_url("postgres://app:hunter2-db-password@db.internal/jobs");
        for debug in [format!("{:?}", config.database), format!("{:?}", config)] {
            assert!(!debug.contains("hunter2-db-password"), "{debug}");
            assert!(
                debug.contains("postgres://app:***@db.internal/jobs"),
                "{debug}"
            );
        }
    }

    #[test]
    fn test_encryption_section_is_loaded_from_an_application_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hammerwork.toml");
        // Only the [encryption] section is read; the rest of the file may be partial.
        std::fs::write(
            &path,
            "[database]\nurl = \"postgres://db/jobs\"\n\n[encryption]\nenabled = true\n\
             encrypted_queues = [\"payments\"]\n",
        )
        .unwrap();
        let section =
            PayloadEncryptionConfig::from_toml_document(&std::fs::read_to_string(&path).unwrap())
                .unwrap();
        assert!(section.enabled);
        assert!(section.encrypts_queue("payments"));
        assert!(!section.encrypts_queue("emails"));

        // No section: disabled.
        let none = PayloadEncryptionConfig::from_toml_document("[database]\n").unwrap();
        assert!(!none.enabled);
        assert!(!none.encrypts_queue("payments"));

        // A wildcard covers every queue, but only when enabled.
        let mut all = PayloadEncryptionConfig {
            enabled: true,
            encrypted_queues: vec!["*".to_string()],
            ..Default::default()
        };
        assert!(all.encrypts_queue("anything"));
        all.enabled = false;
        assert!(!all.encrypts_queue("anything"));

        // Errors name the file.
        let missing = dir.path().join("missing.toml");
        let err = PayloadEncryptionConfig::load(Some(&missing)).unwrap_err();
        assert!(err.to_string().contains("missing.toml"), "{err}");
        std::fs::write(&path, "[encryption]\nenabled = \"yes\"\n").unwrap();
        let err = PayloadEncryptionConfig::load(Some(&path)).unwrap_err();
        assert!(err.to_string().contains("hammerwork.toml"), "{err}");
    }
}
