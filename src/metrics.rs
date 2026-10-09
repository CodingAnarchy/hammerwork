use crate::{Result, error::HammerworkError, stats::JobEvent};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;

// Serde helper functions
fn serialize_socket_addr<S>(
    addr: &Option<SocketAddr>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match addr {
        Some(a) => serializer.serialize_some(&a.to_string()),
        None => serializer.serialize_none(),
    }
}

fn deserialize_socket_addr<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<SocketAddr>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let addr_str: Option<String> = Option::deserialize(deserializer)?;
    match addr_str {
        Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),
        None => Ok(None),
    }
}

fn serialize_duration_secs<S>(
    duration: &Duration,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_u64(duration.as_secs())
}

fn deserialize_duration_secs<'de, D>(deserializer: D) -> std::result::Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let secs: u64 = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}

#[cfg(feature = "metrics")]
use prometheus::{CounterVec, Encoder, GaugeVec, HistogramVec, Registry, TextEncoder};

#[cfg(feature = "metrics")]
use warp::Filter;

/// Configuration for metrics collection
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsConfig {
    /// Prometheus registry name
    pub registry_name: String,
    /// HTTP server address for metrics exposition (as string)
    #[serde(
        serialize_with = "serialize_socket_addr",
        deserialize_with = "deserialize_socket_addr",
        skip_serializing_if = "Option::is_none",
        default
    )]
    pub exposition_addr: Option<SocketAddr>,
    /// Custom metric labels to include
    pub custom_labels: HashMap<String, String>,
    /// Whether to record job durations in `hammerwork_job_duration_seconds`
    pub collect_histograms: bool,
    /// Custom gauge metric names to track
    pub custom_gauges: Vec<String>,
    /// Custom histogram metric names to track
    pub custom_histograms: Vec<String>,
    /// Update interval for gauge metrics (in seconds)
    #[serde(
        serialize_with = "serialize_duration_secs",
        deserialize_with = "deserialize_duration_secs"
    )]
    pub update_interval: Duration,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            registry_name: "hammerwork".to_string(),
            exposition_addr: None,
            custom_labels: HashMap::new(),
            collect_histograms: true,
            custom_gauges: Vec::new(),
            custom_histograms: Vec::new(),
            update_interval: Duration::from_secs(15),
        }
    }
}

impl MetricsConfig {
    /// Create a new metrics configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the Prometheus exposition address
    pub fn with_prometheus_exporter(mut self, addr: SocketAddr) -> Self {
        self.exposition_addr = Some(addr);
        self
    }

    /// Add custom gauge metrics
    pub fn with_custom_gauges(mut self, gauges: Vec<&str>) -> Self {
        self.custom_gauges = gauges.into_iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add custom histogram metrics
    pub fn with_histograms(mut self, histograms: Vec<&str>) -> Self {
        self.custom_histograms = histograms.into_iter().map(|s| s.to_string()).collect();
        self
    }

    /// Add custom labels to all metrics
    pub fn with_labels(mut self, labels: HashMap<String, String>) -> Self {
        self.custom_labels = labels;
        self
    }

    /// Set update interval for metrics
    pub fn with_update_interval(mut self, interval: Duration) -> Self {
        self.update_interval = interval;
        self
    }
}

/// Prometheus metrics collector for job queue metrics
#[cfg(feature = "metrics")]
pub struct PrometheusMetricsCollector {
    config: MetricsConfig,
    registry: Registry,
    // Core job metrics
    jobs_total: CounterVec,
    jobs_duration: HistogramVec,
    jobs_failed_total: CounterVec,
    queue_depth: GaugeVec,
    worker_utilization: GaugeVec,
    // Custom metrics
    custom_gauges: Arc<RwLock<HashMap<String, GaugeVec>>>,
    custom_histograms: Arc<RwLock<HashMap<String, HistogramVec>>>,
    // HTTP server handle
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(feature = "metrics")]
impl PrometheusMetricsCollector {
    /// Create a new Prometheus metrics collector
    ///
    /// `custom_labels` are added to every metric as constant labels. Fails with
    /// [`HammerworkError::Metrics`] when a label or custom metric name is not a valid
    /// Prometheus name, or a custom metric name is used twice.
    pub fn new(config: MetricsConfig) -> Result<Self> {
        if let Some(name) = config.custom_labels.keys().find(|name| {
            let mut chars = name.chars();
            !chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
                || name.starts_with("__")
        }) {
            return Err(HammerworkError::Metrics {
                message: format!("Invalid custom metric label name: {name:?}"),
            });
        }
        let registry = if config.custom_labels.is_empty() {
            Registry::new()
        } else {
            Registry::new_custom(None, Some(config.custom_labels.clone())).map_err(|e| {
                HammerworkError::Metrics {
                    message: format!("Invalid custom metric labels: {}", e),
                }
            })?
        };

        // Register core job metrics
        let jobs_total = prometheus::CounterVec::new(
            prometheus::Opts::new("hammerwork_jobs_total", "Total number of jobs processed"),
            &["queue", "status", "priority"],
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create jobs_total metric: {}", e),
        })?;

        let jobs_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new(
                "hammerwork_job_duration_seconds",
                "Job processing duration in seconds",
            )
            .buckets(vec![
                0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
            ]),
            &["queue", "priority"],
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create jobs_duration metric: {}", e),
        })?;

        let jobs_failed_total = prometheus::CounterVec::new(
            prometheus::Opts::new(
                "hammerwork_jobs_failed_total",
                "Total number of failed jobs",
            ),
            &["queue", "error_type", "priority"],
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create jobs_failed_total metric: {}", e),
        })?;

        let queue_depth = prometheus::GaugeVec::new(
            prometheus::Opts::new(
                "hammerwork_queue_depth",
                "Current number of pending jobs in queue",
            ),
            &["queue"],
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create queue_depth metric: {}", e),
        })?;

        let worker_utilization = prometheus::GaugeVec::new(
            prometheus::Opts::new(
                "hammerwork_worker_utilization",
                "Worker utilization percentage",
            ),
            &["queue", "worker_id"],
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create worker_utilization metric: {}", e),
        })?;

        // Register with custom registry
        registry
            .register(Box::new(jobs_total.clone()))
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to register jobs_total with registry: {}", e),
            })?;

        registry
            .register(Box::new(jobs_duration.clone()))
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to register jobs_duration with registry: {}", e),
            })?;

        registry
            .register(Box::new(jobs_failed_total.clone()))
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to register jobs_failed_total with registry: {}", e),
            })?;

        registry
            .register(Box::new(queue_depth.clone()))
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to register queue_depth with registry: {}", e),
            })?;

        registry
            .register(Box::new(worker_utilization.clone()))
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to register worker_utilization with registry: {}", e),
            })?;

        let mut collector = Self {
            config,
            registry,
            jobs_total,
            jobs_duration,
            jobs_failed_total,
            queue_depth,
            worker_utilization,
            custom_gauges: Arc::new(RwLock::new(HashMap::new())),
            custom_histograms: Arc::new(RwLock::new(HashMap::new())),
            server_handle: None,
        };

        // Register custom metrics
        collector.register_custom_metrics()?;

        Ok(collector)
    }

    /// Start the Prometheus HTTP exposition server.
    ///
    /// The listening socket is bound before this method returns, so a failure to
    /// bind the configured address (port in use, privileged port, bad interface)
    /// is reported as `Err(HammerworkError::Metrics)` instead of the server task
    /// dying silently. Does nothing when no exposition address is configured.
    pub async fn start_exposition_server(&mut self) -> Result<()> {
        if let Some(addr) = self.config.exposition_addr {
            let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
                HammerworkError::Metrics {
                    message: format!("Failed to bind metrics exposition server to {addr}: {e}"),
                }
            })?;

            let registry = self.registry.clone();
            let app = warp::path("metrics").map(move || {
                let encoder = TextEncoder::new();
                let metric_families = registry.gather();
                let mut buffer = Vec::new();
                let body = match encoder
                    .encode(&metric_families, &mut buffer)
                    .map_err(|e| e.to_string())
                    .and_then(|()| String::from_utf8(buffer).map_err(|e| e.to_string()))
                {
                    Ok(body) => body,
                    Err(e) => {
                        tracing::error!("Failed to encode metrics: {}", e);
                        let mut reply = warp::reply::Response::new(
                            format!("failed to encode metrics: {e}").into(),
                        );
                        *reply.status_mut() = warp::http::StatusCode::INTERNAL_SERVER_ERROR;
                        return reply;
                    }
                };
                let mut reply = warp::reply::Response::new(body.into());
                reply.headers_mut().insert(
                    warp::http::header::CONTENT_TYPE,
                    warp::http::HeaderValue::from_static("text/plain"),
                );
                reply
            });

            // Spawn the server future directly (not wrapped in an `async` block) to
            // avoid a rustc higher-ranked lifetime inference issue with warp 0.4.
            let handle = tokio::spawn(warp::serve(app).incoming(listener).run());

            if let Some(previous) = self.server_handle.replace(handle) {
                previous.abort();
            }
        }

        Ok(())
    }

    /// Record a job event as metrics
    pub async fn record_job_event(&self, event: &JobEvent) -> Result<()> {
        let queue = &event.queue_name;
        let priority = event.priority.to_string();

        match event.event_type {
            crate::stats::JobEventType::Completed => {
                self.jobs_total
                    .with_label_values(&[queue.as_str(), "completed", priority.as_str()])
                    .inc();

                if let Some(duration_ms) = event.processing_time_ms
                    && self.config.collect_histograms
                {
                    let duration_secs = duration_ms as f64 / 1000.0;
                    self.jobs_duration
                        .with_label_values(&[queue.as_str(), priority.as_str()])
                        .observe(duration_secs);
                }
            }
            crate::stats::JobEventType::Failed => {
                self.jobs_total
                    .with_label_values(&[queue.as_str(), "failed", priority.as_str()])
                    .inc();

                let error_type = event
                    .error_message
                    .as_ref()
                    .map(|msg| {
                        // Extract error type from message
                        if msg.contains("timeout") {
                            "timeout"
                        } else if msg.contains("connection") {
                            "connection"
                        } else {
                            "other"
                        }
                    })
                    .unwrap_or("unknown");

                self.jobs_failed_total
                    .with_label_values(&[queue.as_str(), error_type, priority.as_str()])
                    .inc();
            }
            crate::stats::JobEventType::TimedOut => {
                self.jobs_total
                    .with_label_values(&[queue.as_str(), "timed_out", priority.as_str()])
                    .inc();

                self.jobs_failed_total
                    .with_label_values(&[queue.as_str(), "timeout", priority.as_str()])
                    .inc();
            }
            crate::stats::JobEventType::Dead => {
                self.jobs_total
                    .with_label_values(&[queue.as_str(), "dead", priority.as_str()])
                    .inc();

                self.jobs_failed_total
                    .with_label_values(&[queue.as_str(), "exhausted", priority.as_str()])
                    .inc();
            }
            crate::stats::JobEventType::Retried => {
                self.jobs_total
                    .with_label_values(&[queue.as_str(), "retried", priority.as_str()])
                    .inc();
            }
            crate::stats::JobEventType::Started => {
                self.jobs_total
                    .with_label_values(&[queue.as_str(), "started", priority.as_str()])
                    .inc();
            }
        }

        Ok(())
    }

    /// Update queue depth metric
    pub async fn update_queue_depth(&self, queue_name: &str, depth: u64) -> Result<()> {
        self.queue_depth
            .with_label_values(&[queue_name])
            .set(depth as f64);
        Ok(())
    }

    /// Update worker utilization metric
    pub async fn update_worker_utilization(
        &self,
        queue_name: &str,
        worker_id: &str,
        utilization: f64,
    ) -> Result<()> {
        self.worker_utilization
            .with_label_values(&[queue_name, worker_id])
            .set(utilization);
        Ok(())
    }

    /// Get metrics as Prometheus text format
    pub fn get_metrics_text(&self) -> Result<String> {
        let encoder = TextEncoder::new();
        let metric_families = self.registry.gather();
        let mut buffer = Vec::new();
        encoder
            .encode(&metric_families, &mut buffer)
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to encode metrics: {}", e),
            })?;

        String::from_utf8(buffer).map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to convert metrics to string: {}", e),
        })
    }

    /// Register custom metrics based on configuration
    ///
    /// Called from `new`, before the collector is shared, so the maps are filled
    /// directly (this must not block on a runtime: `new` is synchronous).
    fn register_custom_metrics(&mut self) -> Result<()> {
        let mut gauges = HashMap::new();
        let mut histograms = HashMap::new();

        // Register custom gauges
        for gauge_name in &self.config.custom_gauges {
            let gauge = prometheus::GaugeVec::new(
                prometheus::Opts::new(
                    format!("hammerwork_{}", gauge_name),
                    format!("Custom gauge metric: {}", gauge_name),
                ),
                &["queue"],
            )
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to create custom gauge {}: {}", gauge_name, e),
            })?;

            self.registry
                .register(Box::new(gauge.clone()))
                .map_err(|e| HammerworkError::Metrics {
                    message: format!(
                        "Failed to register custom gauge {} with registry: {}",
                        gauge_name, e
                    ),
                })?;

            gauges.insert(gauge_name.clone(), gauge);
        }

        // Register custom histograms
        for histogram_name in &self.config.custom_histograms {
            let histogram = prometheus::HistogramVec::new(
                prometheus::HistogramOpts::new(
                    format!("hammerwork_{}", histogram_name),
                    format!("Custom histogram metric: {}", histogram_name),
                ),
                &["queue"],
            )
            .map_err(|e| HammerworkError::Metrics {
                message: format!(
                    "Failed to create custom histogram {}: {}",
                    histogram_name, e
                ),
            })?;

            self.registry
                .register(Box::new(histogram.clone()))
                .map_err(|e| HammerworkError::Metrics {
                    message: format!(
                        "Failed to register custom histogram {} with registry: {}",
                        histogram_name, e
                    ),
                })?;

            histograms.insert(histogram_name.clone(), histogram);
        }

        self.custom_gauges = Arc::new(RwLock::new(gauges));
        self.custom_histograms = Arc::new(RwLock::new(histograms));
        Ok(())
    }

    /// Update a custom gauge metric
    pub async fn update_custom_gauge(
        &self,
        metric_name: &str,
        queue_name: &str,
        value: f64,
    ) -> Result<()> {
        let gauges = self.custom_gauges.read().await;
        if let Some(gauge) = gauges.get(metric_name) {
            gauge.with_label_values(&[queue_name]).set(value);
        }
        Ok(())
    }

    /// Observe a custom histogram metric
    pub async fn observe_custom_histogram(
        &self,
        metric_name: &str,
        queue_name: &str,
        value: f64,
    ) -> Result<()> {
        let histograms = self.custom_histograms.read().await;
        if let Some(histogram) = histograms.get(metric_name) {
            histogram.with_label_values(&[queue_name]).observe(value);
        }
        Ok(())
    }
}

#[cfg(feature = "metrics")]
impl Drop for PrometheusMetricsCollector {
    fn drop(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

/// No-op metrics collector when metrics feature is disabled
#[cfg(not(feature = "metrics"))]
pub struct PrometheusMetricsCollector {
    _config: MetricsConfig,
}

#[cfg(not(feature = "metrics"))]
impl PrometheusMetricsCollector {
    pub fn new(config: MetricsConfig) -> Result<Self> {
        Ok(Self { _config: config })
    }

    pub async fn start_exposition_server(&mut self) -> Result<()> {
        Ok(())
    }

    pub async fn record_job_event(&self, _event: &JobEvent) -> Result<()> {
        Ok(())
    }

    pub async fn update_queue_depth(&self, _queue_name: &str, _depth: u64) -> Result<()> {
        Ok(())
    }

    pub async fn update_worker_utilization(
        &self,
        _queue_name: &str,
        _worker_id: &str,
        _utilization: f64,
    ) -> Result<()> {
        Ok(())
    }

    pub fn get_metrics_text(&self) -> Result<String> {
        Ok("# Metrics collection disabled\n".to_string())
    }

    pub async fn update_custom_gauge(
        &self,
        _metric_name: &str,
        _queue_name: &str,
        _value: f64,
    ) -> Result<()> {
        Ok(())
    }

    pub async fn observe_custom_histogram(
        &self,
        _metric_name: &str,
        _queue_name: &str,
        _value: f64,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{JobEvent, JobEventType};
    use std::time::Duration;

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_exposition_server_reports_bind_failure() {
        // Occupy a port, then ask the collector to bind the very same address.
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = blocker.local_addr().unwrap();

        let mut collector =
            PrometheusMetricsCollector::new(MetricsConfig::new().with_prometheus_exporter(addr))
                .unwrap();
        let err = collector.start_exposition_server().await.unwrap_err();
        assert!(
            matches!(err, HammerworkError::Metrics { .. }),
            "unexpected error: {err}"
        );
        assert!(err.to_string().contains(&addr.to_string()), "{err}");
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_exposition_server_serves_when_bind_succeeds() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);

        let mut collector =
            PrometheusMetricsCollector::new(MetricsConfig::new().with_prometheus_exporter(addr))
                .unwrap();
        collector.start_exposition_server().await.unwrap();
        assert!(collector.server_handle.is_some());
        assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
    }

    #[test]
    fn test_metrics_config_creation() {
        let config = MetricsConfig::new()
            .with_prometheus_exporter("127.0.0.1:9090".parse().unwrap())
            .with_custom_gauges(vec!["queue_depth", "worker_utilization"])
            .with_histograms(vec!["job_duration", "queue_wait_time"])
            .with_update_interval(Duration::from_secs(30));

        assert!(config.exposition_addr.is_some());
        assert_eq!(config.custom_gauges.len(), 2);
        assert_eq!(config.custom_histograms.len(), 2);
        assert_eq!(config.update_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_metrics_config_defaults() {
        let config = MetricsConfig::default();
        assert_eq!(config.registry_name, "hammerwork");
        assert!(config.exposition_addr.is_none());
        assert!(config.collect_histograms);
        assert_eq!(config.update_interval, Duration::from_secs(15));
    }

    #[test]
    fn test_metrics_config_labels() {
        let mut labels = HashMap::new();
        labels.insert("service".to_string(), "hammerwork".to_string());
        labels.insert("environment".to_string(), "production".to_string());

        let config = MetricsConfig::new().with_labels(labels.clone());
        assert_eq!(config.custom_labels, labels);
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_prometheus_collector_creation() {
        let config = MetricsConfig::new();
        let collector = PrometheusMetricsCollector::new(config);
        assert!(collector.is_ok());
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_metrics_recording() {
        let config = MetricsConfig::new();
        let collector = PrometheusMetricsCollector::new(config).unwrap();

        let event = JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            event_type: JobEventType::Completed,
            priority: crate::priority::JobPriority::Normal,
            processing_time_ms: Some(1500),
            error_message: None,
            timestamp: chrono::Utc::now(),
        };

        let result = collector.record_job_event(&event).await;
        assert!(result.is_ok());

        // Test metrics text generation
        let metrics_text = collector.get_metrics_text();
        assert!(metrics_text.is_ok());
        let text = metrics_text.unwrap();
        assert!(text.contains("hammerwork_jobs_total"));
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_queue_depth_update() {
        let config = MetricsConfig::new();
        let collector = PrometheusMetricsCollector::new(config).unwrap();

        let result = collector.update_queue_depth("test_queue", 42).await;
        assert!(result.is_ok());

        let metrics_text = collector.get_metrics_text().unwrap();
        assert!(metrics_text.contains("hammerwork_queue_depth"));
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_worker_utilization_update() {
        let config = MetricsConfig::new();
        let collector = PrometheusMetricsCollector::new(config).unwrap();

        let result = collector
            .update_worker_utilization("test_queue", "worker_1", 0.85)
            .await;
        assert!(result.is_ok());

        let metrics_text = collector.get_metrics_text().unwrap();
        assert!(metrics_text.contains("hammerwork_worker_utilization"));
    }

    #[cfg(not(feature = "metrics"))]
    #[tokio::test]
    async fn test_noop_collector() {
        let config = MetricsConfig::new();
        let collector = PrometheusMetricsCollector::new(config).unwrap();

        // All operations should succeed but do nothing
        let result = collector.update_queue_depth("test_queue", 42).await;
        assert!(result.is_ok());

        let metrics_text = collector.get_metrics_text().unwrap();
        assert!(metrics_text.contains("disabled"));
    }

    #[test]
    fn test_custom_metrics_configuration() {
        let config = MetricsConfig::new()
            .with_custom_gauges(vec!["active_connections", "memory_usage"])
            .with_histograms(vec!["request_duration", "response_size"]);

        assert_eq!(config.custom_gauges.len(), 2);
        assert!(
            config
                .custom_gauges
                .contains(&"active_connections".to_string())
        );
        assert!(config.custom_gauges.contains(&"memory_usage".to_string()));

        assert_eq!(config.custom_histograms.len(), 2);
        assert!(
            config
                .custom_histograms
                .contains(&"request_duration".to_string())
        );
        assert!(
            config
                .custom_histograms
                .contains(&"response_size".to_string())
        );
    }

    #[test]
    fn test_metrics_config_serde_roundtrip() {
        let config = MetricsConfig::new()
            .with_prometheus_exporter("127.0.0.1:9464".parse().unwrap())
            .with_update_interval(Duration::from_secs(42));
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["exposition_addr"], "127.0.0.1:9464");
        assert_eq!(json["update_interval"], 42);
        let back: MetricsConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.exposition_addr, config.exposition_addr);
        assert_eq!(back.update_interval, Duration::from_secs(42));

        // No address: omitted when serializing, `None` when missing.
        let json = serde_json::to_value(MetricsConfig::new()).unwrap();
        assert!(json.get("exposition_addr").is_none());
        let back: MetricsConfig = serde_json::from_value(json.clone()).unwrap();
        assert!(back.exposition_addr.is_none());

        let mut bad = json;
        bad["exposition_addr"] = "not an address".into();
        assert!(serde_json::from_value::<MetricsConfig>(bad).is_err());
    }

    #[cfg(feature = "metrics")]
    fn event(event_type: JobEventType, error: Option<&str>, ms: Option<u64>) -> JobEvent {
        JobEvent {
            job_id: uuid::Uuid::new_v4(),
            queue_name: "q".to_string(),
            event_type,
            priority: crate::priority::JobPriority::High,
            processing_time_ms: ms,
            error_message: error.map(str::to_string),
            timestamp: chrono::Utc::now(),
        }
    }

    /// The value of the sample line starting with `prefix` and containing `labels`.
    #[cfg(feature = "metrics")]
    fn sample(text: &str, prefix: &str, labels: &[&str]) -> Option<f64> {
        text.lines()
            .filter(|line| !line.starts_with('#'))
            .find(|line| {
                line.starts_with(prefix) && labels.iter().all(|label| line.contains(label))
            })
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|value| value.parse().ok())
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_record_job_event_counts_every_outcome() {
        let collector = PrometheusMetricsCollector::new(MetricsConfig::new()).unwrap();
        for e in [
            event(JobEventType::Started, None, None),
            event(JobEventType::Completed, None, Some(250)),
            event(JobEventType::Failed, Some("request timeout"), None),
            event(JobEventType::Failed, Some("connection reset"), None),
            event(JobEventType::Failed, Some("bad input"), None),
            event(JobEventType::Failed, None, None),
            event(JobEventType::TimedOut, Some("timed out"), None),
            event(JobEventType::Dead, Some("gave up"), None),
            event(JobEventType::Retried, Some("again"), None),
        ] {
            collector.record_job_event(&e).await.unwrap();
        }
        let text = collector.get_metrics_text().unwrap();
        let total = |status: &str| {
            sample(
                &text,
                "hammerwork_jobs_total",
                &[&format!("status=\"{status}\""), "priority=\"high\""],
            )
        };
        assert_eq!(total("started"), Some(1.0));
        assert_eq!(total("completed"), Some(1.0));
        assert_eq!(total("failed"), Some(4.0));
        assert_eq!(total("timed_out"), Some(1.0));
        assert_eq!(total("dead"), Some(1.0));
        assert_eq!(total("retried"), Some(1.0));

        let failed = |error_type: &str| {
            sample(
                &text,
                "hammerwork_jobs_failed_total",
                &[&format!("error_type=\"{error_type}\"")],
            )
        };
        // A timed-out job and a failure mentioning a timeout are both "timeout".
        assert_eq!(failed("timeout"), Some(2.0));
        assert_eq!(failed("connection"), Some(1.0));
        assert_eq!(failed("other"), Some(1.0));
        assert_eq!(failed("unknown"), Some(1.0));
        assert_eq!(failed("exhausted"), Some(1.0));

        assert_eq!(
            sample(
                &text,
                "hammerwork_job_duration_seconds_sum",
                &["queue=\"q\""]
            ),
            Some(0.25)
        );
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_collect_histograms_false_skips_durations() {
        let collector = PrometheusMetricsCollector::new(MetricsConfig {
            collect_histograms: false,
            ..MetricsConfig::new()
        })
        .unwrap();
        collector
            .record_job_event(&event(JobEventType::Completed, None, Some(250)))
            .await
            .unwrap();
        let text = collector.get_metrics_text().unwrap();
        assert_eq!(
            sample(&text, "hammerwork_jobs_total", &["status=\"completed\""]),
            Some(1.0)
        );
        assert_eq!(
            sample(&text, "hammerwork_job_duration_seconds_count", &[]),
            None
        );
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_gauges_and_custom_metrics_are_exported() {
        let collector = PrometheusMetricsCollector::new(
            MetricsConfig::new()
                .with_custom_gauges(vec!["backlog_bytes"])
                .with_histograms(vec!["payload_size"]),
        )
        .unwrap();
        collector.update_queue_depth("q", 42).await.unwrap();
        collector
            .update_worker_utilization("q", "worker-1", 0.75)
            .await
            .unwrap();
        collector
            .update_custom_gauge("backlog_bytes", "q", 1024.0)
            .await
            .unwrap();
        collector
            .observe_custom_histogram("payload_size", "q", 3.0)
            .await
            .unwrap();
        // Unknown custom metrics are ignored.
        collector
            .update_custom_gauge("missing", "q", 1.0)
            .await
            .unwrap();
        collector
            .observe_custom_histogram("missing", "q", 1.0)
            .await
            .unwrap();

        let text = collector.get_metrics_text().unwrap();
        assert_eq!(
            sample(&text, "hammerwork_queue_depth", &["queue=\"q\""]),
            Some(42.0)
        );
        assert_eq!(
            sample(
                &text,
                "hammerwork_worker_utilization",
                &["worker_id=\"worker-1\""]
            ),
            Some(0.75)
        );
        assert_eq!(sample(&text, "hammerwork_backlog_bytes", &[]), Some(1024.0));
        assert_eq!(sample(&text, "hammerwork_payload_size_sum", &[]), Some(3.0));
        assert!(!text.contains("hammerwork_missing"));
    }

    /// `new` is synchronous: registering custom metrics must not need (or block) a
    /// Tokio runtime. It used `block_in_place`, which panics outside a multi-threaded
    /// runtime.
    #[cfg(feature = "metrics")]
    #[test]
    fn test_custom_metrics_without_a_runtime() {
        let collector = PrometheusMetricsCollector::new(
            MetricsConfig::new()
                .with_custom_gauges(vec!["g"])
                .with_histograms(vec!["h"]),
        )
        .unwrap();
        assert!(collector.get_metrics_text().is_ok());
    }

    #[cfg(feature = "metrics")]
    #[tokio::test(flavor = "current_thread")]
    async fn test_custom_metrics_on_a_current_thread_runtime() {
        let collector =
            PrometheusMetricsCollector::new(MetricsConfig::new().with_custom_gauges(vec!["g"]))
                .unwrap();
        collector.update_custom_gauge("g", "q", 2.0).await.unwrap();
        let text = collector.get_metrics_text().unwrap();
        assert_eq!(sample(&text, "hammerwork_g", &["queue=\"q\""]), Some(2.0));
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_custom_labels_apply_to_every_metric() {
        let labels = HashMap::from([("service".to_string(), "billing".to_string())]);
        let collector = PrometheusMetricsCollector::new(
            MetricsConfig::new()
                .with_labels(labels)
                .with_custom_gauges(vec!["g"]),
        )
        .unwrap();
        collector.update_queue_depth("q", 1).await.unwrap();
        collector.update_custom_gauge("g", "q", 1.0).await.unwrap();
        collector
            .record_job_event(&event(JobEventType::Completed, None, None))
            .await
            .unwrap();
        let text = collector.get_metrics_text().unwrap();
        let samples: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
        assert!(samples.len() >= 3);
        for line in samples {
            assert!(line.contains("service=\"billing\""), "{line}");
        }
    }

    #[cfg(feature = "metrics")]
    #[test]
    fn test_invalid_names_are_rejected() {
        for config in [
            MetricsConfig::new().with_custom_gauges(vec!["bad-name"]),
            MetricsConfig::new().with_histograms(vec!["bad name"]),
            MetricsConfig::new().with_custom_gauges(vec!["dup", "dup"]),
            MetricsConfig::new()
                .with_labels(HashMap::from([("bad-label".to_string(), "x".to_string())])),
        ] {
            let err = PrometheusMetricsCollector::new(config.clone())
                .err()
                .unwrap_or_else(|| panic!("{config:?} should be rejected"));
            assert!(matches!(err, HammerworkError::Metrics { .. }), "{err}");
        }
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_exposition_server_serves_metrics_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let mut collector =
            PrometheusMetricsCollector::new(MetricsConfig::new().with_prometheus_exporter(addr))
                .unwrap();
        collector.start_exposition_server().await.unwrap();
        collector.update_queue_depth("served", 7).await.unwrap();

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(b"GET /metrics HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.to_lowercase().contains("content-type: text/plain"));
        assert!(response.contains("hammerwork_queue_depth{queue=\"served\"} 7"));

        // Dropping the collector stops the server.
        drop(collector);
        let stopped = async {
            loop {
                if tokio::net::TcpStream::connect(addr).await.is_err() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), stopped)
            .await
            .expect("the server stops with the collector");
    }

    #[cfg(feature = "metrics")]
    #[tokio::test]
    async fn test_exposition_server_without_address_is_a_noop() {
        let mut collector = PrometheusMetricsCollector::new(MetricsConfig::new()).unwrap();
        collector.start_exposition_server().await.unwrap();
        assert!(collector.server_handle.is_none());
    }
}
