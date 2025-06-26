use crate::{error::HammerworkError, stats::JobStatistics, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

/// Configuration for alerting thresholds and targets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertingConfig {
    /// Error rate threshold (0.0 to 1.0) that triggers alerts
    pub error_rate_threshold: Option<f64>,
    /// Queue depth threshold that triggers alerts
    pub queue_depth_threshold: Option<u64>,
    /// Worker starvation time threshold
    pub worker_starvation_threshold: Option<Duration>,
    /// Processing time threshold that triggers alerts
    pub processing_time_threshold: Option<Duration>,
    /// Alert targets configuration
    pub targets: Vec<AlertTarget>,
    /// Cooldown period between alerts of the same type
    pub cooldown_period: Duration,
    /// Whether alerting is enabled
    pub enabled: bool,
    /// Custom alert thresholds
    pub custom_thresholds: HashMap<String, f64>,
}

impl Default for AlertingConfig {
    fn default() -> Self {
        Self {
            error_rate_threshold: None,
            queue_depth_threshold: None,
            worker_starvation_threshold: None,
            processing_time_threshold: None,
            targets: Vec::new(),
            cooldown_period: Duration::from_secs(300),
            enabled: true,
            custom_thresholds: HashMap::new(),
        }
    }
}

impl AlertingConfig {
    /// Create a new alerting configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set error rate threshold for alerts
    pub fn alert_on_high_error_rate(mut self, threshold: f64) -> Self {
        self.error_rate_threshold = Some(threshold.clamp(0.0, 1.0));
        self
    }

    /// Set queue depth threshold for alerts
    pub fn alert_on_queue_depth(mut self, threshold: u64) -> Self {
        self.queue_depth_threshold = Some(threshold);
        self
    }

    /// Set worker starvation threshold for alerts
    pub fn alert_on_worker_starvation(mut self, threshold: Duration) -> Self {
        self.worker_starvation_threshold = Some(threshold);
        self
    }

    /// Set processing time threshold for alerts
    pub fn alert_on_slow_processing(mut self, threshold: Duration) -> Self {
        self.processing_time_threshold = Some(threshold);
        self
    }

    /// Add a webhook alert target
    pub fn webhook(mut self, url: &str) -> Self {
        self.targets.push(AlertTarget::Webhook {
            url: url.to_string(),
            headers: HashMap::new(),
        });
        self
    }

    /// Add a webhook with custom headers
    pub fn webhook_with_headers(mut self, url: &str, headers: HashMap<String, String>) -> Self {
        self.targets.push(AlertTarget::Webhook {
            url: url.to_string(),
            headers,
        });
        self
    }

    /// Add an email alert target
    pub fn email(mut self, recipient: &str) -> Self {
        self.targets.push(AlertTarget::Email {
            recipient: recipient.to_string(),
        });
        self
    }

    /// Add a Slack alert target
    pub fn slack(mut self, webhook_url: &str, channel: &str) -> Self {
        self.targets.push(AlertTarget::Slack {
            webhook_url: webhook_url.to_string(),
            channel: channel.to_string(),
        });
        self
    }

    /// Set cooldown period between alerts
    pub fn with_cooldown(mut self, cooldown: Duration) -> Self {
        self.cooldown_period = cooldown;
        self
    }

    /// Enable or disable alerting
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Add custom threshold
    pub fn with_custom_threshold(mut self, name: String, threshold: f64) -> Self {
        self.custom_thresholds.insert(name, threshold);
        self
    }
}

/// Alert target configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertTarget {
    /// Webhook alert target
    Webhook {
        url: String,
        headers: HashMap<String, String>,
    },
    /// Email alert target
    Email { recipient: String },
    /// Slack alert target
    Slack {
        webhook_url: String,
        channel: String,
    },
}

/// Alert severity levels
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Info => write!(f, "Info"),
            AlertSeverity::Warning => write!(f, "Warning"),
            AlertSeverity::Critical => write!(f, "Critical"),
        }
    }
}

/// Alert types that can be triggered
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertType {
    HighErrorRate,
    QueueDepthExceeded,
    WorkerStarvation,
    SlowProcessing,
    Custom(String),
}

impl std::fmt::Display for AlertType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertType::HighErrorRate => write!(f, "High Error Rate"),
            AlertType::QueueDepthExceeded => write!(f, "Queue Depth Exceeded"),
            AlertType::WorkerStarvation => write!(f, "Worker Starvation"),
            AlertType::SlowProcessing => write!(f, "Slow Processing"),
            AlertType::Custom(name) => write!(f, "Custom: {}", name),
        }
    }
}

/// An alert that has been triggered
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    /// Type of alert
    pub alert_type: AlertType,
    /// Severity level
    pub severity: AlertSeverity,
    /// Queue name that triggered the alert
    pub queue_name: String,
    /// Alert message
    pub message: String,
    /// Current value that triggered the alert
    pub current_value: f64,
    /// Threshold that was exceeded
    pub threshold: f64,
    /// When the alert was triggered
    pub timestamp: DateTime<Utc>,
    /// Additional context data
    pub context: HashMap<String, String>,
}

/// Alert manager for monitoring thresholds and sending notifications
pub struct AlertManager {
    config: AlertingConfig,
    last_alerts: Arc<RwLock<HashMap<(String, AlertType), DateTime<Utc>>>>,
    #[cfg(feature = "alerting")]
    http_client: reqwest::Client,
}

impl AlertManager {
    /// Create a new alert manager
    pub fn new(config: AlertingConfig) -> Self {
        Self {
            config,
            last_alerts: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "alerting")]
            http_client: reqwest::Client::new(),
        }
    }

    /// Check statistics against thresholds and trigger alerts if needed
    pub async fn check_thresholds(&self, queue_name: &str, stats: &JobStatistics) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check error rate threshold
        if let Some(threshold) = self.config.error_rate_threshold {
            if stats.error_rate > threshold {
                let alert = Alert {
                    alert_type: AlertType::HighErrorRate,
                    severity: if stats.error_rate > threshold * 2.0 {
                        AlertSeverity::Critical
                    } else {
                        AlertSeverity::Warning
                    },
                    queue_name: queue_name.to_string(),
                    message: format!(
                        "High error rate detected: {:.2}% (threshold: {:.2}%)",
                        stats.error_rate * 100.0,
                        threshold * 100.0
                    ),
                    current_value: stats.error_rate,
                    threshold,
                    timestamp: Utc::now(),
                    context: self.build_context(stats),
                };

                self.send_alert_if_needed(alert).await?;
            }
        }

        // Check processing time threshold
        if let Some(threshold) = self.config.processing_time_threshold {
            let threshold_ms = threshold.as_millis() as f64;
            if stats.avg_processing_time_ms > threshold_ms {
                let alert = Alert {
                    alert_type: AlertType::SlowProcessing,
                    severity: if stats.avg_processing_time_ms > threshold_ms * 2.0 {
                        AlertSeverity::Critical
                    } else {
                        AlertSeverity::Warning
                    },
                    queue_name: queue_name.to_string(),
                    message: format!(
                        "Slow job processing detected: {:.0}ms average (threshold: {:.0}ms)",
                        stats.avg_processing_time_ms, threshold_ms
                    ),
                    current_value: stats.avg_processing_time_ms,
                    threshold: threshold_ms,
                    timestamp: Utc::now(),
                    context: self.build_context(stats),
                };

                self.send_alert_if_needed(alert).await?;
            }
        }

        Ok(())
    }

    /// Check queue depth and trigger alerts if needed
    pub async fn check_queue_depth(&self, queue_name: &str, depth: u64) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if let Some(threshold) = self.config.queue_depth_threshold {
            if depth > threshold {
                let alert = Alert {
                    alert_type: AlertType::QueueDepthExceeded,
                    severity: if depth > threshold * 2 {
                        AlertSeverity::Critical
                    } else {
                        AlertSeverity::Warning
                    },
                    queue_name: queue_name.to_string(),
                    message: format!(
                        "Queue depth exceeded: {} jobs (threshold: {})",
                        depth, threshold
                    ),
                    current_value: depth as f64,
                    threshold: threshold as f64,
                    timestamp: Utc::now(),
                    context: HashMap::new(),
                };

                self.send_alert_if_needed(alert).await?;
            }
        }

        Ok(())
    }

    /// Check for worker starvation and trigger alerts if needed
    pub async fn check_worker_starvation(
        &self,
        queue_name: &str,
        last_job_time: DateTime<Utc>,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        if let Some(threshold) = self.config.worker_starvation_threshold {
            let time_since_last_job = Utc::now() - last_job_time;
            let threshold_duration = chrono::Duration::from_std(threshold).unwrap();

            if time_since_last_job > threshold_duration {
                let alert = Alert {
                    alert_type: AlertType::WorkerStarvation,
                    severity: AlertSeverity::Warning,
                    queue_name: queue_name.to_string(),
                    message: format!(
                        "Worker starvation detected: no jobs processed for {} minutes (threshold: {} minutes)",
                        time_since_last_job.num_minutes(),
                        threshold_duration.num_minutes()
                    ),
                    current_value: time_since_last_job.num_minutes() as f64,
                    threshold: threshold_duration.num_minutes() as f64,
                    timestamp: Utc::now(),
                    context: HashMap::new(),
                };

                self.send_alert_if_needed(alert).await?;
            }
        }

        Ok(())
    }

    /// Send a custom alert
    pub async fn send_custom_alert(
        &self,
        queue_name: &str,
        alert_name: &str,
        message: &str,
        current_value: f64,
        threshold: f64,
        severity: AlertSeverity,
    ) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        let alert = Alert {
            alert_type: AlertType::Custom(alert_name.to_string()),
            severity,
            queue_name: queue_name.to_string(),
            message: message.to_string(),
            current_value,
            threshold,
            timestamp: Utc::now(),
            context: HashMap::new(),
        };

        self.send_alert_if_needed(alert).await
    }

    /// Send alert if cooldown period has passed
    async fn send_alert_if_needed(&self, alert: Alert) -> Result<()> {
        let alert_key = (alert.queue_name.clone(), alert.alert_type.clone());

        // Check cooldown period
        {
            let last_alerts = self.last_alerts.read().await;
            if let Some(last_time) = last_alerts.get(&alert_key) {
                let cooldown_duration = chrono::Duration::from_std(self.config.cooldown_period)
                    .map_err(|e| HammerworkError::Alerting {
                        message: format!("Invalid cooldown duration: {}", e),
                    })?;

                if Utc::now() - *last_time < cooldown_duration {
                    return Ok(()); // Still in cooldown period
                }
            }
        }

        // Send alert to all targets
        for target in &self.config.targets {
            if let Err(e) = self.send_to_target(&alert, target).await {
                tracing::warn!("Failed to send alert to target: {}", e);
            }
        }

        // Update last alert time
        {
            let mut last_alerts = self.last_alerts.write().await;
            last_alerts.insert(alert_key, alert.timestamp);
        }

        Ok(())
    }

    /// Send alert to a specific target
    async fn send_to_target(&self, alert: &Alert, target: &AlertTarget) -> Result<()> {
        match target {
            AlertTarget::Webhook { url, headers } => {
                self.send_webhook_alert(alert, url, headers).await
            }
            AlertTarget::Email { recipient } => {
                self.send_email_alert(alert, recipient).await
            }
            AlertTarget::Slack {
                webhook_url,
                channel,
            } => self.send_slack_alert(alert, webhook_url, channel).await,
        }
    }

    /// Send webhook alert
    #[cfg(feature = "alerting")]
    async fn send_webhook_alert(
        &self,
        alert: &Alert,
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "alert_type": alert.alert_type,
            "severity": alert.severity,
            "queue_name": alert.queue_name,
            "message": alert.message,
            "current_value": alert.current_value,
            "threshold": alert.threshold,
            "timestamp": alert.timestamp,
            "context": alert.context
        });

        let mut request = self.http_client.post(url).json(&payload);

        for (key, value) in headers {
            request = request.header(key, value);
        }

        let response = request.send().await.map_err(|e| HammerworkError::Alerting {
            message: format!("Failed to send webhook alert: {}", e),
        })?;

        if !response.status().is_success() {
            return Err(HammerworkError::Alerting {
                message: format!("Webhook alert failed with status: {}", response.status()),
            });
        }

        Ok(())
    }

    #[cfg(not(feature = "alerting"))]
    async fn send_webhook_alert(
        &self,
        _alert: &Alert,
        _url: &str,
        _headers: &HashMap<String, String>,
    ) -> Result<()> {
        tracing::info!("Webhook alerting disabled (alerting feature not enabled)");
        Ok(())
    }

    /// Send email alert (placeholder - requires email service integration)
    async fn send_email_alert(&self, alert: &Alert, _recipient: &str) -> Result<()> {
        tracing::info!(
            "Email alert: {} - {} ({})",
            alert.alert_type,
            alert.message,
            alert.severity
        );
        // TODO: Implement email sending when email service is integrated
        Ok(())
    }

    /// Send Slack alert
    #[cfg(feature = "alerting")]
    async fn send_slack_alert(
        &self,
        alert: &Alert,
        webhook_url: &str,
        channel: &str,
    ) -> Result<()> {
        let color = match alert.severity {
            AlertSeverity::Info => "#36a64f",      // Green
            AlertSeverity::Warning => "#ffaa00",   // Orange
            AlertSeverity::Critical => "#ff0000",  // Red
        };

        let payload = serde_json::json!({
            "channel": channel,
            "attachments": [{
                "color": color,
                "title": format!("Hammerwork Alert: {:?}", alert.alert_type),
                "text": alert.message,
                "fields": [
                    {
                        "title": "Queue",
                        "value": alert.queue_name,
                        "short": true
                    },
                    {
                        "title": "Current Value",
                        "value": format!("{:.2}", alert.current_value),
                        "short": true
                    },
                    {
                        "title": "Threshold",
                        "value": format!("{:.2}", alert.threshold),
                        "short": true
                    },
                    {
                        "title": "Severity",
                        "value": format!("{:?}", alert.severity),
                        "short": true
                    }
                ],
                "timestamp": alert.timestamp.timestamp()
            }]
        });

        let response = self
            .http_client
            .post(webhook_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| HammerworkError::Alerting {
                message: format!("Failed to send Slack alert: {}", e),
            })?;

        if !response.status().is_success() {
            return Err(HammerworkError::Alerting {
                message: format!("Slack alert failed with status: {}", response.status()),
            });
        }

        Ok(())
    }

    #[cfg(not(feature = "alerting"))]
    async fn send_slack_alert(
        &self,
        alert: &Alert,
        _webhook_url: &str,
        _channel: &str,
    ) -> Result<()> {
        tracing::info!(
            "Slack alert: {} - {} ({})",
            alert.alert_type,
            alert.message,
            alert.severity
        );
        Ok(())
    }

    /// Build context information for alerts
    fn build_context(&self, stats: &JobStatistics) -> HashMap<String, String> {
        let mut context = HashMap::new();
        context.insert("total_processed".to_string(), stats.total_processed.to_string());
        context.insert("completed".to_string(), stats.completed.to_string());
        context.insert("failed".to_string(), stats.failed.to_string());
        context.insert("dead".to_string(), stats.dead.to_string());
        context.insert("timed_out".to_string(), stats.timed_out.to_string());
        context.insert("running".to_string(), stats.running.to_string());
        context.insert(
            "throughput_per_minute".to_string(),
            format!("{:.2}", stats.throughput_per_minute),
        );
        context
    }

    /// Get alerting configuration
    pub fn config(&self) -> &AlertingConfig {
        &self.config
    }

    /// Update alerting configuration
    pub fn update_config(&mut self, config: AlertingConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_alerting_config_creation() {
        let config = AlertingConfig::new()
            .alert_on_high_error_rate(0.1)
            .alert_on_queue_depth(1000)
            .alert_on_worker_starvation(Duration::from_secs(300))
            .webhook("https://example.com/webhook")
            .email("admin@example.com")
            .slack("https://hooks.slack.com/webhook", "#alerts")
            .with_cooldown(Duration::from_secs(600));

        assert_eq!(config.error_rate_threshold, Some(0.1));
        assert_eq!(config.queue_depth_threshold, Some(1000));
        assert_eq!(
            config.worker_starvation_threshold,
            Some(Duration::from_secs(300))
        );
        assert_eq!(config.targets.len(), 3);
        assert_eq!(config.cooldown_period, Duration::from_secs(600));
    }

    #[test]
    fn test_alerting_config_defaults() {
        let config = AlertingConfig::default();
        assert!(config.error_rate_threshold.is_none());
        assert!(config.queue_depth_threshold.is_none());
        assert!(config.worker_starvation_threshold.is_none());
        assert!(config.targets.is_empty());
        assert_eq!(config.cooldown_period, Duration::from_secs(300));
        assert!(config.enabled);
    }

    #[test]
    fn test_alert_creation() {
        let alert = Alert {
            alert_type: AlertType::HighErrorRate,
            severity: AlertSeverity::Critical,
            queue_name: "test_queue".to_string(),
            message: "High error rate detected".to_string(),
            current_value: 0.15,
            threshold: 0.1,
            timestamp: Utc::now(),
            context: HashMap::new(),
        };

        assert_eq!(alert.alert_type, AlertType::HighErrorRate);
        assert_eq!(alert.severity, AlertSeverity::Critical);
        assert_eq!(alert.current_value, 0.15);
        assert_eq!(alert.threshold, 0.1);
    }

    #[test]
    fn test_alert_manager_creation() {
        let config = AlertingConfig::new();
        let manager = AlertManager::new(config);
        assert!(manager.config.enabled);
    }

    #[tokio::test]
    async fn test_error_rate_threshold_check() {
        let config = AlertingConfig::new().alert_on_high_error_rate(0.1);
        let manager = AlertManager::new(config);

        let stats = JobStatistics {
            error_rate: 0.15, // Above threshold
            total_processed: 100,
            completed: 85,
            failed: 15,
            ..Default::default()
        };

        // Should not fail even without targets configured
        let result = manager.check_thresholds("test_queue", &stats).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_queue_depth_threshold_check() {
        let config = AlertingConfig::new().alert_on_queue_depth(1000);
        let manager = AlertManager::new(config);

        let result = manager.check_queue_depth("test_queue", 1500).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_custom_alert() {
        let config = AlertingConfig::new();
        let manager = AlertManager::new(config);

        let result = manager
            .send_custom_alert(
                "test_queue",
                "custom_metric",
                "Custom metric exceeded threshold",
                42.0,
                30.0,
                AlertSeverity::Warning,
            )
            .await;

        assert!(result.is_ok());
    }

    #[test]
    fn test_alert_target_types() {
        let webhook = AlertTarget::Webhook {
            url: "https://example.com".to_string(),
            headers: HashMap::new(),
        };

        let email = AlertTarget::Email {
            recipient: "test@example.com".to_string(),
        };

        let slack = AlertTarget::Slack {
            webhook_url: "https://hooks.slack.com".to_string(),
            channel: "#alerts".to_string(),
        };

        match webhook {
            AlertTarget::Webhook { url, .. } => assert_eq!(url, "https://example.com"),
            _ => panic!("Expected webhook target"),
        }

        match email {
            AlertTarget::Email { recipient } => assert_eq!(recipient, "test@example.com"),
            _ => panic!("Expected email target"),
        }

        match slack {
            AlertTarget::Slack { channel, .. } => assert_eq!(channel, "#alerts"),
            _ => panic!("Expected slack target"),
        }
    }

    #[test]
    fn test_error_rate_clamping() {
        let config = AlertingConfig::new().alert_on_high_error_rate(1.5); // Invalid rate > 1.0
        assert_eq!(config.error_rate_threshold, Some(1.0)); // Should be clamped

        let config = AlertingConfig::new().alert_on_high_error_rate(-0.1); // Invalid rate < 0.0
        assert_eq!(config.error_rate_threshold, Some(0.0)); // Should be clamped
    }

    #[test]
    fn test_custom_thresholds() {
        let config = AlertingConfig::new()
            .with_custom_threshold("memory_usage".to_string(), 80.0)
            .with_custom_threshold("cpu_usage".to_string(), 90.0);

        assert_eq!(config.custom_thresholds.get("memory_usage"), Some(&80.0));
        assert_eq!(config.custom_thresholds.get("cpu_usage"), Some(&90.0));
    }
}