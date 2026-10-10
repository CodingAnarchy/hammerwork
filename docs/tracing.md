# Job Tracing & Correlation

Hammerwork integrates with OpenTelemetry so you can follow a job through your system with trace IDs, correlation IDs and parent span IDs, export spans over OTLP, and observe job lifecycle events with hooks.

## Overview

The tracing support lets you:
- Attach a trace ID, correlation ID, parent span ID and span context to a job
- Correlate related business operations through a shared correlation ID
- Link child jobs to a parent
- Export spans to any OTLP/gRPC collector (Jaeger, the OpenTelemetry Collector, the Datadog agent with OTLP ingest enabled, and others)
- React to job lifecycle events (start, complete, fail, timeout, retry) with hooks

The trace fields (`with_trace_id`, `with_correlation_id`, ...) are available on every build. `TracingConfig`, `init_tracing`, `shutdown_tracing`, `create_job_span` and `set_job_trace_context` need the `tracing` feature.

## Setup

### Enable the Tracing Feature

```toml
[dependencies]
hammerwork = { version = "2.0", features = ["postgres", "tracing"] }
```

### Initialize Tracing

`init_tracing` installs an OpenTelemetry tracer (and a `tracing` subscriber) for the process. Call `shutdown_tracing` before exit to flush pending spans.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::tracing::{TracingConfig, init_tracing, shutdown_tracing};

let tracing_config = TracingConfig::new()
    .with_service_name("job-processor")
    .with_service_version("1.0.0")
    .with_environment("production")
    .with_otlp_endpoint("http://jaeger:4317");

init_tracing(tracing_config).await?;

// ... run your application ...

shutdown_tracing().await;
# Ok(())
# }
```

### Configuration Options

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::tracing::TracingConfig;

let config = TracingConfig::new()
    .with_service_name("my-service")           // Service name in traces
    .with_service_version("1.2.0")             // Service version
    .with_environment("staging")               // Deployment environment
    .with_otlp_endpoint("http://jaeger:4317")  // OTLP/gRPC endpoint for export
    .with_console_exporter(true)               // Also print spans to the console
    .with_resource_attribute("team", "data-platform")    // Custom resource attributes,
    .with_resource_attribute("component", "job-processor"); // one call per attribute
# Ok(())
# }
```

The OTLP exporter speaks gRPC, so point it at the collector's gRPC port (4317 by default).
There is no sampling option in `TracingConfig`.

## Creating Traced Jobs

### Basic Tracing

`TraceId` and `CorrelationId` generate identifiers (`trace-<uuid>` and `corr-<uuid>`) or wrap your own with `from_string`. Jobs store them as plain strings.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, tracing::{CorrelationId, TraceId}};
use serde_json::json;

let trace_id = TraceId::new();
let correlation_id = CorrelationId::new();

let job = Job::new("email_queue".to_string(), json!({
    "to": "user@example.com",
    "subject": "Welcome to our service"
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string());

assert_eq!(job.get_trace_id(), Some(trace_id.as_str()));
assert_eq!(job.get_correlation_id(), Some(correlation_id.as_str()));

queue.enqueue(job).await?;
# Ok(())
# }
```

### Business Process Correlation

Use a business identifier as the correlation ID to group related operations across queues:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, tracing::{CorrelationId, TraceId}};
use serde_json::json;

let order_id = "order-12345";
let trace_id = TraceId::new();
let correlation_id = CorrelationId::from_string(order_id);

// Payment processing job
let payment_job = Job::new("payment_queue".to_string(), json!({
    "order_id": order_id,
    "amount": 299.99,
    "currency": "USD"
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string());

// Email confirmation job (runs after payment)
let email_job = Job::new("email_queue".to_string(), json!({
    "order_id": order_id,
    "template": "order_confirmation",
    "customer_email": "customer@example.com"
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string())
.depends_on(&payment_job.id);

// Inventory update job (also runs after payment)
let inventory_job = Job::new("inventory_queue".to_string(), json!({
    "order_id": order_id,
    "items": [{"sku": "PROD-001", "quantity": 2}]
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string())
.depends_on(&payment_job.id);

queue.enqueue(payment_job).await?;
queue.enqueue(email_job).await?;
queue.enqueue(inventory_job).await?;
# Ok(())
# }
```

### Hierarchical Tracing

Link child jobs to a parent with `with_parent_span_id`:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, tracing::TraceId};
use serde_json::json;

let trace_id = TraceId::new();

let parent_job = Job::new("data_import".to_string(), json!({
    "source": "customer_data.csv",
    "batch_size": 1000
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id("batch-import-001");

let parent_id = queue.enqueue(parent_job).await?;

for i in 0..10 {
    let child_job = Job::new("process_batch".to_string(), json!({
        "batch_id": i,
        "start_row": i * 1000,
        "end_row": (i + 1) * 1000
    }))
    .with_trace_id(trace_id.to_string())
    .with_correlation_id("batch-import-001")
    .with_parent_span_id(parent_id.to_string());

    queue.enqueue(child_job).await?;
}
# Ok(())
# }
```

## Worker Integration

With the `tracing` feature enabled, a worker creates a `job.process` span (via `create_job_span`) around each job it processes. The span carries the job's ID, queue, priority, attempts, trace ID, correlation ID and parent span ID as attributes. Inside the handler you can read the same fields from the job:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, Worker, worker::JobHandler};
use std::sync::Arc;

let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        println!("Processing job {} with trace_id: {:?}", job.id, job.trace_id);
        // Your business logic here
        Ok(())
    })
});

let worker = Worker::new(queue.clone(), "email_queue".to_string(), handler);
# Ok(())
# }
```

## Lifecycle Event Hooks

Register `JobEventHooks` on a worker with `Worker::with_event_hooks`. Each hook receives a `JobHookEvent` with the job, a timestamp, an optional processing duration (completion events) and an optional error message (failure events). Hooks are plain synchronous closures.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Worker, worker::{JobEventHooks, JobHookEvent}};

let hooks = JobEventHooks::new()
    .on_start(|event: JobHookEvent| {
        ::tracing::info!(
            job_id = %event.job.id,
            queue = %event.job.queue_name,
            trace_id = event.job.trace_id.as_deref().unwrap_or(""),
            correlation_id = event.job.correlation_id.as_deref().unwrap_or(""),
            "Job processing started"
        );
    })
    .on_complete(|event: JobHookEvent| {
        ::tracing::info!(
            job_id = %event.job.id,
            duration_ms = event.duration.unwrap_or_default().as_millis() as u64,
            trace_id = event.job.trace_id.as_deref().unwrap_or(""),
            "Job completed successfully"
        );
    })
    .on_fail(|event: JobHookEvent| {
        ::tracing::error!(
            job_id = %event.job.id,
            error = event.error.as_deref().unwrap_or(""),
            trace_id = event.job.trace_id.as_deref().unwrap_or(""),
            attempt = event.job.attempts,
            "Job failed"
        );
    })
    .on_timeout(|event: JobHookEvent| {
        ::tracing::warn!(job_id = %event.job.id, "Job timed out");
    })
    .on_retry(|event: JobHookEvent| {
        ::tracing::warn!(
            job_id = %event.job.id,
            attempt = event.job.attempts,
            max_attempts = event.job.max_attempts,
            "Job retry scheduled"
        );
    });

let worker = Worker::new(queue.clone(), "data_processing".to_string(), handler)
    .with_event_hooks(hooks);
# Ok(())
# }
```

## Trace Context Propagation

### Span Creation

`create_job_span` builds the span the worker uses. You can call it yourself, for example in tests or when processing a job outside a worker:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, tracing::create_job_span};
use serde_json::json;

let job = Job::new("email_queue".to_string(), json!({"to": "user@example.com"}))
    .with_trace_id("trace-123")
    .with_correlation_id("order-456");

let span = create_job_span(&job, "job.process");
let _enter = span.enter();
// Work done here runs inside the span
# Ok(())
# }
```

### Propagating the Current Span into a Job

`set_job_trace_context` copies the trace ID, span ID and span context of an OpenTelemetry-backed span onto a job so work enqueued from a traced request continues the same trace. It does nothing if the span has no valid OpenTelemetry context.

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, tracing::set_job_trace_context};
use serde_json::json;

let mut job = Job::new("process_data".to_string(), json!({"data": "example"}));
set_job_trace_context(&mut job, &::tracing::Span::current());
queue.enqueue(job).await?;
# Ok(())
# }
```

### Custom Spans in Handlers

Create your own spans inside a handler for finer-grained timing:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::{Job, worker::JobHandler};
use std::sync::Arc;
use tracing::{Instrument, info_span};

let handler: JobHandler = Arc::new(|job: Job| {
    Box::pin(async move {
        let business_span = info_span!(
            "process_order",
            order_id = job.payload.get("order_id").and_then(|v| v.as_str()).unwrap_or(""),
            trace_id = job.trace_id.as_deref().unwrap_or(""),
            correlation_id = job.correlation_id.as_deref().unwrap_or("")
        );

        async move {
            // validate, charge, confirm ...
            Ok(())
        }
        .instrument(business_span)
        .await
    })
});
# Ok(())
# }
```

## Integration with Observability Platforms

`TracingConfig::with_otlp_endpoint` takes an OTLP/gRPC endpoint. Any backend that ingests OTLP works; typically you send to a collector or agent that forwards to the backend.

### Jaeger

Jaeger accepts OTLP natively:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::tracing::{TracingConfig, init_tracing};

let config = TracingConfig::new()
    .with_service_name("hammerwork-jobs")
    .with_otlp_endpoint("http://jaeger:4317");

init_tracing(config).await?;
# Ok(())
# }
```

### Zipkin, Datadog and Others

Run an OpenTelemetry Collector (or the Datadog agent with OTLP ingest enabled) that exports to the backend, and point Hammerwork at its gRPC receiver:

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::tracing::{TracingConfig, init_tracing};

let config = TracingConfig::new()
    .with_service_name("hammerwork-jobs")
    .with_environment("production")
    .with_otlp_endpoint("http://otel-collector:4317");

init_tracing(config).await?;
# Ok(())
# }
```

## Best Practices

### 1. Use Correlation IDs for Business Processes

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Job;

let order_id = 12345;
let correlation_id = format!("order-{}", order_id);
let job = Job::new("process_order".to_string(), payload.clone())
    .with_correlation_id(correlation_id);
# Ok(())
# }
```

### 2. Propagate Trace Context to Child Jobs

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
use hammerwork::Job;

let child_job = Job::new("child_task".to_string(), payload.clone())
    .with_trace_id(job.trace_id.clone().unwrap_or_default())
    .with_correlation_id(job.correlation_id.clone().unwrap_or_default())
    .with_parent_span_id(job.id.to_string());
# Ok(())
# }
```

### 3. Use Structured Logging

```rust,no_run
# #[allow(unused_imports)] use hammerwork::queue::DatabaseQueue;
# #[allow(unused_variables, unused_mut, dead_code, unreachable_code)]
# async fn doc(queue: std::sync::Arc<hammerwork::JobQueue<sqlx::Postgres>>, handler: hammerwork::worker::JobHandler, payload: serde_json::Value, job: hammerwork::Job) -> std::result::Result<(), Box<dyn std::error::Error>> {
::tracing::info!(
    job_id = %job.id,
    trace_id = job.get_trace_id().unwrap_or(""),
    correlation_id = job.get_correlation_id().unwrap_or(""),
    queue = %job.queue_name,
    "Processing job"
);
# Ok(())
# }
```

## Troubleshooting

1. **Missing spans**: enable the `tracing` feature and call `init_tracing` before workers start.
2. **Trace gaps**: make sure the trace ID is copied onto every child job (see above).
3. **Export failures**: check connectivity to the OTLP gRPC endpoint; `init_tracing` returns an error if the exporter cannot be built.
4. **No console output**: set `with_console_exporter(true)` while debugging.

## Security

- Trace IDs and correlation IDs end up in logs and spans; secure those systems accordingly.
- Do not put sensitive data in span attributes.
- Use authenticated, encrypted connections to the collector in production.
