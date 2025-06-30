//! Distributed tracing and correlation support for Hammerwork jobs.
//!
//! This module provides comprehensive tracing capabilities including distributed trace IDs,
//! correlation IDs, span context propagation, and OpenTelemetry integration for observability
//! in distributed systems.
//!
//! ## Features
//!
//! - **Trace ID**: Track jobs across service boundaries
//! - **Correlation ID**: Group related business operations  
//! - **Span Context**: Hierarchical tracing with parent-child relationships
//! - **OpenTelemetry Integration**: Export traces to observability platforms
//! - **Job Lifecycle Events**: Automatic span creation for job processing
//!
//! ## Usage
//!
//! ### Basic Tracing
//!
//! ```rust
//! use hammerwork::{Job, tracing::TraceId};
//! use serde_json::json;
//!
//! // Create a job with trace ID
//! let trace_id = TraceId::new();
//! let job = Job::new("data_processing".to_string(), json!({"data": "example"}))
//!     .with_trace_id(trace_id.to_string())
//!     .with_correlation_id("business-operation-123");
//! ```
//!
//! ### OpenTelemetry Integration
//!
//! ```rust,no_run
//! # #[cfg(feature = "tracing")]
//! use hammerwork::tracing::{TracingConfig, init_tracing};
//!
//! # #[cfg(feature = "tracing")]
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = TracingConfig::new()
//!     .with_service_name("my-job-processor")
//!     .with_otlp_endpoint("http://localhost:4317");
//!     
//! init_tracing(config).await?;
//! # Ok(())
//! # }
//! ```

use std::fmt;
use uuid::Uuid;

#[cfg(feature = "tracing")]
use opentelemetry::{KeyValue, global, trace::TracerProvider};
#[cfg(feature = "tracing")]
use opentelemetry_otlp::WithExportConfig;
#[cfg(feature = "tracing")]
use tracing_opentelemetry::OpenTelemetryLayer;

#[cfg(feature = "tracing")]
use crate::Result;

/// A distributed trace identifier for tracking operations across services.
///
/// Trace IDs are used to correlate all operations that are part of a single
/// request or workflow across multiple services and components.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraceId(String);

impl TraceId {
    /// Generate a new random trace ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::tracing::TraceId;
    ///
    /// let trace_id = TraceId::new();
    /// assert!(!trace_id.to_string().is_empty());
    /// ```
    pub fn new() -> Self {
        Self(format!("trace-{}", Uuid::new_v4()))
    }

    /// Create a trace ID from an existing string.
    ///
    /// # Arguments
    ///
    /// * `id` - The trace identifier string
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::tracing::TraceId;
    ///
    /// let trace_id = TraceId::from_string("my-custom-trace-123");
    /// assert_eq!(trace_id.to_string(), "my-custom-trace-123");
    /// ```
    pub fn from_string(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the trace ID as a string.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::tracing::TraceId;
    ///
    /// let trace_id = TraceId::from_string("test-trace");
    /// assert_eq!(trace_id.to_string(), "test-trace");
    /// ```
    pub fn into_string(self) -> String {
        self.0
    }

    /// Get the trace ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TraceId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for TraceId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

/// A business correlation identifier for grouping related operations.
///
/// Correlation IDs are used to link operations that are related from a business
/// perspective, even if they span multiple traces or occur at different times.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// Generate a new random correlation ID.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::tracing::CorrelationId;
    ///
    /// let correlation_id = CorrelationId::new();
    /// assert!(!correlation_id.to_string().is_empty());
    /// ```
    pub fn new() -> Self {
        Self(format!("corr-{}", Uuid::new_v4()))
    }

    /// Create a correlation ID from an existing string.
    ///
    /// # Arguments
    ///
    /// * `id` - The correlation identifier string
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::tracing::CorrelationId;
    ///
    /// let correlation_id = CorrelationId::from_string("order-12345");
    /// assert_eq!(correlation_id.to_string(), "order-12345");
    /// ```
    pub fn from_string(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get the correlation ID as a string.
    pub fn into_string(self) -> String {
        self.0
    }

    /// Get the correlation ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for CorrelationId {
    fn from(id: String) -> Self {
        Self(id)
    }
}

impl From<&str> for CorrelationId {
    fn from(id: &str) -> Self {
        Self(id.to_string())
    }
}

/// Configuration for distributed tracing setup.
///
/// This struct contains all the settings needed to configure OpenTelemetry
/// tracing for job processing, including service information and export settings.
#[cfg(feature = "tracing")]
#[derive(Debug, Clone)]
pub struct TracingConfig {
    /// Name of the service for tracing identification
    pub service_name: String,
    /// Version of the service
    pub service_version: Option<String>,
    /// Environment (e.g., "production", "staging", "development")
    pub environment: Option<String>,
    /// OTLP endpoint for exporting traces
    pub otlp_endpoint: Option<String>,
    /// Additional resource attributes
    pub resource_attributes: Vec<(String, String)>,
    /// Whether to enable console exporter for debugging
    pub console_exporter: bool,
}

#[cfg(feature = "tracing")]
impl TracingConfig {
    /// Create a new tracing configuration with default values.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new();
    /// ```
    pub fn new() -> Self {
        Self {
            service_name: "hammerwork".to_string(),
            service_version: None,
            environment: None,
            otlp_endpoint: None,
            resource_attributes: Vec::new(),
            console_exporter: false,
        }
    }

    /// Set the service name for tracing.
    ///
    /// # Arguments
    ///
    /// * `name` - The service name to use in traces
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new()
    ///     .with_service_name("my-job-processor");
    /// ```
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Set the service version for tracing.
    ///
    /// # Arguments
    ///
    /// * `version` - The service version to include in traces
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new()
    ///     .with_service_version("1.0.0");
    /// ```
    pub fn with_service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Set the environment for tracing.
    ///
    /// # Arguments
    ///
    /// * `environment` - The environment name (e.g., "production", "staging")
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new()
    ///     .with_environment("production");
    /// ```
    pub fn with_environment(mut self, environment: impl Into<String>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    /// Set the OTLP endpoint for exporting traces.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The OTLP endpoint URL
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new()
    ///     .with_otlp_endpoint("http://localhost:4317");
    /// ```
    pub fn with_otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    /// Add a resource attribute to the tracing configuration.
    ///
    /// # Arguments
    ///
    /// * `key` - The attribute key
    /// * `value` - The attribute value
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new()
    ///     .with_resource_attribute("deployment.environment", "production")
    ///     .with_resource_attribute("service.instance.id", "worker-01");
    /// ```
    pub fn with_resource_attribute(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.resource_attributes.push((key.into(), value.into()));
        self
    }

    /// Enable console exporter for debugging.
    ///
    /// When enabled, traces will be printed to the console in addition to
    /// any other configured exporters.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # #[cfg(feature = "tracing")]
    /// use hammerwork::tracing::TracingConfig;
    ///
    /// # #[cfg(feature = "tracing")]
    /// let config = TracingConfig::new()
    ///     .with_console_exporter(true);
    /// ```
    pub fn with_console_exporter(mut self, enabled: bool) -> Self {
        self.console_exporter = enabled;
        self
    }
}

#[cfg(feature = "tracing")]
impl Default for TracingConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialize distributed tracing with OpenTelemetry.
///
/// This function sets up the global tracing infrastructure including OpenTelemetry
/// providers, exporters, and the tracing subscriber for collecting and exporting
/// job processing traces.
///
/// # Arguments
///
/// * `config` - Tracing configuration including service info and export settings
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "tracing")]
/// use hammerwork::tracing::{TracingConfig, init_tracing};
///
/// # #[cfg(feature = "tracing")]
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = TracingConfig::new()
///     .with_service_name("job-processor")
///     .with_otlp_endpoint("http://jaeger:4317");
///     
/// init_tracing(config).await?;
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "tracing")]
pub async fn init_tracing(config: TracingConfig) -> Result<()> {
    use opentelemetry_sdk::Resource;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    // Build resource with service information
    let mut resource = Resource::new(vec![KeyValue::new(
        "service.name",
        config.service_name.clone(),
    )]);

    if let Some(version) = &config.service_version {
        resource = resource.merge(&Resource::new(vec![KeyValue::new(
            "service.version",
            version.clone(),
        )]));
    }

    if let Some(environment) = &config.environment {
        resource = resource.merge(&Resource::new(vec![KeyValue::new(
            "deployment.environment",
            environment.clone(),
        )]));
    }

    // Add custom resource attributes
    for (key, value) in &config.resource_attributes {
        resource = resource.merge(&Resource::new(vec![KeyValue::new(
            key.clone(),
            value.clone(),
        )]));
    }

    // Set up tracer provider based on configuration
    let tracer_provider = if let Some(endpoint) = &config.otlp_endpoint {
        // Create OTLP exporter
        let exporter = opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(endpoint.clone())
            .build_span_exporter()
            .map_err(|e| crate::HammerworkError::Tracing {
                message: format!("Failed to build OTLP span exporter: {}", e),
            })?;

        // Create batch span processor
        let span_processor = opentelemetry_sdk::trace::BatchSpanProcessor::builder(
            exporter,
            opentelemetry_sdk::runtime::Tokio,
        )
        .build();

        // Build tracer provider with OTLP exporter
        opentelemetry_sdk::trace::TracerProvider::builder()
            .with_config(
                opentelemetry_sdk::trace::config()
                    .with_resource(resource)
                    .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn),
            )
            .with_span_processor(span_processor)
            .build()
    } else {
        // No-op tracer if no endpoint is configured
        opentelemetry_sdk::trace::TracerProvider::builder()
            .with_config(
                opentelemetry_sdk::trace::config()
                    .with_resource(resource)
                    .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOff),
            )
            .build()
    };

    // Set global tracer provider
    global::set_tracer_provider(tracer_provider.clone());

    // Create OpenTelemetry layer
    let telemetry_layer = OpenTelemetryLayer::new(tracer_provider.tracer("hammerwork"));

    // Initialize tracing subscriber
    let subscriber = tracing_subscriber::registry().with(telemetry_layer);

    if config.console_exporter {
        // Add console layer for debugging
        subscriber
            .with(tracing_subscriber::fmt::layer())
            .try_init()
            .map_err(|e| crate::HammerworkError::Tracing {
                message: format!("Failed to initialize tracing subscriber: {}", e),
            })?;
    } else {
        subscriber
            .try_init()
            .map_err(|e| crate::HammerworkError::Tracing {
                message: format!("Failed to initialize tracing subscriber: {}", e),
            })?;
    }

    Ok(())
}

/// Shutdown the global tracing infrastructure.
///
/// This function properly shuts down the OpenTelemetry providers and flushes
/// any pending traces. It should be called before application shutdown.
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "tracing")]
/// use hammerwork::tracing::shutdown_tracing;
///
/// # #[cfg(feature = "tracing")]
/// # #[tokio::main]
/// # async fn main() {
/// // At application shutdown
/// shutdown_tracing().await;
/// # }
/// ```
#[cfg(feature = "tracing")]
pub async fn shutdown_tracing() {
    global::shutdown_tracer_provider();
}

/// Create a span for job processing with trace context propagation.
///
/// This function creates an OpenTelemetry span for a job execution, setting up
/// the proper trace context and span attributes from the job data.
///
/// # Arguments
///
/// * `job` - The job to create a span for
/// * `operation_name` - Name of the operation (e.g., "job.process", "job.retry")
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "tracing")]
/// use hammerwork::tracing::create_job_span;
/// use hammerwork::Job;
/// use serde_json::json;
///
/// # #[cfg(feature = "tracing")]
/// # async fn example() {
/// let job = Job::new("email_queue".to_string(), json!({"to": "user@example.com"}))
///     .with_trace_id("trace-123")
///     .with_correlation_id("order-456");
///
/// let span = create_job_span(&job, "job.process");
/// let _enter = span.enter(); // Activate the span for the current scope
///
/// // Job processing happens here within the span context
/// # }
/// ```
#[cfg(feature = "tracing")]
pub fn create_job_span(job: &crate::Job, operation_name: &'static str) -> tracing::Span {
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    // Create the span with job context
    let span = tracing::info_span!(
        "job.process",
        operation_name = operation_name,
        job.id = %job.id,
        job.queue_name = %job.queue_name,
        job.priority = ?job.priority,
        job.status = ?job.status,
        job.attempts = job.attempts,
        job.trace_id = job.trace_id.as_deref().unwrap_or(""),
        job.correlation_id = job.correlation_id.as_deref().unwrap_or(""),
        job.parent_span_id = job.parent_span_id.as_deref().unwrap_or(""),
        otel.kind = "consumer",
        otel.name = operation_name,
    );

    // Set OpenTelemetry span context if available
    if let Some(trace_id_str) = &job.trace_id {
        // Try to restore OpenTelemetry trace context from job data
        if let Some(_span_context_str) = &job.span_context {
            // Here we would deserialize the span context, but for now we'll
            // set trace attributes to maintain the trace chain
            span.set_attribute("trace.id", trace_id_str.clone());
        }
    }

    // Set correlation ID for business operation tracking
    if let Some(correlation_id) = &job.correlation_id {
        span.set_attribute("correlation.id", correlation_id.clone());
    }

    // Set parent span ID for hierarchical tracing
    if let Some(parent_span_id) = &job.parent_span_id {
        span.set_attribute("parent.span.id", parent_span_id.clone());
    }

    // Add job-specific attributes
    span.set_attribute("job.scheduled_at", job.scheduled_at.to_rfc3339());

    if let Some(cron_schedule) = &job.cron_schedule {
        span.set_attribute("job.cron_schedule", cron_schedule.clone());
    }

    if job.is_recurring() {
        span.set_attribute("job.recurring", true);
    }

    span
}

/// Extract trace context from an OpenTelemetry span and set it on a job.
///
/// This function extracts the current span context and stores it in the job's
/// tracing fields, allowing the trace context to be propagated across service
/// boundaries and job queue operations.
///
/// # Arguments
///
/// * `job` - The job to set trace context on
/// * `span` - The current OpenTelemetry span
///
/// # Examples
///
/// ```rust,no_run
/// # #[cfg(feature = "tracing")]
/// use hammerwork::tracing::set_job_trace_context;
/// use hammerwork::Job;
/// use serde_json::json;
///
/// # #[cfg(feature = "tracing")]
/// # async fn example() {
/// let mut job = Job::new("process_data".to_string(), json!({"data": "example"}));
///
/// // In an instrumented function with an active span
/// let span = tracing::Span::current();
/// set_job_trace_context(&mut job, &span);
///
/// // Job now carries the trace context for propagation
/// # }
/// ```
#[cfg(feature = "tracing")]
pub fn set_job_trace_context(job: &mut crate::Job, span: &tracing::Span) {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    // Get OpenTelemetry context from the tracing span
    let otel_context = span.context();
    let otel_span = otel_context.span();
    let span_context = otel_span.span_context();

    if span_context.is_valid() {
        // Set trace ID
        let trace_id = span_context.trace_id().to_string();
        job.trace_id = Some(trace_id);

        // Set span ID as parent for child operations
        let span_id = span_context.span_id().to_string();
        job.parent_span_id = Some(span_id);

        // Serialize and store the full span context for complete propagation
        // In a production implementation, you might want to use the W3C Trace Context
        // format or OpenTelemetry's context propagation format
        job.span_context = Some(format!(
            "trace_id={};span_id={};trace_flags={:?}",
            span_context.trace_id(),
            span_context.span_id(),
            span_context.trace_flags()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_id_new() {
        let trace_id = TraceId::new();
        assert!(!trace_id.to_string().is_empty());
        assert!(trace_id.to_string().starts_with("trace-"));
    }

    #[test]
    fn test_trace_id_from_string() {
        let trace_id = TraceId::from_string("custom-trace-123");
        assert_eq!(trace_id.to_string(), "custom-trace-123");
        assert_eq!(trace_id.as_str(), "custom-trace-123");
    }

    #[test]
    fn test_trace_id_display() {
        let trace_id = TraceId::from_string("test-trace");
        assert_eq!(format!("{}", trace_id), "test-trace");
    }

    #[test]
    fn test_trace_id_from_str() {
        let trace_id: TraceId = "test-trace".into();
        assert_eq!(trace_id.to_string(), "test-trace");
    }

    #[test]
    fn test_correlation_id_new() {
        let correlation_id = CorrelationId::new();
        assert!(!correlation_id.to_string().is_empty());
        assert!(correlation_id.to_string().starts_with("corr-"));
    }

    #[test]
    fn test_correlation_id_from_string() {
        let correlation_id = CorrelationId::from_string("order-12345");
        assert_eq!(correlation_id.to_string(), "order-12345");
        assert_eq!(correlation_id.as_str(), "order-12345");
    }

    #[test]
    fn test_correlation_id_display() {
        let correlation_id = CorrelationId::from_string("test-correlation");
        assert_eq!(format!("{}", correlation_id), "test-correlation");
    }

    #[test]
    fn test_correlation_id_from_str() {
        let correlation_id: CorrelationId = "test-correlation".into();
        assert_eq!(correlation_id.to_string(), "test-correlation");
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn test_tracing_config_builder() {
        let config = TracingConfig::new()
            .with_service_name("test-service")
            .with_service_version("1.0.0")
            .with_environment("test")
            .with_otlp_endpoint("http://localhost:4317")
            .with_resource_attribute("custom.key", "custom.value")
            .with_console_exporter(true);

        assert_eq!(config.service_name, "test-service");
        assert_eq!(config.service_version, Some("1.0.0".to_string()));
        assert_eq!(config.environment, Some("test".to_string()));
        assert_eq!(
            config.otlp_endpoint,
            Some("http://localhost:4317".to_string())
        );
        assert_eq!(config.resource_attributes.len(), 1);
        assert_eq!(
            config.resource_attributes[0],
            ("custom.key".to_string(), "custom.value".to_string())
        );
        assert!(config.console_exporter);
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn test_tracing_config_default() {
        let config = TracingConfig::default();
        assert_eq!(config.service_name, "hammerwork");
        assert_eq!(config.service_version, None);
        assert_eq!(config.environment, None);
        assert_eq!(config.otlp_endpoint, None);
        assert_eq!(config.resource_attributes.len(), 0);
        assert!(!config.console_exporter);
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn test_create_job_span() {
        use crate::{Job, JobPriority};
        use serde_json::json;

        let job = Job::new("test_queue".to_string(), json!({"test": "data"}))
            .with_trace_id("trace-123")
            .with_correlation_id("corr-456")
            .with_parent_span_id("parent-789")
            .with_priority(JobPriority::High);

        let span = create_job_span(&job, "job.test");

        // Verify span was created successfully
        assert_eq!(
            span.metadata().expect("span should have metadata").name(),
            "job.process"
        );

        // The span should contain the job context as fields
        // We can't easily test the actual field values without more complex setup,
        // but we can verify the span was created without panicking
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn test_create_job_span_with_minimal_job() {
        use crate::Job;
        use serde_json::json;

        // Test with a job that has no tracing fields set
        let job = Job::new("minimal_queue".to_string(), json!({"data": "test"}));

        let span = create_job_span(&job, "job.minimal");

        // Should still create a span successfully even without tracing fields
        assert_eq!(
            span.metadata().expect("span should have metadata").name(),
            "job.process"
        );
    }

    #[cfg(feature = "tracing")]
    #[test]
    fn test_create_job_span_with_cron_job() {
        use crate::Job;
        use serde_json::json;

        let mut job = Job::new("cron_queue".to_string(), json!({"data": "test"}))
            .with_trace_id("trace-cron-123");

        // Set cron schedule
        job.cron_schedule = Some("0 0 * * *".to_string());

        let span = create_job_span(&job, "job.cron");

        // Should create span with cron schedule information
        assert_eq!(
            span.metadata().expect("span should have metadata").name(),
            "job.process"
        );
    }

    #[test]
    fn test_trace_id_uniqueness() {
        let trace_id1 = TraceId::new();
        let trace_id2 = TraceId::new();

        // Each generated trace ID should be unique
        assert_ne!(trace_id1.to_string(), trace_id2.to_string());
    }

    #[test]
    fn test_correlation_id_uniqueness() {
        let corr_id1 = CorrelationId::new();
        let corr_id2 = CorrelationId::new();

        // Each generated correlation ID should be unique
        assert_ne!(corr_id1.to_string(), corr_id2.to_string());
    }

    #[test]
    fn test_trace_id_equality() {
        let id1 = TraceId::from_string("test-id-123");
        let id2 = TraceId::from_string("test-id-123");
        let id3 = TraceId::from_string("different-id");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_correlation_id_equality() {
        let id1 = CorrelationId::from_string("order-12345");
        let id2 = CorrelationId::from_string("order-12345");
        let id3 = CorrelationId::from_string("order-67890");

        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
}
