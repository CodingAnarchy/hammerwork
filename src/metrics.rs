use crate::{error::HammerworkError, stats::JobEvent, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;

#[cfg(feature = "metrics")]
use prometheus::{
    CounterVec, Encoder, GaugeVec, HistogramVec, Registry, TextEncoder,
};

#[cfg(feature = "metrics")]
use warp::Filter;

/// Configuration for metrics collection
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Prometheus registry name
    pub registry_name: String,
    /// HTTP server address for metrics exposition
    pub exposition_addr: Option<SocketAddr>,
    /// Custom metric labels to include
    pub custom_labels: HashMap<String, String>,
    /// Whether to collect detailed timing histograms
    pub collect_histograms: bool,
    /// Custom gauge metric names to track
    pub custom_gauges: Vec<String>,
    /// Custom histogram metric names to track
    pub custom_histograms: Vec<String>,
    /// Update interval for gauge metrics
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
    pub fn new(config: MetricsConfig) -> Result<Self> {
        let registry = Registry::new();

        // Register core job metrics
        let jobs_total = prometheus::CounterVec::new(
            prometheus::Opts::new("hammerwork_jobs_total", "Total number of jobs processed"),
            &["queue", "status", "priority"]
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create jobs_total metric: {}", e),
        })?;

        let jobs_duration = prometheus::HistogramVec::new(
            prometheus::HistogramOpts::new("hammerwork_job_duration_seconds", "Job processing duration in seconds")
                .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0]),
            &["queue", "priority"]
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create jobs_duration metric: {}", e),
        })?;

        let jobs_failed_total = prometheus::CounterVec::new(
            prometheus::Opts::new("hammerwork_jobs_failed_total", "Total number of failed jobs"),
            &["queue", "error_type", "priority"]
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create jobs_failed_total metric: {}", e),
        })?;

        let queue_depth = prometheus::GaugeVec::new(
            prometheus::Opts::new("hammerwork_queue_depth", "Current number of pending jobs in queue"),
            &["queue"]
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create queue_depth metric: {}", e),
        })?;

        let worker_utilization = prometheus::GaugeVec::new(
            prometheus::Opts::new("hammerwork_worker_utilization", "Worker utilization percentage"),
            &["queue", "worker_id"]
        )
        .map_err(|e| HammerworkError::Metrics {
            message: format!("Failed to create worker_utilization metric: {}", e),
        })?;

        // Register with custom registry
        registry.register(Box::new(jobs_total.clone())).map_err(|e| {
            HammerworkError::Metrics {
                message: format!("Failed to register jobs_total with registry: {}", e),
            }
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

    /// Start the Prometheus HTTP exposition server
    pub async fn start_exposition_server(&mut self) -> Result<()> {
        if let Some(addr) = self.config.exposition_addr {
            let registry = self.registry.clone();
            let handle = tokio::spawn(async move {
                let app = warp::path("metrics")
                    .map(move || {
                        let encoder = TextEncoder::new();
                        let metric_families = registry.gather();
                        let mut buffer = Vec::new();
                        encoder.encode(&metric_families, &mut buffer).unwrap();
                        String::from_utf8(buffer).unwrap()
                    })
                    .with(warp::reply::with::header("content-type", "text/plain"));

                warp::serve(app).run(addr).await;
            });

            self.server_handle = Some(handle);
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
                    .with_label_values(&[queue, "completed", &priority])
                    .inc();

                if let Some(duration_ms) = event.processing_time_ms {
                    let duration_secs = duration_ms as f64 / 1000.0;
                    self.jobs_duration
                        .with_label_values(&[queue, &priority])
                        .observe(duration_secs);
                }
            }
            crate::stats::JobEventType::Failed => {
                self.jobs_total
                    .with_label_values(&[queue, "failed", &priority])
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
                    .with_label_values(&[queue, error_type, &priority])
                    .inc();
            }
            crate::stats::JobEventType::TimedOut => {
                self.jobs_total
                    .with_label_values(&[queue, "timed_out", &priority])
                    .inc();

                self.jobs_failed_total
                    .with_label_values(&[queue, "timeout", &priority])
                    .inc();
            }
            crate::stats::JobEventType::Dead => {
                self.jobs_total
                    .with_label_values(&[queue, "dead", &priority])
                    .inc();

                self.jobs_failed_total
                    .with_label_values(&[queue, "exhausted", &priority])
                    .inc();
            }
            crate::stats::JobEventType::Retried => {
                self.jobs_total
                    .with_label_values(&[queue, "retried", &priority])
                    .inc();
            }
            crate::stats::JobEventType::Started => {
                self.jobs_total
                    .with_label_values(&[queue, "started", &priority])
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
    fn register_custom_metrics(&mut self) -> Result<()> {
        // Register custom gauges
        for gauge_name in &self.config.custom_gauges {
            let gauge = prometheus::GaugeVec::new(
                prometheus::Opts::new(
                    format!("hammerwork_{}", gauge_name),
                    format!("Custom gauge metric: {}", gauge_name)
                ),
                &["queue"]
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

            tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    let mut gauges = self.custom_gauges.write().await;
                    gauges.insert(gauge_name.clone(), gauge);
                });
            });
        }

        // Register custom histograms
        for histogram_name in &self.config.custom_histograms {
            let histogram = prometheus::HistogramVec::new(
                prometheus::HistogramOpts::new(
                    format!("hammerwork_{}", histogram_name),
                    format!("Custom histogram metric: {}", histogram_name)
                ),
                &["queue"]
            )
            .map_err(|e| HammerworkError::Metrics {
                message: format!("Failed to create custom histogram {}: {}", histogram_name, e),
            })?;

            self.registry
                .register(Box::new(histogram.clone()))
                .map_err(|e| HammerworkError::Metrics {
                    message: format!(
                        "Failed to register custom histogram {} with registry: {}",
                        histogram_name, e
                    ),
                })?;

            tokio::task::block_in_place(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    let mut histograms = self.custom_histograms.write().await;
                    histograms.insert(histogram_name.clone(), histogram);
                });
            });
        }

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
        assert!(config.custom_gauges.contains(&"active_connections".to_string()));
        assert!(config.custom_gauges.contains(&"memory_usage".to_string()));

        assert_eq!(config.custom_histograms.len(), 2);
        assert!(config
            .custom_histograms
            .contains(&"request_duration".to_string()));
        assert!(config
            .custom_histograms
            .contains(&"response_size".to_string()));
    }
}