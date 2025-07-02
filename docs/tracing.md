# Job Tracing & Correlation

Hammerwork provides comprehensive distributed tracing capabilities with OpenTelemetry integration, enabling you to track job execution across your entire system with trace IDs, correlation IDs, and lifecycle event hooks.

## Overview

The tracing system in Hammerwork allows you to:
- Track job execution across multiple services and systems
- Correlate related business operations with correlation IDs
- Create hierarchical trace relationships between parent and child jobs
- Export traces to observability platforms like Jaeger, Zipkin, or DataDog
- Monitor job lifecycle events with custom hooks
- Debug complex workflows and data processing pipelines

## Setup

### Enable Tracing Feature

Add the `tracing` feature to your `Cargo.toml`:

```toml
[dependencies]
hammerwork = { version = "1.4", features = ["postgres", "tracing"] }
```

### Initialize Tracing

```rust
use hammerwork::tracing::{TracingConfig, init_tracing};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure distributed tracing
    let tracing_config = TracingConfig::new()
        .with_service_name("job-processor")
        .with_service_version("1.0.0")
        .with_environment("production")
        .with_otlp_endpoint("http://jaeger:4317");
    
    // Initialize tracing
    init_tracing(tracing_config).await?;
    
    // Your application code here
    Ok(())
}
```

### Configuration Options

```rust
let config = TracingConfig::new()
    .with_service_name("my-service")           // Service name in traces
    .with_service_version("1.2.0")            // Service version
    .with_environment("staging")              // Environment (dev, staging, prod)
    .with_otlp_endpoint("http://jaeger:4317") // OTLP endpoint for trace export
    .with_console_exporter(true)              // Enable console output for debugging
    .with_resource_attributes(vec![           // Custom resource attributes
        ("team", "data-platform"),
        ("component", "job-processor")
    ]);
```

## Creating Traced Jobs

### Basic Tracing

```rust
use hammerwork::{Job, tracing::{TraceId, CorrelationId}};
use serde_json::json;

// Generate trace and correlation IDs
let trace_id = TraceId::new();
let correlation_id = CorrelationId::new();

// Create a job with tracing information
let job = Job::new("email_queue".to_string(), json!({
    "to": "user@example.com",
    "subject": "Welcome to our service"
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string());

queue.enqueue(job).await?;
```

### Business Process Correlation

Use correlation IDs to group related operations across different queues:

```rust
// Order processing workflow
let order_id = "order-12345";
let trace_id = TraceId::new();
let correlation_id = CorrelationId::from_business_id(order_id);

// Payment processing job
let payment_job = Job::new("payment_queue".to_string(), json!({
    "order_id": order_id,
    "amount": 299.99,
    "currency": "USD"
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string());

// Email confirmation job (depends on payment)
let email_job = Job::new("email_queue".to_string(), json!({
    "order_id": order_id,
    "template": "order_confirmation",
    "customer_email": "customer@example.com"
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string())
.depends_on(&payment_job.id);

// Inventory update job (also depends on payment)
let inventory_job = Job::new("inventory_queue".to_string(), json!({
    "order_id": order_id,
    "items": [{"sku": "PROD-001", "quantity": 2}]
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id(correlation_id.to_string())
.depends_on(&payment_job.id);

// Enqueue all jobs
queue.enqueue(payment_job).await?;
queue.enqueue(email_job).await?;
queue.enqueue(inventory_job).await?;
```

### Hierarchical Tracing

Create parent-child relationships between jobs:

```rust
// Parent job
let parent_job = Job::new("data_import".to_string(), json!({
    "source": "customer_data.csv",
    "batch_size": 1000
}))
.with_trace_id(trace_id.to_string())
.with_correlation_id("batch-import-001");

let parent_id = queue.enqueue(parent_job).await?;

// Child jobs with parent relationship
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
```

## Worker Integration

Workers automatically create OpenTelemetry spans when processing jobs:

```rust
use hammerwork::{Worker, WorkerPool};
use std::sync::Arc;

// Create job handler
let handler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Job processing happens inside an automatic span
        println!("Processing job {} with trace_id: {:?}", 
                 job.id, job.trace_id);
        
        // Your business logic here
        process_customer_data(&job.payload).await?;
        
        Ok(())
    })
});

// Create worker with lifecycle event hooks
let worker = Worker::new(queue.clone(), "email_queue".to_string(), handler)
    .on_job_start(|event| {
        println!("Job {} started - Trace: {}, Correlation: {}", 
                 event.job.id,
                 event.job.trace_id.unwrap_or_default(),
                 event.job.correlation_id.unwrap_or_default());
    })
    .on_job_complete(|event| {
        println!("Job {} completed in {:?} - Trace: {}", 
                 event.job.id, 
                 event.duration.unwrap_or_default(),
                 event.job.trace_id.unwrap_or_default());
    })
    .on_job_fail(|event| {
        eprintln!("Job {} failed - Trace: {}, Error: {}", 
                  event.job.id,
                  event.job.trace_id.unwrap_or_default(),
                  event.error.unwrap_or_default());
    });
```

## Lifecycle Event Hooks

Monitor job execution with detailed lifecycle events:

```rust
use hammerwork::events::{JobLifecycleEvent, EventType};

let worker = Worker::new(queue.clone(), "data_processing".to_string(), handler)
    .on_job_start(|event: JobLifecycleEvent| {
        // Log job start
        tracing::info!(
            job_id = %event.job.id,
            queue = %event.job.queue_name,
            trace_id = %event.job.trace_id.unwrap_or_default(),
            correlation_id = %event.job.correlation_id.unwrap_or_default(),
            "Job processing started"
        );
    })
    .on_job_complete(|event: JobLifecycleEvent| {
        // Log successful completion
        tracing::info!(
            job_id = %event.job.id,
            duration_ms = %event.duration.unwrap_or_default().as_millis(),
            trace_id = %event.job.trace_id.unwrap_or_default(),
            "Job completed successfully"
        );
    })
    .on_job_fail(|event: JobLifecycleEvent| {
        // Log failure with error details
        tracing::error!(
            job_id = %event.job.id,
            error = %event.error.unwrap_or_default(),
            trace_id = %event.job.trace_id.unwrap_or_default(),
            attempt = %event.job.attempts,
            "Job failed"
        );
    })
    .on_job_retry(|event: JobLifecycleEvent| {
        // Log retry attempts
        tracing::warn!(
            job_id = %event.job.id,
            attempt = %event.job.attempts,
            max_attempts = %event.job.max_attempts,
            trace_id = %event.job.trace_id.unwrap_or_default(),
            "Job retry scheduled"
        );
    });
```

## Trace Context Propagation

### Automatic Span Creation

When the `tracing` feature is enabled, workers automatically create spans for job processing:

```rust
// Worker automatically creates spans like this:
#[cfg(feature = "tracing")]
let _span = crate::tracing::create_job_span(&job, "job.process");
```

### Custom Span Creation

Create custom spans within your job handlers:

```rust
use tracing::{info_span, Instrument};

let handler = Arc::new(|job: Job| {
    Box::pin(async move {
        // Create custom span for business logic
        let business_span = info_span!(
            "process_order",
            order_id = %job.payload.get("order_id").unwrap_or(&json!("")),
            trace_id = %job.trace_id.unwrap_or_default(),
            correlation_id = %job.correlation_id.unwrap_or_default()
        );
        
        async move {
            // Your business logic here
            validate_order(&job.payload).await?;
            process_payment(&job.payload).await?;
            send_confirmation(&job.payload).await?;
            Ok(())
        }
        .instrument(business_span)
        .await
    })
});
```

## Integration with Observability Platforms

### Jaeger

```rust
let config = TracingConfig::new()
    .with_service_name("hammerwork-jobs")
    .with_otlp_endpoint("http://jaeger:4317");

init_tracing(config).await?;
```

### Zipkin

```rust
let config = TracingConfig::new()
    .with_service_name("hammerwork-jobs")
    .with_otlp_endpoint("http://zipkin:9411/api/v2/spans");

init_tracing(config).await?;
```

### DataDog

```rust
let config = TracingConfig::new()
    .with_service_name("hammerwork-jobs")
    .with_environment("production")
    .with_otlp_endpoint("https://trace.agent.datadoghq.com:4318");

init_tracing(config).await?;
```

## Querying Jobs by Trace

Use the CLI to find jobs by trace or correlation ID:

```bash
# Find jobs by trace ID
cargo hammerwork job list --trace-id "550e8400-e29b-41d4-a716-446655440000"

# Find jobs by correlation ID
cargo hammerwork job list --correlation-id "order-12345"

# Show tracing information for a specific job
cargo hammerwork job show abc123 --include-tracing
```

## Best Practices

### 1. Use Correlation IDs for Business Processes

```rust
// Good: Use business identifiers for correlation
let correlation_id = format!("order-{}", order.id);
let job = Job::new("process_order".to_string(), payload)
    .with_correlation_id(correlation_id);
```

### 2. Propagate Trace Context

```rust
// When creating child jobs, propagate trace context
let child_job = Job::new("child_task".to_string(), payload)
    .with_trace_id(parent_job.trace_id.clone().unwrap_or_default())
    .with_correlation_id(parent_job.correlation_id.clone().unwrap_or_default())
    .with_parent_span_id(parent_job.id.to_string());
```

### 3. Use Structured Logging

```rust
// Include tracing information in logs
tracing::info!(
    job_id = %job.id,
    trace_id = %job.trace_id.unwrap_or_default(),
    correlation_id = %job.correlation_id.unwrap_or_default(),
    queue = %job.queue_name,
    "Processing job"
);
```

### 4. Monitor Trace Completion

```rust
// Track when entire traces are complete
let handler = Arc::new(|job: Job| {
    Box::pin(async move {
        let result = process_job(&job).await;
        
        // Log trace completion if this is the final job
        if job.is_final_job_in_trace() {
            tracing::info!(
                trace_id = %job.trace_id.unwrap_or_default(),
                correlation_id = %job.correlation_id.unwrap_or_default(),
                "Trace completed"
            );
        }
        
        result
    })
});
```

## Troubleshooting

### Common Issues

1. **Missing Spans**: Ensure the `tracing` feature is enabled and `init_tracing()` is called
2. **Trace Gaps**: Check that trace IDs are properly propagated between jobs
3. **Performance Impact**: Tracing adds minimal overhead, but disable in performance-critical scenarios if needed
4. **Export Failures**: Verify OTLP endpoint connectivity and authentication

### Debug Mode

Enable console output for debugging:

```rust
let config = TracingConfig::new()
    .with_service_name("debug-service")
    .with_console_exporter(true);
```

### Sampling

Configure trace sampling for high-throughput systems:

```rust
let config = TracingConfig::new()
    .with_service_name("high-throughput-service")
    .with_sampling_ratio(0.1); // Sample 10% of traces
```

## Performance Considerations

- Tracing adds minimal overhead (< 1% CPU impact)
- Use sampling for high-throughput systems
- Trace export is asynchronous and non-blocking
- Consider trace retention policies in your observability platform

## Security

- Trace IDs and correlation IDs are included in logs - ensure log security
- Avoid including sensitive data in span attributes
- Use proper authentication for OTLP endpoints
- Consider data residency requirements for trace export