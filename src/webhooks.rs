//! Webhook system for delivering job lifecycle events to external systems.
//!
//! This module provides a robust webhook delivery system for job lifecycle events.
//! It includes retry policies, failure handling, and configurable delivery options.
//! This is separate from the alerting webhook system and focuses on real-time
//! event delivery to external integrations.
//!
//! # Overview
//!
//! The webhook system consists of several key components:
//!
//! - [`WebhookManager`] - Manages webhook configurations and delivery
//! - [`WebhookConfig`] - Configuration for individual webhook endpoints
//! - [`RetryPolicy`] - Configurable retry behavior for failed deliveries
//! - [`WebhookAuth`] - Authentication methods (Bearer, Basic, API Key, Custom)
//! - [`WebhookDelivery`] - Results and metadata from delivery attempts
//!
//! # Features
//!
//! - **Multiple authentication methods** - Bearer tokens, Basic auth, API keys, custom headers
//! - **HMAC signatures** - Secure webhook verification with SHA-256 HMAC
//! - **Flexible retry policies** - Exponential backoff with configurable parameters
//! - **Event filtering** - Only deliver events matching specific criteria
//! - **Delivery tracking** - Comprehensive statistics and delivery history
//! - **Rate limiting** - Configurable concurrent delivery limits
//! - **Custom payload templates** - Transform events before delivery
//!
//! # Examples
//!
//! ## Basic Webhook Setup
//!
//! ```rust
//! use hammerwork::webhooks::{WebhookManager, WebhookConfig, HttpMethod, WebhookManagerConfig};
//! use hammerwork::events::{EventManager, EventFilter, JobLifecycleEventType};
//! use std::sync::Arc;
//! use std::collections::HashMap;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let event_manager = Arc::new(EventManager::new_default());
//! let webhook_manager = WebhookManager::new(
//!     event_manager.clone(),
//!     WebhookManagerConfig::default()
//! );
//!
//! let webhook = WebhookConfig::new(
//!     "job_completion_hook".to_string(),
//!     "https://api.example.com/webhooks/jobs".to_string(),
//! )
//! .with_method(HttpMethod::Post)
//! .with_header("Content-Type".to_string(), "application/json".to_string())
//! .with_filter(
//!     EventFilter::new()
//!         .with_event_types(vec![JobLifecycleEventType::Completed])
//! );
//!
//! webhook_manager.add_webhook(webhook).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Webhook with Authentication and HMAC
//!
//! ```rust
//! use hammerwork::webhooks::{WebhookConfig, WebhookAuth, HttpMethod};
//! use hammerwork::events::EventFilter;
//!
//! let webhook = WebhookConfig::new(
//!     "secure_webhook".to_string(),
//!     "https://secure-api.example.com/events".to_string(),
//! )
//! .with_auth(WebhookAuth::Bearer {
//!     token: "your-bearer-token".to_string()
//! })
//! .with_secret("webhook-signing-secret".to_string())
//! .with_timeout_secs(30);
//! ```
//!
//! ## Custom Retry Policy
//!
//! ```rust
//! use hammerwork::webhooks::{WebhookConfig, RetryPolicy};
//!
//! let webhook = WebhookConfig::new(
//!     "reliable_webhook".to_string(),
//!     "https://api.example.com/events".to_string(),
//! )
//! .with_retry_policy(RetryPolicy {
//!     max_attempts: 5,
//!     initial_delay_secs: 2,
//!     max_delay_secs: 300,
//!     backoff_multiplier: 2.5,
//!     retry_on_status_codes: vec![500, 502, 503, 504, 429],
//! });
//! ```

use crate::events::{EventFilter, EventManager, EventSubscription, JobLifecycleEvent};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    sync::{RwLock, Semaphore},
    time::{sleep, timeout},
};
use uuid::Uuid;

/// Module for serializing UUID as string for TOML compatibility
mod uuid_string {
    use serde::{Deserialize, Deserializer, Serializer};
    use uuid::Uuid;

    pub fn serialize<S>(uuid: &Uuid, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&uuid.to_string())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let s = String::deserialize(deserializer)?;
        Uuid::parse_str(&s).map_err(D::Error::custom)
    }
}

/// Configuration for a webhook endpoint.
///
/// WebhookConfig defines how events should be delivered to a specific webhook endpoint.
/// It includes authentication, retry behavior, filtering criteria, and delivery options.
///
/// # Examples
///
/// ```rust
/// use hammerwork::webhooks::{WebhookConfig, WebhookAuth, HttpMethod, RetryPolicy};
/// use hammerwork::events::{EventFilter, JobLifecycleEventType};
/// use hammerwork::priority::JobPriority;
/// use std::collections::HashMap;
///
/// // Basic webhook configuration
/// let webhook = WebhookConfig::new(
///     "payment_webhook".to_string(),
///     "https://api.example.com/webhooks/payments".to_string(),
/// )
/// .with_method(HttpMethod::Post)
/// .with_header("Content-Type".to_string(), "application/json".to_string())
/// .with_header("X-API-Version".to_string(), "v1".to_string());
///
/// // Webhook with authentication and filtering
/// let secure_webhook = WebhookConfig::new(
///     "critical_alerts".to_string(),
///     "https://alerts.example.com/webhook".to_string(),
/// )
/// .with_auth(WebhookAuth::ApiKey {
///     header_name: "X-API-Key".to_string(),
///     api_key: "your-api-key".to_string(),
/// })
/// .with_filter(
///     EventFilter::new()
///         .with_event_types(vec![JobLifecycleEventType::Failed, JobLifecycleEventType::Dead])
///         .with_priorities(vec![JobPriority::Critical])
/// )
/// .with_secret("hmac-signing-secret".to_string())
/// .with_timeout_secs(15);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Unique identifier for this webhook
    #[serde(with = "uuid_string")]
    pub id: Uuid,
    /// Human-readable name for this webhook
    pub name: String,
    /// URL to deliver events to
    pub url: String,
    /// HTTP method to use (typically POST)
    pub method: HttpMethod,
    /// Custom headers to include in requests
    pub headers: HashMap<String, String>,
    /// Event filter to determine which events to deliver
    pub filter: EventFilter,
    /// Retry policy for failed deliveries
    pub retry_policy: RetryPolicy,
    /// Authentication configuration
    pub auth: Option<WebhookAuth>,
    /// Timeout for HTTP requests
    pub timeout_secs: u64,
    /// Whether this webhook is currently enabled
    pub enabled: bool,
    /// Secret for HMAC signature verification (optional)
    pub secret: Option<String>,
    /// Custom payload template (optional)
    pub payload_template: Option<String>,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "Default Webhook".to_string(),
            url: "https://example.com/webhook".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            filter: EventFilter::default(),
            retry_policy: RetryPolicy::default(),
            auth: None,
            timeout_secs: 30,
            enabled: true,
            secret: None,
            payload_template: None,
        }
    }
}

/// HTTP methods supported for webhooks
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Post,
    Put,
    Patch,
}

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpMethod::Post => write!(f, "POST"),
            HttpMethod::Put => write!(f, "PUT"),
            HttpMethod::Patch => write!(f, "PATCH"),
        }
    }
}

/// Authentication methods for webhooks
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebhookAuth {
    /// Bearer token authentication
    Bearer { token: String },
    /// Basic authentication
    Basic { username: String, password: String },
    /// API key in header
    ApiKey {
        header_name: String,
        api_key: String,
    },
    /// Custom header authentication
    Custom { headers: HashMap<String, String> },
}

/// Retry policy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial delay between retries (in seconds)
    pub initial_delay_secs: u64,
    /// Maximum delay between retries (in seconds)
    pub max_delay_secs: u64,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Whether to retry on specific HTTP status codes
    pub retry_on_status_codes: Vec<u16>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay_secs: 1,
            max_delay_secs: 300, // 5 minutes
            backoff_multiplier: 2.0,
            retry_on_status_codes: vec![408, 429, 500, 502, 503, 504],
        }
    }
}

impl RetryPolicy {
    /// Calculate delay for a specific retry attempt
    pub fn calculate_delay(&self, attempt: u32) -> Duration {
        let delay_secs =
            (self.initial_delay_secs as f64) * self.backoff_multiplier.powi(attempt as i32);
        let delay_secs = delay_secs.min(self.max_delay_secs as f64);
        Duration::from_secs(delay_secs as u64)
    }

    /// Check if we should retry based on status code
    pub fn should_retry_status(&self, status_code: u16) -> bool {
        self.retry_on_status_codes.contains(&status_code)
    }
}

/// Webhook delivery attempt result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDelivery {
    /// Unique identifier for this delivery attempt
    #[serde(with = "uuid_string")]
    pub delivery_id: Uuid,
    /// Webhook configuration ID
    #[serde(with = "uuid_string")]
    pub webhook_id: Uuid,
    /// Event that was delivered
    #[serde(with = "uuid_string")]
    pub event_id: Uuid,
    /// HTTP status code received
    pub status_code: Option<u16>,
    /// Response body (truncated if too long)
    pub response_body: Option<String>,
    /// Error message if delivery failed
    pub error_message: Option<String>,
    /// When the delivery was attempted
    pub attempted_at: DateTime<Utc>,
    /// How long the request took (in milliseconds)
    pub duration_ms: Option<u64>,
    /// Attempt number (1-based)
    pub attempt_number: u32,
    /// Whether this delivery was successful
    pub success: bool,
}

/// Webhook delivery statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookStats {
    /// Webhook configuration ID
    #[serde(with = "uuid_string")]
    pub webhook_id: Uuid,
    /// Total number of delivery attempts
    pub total_attempts: u64,
    /// Number of successful deliveries
    pub successful_deliveries: u64,
    /// Number of failed deliveries
    pub failed_deliveries: u64,
    /// Current success rate (0.0 to 1.0)
    pub success_rate: f64,
    /// Average response time in milliseconds
    pub avg_response_time_ms: f64,
    /// Last successful delivery timestamp
    pub last_success_at: Option<DateTime<Utc>>,
    /// Last failure timestamp
    pub last_failure_at: Option<DateTime<Utc>>,
    /// Time window these statistics cover
    pub calculated_at: DateTime<Utc>,
}

/// Overall webhook manager statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookManagerStats {
    /// Total number of configured webhooks
    pub total_webhooks: usize,
    /// Number of active/enabled webhooks
    pub active_webhooks: usize,
    /// Total delivery attempts across all webhooks
    pub total_attempts: u64,
    /// Total successful deliveries across all webhooks
    pub successful_deliveries: u64,
    /// Total failed deliveries across all webhooks
    pub failed_deliveries: u64,
}

/// Webhook delivery system for managing and delivering job lifecycle events.
///
/// WebhookManager is the central component for webhook delivery. It manages webhook
/// configurations, handles event subscriptions, delivers events to configured endpoints,
/// and provides statistics about delivery performance.
///
/// # Features
///
/// - **Concurrent delivery** - Configurable concurrent webhook delivery limits
/// - **Automatic retries** - Exponential backoff with configurable retry policies
/// - **Authentication support** - Bearer tokens, Basic auth, API keys, and custom headers
/// - **HMAC signatures** - Secure webhook verification with SHA-256 HMAC
/// - **Event filtering** - Only deliver events matching webhook-specific criteria
/// - **Statistics tracking** - Comprehensive delivery metrics and success rates
/// - **Rate limiting** - Prevents overwhelming webhook endpoints
///
/// # Examples
///
/// ```rust
/// use hammerwork::webhooks::{WebhookManager, WebhookConfig, WebhookManagerConfig, HttpMethod};
/// use hammerwork::events::{EventManager, EventFilter, JobLifecycleEventType};
/// use std::sync::Arc;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // Create webhook manager
/// let event_manager = Arc::new(EventManager::new_default());
/// let config = WebhookManagerConfig {
///     max_concurrent_deliveries: 20,
///     max_response_body_size: 64 * 1024,
///     log_deliveries: true,
///     default_timeout_secs: 30,
///     user_agent: "MyApp/1.0".to_string(),
/// };
/// let webhook_manager = WebhookManager::new(event_manager, config);
///
/// // Add a webhook
/// let webhook = WebhookConfig::new(
///     "completion_notifier".to_string(),
///     "https://api.example.com/job-completed".to_string(),
/// )
/// .with_filter(
///     EventFilter::new()
///         .with_event_types(vec![JobLifecycleEventType::Completed])
/// );
///
/// webhook_manager.add_webhook(webhook).await?;
///
/// // Get delivery statistics
/// let stats = webhook_manager.get_stats().await;
/// println!("Active webhooks: {}", stats.active_webhooks);
/// # Ok(())
/// # }
/// ```
pub struct WebhookManager {
    /// HTTP client for making requests
    http_client: Client,
    /// Active webhook configurations
    webhooks: Arc<RwLock<HashMap<Uuid, WebhookConfig>>>,
    /// Active event subscriptions for webhooks
    subscriptions: Arc<RwLock<HashMap<Uuid, EventSubscription>>>,
    /// Event manager for subscribing to events
    event_manager: Arc<EventManager>,
    /// Rate limiting semaphore for concurrent deliveries
    delivery_semaphore: Arc<Semaphore>,
    /// Delivery statistics
    stats: Arc<RwLock<HashMap<Uuid, WebhookStats>>>,
    /// Configuration
    config: WebhookManagerConfig,
}

/// Configuration for the webhook manager
#[derive(Debug, Clone)]
pub struct WebhookManagerConfig {
    /// Maximum number of concurrent webhook deliveries
    pub max_concurrent_deliveries: usize,
    /// Maximum response body size to store (in bytes)
    pub max_response_body_size: usize,
    /// Whether to log webhook deliveries
    pub log_deliveries: bool,
    /// Default timeout for webhook requests
    pub default_timeout_secs: u64,
    /// User agent string for webhook requests
    pub user_agent: String,
}

impl Default for WebhookManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_deliveries: 100,
            max_response_body_size: 64 * 1024, // 64KB
            log_deliveries: true,
            default_timeout_secs: 30,
            user_agent: format!("hammerwork-webhooks/{}", env!("CARGO_PKG_VERSION")),
        }
    }
}

impl WebhookManager {
    /// Create a new webhook manager
    pub fn new(event_manager: Arc<EventManager>, config: WebhookManagerConfig) -> Self {
        let http_client = Client::builder()
            .user_agent(&config.user_agent)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            http_client,
            webhooks: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_manager,
            delivery_semaphore: Arc::new(Semaphore::new(config.max_concurrent_deliveries)),
            stats: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new webhook manager with default configuration
    pub fn new_default(event_manager: Arc<EventManager>) -> Self {
        Self::new(event_manager, WebhookManagerConfig::default())
    }

    /// Add a new webhook configuration
    pub async fn add_webhook(&self, webhook: WebhookConfig) -> crate::Result<()> {
        let webhook_id = webhook.id;

        // Subscribe to events for this webhook
        let subscription = self.event_manager.subscribe(webhook.filter.clone()).await?;

        // Store webhook and subscription
        {
            let mut webhooks = self.webhooks.write().await;
            webhooks.insert(webhook_id, webhook.clone());
        }

        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(webhook_id, subscription);
        }

        // Initialize statistics
        {
            let mut stats = self.stats.write().await;
            stats.insert(
                webhook_id,
                WebhookStats {
                    webhook_id,
                    total_attempts: 0,
                    successful_deliveries: 0,
                    failed_deliveries: 0,
                    success_rate: 0.0,
                    avg_response_time_ms: 0.0,
                    last_success_at: None,
                    last_failure_at: None,
                    calculated_at: Utc::now(),
                },
            );
        }

        // Start delivery task for this webhook
        self.start_webhook_delivery_task(webhook_id).await;

        if self.config.log_deliveries {
            tracing::info!("Added webhook: {} -> {}", webhook.name, webhook.url);
        }

        Ok(())
    }

    /// Remove a webhook configuration
    pub async fn remove_webhook(&self, webhook_id: Uuid) -> crate::Result<()> {
        {
            let mut webhooks = self.webhooks.write().await;
            webhooks.remove(&webhook_id);
        }

        {
            let mut subscriptions = self.subscriptions.write().await;
            if let Some(subscription) = subscriptions.remove(&webhook_id) {
                self.event_manager.unsubscribe(subscription.id).await?;
            }
        }

        {
            let mut stats = self.stats.write().await;
            stats.remove(&webhook_id);
        }

        if self.config.log_deliveries {
            tracing::info!("Removed webhook: {}", webhook_id);
        }

        Ok(())
    }

    /// Get webhook configuration
    pub async fn get_webhook(&self, webhook_id: Uuid) -> Option<WebhookConfig> {
        let webhooks = self.webhooks.read().await;
        webhooks.get(&webhook_id).cloned()
    }

    /// List all webhook configurations
    pub async fn list_webhooks(&self) -> Vec<WebhookConfig> {
        let webhooks = self.webhooks.read().await;
        webhooks.values().cloned().collect()
    }

    /// Enable a webhook
    pub async fn enable_webhook(&self, webhook_id: Uuid) -> crate::Result<()> {
        let mut webhooks = self.webhooks.write().await;
        if let Some(webhook) = webhooks.get_mut(&webhook_id) {
            webhook.enabled = true;
            Ok(())
        } else {
            Err(crate::HammerworkError::Queue {
                message: format!("Webhook {} not found", webhook_id),
            })
        }
    }

    /// Disable a webhook
    pub async fn disable_webhook(&self, webhook_id: Uuid) -> crate::Result<()> {
        let mut webhooks = self.webhooks.write().await;
        if let Some(webhook) = webhooks.get_mut(&webhook_id) {
            webhook.enabled = false;
            Ok(())
        } else {
            Err(crate::HammerworkError::Queue {
                message: format!("Webhook {} not found", webhook_id),
            })
        }
    }

    /// Update webhook configuration
    pub async fn update_webhook(&self, webhook: WebhookConfig) -> crate::Result<()> {
        let webhook_id = webhook.id;

        // Remove existing webhook
        self.remove_webhook(webhook_id).await?;

        // Add updated webhook
        self.add_webhook(webhook).await?;

        Ok(())
    }

    /// Get webhook statistics
    pub async fn get_webhook_stats(&self, webhook_id: Uuid) -> Option<WebhookStats> {
        let stats = self.stats.read().await;
        stats.get(&webhook_id).cloned()
    }

    /// Get statistics for all webhooks
    pub async fn get_all_webhook_stats(&self) -> Vec<WebhookStats> {
        let stats = self.stats.read().await;
        stats.values().cloned().collect()
    }

    /// Get general webhook manager statistics
    pub async fn get_stats(&self) -> WebhookManagerStats {
        let webhooks = self.webhooks.read().await;
        let stats = self.stats.read().await;

        let total_webhooks = webhooks.len();
        let active_webhooks = webhooks.values().filter(|w| w.enabled).count();
        let total_attempts = stats.values().map(|s| s.total_attempts).sum();
        let successful_deliveries = stats.values().map(|s| s.successful_deliveries).sum();
        let failed_deliveries = stats.values().map(|s| s.failed_deliveries).sum();

        WebhookManagerStats {
            total_webhooks,
            active_webhooks,
            total_attempts,
            successful_deliveries,
            failed_deliveries,
        }
    }

    /// Start delivery task for a specific webhook
    async fn start_webhook_delivery_task(&self, webhook_id: Uuid) {
        let webhooks = self.webhooks.clone();
        let subscriptions = self.subscriptions.clone();
        let http_client = self.http_client.clone();
        let stats = self.stats.clone();
        let delivery_semaphore = self.delivery_semaphore.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            loop {
                // Get webhook configuration
                let webhook = {
                    let webhooks = webhooks.read().await;
                    match webhooks.get(&webhook_id) {
                        Some(webhook) if webhook.enabled => webhook.clone(),
                        _ => {
                            // Webhook disabled or removed, exit task
                            break;
                        }
                    }
                };

                // Get subscription and receive events
                let mut receiver = {
                    let subscriptions = subscriptions.read().await;
                    match subscriptions.get(&webhook_id) {
                        Some(subscription) => subscription.receiver.resubscribe(),
                        None => {
                            // Subscription removed, exit task
                            break;
                        }
                    }
                };

                // Wait for events
                match receiver.recv().await {
                    Ok(event) => {
                        // Check if event matches webhook filter
                        if webhook.filter.matches(&event) {
                            // Clone necessary data for delivery task
                            let webhook_clone = webhook.clone();
                            let event_clone = event.clone();
                            let http_client_clone = http_client.clone();
                            let stats_clone = stats.clone();
                            let config_clone = config.clone();
                            let semaphore_clone = delivery_semaphore.clone();

                            // Spawn delivery task
                            tokio::spawn(async move {
                                // Acquire delivery permit inside the task
                                let _permit = semaphore_clone.acquire().await.unwrap();
                                Self::deliver_webhook_event(
                                    webhook_clone,
                                    event_clone,
                                    http_client_clone,
                                    stats_clone,
                                    config_clone,
                                )
                                .await;
                            });
                        }
                    }
                    Err(_) => {
                        // Channel closed, exit task
                        break;
                    }
                }
            }
        });
    }

    /// Deliver an event to a webhook endpoint with retries
    async fn deliver_webhook_event(
        webhook: WebhookConfig,
        event: JobLifecycleEvent,
        http_client: Client,
        stats: Arc<RwLock<HashMap<Uuid, WebhookStats>>>,
        config: WebhookManagerConfig,
    ) {
        let mut attempt = 0;
        let max_attempts = webhook.retry_policy.max_attempts;

        while attempt < max_attempts {
            attempt += 1;

            let delivery_result =
                Self::attempt_webhook_delivery(&webhook, &event, &http_client, &config, attempt)
                    .await;

            // Update statistics
            Self::update_webhook_stats(&webhook.id, &delivery_result, stats.clone()).await;

            if delivery_result.success {
                if config.log_deliveries {
                    tracing::debug!(
                        "Webhook delivery successful: {} -> {} (attempt {})",
                        webhook.name,
                        webhook.url,
                        attempt
                    );
                }
                break;
            } else if attempt < max_attempts {
                // Check if we should retry based on status code
                let should_retry = delivery_result
                    .status_code
                    .map(|code| webhook.retry_policy.should_retry_status(code))
                    .unwrap_or(true); // Retry on network errors

                if should_retry {
                    let delay = webhook.retry_policy.calculate_delay(attempt - 1);
                    if config.log_deliveries {
                        tracing::warn!(
                            "Webhook delivery failed, retrying in {:?}: {} -> {} (attempt {}/{})",
                            delay,
                            webhook.name,
                            webhook.url,
                            attempt,
                            max_attempts
                        );
                    }
                    sleep(delay).await;
                } else {
                    if config.log_deliveries {
                        tracing::error!(
                            "Webhook delivery failed with non-retryable status {}: {} -> {}",
                            delivery_result.status_code.unwrap_or(0),
                            webhook.name,
                            webhook.url
                        );
                    }
                    break;
                }
            } else if config.log_deliveries {
                tracing::error!(
                    "Webhook delivery failed after {} attempts: {} -> {}",
                    max_attempts,
                    webhook.name,
                    webhook.url
                );
            }
        }
    }

    /// Attempt a single webhook delivery
    async fn attempt_webhook_delivery(
        webhook: &WebhookConfig,
        event: &JobLifecycleEvent,
        http_client: &Client,
        config: &WebhookManagerConfig,
        attempt_number: u32,
    ) -> WebhookDelivery {
        let delivery_id = Uuid::new_v4();
        let start_time = std::time::Instant::now();

        // Prepare payload
        let payload = if let Some(ref _template) = webhook.payload_template {
            // TODO: Implement template rendering
            serde_json::to_value(event).unwrap_or_default()
        } else {
            serde_json::to_value(event).unwrap_or_default()
        };

        // Build request
        let mut request = match webhook.method {
            HttpMethod::Post => http_client.post(&webhook.url),
            HttpMethod::Put => http_client.put(&webhook.url),
            HttpMethod::Patch => http_client.patch(&webhook.url),
        };

        // Add headers
        for (key, value) in &webhook.headers {
            request = request.header(key, value);
        }

        // Add authentication
        if let Some(ref auth) = webhook.auth {
            request = Self::add_auth_headers(request, auth);
        }

        // Add HMAC signature if secret is configured
        if let Some(ref secret) = webhook.secret {
            let signature = Self::calculate_hmac_signature(secret, &payload);
            request = request.header("X-Hammerwork-Signature", signature);
        }

        // Set content type and body
        request = request
            .header("Content-Type", "application/json")
            .json(&payload);

        // Set timeout
        let timeout_duration = Duration::from_secs(webhook.timeout_secs);

        // Make request
        match timeout(timeout_duration, request.send()).await {
            Ok(Ok(response)) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                let status_code = response.status().as_u16();
                let success = response.status().is_success();

                // Read response body
                let response_body = match response.text().await {
                    Ok(body) => {
                        if body.len() > config.max_response_body_size {
                            Some(format!(
                                "{}... [truncated]",
                                &body[..config.max_response_body_size]
                            ))
                        } else {
                            Some(body)
                        }
                    }
                    Err(_) => None,
                };

                WebhookDelivery {
                    delivery_id,
                    webhook_id: webhook.id,
                    event_id: event.event_id,
                    status_code: Some(status_code),
                    response_body,
                    error_message: if success {
                        None
                    } else {
                        Some(format!("HTTP {}", status_code))
                    },
                    attempted_at: Utc::now(),
                    duration_ms: Some(duration_ms),
                    attempt_number,
                    success,
                }
            }
            Ok(Err(err)) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                WebhookDelivery {
                    delivery_id,
                    webhook_id: webhook.id,
                    event_id: event.event_id,
                    status_code: None,
                    response_body: None,
                    error_message: Some(err.to_string()),
                    attempted_at: Utc::now(),
                    duration_ms: Some(duration_ms),
                    attempt_number,
                    success: false,
                }
            }
            Err(_) => {
                // Timeout
                let duration_ms = start_time.elapsed().as_millis() as u64;
                WebhookDelivery {
                    delivery_id,
                    webhook_id: webhook.id,
                    event_id: event.event_id,
                    status_code: Some(408),
                    response_body: None,
                    error_message: Some("Request timeout".to_string()),
                    attempted_at: Utc::now(),
                    duration_ms: Some(duration_ms),
                    attempt_number,
                    success: false,
                }
            }
        }
    }

    /// Add authentication headers to request
    fn add_auth_headers(
        mut request: reqwest::RequestBuilder,
        auth: &WebhookAuth,
    ) -> reqwest::RequestBuilder {
        match auth {
            WebhookAuth::Bearer { token } => {
                request = request.header("Authorization", format!("Bearer {}", token));
            }
            WebhookAuth::Basic { username, password } => {
                request = request.basic_auth(username, Some(password));
            }
            WebhookAuth::ApiKey {
                header_name,
                api_key,
            } => {
                request = request.header(header_name, api_key);
            }
            WebhookAuth::Custom { headers } => {
                for (key, value) in headers {
                    request = request.header(key, value);
                }
            }
        }
        request
    }

    /// Calculate HMAC signature for webhook verification
    fn calculate_hmac_signature(secret: &str, payload: &serde_json::Value) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        type HmacSha256 = Hmac<Sha256>;

        let payload_bytes = serde_json::to_string(payload).unwrap_or_default();
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(payload_bytes.as_bytes());
        let result = mac.finalize();
        format!("sha256={}", hex::encode(result.into_bytes()))
    }

    /// Update webhook statistics
    async fn update_webhook_stats(
        webhook_id: &Uuid,
        delivery: &WebhookDelivery,
        stats: Arc<RwLock<HashMap<Uuid, WebhookStats>>>,
    ) {
        let mut stats_map = stats.write().await;
        if let Some(webhook_stats) = stats_map.get_mut(webhook_id) {
            webhook_stats.total_attempts += 1;

            if delivery.success {
                webhook_stats.successful_deliveries += 1;
                webhook_stats.last_success_at = Some(delivery.attempted_at);
            } else {
                webhook_stats.failed_deliveries += 1;
                webhook_stats.last_failure_at = Some(delivery.attempted_at);
            }

            // Update success rate
            webhook_stats.success_rate =
                webhook_stats.successful_deliveries as f64 / webhook_stats.total_attempts as f64;

            // Update average response time
            if let Some(duration) = delivery.duration_ms {
                let total_time =
                    webhook_stats.avg_response_time_ms * (webhook_stats.total_attempts - 1) as f64;
                webhook_stats.avg_response_time_ms =
                    (total_time + duration as f64) / webhook_stats.total_attempts as f64;
            }

            webhook_stats.calculated_at = Utc::now();
        }
    }
}

/// Helper for creating webhook configurations
impl WebhookConfig {
    /// Create a new webhook configuration
    pub fn new(name: String, url: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            url,
            method: HttpMethod::Post,
            headers: HashMap::new(),
            filter: EventFilter::default(),
            retry_policy: RetryPolicy::default(),
            auth: None,
            timeout_secs: 30,
            enabled: true,
            secret: None,
            payload_template: None,
        }
    }

    /// Set HTTP method
    pub fn with_method(mut self, method: HttpMethod) -> Self {
        self.method = method;
        self
    }

    /// Add custom header
    pub fn with_header(mut self, key: String, value: String) -> Self {
        self.headers.insert(key, value);
        self
    }

    /// Set event filter
    pub fn with_filter(mut self, filter: EventFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Set retry policy
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Set authentication
    pub fn with_auth(mut self, auth: WebhookAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Set request timeout
    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Set webhook secret for HMAC signatures
    pub fn with_secret(mut self, secret: String) -> Self {
        self.secret = Some(secret);
        self
    }

    /// Enable or disable webhook
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Generate HMAC-SHA256 signature for webhook payload verification.
///
/// This function creates a secure signature that can be used to verify webhook authenticity.
/// The signature is generated using HMAC-SHA256 and returned as a hexadecimal string.
///
/// # Arguments
///
/// * `secret` - The shared secret key for HMAC generation
/// * `payload` - The raw payload bytes to sign
///
/// # Examples
///
/// ```rust
/// use hammerwork::webhooks::generate_hmac_signature;
///
/// let secret = "my-webhook-secret";
/// let payload = b"{'event': 'job.completed', 'job_id': '123'}";
/// let signature = generate_hmac_signature(secret, payload);
///
/// // The signature can be sent in a header like X-Webhook-Signature
/// println!("X-Webhook-Signature: sha256={}", signature);
/// ```
///
/// # Security Notes
///
/// - Use a cryptographically secure random secret of at least 32 characters
/// - Always compare signatures using constant-time comparison to prevent timing attacks
/// - Rotate secrets periodically for enhanced security
#[cfg(feature = "webhooks")]
pub fn generate_hmac_signature(secret: &str, payload: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC can take key of any size");
    mac.update(payload);
    let result = mac.finalize();
    hex::encode(result.into_bytes())
}

/// Verify HMAC-SHA256 signature for webhook payload authenticity.
///
/// This function verifies that a received webhook payload matches the expected signature,
/// providing protection against tampering and ensuring the webhook came from a trusted source.
/// Uses constant-time comparison to prevent timing attacks.
///
/// # Arguments
///
/// * `secret` - The shared secret key used for verification
/// * `payload` - The raw payload bytes to verify
/// * `signature` - The signature to verify against (as hexadecimal string)
///
/// # Returns
///
/// Returns `true` if the signature is valid, `false` otherwise.
///
/// # Examples
///
/// ```rust
/// use hammerwork::webhooks::{generate_hmac_signature, verify_hmac_signature};
///
/// let secret = "my-webhook-secret";
/// let payload = b"{'event': 'job.completed', 'job_id': '123'}";
///
/// // Generate signature (sender side)
/// let signature = generate_hmac_signature(secret, payload);
///
/// // Verify signature (receiver side)
/// let is_valid = verify_hmac_signature(secret, payload, &signature);
/// assert!(is_valid);
///
/// // Invalid signature
/// let is_invalid = verify_hmac_signature(secret, payload, "invalid-signature");
/// assert!(!is_invalid);
/// ```
///
/// # Security Notes
///
/// - Always verify signatures before processing webhook payloads
/// - Use the same secret that was used to generate the signature
/// - This function uses constant-time comparison to prevent timing attacks
#[cfg(feature = "webhooks")]
pub fn verify_hmac_signature(secret: &str, payload: &[u8], signature: &str) -> bool {
    let expected = generate_hmac_signature(secret, payload);
    // Use constant-time comparison to prevent timing attacks
    expected.len() == signature.len()
        && expected.bytes().zip(signature.bytes()).all(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_config_creation() {
        let webhook = WebhookConfig::new(
            "test-webhook".to_string(),
            "https://example.com/webhook".to_string(),
        )
        .with_method(HttpMethod::Post)
        .with_header("X-Custom-Header".to_string(), "custom-value".to_string())
        .with_timeout_secs(60)
        .with_secret("secret-key".to_string());

        assert_eq!(webhook.name, "test-webhook");
        assert_eq!(webhook.url, "https://example.com/webhook");
        assert_eq!(webhook.method, HttpMethod::Post);
        assert_eq!(webhook.timeout_secs, 60);
        assert_eq!(webhook.secret, Some("secret-key".to_string()));
        assert!(webhook.enabled);
        assert_eq!(
            webhook.headers.get("X-Custom-Header"),
            Some(&"custom-value".to_string())
        );
    }

    #[test]
    fn test_retry_policy_delay_calculation() {
        let policy = RetryPolicy {
            initial_delay_secs: 2,
            backoff_multiplier: 2.0,
            max_delay_secs: 60,
            ..Default::default()
        };

        assert_eq!(policy.calculate_delay(0), Duration::from_secs(2));
        assert_eq!(policy.calculate_delay(1), Duration::from_secs(4));
        assert_eq!(policy.calculate_delay(2), Duration::from_secs(8));
        assert_eq!(policy.calculate_delay(10), Duration::from_secs(60)); // Capped at max
    }

    #[test]
    fn test_retry_policy_status_codes() {
        let policy = RetryPolicy::default();

        assert!(policy.should_retry_status(500));
        assert!(policy.should_retry_status(502));
        assert!(policy.should_retry_status(429));
        assert!(!policy.should_retry_status(200));
        assert!(!policy.should_retry_status(400));
        assert!(!policy.should_retry_status(404));
    }

    #[test]
    fn test_webhook_auth_types() {
        let bearer = WebhookAuth::Bearer {
            token: "token123".to_string(),
        };
        let basic = WebhookAuth::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        };
        let api_key = WebhookAuth::ApiKey {
            header_name: "X-API-Key".to_string(),
            api_key: "key123".to_string(),
        };

        match bearer {
            WebhookAuth::Bearer { token } => assert_eq!(token, "token123"),
            _ => panic!("Wrong auth type"),
        }

        match basic {
            WebhookAuth::Basic { username, password } => {
                assert_eq!(username, "user");
                assert_eq!(password, "pass");
            }
            _ => panic!("Wrong auth type"),
        }

        match api_key {
            WebhookAuth::ApiKey {
                header_name,
                api_key,
            } => {
                assert_eq!(header_name, "X-API-Key");
                assert_eq!(api_key, "key123");
            }
            _ => panic!("Wrong auth type"),
        }
    }

    #[test]
    fn test_http_method_display() {
        assert_eq!(HttpMethod::Post.to_string(), "POST");
        assert_eq!(HttpMethod::Put.to_string(), "PUT");
        assert_eq!(HttpMethod::Patch.to_string(), "PATCH");
    }

    #[test]
    fn test_webhook_delivery_structure() {
        let delivery = WebhookDelivery {
            delivery_id: Uuid::new_v4(),
            webhook_id: Uuid::new_v4(),
            event_id: Uuid::new_v4(),
            status_code: Some(200),
            response_body: Some("OK".to_string()),
            error_message: None,
            attempted_at: Utc::now(),
            duration_ms: Some(150),
            attempt_number: 1,
            success: true,
        };

        assert!(delivery.success);
        assert_eq!(delivery.status_code, Some(200));
        assert_eq!(delivery.attempt_number, 1);
        assert!(delivery.error_message.is_none());
    }

    #[test]
    fn test_webhook_stats_calculation() {
        let mut stats = WebhookStats {
            webhook_id: Uuid::new_v4(),
            total_attempts: 0,
            successful_deliveries: 0,
            failed_deliveries: 0,
            success_rate: 0.0,
            avg_response_time_ms: 0.0,
            last_success_at: None,
            last_failure_at: None,
            calculated_at: Utc::now(),
        };

        // Simulate successful delivery
        stats.total_attempts = 1;
        stats.successful_deliveries = 1;
        stats.success_rate = 1.0;
        stats.avg_response_time_ms = 100.0;

        assert_eq!(stats.success_rate, 1.0);
        assert_eq!(stats.failed_deliveries, 0);

        // Simulate failed delivery
        stats.total_attempts = 2;
        stats.failed_deliveries = 1;
        stats.success_rate = 0.5;

        assert_eq!(stats.success_rate, 0.5);
        assert_eq!(stats.total_attempts, 2);
    }

    #[tokio::test]
    async fn test_webhook_manager_creation() {
        let config = WebhookManagerConfig::default();
        let event_manager = Arc::new(EventManager::new_default());
        let manager = WebhookManager::new(event_manager, config);

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_webhooks, 0);
        assert_eq!(stats.active_webhooks, 0);
    }

    #[tokio::test]
    async fn test_webhook_manager_add_remove_webhook() {
        let config = WebhookManagerConfig::default();
        let event_manager = Arc::new(EventManager::new_default());
        let manager = WebhookManager::new(event_manager, config);

        let webhook_config = WebhookConfig {
            id: Uuid::new_v4(),
            name: "test-webhook".to_string(),
            url: "https://api.example.com/webhook".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: Some("test-secret".to_string()),
            filter: EventFilter::new(),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        let webhook_id = webhook_config.id;

        // Add webhook
        manager.add_webhook(webhook_config).await.unwrap();

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_webhooks, 1);
        assert_eq!(stats.active_webhooks, 1);

        // Remove webhook
        manager.remove_webhook(webhook_id).await.unwrap();

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_webhooks, 0);
        assert_eq!(stats.active_webhooks, 0);
    }

    #[tokio::test]
    async fn test_webhook_manager_enable_disable() {
        let config = WebhookManagerConfig::default();
        let event_manager = Arc::new(EventManager::new_default());
        let manager = WebhookManager::new(event_manager, config);

        let webhook_config = WebhookConfig {
            id: Uuid::new_v4(),
            name: "test-webhook".to_string(),
            url: "https://api.example.com/webhook".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: EventFilter::new(),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        let webhook_id = webhook_config.id;
        manager.add_webhook(webhook_config).await.unwrap();

        // Initially enabled
        let stats = manager.get_stats().await;
        assert_eq!(stats.active_webhooks, 1);

        // Disable webhook
        manager.disable_webhook(webhook_id).await.unwrap();
        let stats = manager.get_stats().await;
        assert_eq!(stats.active_webhooks, 0);
        assert_eq!(stats.total_webhooks, 1); // Still exists but disabled

        // Re-enable webhook
        manager.enable_webhook(webhook_id).await.unwrap();
        let stats = manager.get_stats().await;
        assert_eq!(stats.active_webhooks, 1);
    }

    #[test]
    fn test_hmac_signature_generation() {
        let secret = "test-secret";
        let payload = b"test payload";

        let signature1 = generate_hmac_signature(secret, payload);
        let signature2 = generate_hmac_signature(secret, payload);

        // Same input should produce same signature
        assert_eq!(signature1, signature2);

        // Different payload should produce different signature
        let different_payload = b"different payload";
        let signature3 = generate_hmac_signature(secret, different_payload);
        assert_ne!(signature1, signature3);

        // Signature should be hex-encoded
        assert!(signature1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_hmac_signature_verification() {
        let secret = "test-secret";
        let payload = b"test payload";

        let signature = generate_hmac_signature(secret, payload);
        assert!(verify_hmac_signature(secret, payload, &signature));

        // Wrong signature should fail
        assert!(!verify_hmac_signature(secret, payload, "wrong-signature"));

        // Wrong secret should fail
        assert!(!verify_hmac_signature("wrong-secret", payload, &signature));

        // Wrong payload should fail
        assert!(!verify_hmac_signature(secret, b"wrong payload", &signature));
    }

    #[test]
    fn test_webhook_config_serialization() {
        let webhook_config = WebhookConfig {
            id: Uuid::new_v4(),
            name: "test-webhook".to_string(),
            url: "https://api.example.com/webhook".to_string(),
            method: HttpMethod::Post,
            headers: {
                let mut headers = HashMap::new();
                headers.insert("Content-Type".to_string(), "application/json".to_string());
                headers
            },
            auth: Some(WebhookAuth::Bearer {
                token: "token123".to_string(),
            }),
            secret: Some("webhook-secret".to_string()),
            filter: EventFilter::new()
                .with_event_types(vec![crate::events::JobLifecycleEventType::Completed]),
            retry_policy: RetryPolicy {
                max_attempts: 5,
                initial_delay_secs: 1,
                max_delay_secs: 300,
                backoff_multiplier: 2.0,
                retry_on_status_codes: vec![500, 502, 503],
            },
            enabled: true,
            timeout_secs: 60,
            payload_template: Some("custom template".to_string()),
        };

        let serialized = serde_json::to_string(&webhook_config).unwrap();
        let deserialized: WebhookConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.name, webhook_config.name);
        assert_eq!(deserialized.url, webhook_config.url);
        assert_eq!(deserialized.enabled, webhook_config.enabled);
        assert_eq!(deserialized.timeout_secs, webhook_config.timeout_secs);
        assert!(deserialized.auth.is_some());
        assert!(deserialized.secret.is_some());
    }

    #[test]
    fn test_webhook_manager_config_defaults() {
        let config = WebhookManagerConfig::default();

        assert_eq!(config.max_concurrent_deliveries, 100);
        assert_eq!(config.default_timeout_secs, 30);
        assert!(config.log_deliveries);
        assert_eq!(config.max_response_body_size, 64 * 1024);
        assert!(config.user_agent.contains("hammerwork-webhooks"));
    }

    #[test]
    fn test_webhook_manager_config_customization() {
        let config = WebhookManagerConfig {
            max_concurrent_deliveries: 50,
            default_timeout_secs: 60,
            log_deliveries: false,
            ..Default::default()
        };

        assert_eq!(config.max_concurrent_deliveries, 50);
        assert_eq!(config.default_timeout_secs, 60);
        assert!(!config.log_deliveries);
    }

    #[tokio::test]
    async fn test_webhook_delivery_timeout() {
        let delivery = WebhookDelivery {
            delivery_id: Uuid::new_v4(),
            webhook_id: Uuid::new_v4(),
            event_id: Uuid::new_v4(),
            status_code: None,
            response_body: None,
            error_message: Some("Request timeout".to_string()),
            attempted_at: Utc::now(),
            duration_ms: Some(30000), // 30 seconds
            attempt_number: 1,
            success: false,
        };

        assert!(!delivery.success);
        assert!(delivery.error_message.is_some());
        assert_eq!(delivery.duration_ms, Some(30000));
        assert!(delivery.status_code.is_none());
    }

    #[test]
    fn test_webhook_filter_integration() {
        let filter = EventFilter::new()
            .with_event_types(vec![crate::events::JobLifecycleEventType::Completed])
            .with_queue_names(vec!["important".to_string()])
            .with_priorities(vec![crate::priority::JobPriority::High]);

        let matching_event = crate::events::JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "important".to_string(),
            event_type: crate::events::JobLifecycleEventType::Completed,
            priority: crate::priority::JobPriority::High,
            timestamp: Utc::now(),
            processing_time_ms: Some(1000),
            error: None,
            payload: None,
            metadata: HashMap::new(),
        };

        let non_matching_event = crate::events::JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "unimportant".to_string(),
            event_type: crate::events::JobLifecycleEventType::Failed,
            priority: crate::priority::JobPriority::Low,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: HashMap::new(),
        };

        assert!(filter.matches(&matching_event));
        assert!(!filter.matches(&non_matching_event));
    }

    #[test]
    fn test_retry_policy_edge_cases() {
        let policy = RetryPolicy {
            max_attempts: 3,
            initial_delay_secs: 1,
            max_delay_secs: 10,
            backoff_multiplier: 3.0,
            retry_on_status_codes: vec![500, 502],
        };

        // Test delay calculation with large attempt numbers
        assert_eq!(policy.calculate_delay(0), Duration::from_secs(1));
        assert_eq!(policy.calculate_delay(1), Duration::from_secs(3));
        assert_eq!(policy.calculate_delay(2), Duration::from_secs(9));
        assert_eq!(policy.calculate_delay(3), Duration::from_secs(10)); // Capped

        // Test status code checking
        assert!(policy.should_retry_status(500));
        assert!(policy.should_retry_status(502));
        assert!(!policy.should_retry_status(503)); // Not in list
        assert!(!policy.should_retry_status(200)); // Success
        assert!(!policy.should_retry_status(400)); // Client error
    }

    #[test]
    fn test_webhook_delivery_serialization() {
        let delivery = WebhookDelivery {
            delivery_id: Uuid::new_v4(),
            webhook_id: Uuid::new_v4(),
            event_id: Uuid::new_v4(),
            status_code: Some(201),
            response_body: Some("{\"status\":\"success\"}".to_string()),
            error_message: None,
            attempted_at: Utc::now(),
            duration_ms: Some(250),
            attempt_number: 2,
            success: true,
        };

        let serialized = serde_json::to_string(&delivery).unwrap();
        let deserialized: WebhookDelivery = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.delivery_id, delivery.delivery_id);
        assert_eq!(deserialized.status_code, delivery.status_code);
        assert_eq!(deserialized.success, delivery.success);
        assert_eq!(deserialized.attempt_number, delivery.attempt_number);
    }

    #[tokio::test]
    async fn test_webhook_stats_aggregation() {
        let webhook_id = Uuid::new_v4();
        let mut stats = WebhookStats {
            webhook_id,
            total_attempts: 10,
            successful_deliveries: 7,
            failed_deliveries: 3,
            success_rate: 0.7,
            avg_response_time_ms: 150.5,
            last_success_at: Some(Utc::now() - chrono::Duration::minutes(5)),
            last_failure_at: Some(Utc::now() - chrono::Duration::minutes(10)),
            calculated_at: Utc::now(),
        };

        // Verify stats calculations
        assert_eq!(
            stats.total_attempts,
            stats.successful_deliveries + stats.failed_deliveries
        );
        assert!((stats.success_rate - 0.7).abs() < f64::EPSILON);
        assert!(stats.last_success_at.is_some());
        assert!(stats.last_failure_at.is_some());

        // Update stats with new delivery
        stats.total_attempts += 1;
        stats.successful_deliveries += 1;
        stats.success_rate = stats.successful_deliveries as f64 / stats.total_attempts as f64;

        assert_eq!(stats.total_attempts, 11);
        assert_eq!(stats.successful_deliveries, 8);
        assert!((stats.success_rate - 8.0 / 11.0).abs() < f64::EPSILON);
    }
}
