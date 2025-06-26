# Monitoring and Alerting

Hammerwork provides comprehensive monitoring capabilities through Prometheus metrics and an advanced alerting system.

## Prometheus Metrics (enabled by default)

### Setting up Metrics

```rust
use hammerwork::{Worker, MetricsConfig, PrometheusMetricsCollector};
use std::{net::SocketAddr, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure metrics
    let metrics_config = MetricsConfig::new()
        .with_prometheus_exporter("127.0.0.1:9090".parse::<SocketAddr>().unwrap())
        .with_custom_gauges(vec!["active_connections", "memory_usage"])
        .with_update_interval(Duration::from_secs(15));

    // Create metrics collector
    let mut metrics_collector = Arc::new(PrometheusMetricsCollector::new(metrics_config)?);
    
    // Start HTTP server for Prometheus scraping
    metrics_collector.start_exposition_server().await?;

    // Configure worker with metrics
    let worker = Worker::new(queue, "default".to_string(), handler)
        .with_metrics_collector(metrics_collector);

    Ok(())
}
```

### Available Metrics

- `hammerwork_jobs_total` - Total jobs processed by status and priority
- `hammerwork_job_duration_seconds` - Job processing duration histogram
- `hammerwork_jobs_failed_total` - Failed jobs by error type and priority
- `hammerwork_queue_depth` - Current pending jobs in queue
- `hammerwork_worker_utilization` - Worker utilization percentage

## Alerting System (enabled by default)

### Basic Alerting Setup

```rust
use hammerwork::{Worker, AlertingConfig, AlertSeverity};
use std::time::Duration;

let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.1)  // Alert if error rate > 10%
    .alert_on_queue_depth(1000)     // Alert if queue has > 1000 jobs
    .alert_on_worker_starvation(Duration::from_minutes(5))
    .webhook("https://your-webhook.com/alerts")
    .slack("https://hooks.slack.com/your-webhook", "#alerts")
    .email("admin@yourcompany.com")
    .with_cooldown(Duration::from_minutes(5));

let worker = Worker::new(queue, "default".to_string(), handler)
    .with_alerting_config(alerting_config);
```

### Alert Types

- **High Error Rate**: When job failure rate exceeds threshold
- **Queue Depth Exceeded**: When pending jobs exceed threshold
- **Worker Starvation**: When no jobs processed for specified time
- **Slow Processing**: When average processing time exceeds threshold
- **Custom Alerts**: User-defined alerts with custom thresholds

### Notification Targets

#### Webhook Alerts
```rust
let config = AlertingConfig::new()
    .webhook("https://your-webhook.com/alerts")
    .webhook_with_headers("https://api.example.com/alerts", headers);
```

#### Slack Integration
```rust
let config = AlertingConfig::new()
    .slack("https://hooks.slack.com/your-webhook", "#alerts");
```

#### Email Alerts
```rust
let config = AlertingConfig::new()
    .email("admin@yourcompany.com");
```

## Background Monitoring

Workers automatically start a background monitoring task that:

- Updates queue depth metrics every 30 seconds
- Checks for worker starvation
- Monitors statistical thresholds
- Triggers alerts when thresholds are exceeded

## Custom Metrics

```rust
let metrics_config = MetricsConfig::new()
    .with_custom_gauges(vec!["custom_metric_1", "custom_metric_2"])
    .with_histograms(vec!["custom_histogram_1"]);

// Update custom metrics
metrics_collector.update_custom_gauge("custom_metric_1", "queue_name", 42.0).await?;
metrics_collector.observe_custom_histogram("custom_histogram_1", "queue_name", 1.5).await?;
```

## Disabling Monitoring

If you want to disable monitoring features:

```toml
# Disable all monitoring
hammerwork = { version = "0.6", features = ["postgres"], default-features = false }

# Enable only metrics
hammerwork = { version = "0.6", features = ["postgres", "metrics"], default-features = false }

# Enable only alerting
hammerwork = { version = "0.6", features = ["postgres", "alerting"], default-features = false }
```