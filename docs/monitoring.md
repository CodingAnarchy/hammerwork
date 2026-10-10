# Monitoring and Alerting

Hammerwork provides comprehensive monitoring capabilities through Prometheus metrics and an advanced alerting system.

## Prometheus Metrics (enabled by default)

### Setting up Metrics

```rust,no_run
use hammerwork::{Worker, MetricsConfig, PrometheusMetricsCollector};
use std::{net::SocketAddr, sync::Arc, time::Duration};
# use hammerwork::{JobQueue, worker::JobHandler};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    # let pool = sqlx::PgPool::connect("postgresql://localhost/hammerwork").await?;
    # let queue = Arc::new(JobQueue::new(pool));
    # let handler: JobHandler = Arc::new(|_job| Box::pin(async { Ok(()) }));
    // Configure metrics
    let metrics_config = MetricsConfig::new()
        .with_prometheus_exporter("127.0.0.1:9090".parse::<SocketAddr>().unwrap())
        .with_custom_gauges(vec!["active_connections", "memory_usage"])
        .with_update_interval(Duration::from_secs(15));

    // Create metrics collector
    let mut metrics_collector = PrometheusMetricsCollector::new(metrics_config)?;

    // Start HTTP server for Prometheus scraping (fails if the address can't be bound)
    metrics_collector.start_exposition_server().await?;
    let metrics_collector = Arc::new(metrics_collector);

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
# async fn example(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler) {
use hammerwork::{Worker, AlertingConfig, AlertSeverity, SmtpConfig};
use std::time::Duration;

let alerting_config = AlertingConfig::new()
    .alert_on_high_error_rate(0.1)  // Alert if error rate > 10%
    .alert_on_queue_depth(1000)     // Alert if queue has > 1000 jobs
    .alert_on_worker_starvation(Duration::from_secs(5 * 60))
    .webhook("https://your-webhook.com/alerts")
    .slack("https://hooks.slack.com/your-webhook", "#alerts")
    .email_via_smtp(
        "admin@yourcompany.com",
        SmtpConfig::new("smtp.yourcompany.com", "alerts@yourcompany.com"),
    )
    .with_cooldown(Duration::from_secs(5 * 60));

let worker = Worker::new(queue, "default".to_string(), handler)
    .with_alerting_config(alerting_config);
# }
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
# use hammerwork::AlertingConfig;
# let headers = std::collections::HashMap::new();
let config = AlertingConfig::new()
    .webhook("https://your-webhook.com/alerts")
    .webhook_with_headers("https://api.example.com/alerts", headers);
```

#### Slack Integration
```rust
# use hammerwork::AlertingConfig;
let config = AlertingConfig::new()
    .slack("https://hooks.slack.com/your-webhook", "#alerts");
```

#### Email Alerts

Email alerts are sent over SMTP (via [`lettre`](https://crates.io/crates/lettre), part
of the `alerting` feature). Each email target carries its own SMTP settings:

```rust
use hammerwork::alerting::{AlertingConfig, SecretSource, SmtpConfig, SmtpTls};

let smtp = SmtpConfig::new("smtp.yourcompany.com", "Hammerwork <alerts@yourcompany.com>")
    .with_credentials("alerts", SecretSource::Environment("SMTP_PASSWORD".into()))
    .with_tls(SmtpTls::StartTls); // the default
let config = AlertingConfig::new()
    .email_via_smtp("admin@yourcompany.com", smtp);
```

| Setting | Default | Notes |
|---------|---------|-------|
| `host` | (required) | SMTP server |
| `port` | 587 / 465 / 25 | depends on `tls` |
| `from` | (required) | `alerts@example.com` or `Name <alerts@example.com>` |
| `username` + `password` | none | both or neither; `password` is a `SecretSource` |
| `tls` | `start_tls` | `start_tls` (required, never falls back to plain text), `implicit` (SMTPS), or `none` (local relays only) |
| `timeout_secs` | 30 | per SMTP command |

Use `SecretSource::Environment("VAR")` for the password so it stays out of code and
configuration files; it is read when each email is sent. `SecretSource::Static` is for
development only, and is redacted from `Debug` output.

In a TOML configuration file:

```toml
[[alerting.targets]]
[alerting.targets.Email]
recipient = "admin@yourcompany.com"

[alerting.targets.Email.smtp]
host = "smtp.yourcompany.com"
from = "alerts@yourcompany.com"
username = "alerts"
password = { Environment = "SMTP_PASSWORD" }
tls = "start_tls"
```

An email target without SMTP settings is a configuration error:
`AlertingConfig::validate()`, `AlertManager::try_new` and `HammerworkConfig::from_file`
reject it. (`AlertingConfig::email(recipient)` is deprecated because it creates such a
target. `AlertManager::new` does not fail; it logs the error and every alert to that
target fails.)

#### Delivery failures

`check_thresholds`, `check_queue_depth`, `check_worker_starvation` and
`send_custom_alert` return an error when any target could not be reached (an SMTP
error, an unreachable webhook, a non-2xx response); the worker's monitoring task logs
it. Every target is still tried. The cooldown for an alert only starts once at least
one target received it, so an alert that reached no target is sent again on the next
check.

## Background Monitoring

Workers automatically start a background monitoring task that:

- Updates queue depth metrics
- Checks for worker starvation
- Monitors statistical thresholds
- Triggers alerts when thresholds are exceeded

It runs every `MetricsConfig::update_interval` (15 seconds by default, at least one
second) when the worker has a metrics collector, and every 30 seconds otherwise.

## Statistics Collector

`InMemoryStatsCollector` keeps recent job events for the statistics that alert
thresholds and dashboards read. Recording an event is constant time; queries read only
the events inside their window. `StatsConfig` bounds what is kept:

| Field | Default | Meaning |
|-------|---------|---------|
| `max_events` | 100,000 | events kept; the oldest are dropped first |
| `max_event_age_secs` | 3600 | events older than this are pruned whenever an event is recorded (and not recorded at all) |
| `collect_timing` | `true` | keep processing times; when `false`, timing statistics stay at zero |

`AlertingConfig::custom_thresholds` (and `with_custom_threshold`) were removed in 2.0: no
alert ever read them. Configuration files that still set them load; the key is ignored.

## Logging

The `[logging]` section of `hammerwork.toml` is applied by
`config.logging.try_init()`, which installs a global `tracing` subscriber: `level` is an
`EnvFilter` directive (`"info"`, `"warn,hammerwork=debug"`), `json_format` switches to
JSON lines, and `include_location` adds file and line numbers. With
`enable_tracing = true` (and the `tracing` feature), spans are also exported over OTLP
to `tracing_endpoint` as `service_name`; call `hammerwork::tracing::shutdown_tracing()`
before exiting to flush them.

```rust
let logging = hammerwork::config::LoggingConfig {
    level: "warn,hammerwork=debug".to_string(),
    json_format: true,
    ..Default::default()
};
# let _ = &logging;
// logging.try_init()?;
```

## Custom Metrics

```rust
# use hammerwork::{MetricsConfig, PrometheusMetricsCollector};
# async fn example(metrics_collector: &PrometheusMetricsCollector) -> hammerwork::Result<()> {
let metrics_config = MetricsConfig::new()
    .with_custom_gauges(vec!["custom_metric_1", "custom_metric_2"])
    .with_histograms(vec!["custom_histogram_1"]);

// Update custom metrics
metrics_collector.update_custom_gauge("custom_metric_1", "queue_name", 42.0).await?;
metrics_collector.observe_custom_histogram("custom_histogram_1", "queue_name", 1.5).await?;
# let _ = metrics_config;
# Ok(())
# }
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