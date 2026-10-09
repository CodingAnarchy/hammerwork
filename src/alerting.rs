use crate::{Result, error::HammerworkError, stats::JobStatistics};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

/// Timeout of a whole webhook or Slack alert request (connect, send and response).
pub const ALERT_HTTP_TIMEOUT: Duration = Duration::from_secs(10);

/// How long an alert that could not be delivered to any target waits before it is
/// tried again (at most the cooldown period), so a failing endpoint is not retried on
/// every check.
pub const FAILED_ALERT_RETRY_DELAY: Duration = Duration::from_secs(60);

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

    /// Add an email alert target without SMTP settings.
    ///
    /// Such a target cannot send anything: [`validate`](Self::validate) and
    /// [`AlertManager::try_new`] reject it. Use [`email_via_smtp`](Self::email_via_smtp).
    #[deprecated(note = "email targets need SMTP settings; use `email_via_smtp`")]
    pub fn email(mut self, recipient: &str) -> Self {
        self.targets.push(AlertTarget::Email {
            recipient: recipient.to_string(),
            smtp: None,
        });
        self
    }

    /// Add an email alert target delivered through the SMTP server in `smtp`.
    pub fn email_via_smtp(mut self, recipient: &str, smtp: SmtpConfig) -> Self {
        self.targets.push(AlertTarget::Email {
            recipient: recipient.to_string(),
            smtp: Some(smtp),
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

    /// Check the alert targets without contacting them.
    ///
    /// Fails with a configuration error for an email target without SMTP settings,
    /// with invalid SMTP settings, or with an invalid recipient address.
    pub fn validate(&self) -> Result<()> {
        for target in &self.targets {
            if let AlertTarget::Email { recipient, smtp } = target {
                parse_mailbox(recipient).map_err(|e| {
                    HammerworkError::Config(format!(
                        "email alert target: invalid recipient address {e}"
                    ))
                })?;
                let Some(smtp) = smtp else {
                    return Err(HammerworkError::Config(format!(
                        "email alert target '{recipient}' has no SMTP settings \
                         (set `smtp`, or use AlertingConfig::email_via_smtp)"
                    )));
                };
                smtp.validate().map_err(|e| {
                    HammerworkError::Config(format!("email alert target '{recipient}': {e}"))
                })?;
            }
        }
        Ok(())
    }
}

/// Where a secret such as an SMTP password comes from.
///
/// Prefer [`Environment`](Self::Environment) so the secret stays out of configuration
/// files. The value is read each time it is needed, so rotating the variable takes
/// effect on the next alert.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretSource {
    /// The secret itself. Only for development and tests.
    Static(String),
    /// The name of an environment variable holding the secret.
    Environment(String),
}

impl SecretSource {
    /// Resolve the secret.
    pub fn resolve(&self) -> Result<String> {
        match self {
            SecretSource::Static(value) => Ok(value.clone()),
            SecretSource::Environment(name) => {
                std::env::var(name).map_err(|e| HammerworkError::Alerting {
                    message: format!("cannot read SMTP password from ${name}: {e}"),
                })
            }
        }
    }
}

impl std::fmt::Debug for SecretSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretSource::Static(_) => f.write_str("Static(<redacted>)"),
            SecretSource::Environment(name) => f.debug_tuple("Environment").field(name).finish(),
        }
    }
}

/// How the connection to the SMTP server is secured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTls {
    /// Connect in plain text and upgrade with STARTTLS. Sending fails if the server
    /// does not offer STARTTLS; it never falls back to plain text. Default port 587.
    #[default]
    StartTls,
    /// TLS from the start of the connection (SMTPS). Default port 465.
    Implicit,
    /// No encryption at all. Only for a relay on localhost or a trusted network.
    /// Default port 25.
    None,
}

fn default_smtp_timeout_secs() -> u64 {
    30
}

/// SMTP server settings for an email alert target.
///
/// # Examples
///
/// ```rust
/// use hammerwork::alerting::{AlertingConfig, SecretSource, SmtpConfig, SmtpTls};
///
/// let smtp = SmtpConfig::new("smtp.example.com", "Hammerwork <alerts@example.com>")
///     .with_credentials("alerts@example.com", SecretSource::Environment("SMTP_PASSWORD".into()))
///     .with_tls(SmtpTls::StartTls);
///
/// let config = AlertingConfig::new()
///     .alert_on_queue_depth(10_000)
///     .email_via_smtp("oncall@example.com", smtp);
/// config.validate().unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtpConfig {
    /// SMTP server host name.
    pub host: String,
    /// Server port. Defaults to the usual port for the TLS mode (587, 465 or 25).
    #[serde(default)]
    pub port: Option<u16>,
    /// Sender address, e.g. `alerts@example.com` or `Hammerwork <alerts@example.com>`.
    pub from: String,
    /// User name for SMTP authentication. Requires `password`.
    #[serde(default)]
    pub username: Option<String>,
    /// Password for SMTP authentication. Requires `username`.
    #[serde(default)]
    pub password: Option<SecretSource>,
    /// Connection security. Defaults to [`SmtpTls::StartTls`].
    #[serde(default)]
    pub tls: SmtpTls,
    /// Timeout for each SMTP command, in seconds. Defaults to 30.
    #[serde(default = "default_smtp_timeout_secs")]
    pub timeout_secs: u64,
}

impl SmtpConfig {
    /// Settings for `host`, sending as `from`, with STARTTLS and no authentication.
    pub fn new(host: impl Into<String>, from: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            port: None,
            from: from.into(),
            username: None,
            password: None,
            tls: SmtpTls::default(),
            timeout_secs: default_smtp_timeout_secs(),
        }
    }

    /// Use a non-default port.
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Authenticate as `username` with the password from `password`.
    pub fn with_credentials(mut self, username: impl Into<String>, password: SecretSource) -> Self {
        self.username = Some(username.into());
        self.password = Some(password);
        self
    }

    /// Choose how the connection is secured.
    pub fn with_tls(mut self, tls: SmtpTls) -> Self {
        self.tls = tls;
        self
    }

    /// Set the per-command timeout.
    pub fn with_timeout_secs(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Check the settings without connecting to the server.
    pub fn validate(&self) -> Result<()> {
        let invalid =
            |message: String| HammerworkError::Config(format!("SMTP settings: {message}"));
        if self.host.trim().is_empty() {
            return Err(invalid("host is empty".to_string()));
        }
        parse_mailbox(&self.from).map_err(|e| invalid(format!("invalid from address: {e}")))?;
        match (&self.username, &self.password) {
            (Some(_), None) => Err(invalid("username is set without a password".to_string())),
            (None, Some(_)) => Err(invalid("password is set without a username".to_string())),
            _ => Ok(()),
        }
    }

    /// The port that will be used.
    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or(match self.tls {
            SmtpTls::StartTls => 587,
            SmtpTls::Implicit => 465,
            SmtpTls::None => 25,
        })
    }
}

fn parse_mailbox(address: &str) -> std::result::Result<lettre::message::Mailbox, String> {
    address.parse().map_err(|e| format!("'{address}': {e}"))
}

/// Replace control characters (such as line breaks) so a value is safe in a header.
fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// Alert target configuration
///
/// `Debug` never shows header values or the path of a webhook or Slack URL (a Slack
/// webhook URL is itself a credential).
#[derive(Clone, Serialize, Deserialize)]
pub enum AlertTarget {
    /// Webhook alert target
    Webhook {
        url: String,
        headers: HashMap<String, String>,
    },
    /// Email alert target, sent through the SMTP server in `smtp`.
    ///
    /// `smtp` is required; a target without it is rejected by
    /// [`AlertingConfig::validate`].
    Email {
        recipient: String,
        #[serde(default)]
        smtp: Option<SmtpConfig>,
    },
    /// Slack alert target
    Slack {
        webhook_url: String,
        channel: String,
    },
}

impl std::fmt::Debug for AlertTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertTarget::Webhook { url, headers } => {
                let mut names: Vec<&String> = headers.keys().collect();
                names.sort();
                f.debug_struct("Webhook")
                    .field("url", &crate::config::redact_url_path(url))
                    .field("headers", &names)
                    .finish()
            }
            AlertTarget::Email { recipient, smtp } => f
                .debug_struct("Email")
                .field("recipient", recipient)
                .field("smtp", smtp)
                .finish(),
            AlertTarget::Slack {
                webhook_url,
                channel,
            } => f
                .debug_struct("Slack")
                .field("webhook_url", &crate::config::redact_url_path(webhook_url))
                .field("channel", channel)
                .finish(),
        }
    }
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

/// Type alias for last alerts storage
type LastAlertsStorage = Arc<RwLock<HashMap<(String, AlertType), DateTime<Utc>>>>;

/// The HTTP client for webhook and Slack alerts: every request is bounded by
/// [`ALERT_HTTP_TIMEOUT`], so a hanging endpoint cannot stall the caller.
#[cfg(feature = "alerting")]
fn alert_http_client() -> reqwest::Client {
    alert_http_client_with_timeout(ALERT_HTTP_TIMEOUT)
}

#[cfg(feature = "alerting")]
fn alert_http_client_with_timeout(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
        .unwrap_or_else(|e| {
            tracing::error!("Failed to build the alert HTTP client with a timeout: {e}");
            reqwest::Client::new()
        })
}

/// Alert manager for monitoring thresholds and sending notifications
pub struct AlertManager {
    config: AlertingConfig,
    last_alerts: LastAlertsStorage,
    /// When each alert last failed to reach any target
    failed_alerts: LastAlertsStorage,
    #[cfg(feature = "alerting")]
    http_client: reqwest::Client,
}

impl AlertManager {
    /// Create a new alert manager.
    ///
    /// The configuration is not rejected here; an invalid configuration is logged at
    /// error level, and alerts to an invalid target fail when they are sent. Prefer
    /// [`try_new`](Self::try_new), which reports configuration errors immediately.
    pub fn new(config: AlertingConfig) -> Self {
        if let Err(e) = config.validate() {
            tracing::error!("Invalid alerting configuration: {e}");
        }
        Self {
            config,
            last_alerts: Arc::new(RwLock::new(HashMap::new())),
            failed_alerts: Arc::new(RwLock::new(HashMap::new())),
            #[cfg(feature = "alerting")]
            http_client: alert_http_client(),
        }
    }

    /// Create a new alert manager, failing if the configuration is invalid
    /// (see [`AlertingConfig::validate`]).
    pub fn try_new(config: AlertingConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self::new(config))
    }

    /// Check statistics against thresholds and trigger alerts if needed
    pub async fn check_thresholds(&self, queue_name: &str, stats: &JobStatistics) -> Result<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Check error rate threshold
        if let Some(threshold) = self.config.error_rate_threshold
            && stats.error_rate > threshold
        {
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

        if let Some(threshold) = self.config.queue_depth_threshold
            && depth > threshold
        {
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
            let threshold_duration =
                chrono::Duration::from_std(threshold).unwrap_or(chrono::Duration::MAX);

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
        // An alert that reached no target is retried after a shorter delay.
        {
            let failed_alerts = self.failed_alerts.read().await;
            if let Some(failed_at) = failed_alerts.get(&alert_key) {
                let retry_delay = chrono::Duration::from_std(
                    FAILED_ALERT_RETRY_DELAY.min(self.config.cooldown_period),
                )
                .unwrap_or(chrono::Duration::MAX);
                if Utc::now() - *failed_at < retry_delay {
                    return Ok(());
                }
            }
        }

        // Send alert to all targets
        let mut failures = Vec::new();
        for target in &self.config.targets {
            if let Err(e) = self.send_to_target(&alert, target).await {
                tracing::warn!("Failed to send alert to target: {}", e);
                failures.push(e.to_string());
            }
        }

        // Start the cooldown only once some target was reached. An alert that could not
        // be delivered anywhere is tried again after FAILED_ALERT_RETRY_DELAY (or the
        // cooldown, if shorter), not on every check.
        if failures.len() < self.config.targets.len() || self.config.targets.is_empty() {
            self.failed_alerts.write().await.remove(&alert_key);
            let mut last_alerts = self.last_alerts.write().await;
            last_alerts.insert(alert_key, alert.timestamp);
        } else {
            self.failed_alerts
                .write()
                .await
                .insert(alert_key, Utc::now());
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(HammerworkError::Alerting {
                message: format!(
                    "{} of {} alert target(s) failed: {}",
                    failures.len(),
                    self.config.targets.len(),
                    failures.join("; ")
                ),
            })
        }
    }

    /// Send alert to a specific target
    async fn send_to_target(&self, alert: &Alert, target: &AlertTarget) -> Result<()> {
        match target {
            AlertTarget::Webhook { url, headers } => {
                self.send_webhook_alert(alert, url, headers).await
            }
            AlertTarget::Email { recipient, smtp } => match smtp {
                Some(smtp) => self.send_email_alert(alert, recipient, smtp).await,
                None => Err(HammerworkError::Alerting {
                    message: format!("email alert target '{recipient}' has no SMTP settings"),
                }),
            },
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

        let response = request
            .send()
            .await
            .map_err(|e| HammerworkError::Alerting {
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

    /// Send an email alert through the target's SMTP server.
    async fn send_email_alert(
        &self,
        alert: &Alert,
        recipient: &str,
        smtp: &SmtpConfig,
    ) -> Result<()> {
        use lettre::{
            AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
            message::header::ContentType, transport::smtp::authentication::Credentials,
        };

        let failed = |message: String| HammerworkError::Alerting {
            message: format!("Failed to send email alert to {recipient}: {message}"),
        };

        let from = parse_mailbox(&smtp.from).map_err(failed)?;
        let to = parse_mailbox(recipient).map_err(failed)?;
        let message = Message::builder()
            .from(from)
            .to(to)
            .subject(single_line(&format!(
                "[Hammerwork {}] {} on queue {}",
                alert.severity, alert.alert_type, alert.queue_name
            )))
            .header(ContentType::TEXT_PLAIN)
            .body(Self::email_body(alert))
            .map_err(|e| failed(e.to_string()))?;

        let mut transport = match smtp.tls {
            SmtpTls::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)
                .map_err(|e| failed(e.to_string()))?,
            SmtpTls::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)
                .map_err(|e| failed(e.to_string()))?,
            SmtpTls::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&smtp.host),
        }
        .port(smtp.effective_port())
        .timeout(Some(Duration::from_secs(smtp.timeout_secs)));
        if let (Some(username), Some(password)) = (&smtp.username, &smtp.password) {
            let password = password.resolve().map_err(|e| failed(e.to_string()))?;
            transport = transport.credentials(Credentials::new(username.clone(), password));
        }

        transport
            .build()
            .send(message)
            .await
            .map_err(|e| failed(e.to_string()))?;
        Ok(())
    }

    /// Plain-text body of an alert email.
    fn email_body(alert: &Alert) -> String {
        let mut body = format!(
            "{}\n\nAlert: {}\nSeverity: {}\nQueue: {}\nCurrent value: {:.2}\nThreshold: {:.2}\nTime: {}\n",
            alert.message,
            alert.alert_type,
            alert.severity,
            alert.queue_name,
            alert.current_value,
            alert.threshold,
            alert.timestamp.to_rfc3339()
        );
        if !alert.context.is_empty() {
            let mut context: Vec<_> = alert.context.iter().collect();
            context.sort();
            body.push_str("\nContext:\n");
            for (key, value) in context {
                body.push_str(&format!("  {key}: {value}\n"));
            }
        }
        body
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
            AlertSeverity::Info => "#36a64f",     // Green
            AlertSeverity::Warning => "#ffaa00",  // Orange
            AlertSeverity::Critical => "#ff0000", // Red
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
        context.insert(
            "total_processed".to_string(),
            stats.total_processed.to_string(),
        );
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
            .email_via_smtp(
                "admin@example.com",
                SmtpConfig::new("smtp.example.com", "alerts@example.com"),
            )
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
    async fn test_worker_starvation_huge_threshold_does_not_panic() {
        let config = AlertingConfig::new().alert_on_worker_starvation(Duration::MAX);
        let manager = AlertManager::new(config);
        // A threshold beyond chrono's range means "never starved"; it must not panic.
        manager
            .check_worker_starvation("q", Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();
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
            smtp: None,
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
            AlertTarget::Email { recipient, .. } => assert_eq!(recipient, "test@example.com"),
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

    /// What a fake SMTP server saw during one session.
    #[derive(Debug, Default, Clone)]
    struct SmtpSession {
        commands: Vec<String>,
        data: String,
    }

    /// How the fake SMTP server behaves.
    #[derive(Clone, Copy)]
    struct FakeSmtp {
        /// Advertise `AUTH PLAIN LOGIN`.
        auth: bool,
        /// Reply to `RCPT TO` with this code.
        rcpt_reply: u16,
    }

    impl Default for FakeSmtp {
        fn default() -> Self {
            Self {
                auth: false,
                rcpt_reply: 250,
            }
        }
    }

    /// A local SMTP server speaking just enough of the protocol for lettre. Every
    /// finished session is sent on the returned channel.
    async fn fake_smtp_server(
        behaviour: FakeSmtp,
    ) -> (u16, tokio::sync::mpsc::UnboundedReceiver<SmtpSession>) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    let mut session = SmtpSession::default();
                    let _ = write.write_all(b"220 fake.test ESMTP\r\n").await;
                    while let Ok(Some(line)) = lines.next_line().await {
                        let upper = line.to_ascii_uppercase();
                        session.commands.push(line.clone());
                        let reply: String = if upper.starts_with("EHLO") {
                            if behaviour.auth {
                                "250-fake.test\r\n250 AUTH PLAIN LOGIN\r\n".into()
                            } else {
                                "250 fake.test\r\n".into()
                            }
                        } else if upper.starts_with("AUTH") {
                            "235 2.7.0 Authentication successful\r\n".into()
                        } else if upper.starts_with("MAIL FROM") {
                            "250 OK\r\n".into()
                        } else if upper.starts_with("RCPT TO") {
                            format!("{} recipient\r\n", behaviour.rcpt_reply)
                        } else if upper == "DATA" {
                            let _ = write.write_all(b"354 go ahead\r\n").await;
                            while let Ok(Some(data_line)) = lines.next_line().await {
                                if data_line == "." {
                                    break;
                                }
                                session.data.push_str(&data_line);
                                session.data.push('\n');
                            }
                            "250 queued\r\n".into()
                        } else if upper == "QUIT" {
                            let _ = write.write_all(b"221 bye\r\n").await;
                            break;
                        } else {
                            "250 OK\r\n".into()
                        };
                        if write.write_all(reply.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    let _ = tx.send(session);
                });
            }
        });
        (port, rx)
    }

    fn local_smtp(port: u16) -> SmtpConfig {
        SmtpConfig::new("127.0.0.1", "Hammerwork <alerts@example.com>")
            .with_port(port)
            .with_tls(SmtpTls::None)
            .with_timeout_secs(5)
    }

    async fn next_session(
        sessions: &mut tokio::sync::mpsc::UnboundedReceiver<SmtpSession>,
    ) -> SmtpSession {
        tokio::time::timeout(Duration::from_secs(10), sessions.recv())
            .await
            .expect("no SMTP session")
            .unwrap()
    }

    #[tokio::test]
    async fn test_email_alert_is_sent_over_smtp() {
        let (port, mut sessions) = fake_smtp_server(FakeSmtp::default()).await;
        let config = AlertingConfig::new().email_via_smtp("oncall@example.com", local_smtp(port));
        let manager = AlertManager::try_new(config).unwrap();

        manager
            .send_custom_alert(
                "emails",
                "backlog",
                "Backlog is growing",
                42.0,
                30.0,
                AlertSeverity::Critical,
            )
            .await
            .unwrap();

        let session = next_session(&mut sessions).await;
        assert!(
            session
                .commands
                .iter()
                .any(|c| c.eq_ignore_ascii_case("MAIL FROM:<alerts@example.com>")),
            "{:?}",
            session.commands
        );
        assert!(
            session
                .commands
                .iter()
                .any(|c| c.eq_ignore_ascii_case("RCPT TO:<oncall@example.com>")),
            "{:?}",
            session.commands
        );
        assert!(!session.commands.iter().any(|c| c.starts_with("AUTH")));
        assert!(
            session
                .data
                .contains("Subject: [Hammerwork Critical] Custom: backlog on queue emails"),
            "{}",
            session.data
        );
        assert!(session.data.contains("Backlog is growing"));
        assert!(session.data.contains("Threshold: 30.00"));
    }

    #[tokio::test]
    async fn test_email_alert_authenticates_with_credentials() {
        let (port, mut sessions) = fake_smtp_server(FakeSmtp {
            auth: true,
            ..Default::default()
        })
        .await;
        let smtp =
            local_smtp(port).with_credentials("alerts", SecretSource::Static("s3cret".to_string()));
        let manager =
            AlertManager::try_new(AlertingConfig::new().email_via_smtp("oncall@example.com", smtp))
                .unwrap();
        manager
            .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Info)
            .await
            .unwrap();

        let session = next_session(&mut sessions).await;
        use base64::Engine as _;
        let expected = base64::engine::general_purpose::STANDARD.encode("\0alerts\0s3cret");
        assert!(
            session
                .commands
                .iter()
                .any(|c| c == &format!("AUTH PLAIN {expected}")),
            "{:?}",
            session.commands
        );
    }

    #[tokio::test]
    async fn test_email_alert_failure_is_reported() {
        // The server rejects the recipient.
        let (port, _sessions) = fake_smtp_server(FakeSmtp {
            rcpt_reply: 550,
            ..Default::default()
        })
        .await;
        let manager = AlertManager::try_new(
            AlertingConfig::new().email_via_smtp("nobody@example.com", local_smtp(port)),
        )
        .unwrap();
        let err = manager
            .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Warning)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nobody@example.com"), "{err}");

        // A failed alert does not start the cooldown, so it is tried again.
        assert!(manager.last_alerts.read().await.is_empty());

        // Nothing listens on the port.
        let unreachable = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let manager = AlertManager::try_new(
            AlertingConfig::new().email_via_smtp("oncall@example.com", local_smtp(unreachable)),
        )
        .unwrap();
        assert!(
            manager
                .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Warning)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_starttls_is_required_when_configured() {
        // The fake server does not offer STARTTLS; the alert must not go out in plain text.
        let (port, mut sessions) = fake_smtp_server(FakeSmtp::default()).await;
        let smtp = local_smtp(port).with_tls(SmtpTls::StartTls);
        let manager =
            AlertManager::try_new(AlertingConfig::new().email_via_smtp("oncall@example.com", smtp))
                .unwrap();
        assert!(
            manager
                .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Warning)
                .await
                .is_err()
        );
        let session = next_session(&mut sessions).await;
        assert!(!session.commands.iter().any(|c| c.starts_with("MAIL")));
        assert!(session.data.is_empty());
    }

    #[tokio::test]
    async fn test_missing_password_variable_is_reported() {
        let smtp = local_smtp(1).with_credentials(
            "alerts",
            SecretSource::Environment("HAMMERWORK_TEST_SMTP_PASSWORD_THAT_IS_NOT_SET".into()),
        );
        let manager =
            AlertManager::try_new(AlertingConfig::new().email_via_smtp("oncall@example.com", smtp))
                .unwrap();
        let err = manager
            .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Warning)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("HAMMERWORK_TEST_SMTP_PASSWORD_THAT_IS_NOT_SET"),
            "{err}"
        );
    }

    #[test]
    #[allow(deprecated)]
    fn test_email_target_without_smtp_is_a_configuration_error() {
        let config = AlertingConfig::new().email("oncall@example.com");
        match AlertManager::try_new(config) {
            Err(HammerworkError::Config(message)) => {
                assert!(message.contains("no SMTP settings"), "{message}");
            }
            Err(e) => panic!("unexpected error: {e}"),
            Ok(_) => panic!("an email target without SMTP settings must be rejected"),
        }

        // Also when loaded from a configuration file.
        let target: AlertTarget =
            serde_json::from_str(r#"{"Email": {"recipient": "oncall@example.com"}}"#).unwrap();
        let config = AlertingConfig {
            targets: vec![target],
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_smtp_settings_are_validated() {
        let cases = [
            (SmtpConfig::new("", "alerts@example.com"), "host is empty"),
            (
                SmtpConfig::new("smtp.example.com", "not an address"),
                "invalid from",
            ),
            (
                SmtpConfig {
                    username: Some("alerts".into()),
                    ..SmtpConfig::new("smtp.example.com", "alerts@example.com")
                },
                "without a password",
            ),
            (
                SmtpConfig {
                    password: Some(SecretSource::Environment("X".into())),
                    ..SmtpConfig::new("smtp.example.com", "alerts@example.com")
                },
                "without a username",
            ),
        ];
        for (smtp, expected) in cases {
            let config = AlertingConfig::new().email_via_smtp("oncall@example.com", smtp);
            let message = config.validate().unwrap_err().to_string();
            assert!(message.contains(expected), "{message}");
        }

        let bad_recipient = AlertingConfig::new().email_via_smtp(
            "not an address",
            SmtpConfig::new("smtp.example.com", "alerts@example.com"),
        );
        assert!(bad_recipient.validate().is_err());
    }

    #[test]
    fn test_alert_target_debug_hides_urls_and_header_values() {
        let targets = [
            AlertTarget::Webhook {
                url: "https://alerts.example.com/hook/hunter2-path".to_string(),
                headers: HashMap::from([(
                    "Authorization".to_string(),
                    "Bearer hunter2-token".to_string(),
                )]),
            },
            AlertTarget::Slack {
                webhook_url: "https://hooks.slack.com/services/T0/B0/hunter2-path".to_string(),
                channel: "#ops".to_string(),
            },
        ];
        for target in targets {
            let debug = format!("{target:?}");
            assert!(!debug.contains("hunter2"), "{debug}");
            assert!(debug.contains("/***"), "{debug}");
        }
        let debug = format!(
            "{:?}",
            AlertTarget::Email {
                recipient: "ops@example.com".to_string(),
                smtp: None,
            }
        );
        assert!(debug.contains("ops@example.com"), "{debug}");
    }

    #[test]
    fn test_smtp_defaults_and_secret_redaction() {
        let smtp = SmtpConfig::new("smtp.example.com", "alerts@example.com");
        assert_eq!(smtp.tls, SmtpTls::StartTls);
        assert_eq!(smtp.effective_port(), 587);
        assert_eq!(
            smtp.clone().with_tls(SmtpTls::Implicit).effective_port(),
            465
        );
        assert_eq!(smtp.clone().with_tls(SmtpTls::None).effective_port(), 25);
        assert_eq!(smtp.with_port(2525).effective_port(), 2525);

        let secret = SecretSource::Static("hunter2".to_string());
        assert!(!format!("{secret:?}").contains("hunter2"));

        // Deserializes from TOML with defaults filled in.
        let smtp: SmtpConfig = toml::from_str(
            r#"
            host = "smtp.example.com"
            from = "alerts@example.com"
            username = "alerts"
            password = { Environment = "SMTP_PASSWORD" }
            "#,
        )
        .unwrap();
        assert_eq!(smtp.tls, SmtpTls::StartTls);
        assert_eq!(smtp.timeout_secs, 30);
        assert_eq!(
            smtp.password,
            Some(SecretSource::Environment("SMTP_PASSWORD".to_string()))
        );

        // The target layout documented in docs/monitoring.md.
        #[derive(Deserialize)]
        struct Targets {
            targets: Vec<AlertTarget>,
        }
        let parsed: Targets = toml::from_str(
            r#"
            [[targets]]
            [targets.Email]
            recipient = "admin@example.com"

            [targets.Email.smtp]
            host = "smtp.example.com"
            from = "alerts@example.com"
            tls = "implicit"
            "#,
        )
        .unwrap();
        match &parsed.targets[..] {
            [AlertTarget::Email { recipient, smtp }] => {
                assert_eq!(recipient, "admin@example.com");
                assert_eq!(smtp.as_ref().unwrap().tls, SmtpTls::Implicit);
            }
            other => panic!("unexpected targets: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_failed_webhook_target_is_reported() {
        let unreachable = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let manager = AlertManager::try_new(
            AlertingConfig::new().webhook(&format!("http://127.0.0.1:{unreachable}/alert")),
        )
        .unwrap();
        let err = manager
            .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Warning)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("1 of 1 alert target(s) failed"),
            "{err}"
        );
    }

    /// An HTTP request received by [`fake_http_server`].
    #[derive(Debug)]
    struct HttpRequest {
        headers: String,
        body: serde_json::Value,
    }

    /// A local HTTP server that answers every request with `status` and records it.
    async fn fake_http_server(
        status: u16,
    ) -> (String, tokio::sync::mpsc::UnboundedReceiver<HttpRequest>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (headers, body) = loop {
                        if let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&buffer[..end]).to_lowercase();
                            let length: usize = headers
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse().ok())
                                .unwrap_or(0);
                            if buffer.len() >= end + 4 + length {
                                break (headers, buffer[end + 4..end + 4 + length].to_vec());
                            }
                        }
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = tx.send(HttpRequest {
                        headers,
                        body: serde_json::from_slice(&body).unwrap_or_default(),
                    });
                });
            }
        });
        (url, rx)
    }

    async fn next_request(
        requests: &mut tokio::sync::mpsc::UnboundedReceiver<HttpRequest>,
    ) -> HttpRequest {
        tokio::time::timeout(Duration::from_secs(10), requests.recv())
            .await
            .expect("no HTTP request")
            .unwrap()
    }

    fn stats(error_rate: f64, avg_processing_time_ms: f64) -> JobStatistics {
        JobStatistics {
            total_processed: 10,
            completed: 5,
            failed: 5,
            error_rate,
            avg_processing_time_ms,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_webhook_alert_sends_headers_and_alert_fields() {
        let (url, mut requests) = fake_http_server(200).await;
        let headers = HashMap::from([("X-Team".to_string(), "queues".to_string())]);
        let manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_high_error_rate(0.1)
                .webhook_with_headers(&url, headers),
        );
        manager
            .check_thresholds("emails", &stats(0.5, 0.0))
            .await
            .unwrap();
        let request = next_request(&mut requests).await;
        assert!(
            request.headers.contains("x-team: queues"),
            "{}",
            request.headers
        );
        assert_eq!(request.body["alert_type"], "HighErrorRate");
        assert_eq!(
            request.body["severity"], "Critical",
            "more than twice the threshold"
        );
        assert_eq!(request.body["queue_name"], "emails");
        assert_eq!(request.body["context"]["failed"], "5");
    }

    #[tokio::test]
    async fn test_error_status_is_an_error_and_does_not_start_cooldown() {
        let (url, mut requests) = fake_http_server(500).await;
        let manager =
            AlertManager::new(AlertingConfig::new().alert_on_queue_depth(1).webhook(&url));
        let err = manager.check_queue_depth("q", 5).await.unwrap_err();
        assert!(err.to_string().contains("500"), "{err}");
        next_request(&mut requests).await;
        assert!(manager.last_alerts.read().await.is_empty());
        // Not delivered anywhere: the next check within the retry delay does not hit the
        // failing endpoint again.
        manager.check_queue_depth("q", 5).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(200), requests.recv())
                .await
                .is_err(),
            "a failed alert was retried on the next check"
        );

        // With a cooldown shorter than the retry delay, the cooldown applies.
        let manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_queue_depth(1)
                .webhook(&url)
                .with_cooldown(Duration::ZERO),
        );
        assert!(manager.check_queue_depth("q", 5).await.is_err());
        next_request(&mut requests).await;
        assert!(manager.check_queue_depth("q", 5).await.is_err());
        next_request(&mut requests).await;
    }

    #[tokio::test]
    async fn test_hanging_webhook_times_out() {
        // Accepts connections and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/alert", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let mut open = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                open.push(socket);
            }
        });
        let mut manager =
            AlertManager::new(AlertingConfig::new().alert_on_queue_depth(1).webhook(&url));
        // The same client as ALERT_HTTP_TIMEOUT, with a timeout short enough for a test.
        manager.http_client = alert_http_client_with_timeout(Duration::from_millis(200));
        let result =
            tokio::time::timeout(Duration::from_secs(5), manager.check_queue_depth("q", 5))
                .await
                .expect("the alert request did not time out");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unreachable_webhook_is_an_error() {
        let manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_queue_depth(1)
                .webhook("http://127.0.0.1:1/hook"),
        );
        let err = manager.check_queue_depth("q", 5).await.unwrap_err();
        assert!(
            err.to_string().contains("Failed to send webhook alert"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_slack_alert_payload() {
        let (url, mut requests) = fake_http_server(200).await;
        let manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_slow_processing(Duration::from_millis(100))
                .slack(&url, "#alerts"),
        );
        // 150ms average: over the threshold, but not twice it.
        manager
            .check_thresholds("reports", &stats(0.0, 150.0))
            .await
            .unwrap();
        let request = next_request(&mut requests).await;
        assert_eq!(request.body["channel"], "#alerts");
        let attachment = &request.body["attachments"][0];
        assert_eq!(attachment["color"], "#ffaa00", "warning");
        assert_eq!(attachment["title"], "Hammerwork Alert: SlowProcessing");
        assert_eq!(attachment["fields"][0]["value"], "reports");
        assert_eq!(attachment["fields"][1]["value"], "150.00");
        assert_eq!(attachment["fields"][2]["value"], "100.00");

        // Slack error statuses and unreachable endpoints are errors.
        let (url, _requests) = fake_http_server(404).await;
        let manager = AlertManager::new(AlertingConfig::new().slack(&url, "#a"));
        let err = manager
            .send_custom_alert("q", "disk", "full", 1.0, 0.5, AlertSeverity::Info)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Slack alert failed with status"),
            "{err}"
        );
        let manager = AlertManager::new(AlertingConfig::new().slack("http://127.0.0.1:1", "#a"));
        let err = manager
            .send_custom_alert("q", "disk", "full", 1.0, 0.5, AlertSeverity::Critical)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("Failed to send Slack alert"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn test_threshold_severities_and_cooldown() {
        let (url, mut requests) = fake_http_server(200).await;
        let manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_high_error_rate(0.4)
                .alert_on_slow_processing(Duration::from_millis(100))
                .alert_on_queue_depth(10)
                .webhook(&url),
        );
        // Error rate 0.5: over 0.4 but not 0.8 -> warning. Processing 250ms: over
        // twice 100ms -> critical.
        manager
            .check_thresholds("q", &stats(0.5, 250.0))
            .await
            .unwrap();
        let mut seen = HashMap::new();
        for _ in 0..2 {
            let request = next_request(&mut requests).await;
            seen.insert(
                request.body["alert_type"].as_str().unwrap().to_string(),
                request.body["severity"].as_str().unwrap().to_string(),
            );
        }
        assert_eq!(seen["HighErrorRate"], "Warning");
        assert_eq!(seen["SlowProcessing"], "Critical");

        manager.check_queue_depth("q", 25).await.unwrap();
        let request = next_request(&mut requests).await;
        assert_eq!(request.body["severity"], "Critical");
        // Under the threshold: nothing. Within the cooldown: nothing.
        manager.check_queue_depth("q", 5).await.unwrap();
        manager.check_queue_depth("q", 25).await.unwrap();
        manager
            .check_thresholds("q", &stats(0.5, 250.0))
            .await
            .unwrap();
        // Another queue has its own cooldown.
        manager.check_queue_depth("other", 15).await.unwrap();
        let request = next_request(&mut requests).await;
        assert_eq!(request.body["queue_name"], "other");
        assert_eq!(request.body["severity"], "Warning");
        assert!(requests.try_recv().is_err(), "nothing else was sent");
    }

    #[tokio::test]
    async fn test_worker_starvation_alert() {
        let (url, mut requests) = fake_http_server(200).await;
        let manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_worker_starvation(Duration::from_secs(60))
                .webhook(&url),
        );
        manager
            .check_worker_starvation("q", Utc::now() - chrono::Duration::seconds(10))
            .await
            .unwrap();
        manager
            .check_worker_starvation("q", Utc::now() - chrono::Duration::minutes(5))
            .await
            .unwrap();
        let request = next_request(&mut requests).await;
        assert_eq!(request.body["alert_type"], "WorkerStarvation");
        assert_eq!(request.body["current_value"], 5.0);
        assert_eq!(request.body["threshold"], 1.0);
        assert!(requests.try_recv().is_err(), "10 seconds is not starvation");
    }

    #[tokio::test]
    async fn test_disabled_alerting_sends_nothing() {
        let (url, mut requests) = fake_http_server(200).await;
        let mut manager = AlertManager::new(
            AlertingConfig::new()
                .alert_on_high_error_rate(0.1)
                .alert_on_queue_depth(1)
                .alert_on_worker_starvation(Duration::from_secs(1))
                .webhook(&url)
                .enabled(false),
        );
        manager
            .check_thresholds("q", &stats(1.0, 0.0))
            .await
            .unwrap();
        manager.check_queue_depth("q", 100).await.unwrap();
        manager
            .check_worker_starvation("q", Utc::now() - chrono::Duration::hours(1))
            .await
            .unwrap();
        manager
            .send_custom_alert("q", "x", "m", 1.0, 0.0, AlertSeverity::Info)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(requests.try_recv().is_err());

        // Re-enabled through update_config.
        let config = manager.config().clone().enabled(true);
        manager.update_config(config);
        assert!(manager.config().enabled);
        manager.check_queue_depth("q", 100).await.unwrap();
        assert_eq!(
            next_request(&mut requests).await.body["alert_type"],
            "QueueDepthExceeded"
        );
    }

    #[test]
    fn test_alert_type_display() {
        assert_eq!(AlertType::HighErrorRate.to_string(), "High Error Rate");
        assert_eq!(
            AlertType::QueueDepthExceeded.to_string(),
            "Queue Depth Exceeded"
        );
        assert_eq!(AlertType::WorkerStarvation.to_string(), "Worker Starvation");
        assert_eq!(AlertType::SlowProcessing.to_string(), "Slow Processing");
        assert_eq!(AlertType::Custom("disk".into()).to_string(), "Custom: disk");
    }
}
