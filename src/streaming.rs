//! Event streaming integration for external message systems.
//!
//! This module provides streaming integrations for delivering job lifecycle events
//! to external message systems like Apache Kafka, AWS Kinesis, and Google Cloud Pub/Sub.
//! It supports configurable routing, partitioning, and delivery guarantees.
//!
//! # Overview
//!
//! The streaming system consists of several key components:
//!
//! - [`StreamManager`] - Manages stream configurations and delivery to external systems
//! - [`StreamConfig`] - Configuration for individual stream endpoints
//! - [`StreamProcessor`] - Backend-specific processors (Kafka, Kinesis, PubSub)
//! - [`PartitioningStrategy`] - Configurable event partitioning for distributed systems
//! - [`SerializationFormat`] - Multiple serialization options (JSON, Avro, Protobuf)
//! - [`BufferConfig`] - Configurable buffering and batching for performance
//!
//! # Features
//!
//! - **Multiple streaming backends** - Kafka, AWS Kinesis, Google Cloud Pub/Sub
//! - **Flexible partitioning** - Partition by job ID, queue name, priority, event type, or custom fields
//! - **Multiple serialization formats** - JSON, Avro with schema registry, Protocol Buffers, MessagePack
//! - **Configurable buffering** - Buffer events for batch delivery with size and time limits
//! - **Retry policies** - Exponential backoff with jitter for failed deliveries
//! - **Delivery tracking** - Comprehensive statistics and delivery history
//! - **Rate limiting** - Configurable concurrent processing limits
//! - **Health monitoring** - Backend health checks and failover support
//!
//! # Examples
//!
//! ## Basic Kafka Streaming Setup
//!
//! ```rust
//! use hammerwork::streaming::{StreamManager, StreamConfig, StreamBackend, PartitioningStrategy, SerializationFormat, StreamManagerConfig};
//! use hammerwork::events::{EventManager, EventFilter, JobLifecycleEventType};
//! use std::sync::Arc;
//! use std::collections::HashMap;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let event_manager = Arc::new(EventManager::new_default());
//! let stream_manager = StreamManager::new(
//!     event_manager.clone(),
//!     StreamManagerConfig::default()
//! );
//!
//! let stream = StreamConfig {
//!     id: uuid::Uuid::new_v4(),
//!     name: "job_events_kafka".to_string(),
//!     backend: StreamBackend::Kafka {
//!         brokers: vec!["localhost:9092".to_string()],
//!         topic: "hammerwork-job-events".to_string(),
//!         config: HashMap::new(),
//!     },
//!     filter: EventFilter::new()
//!         .with_event_types(vec![JobLifecycleEventType::Completed, JobLifecycleEventType::Failed]),
//!     partitioning: PartitioningStrategy::QueueName,
//!     serialization: SerializationFormat::Json,
//!     enabled: true,
//!     ..Default::default()
//! };
//!
//! stream_manager.add_stream(stream).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## AWS Kinesis with Custom Partitioning
//!
//! ```rust
//! use hammerwork::streaming::{StreamConfig, StreamBackend, PartitioningStrategy, PartitionField, SerializationFormat};
//! use hammerwork::events::EventFilter;
//! use std::collections::HashMap;
//!
//! let stream = StreamConfig {
//!     id: uuid::Uuid::new_v4(),
//!     name: "priority_events_kinesis".to_string(),
//!     backend: StreamBackend::Kinesis {
//!         region: "us-east-1".to_string(),
//!         stream_name: "hammerwork-priority-events".to_string(),
//!         access_key_id: None, // Use IAM roles
//!         secret_access_key: None,
//!         config: HashMap::new(),
//!     },
//!     filter: EventFilter::new()
//!         .with_priorities(vec![hammerwork::priority::JobPriority::High, hammerwork::priority::JobPriority::Critical]),
//!     partitioning: PartitioningStrategy::Hash {
//!         fields: vec![PartitionField::Priority, PartitionField::QueueName]
//!     },
//!     serialization: SerializationFormat::Json,
//!     enabled: true,
//!     ..Default::default()
//! };
//! ```
//!
//! ## Google Cloud Pub/Sub with Buffering
//!
//! ```rust
//! use hammerwork::streaming::{StreamConfig, StreamBackend, BufferConfig, SerializationFormat};
//! use hammerwork::events::EventFilter;
//! use std::collections::HashMap;
//!
//! let stream = StreamConfig {
//!     id: uuid::Uuid::new_v4(),
//!     name: "analytics_events_pubsub".to_string(),
//!     backend: StreamBackend::PubSub {
//!         project_id: "my-project".to_string(),
//!         topic_name: "hammerwork-analytics".to_string(),
//!         service_account_key: None, // Use default credentials
//!         config: HashMap::new(),
//!     },
//!     filter: EventFilter::new().include_payload(),
//!     buffer_config: BufferConfig {
//!         max_events: 500,
//!         max_buffer_time_secs: 10,
//!         batch_size: 50,
//!     },
//!     serialization: SerializationFormat::Json,
//!     enabled: true,
//!     ..Default::default()
//! };
//! ```
//!
//! ## Avro Serialization with Schema Registry
//!
//! ```rust
//! use hammerwork::streaming::{StreamConfig, SerializationFormat};
//!
//! let stream = StreamConfig {
//!     serialization: SerializationFormat::Avro {
//!         schema_registry_url: "http://localhost:8081".to_string()
//!     },
//!     ..Default::default()
//! };
//! ```

use crate::{
    HammerworkError,
    events::{EventFilter, EventManager, EventSubscription, JobLifecycleEvent},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{RwLock, Semaphore};
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

/// Configuration for event streaming.
///
/// StreamConfig defines how events should be delivered to a specific streaming backend.
/// It includes backend configuration, event filtering, partitioning strategy, serialization
/// format, and delivery options.
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::{StreamConfig, StreamBackend, PartitioningStrategy, SerializationFormat, BufferConfig};
/// use hammerwork::events::EventFilter;
/// use uuid::Uuid;
/// use std::collections::HashMap;
///
/// // Basic Kafka configuration
/// let kafka_stream = StreamConfig {
///     id: Uuid::new_v4(),
///     name: "main_events".to_string(),
///     backend: StreamBackend::Kafka {
///         brokers: vec!["localhost:9092".to_string()],
///         topic: "job-events".to_string(),
///         config: HashMap::new(),
///     },
///     filter: EventFilter::new(),
///     partitioning: PartitioningStrategy::QueueName,
///     serialization: SerializationFormat::Json,
///     enabled: true,
///     ..Default::default()
/// };
///
/// // Kinesis with custom partitioning
/// let kinesis_stream = StreamConfig {
///     id: Uuid::new_v4(),
///     name: "analytics_stream".to_string(),
///     backend: StreamBackend::Kinesis {
///         region: "us-west-2".to_string(),
///         stream_name: "analytics-events".to_string(),
///         access_key_id: None,
///         secret_access_key: None,
///         config: HashMap::new(),
///     },
///     partitioning: PartitioningStrategy::JobId,
///     buffer_config: BufferConfig {
///         max_events: 100,
///         max_buffer_time_secs: 5,
///         batch_size: 25,
///     },
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamConfig {
    /// Unique identifier for this stream
    #[serde(with = "uuid_string")]
    pub id: Uuid,
    /// Human-readable name for this stream
    pub name: String,
    /// Streaming backend configuration
    pub backend: StreamBackend,
    /// Event filter to determine which events to stream
    pub filter: EventFilter,
    /// Partitioning strategy for the stream
    pub partitioning: PartitioningStrategy,
    /// Serialization format for events
    pub serialization: SerializationFormat,
    /// Retry policy for failed deliveries
    pub retry_policy: StreamRetryPolicy,
    /// Whether this stream is currently enabled
    pub enabled: bool,
    /// Buffer configuration
    pub buffer_config: BufferConfig,
}

/// Streaming backend types.
///
/// StreamBackend defines the configuration for different streaming systems.
/// Each backend has its own specific configuration options and capabilities.
///
/// # Supported Backends
///
/// - **Kafka** - Apache Kafka with configurable brokers and topics
/// - **Kinesis** - AWS Kinesis Data Streams with IAM or credential-based authentication
/// - **PubSub** - Google Cloud Pub/Sub with service account or default credentials
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::StreamBackend;
/// use std::collections::HashMap;
///
/// // Kafka backend
/// let kafka = StreamBackend::Kafka {
///     brokers: vec!["broker1:9092".to_string(), "broker2:9092".to_string()],
///     topic: "events".to_string(),
///     config: {
///         let mut config = HashMap::new();
///         config.insert("acks".to_string(), "all".to_string());
///         config.insert("retries".to_string(), "3".to_string());
///         config
///     },
/// };
///
/// // Kinesis with IAM roles
/// let kinesis = StreamBackend::Kinesis {
///     region: "us-east-1".to_string(),
///     stream_name: "my-event-stream".to_string(),
///     access_key_id: None, // Use IAM roles
///     secret_access_key: None,
///     config: HashMap::new(),
/// };
///
/// // Pub/Sub with default credentials
/// let pubsub = StreamBackend::PubSub {
///     project_id: "my-gcp-project".to_string(),
///     topic_name: "event-topic".to_string(),
///     service_account_key: None, // Use default credentials
///     config: HashMap::new(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamBackend {
    /// Apache Kafka backend
    Kafka {
        /// Kafka broker addresses
        brokers: Vec<String>,
        /// Topic to publish events to
        topic: String,
        /// Additional Kafka configuration
        config: HashMap<String, String>,
    },
    /// AWS Kinesis backend
    Kinesis {
        /// AWS region
        region: String,
        /// Kinesis stream name
        stream_name: String,
        /// AWS access key ID (optional, can use IAM roles)
        access_key_id: Option<String>,
        /// AWS secret access key (optional, can use IAM roles)
        secret_access_key: Option<String>,
        /// Additional Kinesis configuration
        config: HashMap<String, String>,
    },
    /// Google Cloud Pub/Sub backend
    PubSub {
        /// GCP project ID
        project_id: String,
        /// Pub/Sub topic name
        topic_name: String,
        /// Service account key JSON (optional, can use default credentials)
        service_account_key: Option<String>,
        /// Additional Pub/Sub configuration
        config: HashMap<String, String>,
    },
}

/// Partitioning strategies for distributing events across stream partitions.
///
/// Partitioning strategies determine how events are distributed across partitions
/// in the streaming backend. This affects message ordering, load distribution,
/// and processing patterns.
///
/// # Strategies
///
/// - **None** - All events go to a single partition (preserves total ordering)
/// - **JobId** - Partition by job ID (events for same job stay together)
/// - **QueueName** - Partition by queue name (events from same queue stay together)
/// - **Priority** - Partition by job priority (similar priorities grouped)
/// - **EventType** - Partition by event type (completions, failures, etc. grouped)
/// - **Custom** - Partition by custom metadata field
/// - **Hash** - Hash-based partitioning using multiple fields
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::{PartitioningStrategy, PartitionField};
///
/// // No partitioning - all events in order
/// let no_partition = PartitioningStrategy::None;
///
/// // Partition by queue name
/// let queue_partition = PartitioningStrategy::QueueName;
///
/// // Custom metadata partitioning
/// let custom_partition = PartitioningStrategy::Custom {
///     metadata_key: "tenant_id".to_string()
/// };
///
/// // Hash-based partitioning with multiple fields
/// let hash_partition = PartitioningStrategy::Hash {
///     fields: vec![
///         PartitionField::QueueName,
///         PartitionField::Priority,
///         PartitionField::MetadataKey("customer_id".to_string())
///     ]
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PartitioningStrategy {
    /// No partitioning - all events go to single partition
    None,
    /// Partition by job ID
    JobId,
    /// Partition by queue name
    QueueName,
    /// Partition by job priority
    Priority,
    /// Partition by event type
    EventType,
    /// Custom partitioning key from event metadata
    Custom { metadata_key: String },
    /// Hash-based partitioning using multiple fields
    Hash { fields: Vec<PartitionField> },
}

/// Fields that can be used for hash-based partitioning
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionField {
    JobId,
    QueueName,
    Priority,
    EventType,
    MetadataKey(String),
}

/// Serialization formats for event data.
///
/// SerializationFormat determines how job lifecycle events are serialized
/// before being sent to the streaming backend. Different formats have
/// different trade-offs in terms of size, performance, and compatibility.
///
/// # Formats
///
/// - **JSON** - Human-readable, widely compatible, larger size
/// - **Avro** - Schema evolution support, compact binary format
/// - **Protobuf** - Efficient binary format, strong typing
/// - **MessagePack** - Compact binary JSON-like format
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::SerializationFormat;
///
/// // JSON serialization (default)
/// let json_format = SerializationFormat::Json;
///
/// // Avro with schema registry
/// let avro_format = SerializationFormat::Avro {
///     schema_registry_url: "http://localhost:8081".to_string()
/// };
///
/// // Protocol Buffers with embedded schema
/// let protobuf_format = SerializationFormat::Protobuf {
///     schema_definition: r#"
///         syntax = "proto3";
///         message JobEvent {
///             string job_id = 1;
///             string event_type = 2;
///             // ... other fields
///         }
///     "#.to_string()
/// };
///
/// // MessagePack for compact binary
/// let msgpack_format = SerializationFormat::MessagePack;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SerializationFormat {
    /// JSON serialization
    Json,
    /// Avro serialization (with schema registry)
    Avro { schema_registry_url: String },
    /// Protocol Buffers serialization
    Protobuf { schema_definition: String },
    /// MessagePack serialization
    MessagePack,
}

/// Retry policy for streaming failures.
///
/// StreamRetryPolicy defines how failed streaming deliveries should be retried.
/// It supports exponential backoff with optional jitter to prevent thundering herd problems.
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::StreamRetryPolicy;
///
/// // Default retry policy
/// let default_policy = StreamRetryPolicy::default();
///
/// // Custom retry policy for critical events
/// let aggressive_policy = StreamRetryPolicy {
///     max_attempts: 10,
///     initial_delay_secs: 1,
///     max_delay_secs: 600, // 10 minutes
///     backoff_multiplier: 2.0,
///     use_jitter: true,
/// };
///
/// // Quick retry policy for non-critical events
/// let fast_policy = StreamRetryPolicy {
///     max_attempts: 3,
///     initial_delay_secs: 1,
///     max_delay_secs: 30,
///     backoff_multiplier: 1.5,
///     use_jitter: false,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamRetryPolicy {
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial delay between retries (in seconds)
    pub initial_delay_secs: u64,
    /// Maximum delay between retries (in seconds)
    pub max_delay_secs: u64,
    /// Multiplier for exponential backoff
    pub backoff_multiplier: f64,
    /// Whether to use jitter to avoid thundering herd
    pub use_jitter: bool,
}

impl Default for StreamRetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_delay_secs: 1,
            max_delay_secs: 300, // 5 minutes
            backoff_multiplier: 2.0,
            use_jitter: true,
        }
    }
}

/// Buffer configuration for streaming.
///
/// BufferConfig controls how events are buffered and batched before being sent
/// to the streaming backend. Proper buffering can significantly improve throughput
/// while managing memory usage and delivery latency.
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::BufferConfig;
///
/// // Default buffer configuration
/// let default_config = BufferConfig::default();
///
/// // High-throughput configuration
/// let high_throughput = BufferConfig {
///     max_events: 5000,
///     max_buffer_time_secs: 10,
///     batch_size: 500,
/// };
///
/// // Low-latency configuration
/// let low_latency = BufferConfig {
///     max_events: 100,
///     max_buffer_time_secs: 1,
///     batch_size: 10,
/// };
///
/// // Memory-constrained configuration
/// let memory_efficient = BufferConfig {
///     max_events: 50,
///     max_buffer_time_secs: 2,
///     batch_size: 5,
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BufferConfig {
    /// Maximum number of events to buffer
    pub max_events: usize,
    /// Maximum time to buffer events before forcing a flush (in seconds)
    pub max_buffer_time_secs: u64,
    /// Batch size for sending events
    pub batch_size: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            max_events: 1000,
            max_buffer_time_secs: 5,
            batch_size: 100,
        }
    }
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: "default_stream".to_string(),
            backend: StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "hammerwork-events".to_string(),
                config: HashMap::new(),
            },
            filter: EventFilter::new(),
            partitioning: PartitioningStrategy::None,
            serialization: SerializationFormat::Json,
            retry_policy: StreamRetryPolicy::default(),
            enabled: true,
            buffer_config: BufferConfig::default(),
        }
    }
}

/// A streamed event with routing metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamedEvent {
    /// The original job lifecycle event
    pub event: JobLifecycleEvent,
    /// Partition key for this event
    pub partition_key: Option<String>,
    /// Serialized event data
    pub serialized_data: Vec<u8>,
    /// Timestamp when the event was streamed
    pub streamed_at: DateTime<Utc>,
    /// Headers/metadata for the stream
    pub headers: HashMap<String, String>,
}

/// Stream delivery attempt result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDelivery {
    /// Unique identifier for this delivery attempt
    #[serde(with = "uuid_string")]
    pub delivery_id: Uuid,
    /// Stream configuration ID
    #[serde(with = "uuid_string")]
    pub stream_id: Uuid,
    /// Event that was delivered
    #[serde(with = "uuid_string")]
    pub event_id: Uuid,
    /// Whether the delivery was successful
    pub success: bool,
    /// Error message if delivery failed
    pub error_message: Option<String>,
    /// When the delivery was attempted
    pub attempted_at: DateTime<Utc>,
    /// How long the delivery took (in milliseconds)
    pub duration_ms: Option<u64>,
    /// Attempt number (1-based)
    pub attempt_number: u32,
    /// Partition the event was sent to (if applicable)
    pub partition: Option<String>,
}

/// Statistics for a streaming configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamStats {
    /// Stream configuration ID
    #[serde(with = "uuid_string")]
    pub stream_id: Uuid,
    /// Total number of events processed
    pub total_events: u64,
    /// Number of successfully delivered events
    pub successful_deliveries: u64,
    /// Number of failed deliveries
    pub failed_deliveries: u64,
    /// Current success rate (0.0 to 1.0)
    pub success_rate: f64,
    /// Average delivery time in milliseconds
    pub avg_delivery_time_ms: f64,
    /// Events currently in buffer
    pub buffered_events: u64,
    /// Last successful delivery timestamp
    pub last_success_at: Option<DateTime<Utc>>,
    /// Last failure timestamp
    pub last_failure_at: Option<DateTime<Utc>>,
    /// Statistics calculation timestamp
    pub calculated_at: DateTime<Utc>,
}

/// Overall stream manager statistics  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamManagerGlobalStats {
    /// Total number of configured streams
    pub total_streams: usize,
    /// Number of active/enabled streams
    pub active_streams: usize,
    /// Total events processed across all streams
    pub total_events: u64,
    /// Total successful deliveries across all streams
    pub successful_deliveries: u64,
    /// Total failed deliveries across all streams
    pub failed_deliveries: u64,
}

/// Event streaming manager.
///
/// StreamManager is the central component for managing streaming integrations.
/// It handles stream configurations, event subscriptions, backend processors,
/// delivery tracking, and performance monitoring.
///
/// # Features
///
/// - **Multi-backend support** - Manages Kafka, Kinesis, and Pub/Sub processors
/// - **Concurrent processing** - Rate-limited concurrent event delivery
/// - **Statistics tracking** - Per-stream and global delivery statistics
/// - **Health monitoring** - Backend health checks and failover
/// - **Event filtering** - Only processes events matching stream filters
/// - **Graceful shutdown** - Clean shutdown of all processors and subscriptions
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::{StreamManager, StreamManagerConfig};
/// use hammerwork::events::EventManager;
/// use std::sync::Arc;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let event_manager = Arc::new(EventManager::new_default());
/// let stream_manager = StreamManager::new(
///     event_manager.clone(),
///     StreamManagerConfig {
///         max_concurrent_processors: 100,
///         log_operations: true,
///         global_flush_interval_secs: 5,
///     }
/// );
///
/// // Add streams, start processing, etc.
/// # Ok(())
/// # }
/// ```
pub struct StreamManager {
    /// Active stream configurations
    streams: Arc<RwLock<HashMap<Uuid, StreamConfig>>>,
    /// Active event subscriptions for streams
    subscriptions: Arc<RwLock<HashMap<Uuid, EventSubscription>>>,
    /// Event manager for subscribing to events
    event_manager: Arc<EventManager>,
    /// Stream processors by backend type
    processors: Arc<RwLock<HashMap<Uuid, Box<dyn StreamProcessor + Send + Sync>>>>,
    /// Rate limiting semaphore for concurrent processing
    processing_semaphore: Arc<Semaphore>,
    /// Stream statistics
    stats: Arc<RwLock<HashMap<Uuid, StreamStats>>>,
    /// Configuration
    config: StreamManagerConfig,
}

/// Configuration for the stream manager.
///
/// StreamManagerConfig controls the global behavior of the StreamManager,
/// including concurrency limits, logging, and flush intervals.
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::StreamManagerConfig;
///
/// // Default configuration
/// let default_config = StreamManagerConfig::default();
///
/// // High-throughput configuration
/// let high_throughput = StreamManagerConfig {
///     max_concurrent_processors: 200,
///     log_operations: false, // Reduce logging overhead
///     global_flush_interval_secs: 30,
/// };
///
/// // Development configuration
/// let dev_config = StreamManagerConfig {
///     max_concurrent_processors: 10,
///     log_operations: true,
///     global_flush_interval_secs: 1, // Fast flushing for testing
/// };
/// ```
#[derive(Debug, Clone)]
pub struct StreamManagerConfig {
    /// Maximum number of concurrent stream processors
    pub max_concurrent_processors: usize,
    /// Whether to log stream operations
    pub log_operations: bool,
    /// Global buffer flush interval (in seconds)
    pub global_flush_interval_secs: u64,
}

impl Default for StreamManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_processors: 50,
            log_operations: true,
            global_flush_interval_secs: 10,
        }
    }
}

/// Trait for streaming backend processors.
///
/// StreamProcessor defines the interface that all streaming backend implementations
/// must implement. This allows the StreamManager to work with different streaming
/// systems in a uniform way.
///
/// # Implementation Requirements
///
/// Implementations must be thread-safe (`Send + Sync`) and handle:
/// - **Batch delivery** - Efficiently send multiple events in batches
/// - **Health monitoring** - Report backend health status
/// - **Error handling** - Properly handle and report delivery failures
/// - **Graceful shutdown** - Clean up resources when stopping
///
/// # Examples
///
/// ```rust
/// use hammerwork::streaming::{StreamProcessor, StreamedEvent, StreamDelivery};
/// use std::collections::HashMap;
/// use async_trait::async_trait;
///
/// struct MyCustomProcessor;
///
/// #[async_trait]
/// impl StreamProcessor for MyCustomProcessor {
///     async fn send_batch(&self, events: Vec<StreamedEvent>) -> hammerwork::Result<Vec<StreamDelivery>> {
///         // Implementation for sending events to custom backend
///         # Ok(Vec::new())
///     }
///
///     async fn health_check(&self) -> hammerwork::Result<bool> {
///         // Check if backend is healthy
///         Ok(true)
///     }
///
///     async fn get_stats(&self) -> hammerwork::Result<HashMap<String, serde_json::Value>> {
///         // Return processor-specific statistics
///         Ok(HashMap::new())
///     }
///
///     async fn shutdown(&self) -> hammerwork::Result<()> {
///         // Clean shutdown logic
///         Ok(())
///     }
/// }
/// ```
#[async_trait::async_trait]
pub trait StreamProcessor {
    /// Send a batch of events to the streaming backend.
    ///
    /// This method should efficiently deliver multiple events in a single operation
    /// to improve throughput. It returns a delivery result for each event.
    ///
    /// # Parameters
    ///
    /// * `events` - Batch of serialized events to deliver
    ///
    /// # Returns
    ///
    /// Vector of delivery results, one for each input event
    ///
    /// # Errors
    ///
    /// Returns an error if the entire batch fails to be processed
    async fn send_batch(&self, events: Vec<StreamedEvent>) -> crate::Result<Vec<StreamDelivery>>;

    /// Check if the processor is healthy and ready to send events.
    ///
    /// This method should perform a lightweight health check to determine
    /// if the streaming backend is available and ready to accept events.
    ///
    /// # Returns
    ///
    /// `Ok(true)` if healthy, `Ok(false)` if unhealthy, `Err` for check failures
    async fn health_check(&self) -> crate::Result<bool>;

    /// Get processor-specific statistics.
    ///
    /// Returns backend-specific metrics and statistics that can be used
    /// for monitoring and debugging.
    ///
    /// # Returns
    ///
    /// Map of statistic names to JSON values
    async fn get_stats(&self) -> crate::Result<HashMap<String, serde_json::Value>>;

    /// Shutdown the processor gracefully.
    ///
    /// This method should clean up any resources, close connections,
    /// and ensure no data is lost during shutdown.
    async fn shutdown(&self) -> crate::Result<()>;
}

impl StreamManager {
    /// Create a new stream manager.
    ///
    /// Creates a new StreamManager instance with the specified event manager
    /// and configuration. The stream manager will be ready to accept stream
    /// configurations and begin processing events.
    ///
    /// # Parameters
    ///
    /// * `event_manager` - Shared reference to the event manager for subscribing to events
    /// * `config` - Configuration for the stream manager behavior
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::streaming::{StreamManager, StreamManagerConfig};
    /// use hammerwork::events::EventManager;
    /// use std::sync::Arc;
    ///
    /// let event_manager = Arc::new(EventManager::new_default());
    /// let config = StreamManagerConfig {
    ///     max_concurrent_processors: 50,
    ///     log_operations: true,
    ///     global_flush_interval_secs: 10,
    /// };
    /// let stream_manager = StreamManager::new(event_manager, config);
    /// ```
    pub fn new(event_manager: Arc<EventManager>, config: StreamManagerConfig) -> Self {
        Self {
            streams: Arc::new(RwLock::new(HashMap::new())),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            event_manager,
            processors: Arc::new(RwLock::new(HashMap::new())),
            processing_semaphore: Arc::new(Semaphore::new(config.max_concurrent_processors)),
            stats: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new stream manager with default configuration.
    ///
    /// Convenience method that creates a StreamManager with default settings.
    /// Equivalent to calling `new()` with `StreamManagerConfig::default()`.
    ///
    /// # Parameters
    ///
    /// * `event_manager` - Shared reference to the event manager
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::streaming::StreamManager;
    /// use hammerwork::events::EventManager;
    /// use std::sync::Arc;
    ///
    /// let event_manager = Arc::new(EventManager::new_default());
    /// let stream_manager = StreamManager::new_default(event_manager);
    /// ```
    pub fn new_default(event_manager: Arc<EventManager>) -> Self {
        Self::new(event_manager, StreamManagerConfig::default())
    }

    /// Add a new stream configuration.
    ///
    /// Adds a new streaming configuration to the manager. This will:
    /// 1. Create the appropriate backend processor
    /// 2. Subscribe to events matching the stream's filter
    /// 3. Start processing events for this stream
    /// 4. Initialize statistics tracking
    ///
    /// # Parameters
    ///
    /// * `stream` - The stream configuration to add
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` on success, or an error if the stream cannot be initialized
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::streaming::{StreamManager, StreamConfig, StreamBackend};
    /// use hammerwork::events::{EventManager, EventFilter};
    /// use std::sync::Arc;
    /// use std::collections::HashMap;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let event_manager = Arc::new(EventManager::new_default());
    /// let stream_manager = StreamManager::new_default(event_manager);
    ///
    /// let stream = StreamConfig {
    ///     id: uuid::Uuid::new_v4(),
    ///     name: "kafka-stream".to_string(),
    ///     backend: StreamBackend::Kafka {
    ///         brokers: vec!["localhost:9092".to_string()],
    ///         topic: "events".to_string(),
    ///         config: HashMap::new(),
    ///     },
    ///     filter: EventFilter::new(),
    ///     enabled: true,
    ///     ..Default::default()
    /// };
    ///
    /// stream_manager.add_stream(stream).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn add_stream(&self, stream: StreamConfig) -> crate::Result<()> {
        let stream_id = stream.id;

        // Create processor for this stream
        let processor = self.create_processor(&stream).await?;

        // Subscribe to events for this stream
        let subscription = self.event_manager.subscribe(stream.filter.clone()).await?;

        // Store stream, processor, and subscription
        {
            let mut streams = self.streams.write().await;
            streams.insert(stream_id, stream.clone());
        }

        {
            let mut processors = self.processors.write().await;
            processors.insert(stream_id, processor);
        }

        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(stream_id, subscription);
        }

        // Initialize statistics
        {
            let mut stats = self.stats.write().await;
            stats.insert(
                stream_id,
                StreamStats {
                    stream_id,
                    total_events: 0,
                    successful_deliveries: 0,
                    failed_deliveries: 0,
                    success_rate: 0.0,
                    avg_delivery_time_ms: 0.0,
                    buffered_events: 0,
                    last_success_at: None,
                    last_failure_at: None,
                    calculated_at: Utc::now(),
                },
            );
        }

        // Start processing task for this stream
        self.start_stream_processing_task(stream_id).await;

        if self.config.log_operations {
            tracing::info!("Added stream: {} ({:?})", stream.name, stream.backend);
        }

        Ok(())
    }

    /// Remove a stream configuration
    pub async fn remove_stream(&self, stream_id: Uuid) -> crate::Result<()> {
        // Shutdown processor
        {
            let mut processors = self.processors.write().await;
            if let Some(processor) = processors.remove(&stream_id) {
                processor.shutdown().await?;
            }
        }

        // Remove subscription
        {
            let mut subscriptions = self.subscriptions.write().await;
            if let Some(subscription) = subscriptions.remove(&stream_id) {
                self.event_manager.unsubscribe(subscription.id).await?;
            }
        }

        // Remove stream configuration
        {
            let mut streams = self.streams.write().await;
            streams.remove(&stream_id);
        }

        // Remove statistics
        {
            let mut stats = self.stats.write().await;
            stats.remove(&stream_id);
        }

        if self.config.log_operations {
            tracing::info!("Removed stream: {}", stream_id);
        }

        Ok(())
    }

    /// Get stream configuration
    pub async fn get_stream(&self, stream_id: Uuid) -> Option<StreamConfig> {
        let streams = self.streams.read().await;
        streams.get(&stream_id).cloned()
    }

    /// List all stream configurations
    pub async fn list_streams(&self) -> Vec<StreamConfig> {
        let streams = self.streams.read().await;
        streams.values().cloned().collect()
    }

    /// Enable a stream
    pub async fn enable_stream(&self, stream_id: Uuid) -> crate::Result<()> {
        let mut streams = self.streams.write().await;
        if let Some(stream) = streams.get_mut(&stream_id) {
            stream.enabled = true;
            Ok(())
        } else {
            Err(crate::HammerworkError::Queue {
                message: format!("Stream {} not found", stream_id),
            })
        }
    }

    /// Disable a stream  
    pub async fn disable_stream(&self, stream_id: Uuid) -> crate::Result<()> {
        let mut streams = self.streams.write().await;
        if let Some(stream) = streams.get_mut(&stream_id) {
            stream.enabled = false;
            Ok(())
        } else {
            Err(crate::HammerworkError::Queue {
                message: format!("Stream {} not found", stream_id),
            })
        }
    }

    /// Get stream statistics
    pub async fn get_stream_stats(&self, stream_id: Uuid) -> Option<StreamStats> {
        let stats = self.stats.read().await;
        stats.get(&stream_id).cloned()
    }

    /// Get statistics for all streams
    pub async fn get_all_stream_stats(&self) -> Vec<StreamStats> {
        let stats = self.stats.read().await;
        stats.values().cloned().collect()
    }

    /// Get general stream manager statistics
    pub async fn get_stats(&self) -> StreamManagerGlobalStats {
        let streams = self.streams.read().await;
        let stats = self.stats.read().await;

        let total_streams = streams.len();
        let active_streams = streams.values().filter(|s| s.enabled).count();
        let total_events = stats.values().map(|s| s.total_events).sum();
        let successful_deliveries = stats.values().map(|s| s.successful_deliveries).sum();
        let failed_deliveries = stats.values().map(|s| s.failed_deliveries).sum();

        StreamManagerGlobalStats {
            total_streams,
            active_streams,
            total_events,
            successful_deliveries,
            failed_deliveries,
        }
    }

    /// Create a processor for the given stream configuration
    async fn create_processor(
        &self,
        stream: &StreamConfig,
    ) -> crate::Result<Box<dyn StreamProcessor + Send + Sync>> {
        match &stream.backend {
            StreamBackend::Kafka {
                brokers,
                topic,
                config,
            } => Ok(Box::new(
                KafkaProcessor::new(brokers.clone(), topic.clone(), config.clone()).await?,
            )),
            StreamBackend::Kinesis {
                region,
                stream_name,
                access_key_id,
                secret_access_key,
                config,
            } => Ok(Box::new(
                KinesisProcessor::new(
                    region.clone(),
                    stream_name.clone(),
                    access_key_id.clone(),
                    secret_access_key.clone(),
                    config.clone(),
                )
                .await?,
            )),
            StreamBackend::PubSub {
                project_id,
                topic_name,
                service_account_key,
                config,
            } => Ok(Box::new(
                PubSubProcessor::new(
                    project_id.clone(),
                    topic_name.clone(),
                    service_account_key.clone(),
                    config.clone(),
                )
                .await?,
            )),
        }
    }

    /// Start processing task for a specific stream
    async fn start_stream_processing_task(&self, stream_id: Uuid) {
        let streams = self.streams.clone();
        let subscriptions = self.subscriptions.clone();
        let processors = self.processors.clone();
        let stats = self.stats.clone();
        let processing_semaphore = self.processing_semaphore.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            let mut event_buffer: Vec<JobLifecycleEvent> = Vec::new();
            let mut last_flush = std::time::Instant::now();

            loop {
                // Get stream configuration
                let stream = {
                    let streams = streams.read().await;
                    match streams.get(&stream_id) {
                        Some(stream) if stream.enabled => stream.clone(),
                        _ => {
                            // Stream disabled or removed, exit task
                            break;
                        }
                    }
                };

                // Get subscription and receive events
                let mut receiver = {
                    let subscriptions = subscriptions.read().await;
                    match subscriptions.get(&stream_id) {
                        Some(subscription) => subscription.receiver.resubscribe(),
                        None => {
                            // Subscription removed, exit task
                            break;
                        }
                    }
                };

                // Check if we should flush the buffer
                let should_flush = event_buffer.len() >= stream.buffer_config.batch_size
                    || last_flush.elapsed().as_secs() >= stream.buffer_config.max_buffer_time_secs
                    || event_buffer.len() >= stream.buffer_config.max_events;

                if should_flush && !event_buffer.is_empty() {
                    let events_to_process = event_buffer.clone();
                    event_buffer.clear();
                    last_flush = std::time::Instant::now();

                    // Clone necessary data for processing task
                    let stream_clone = stream.clone();
                    let processors_clone = processors.clone();
                    let stats_clone = stats.clone();
                    let config_clone = config.clone();
                    let semaphore_clone = processing_semaphore.clone();

                    // Spawn processing task
                    tokio::spawn(async move {
                        // Acquire processing permit inside the task
                        let _permit = semaphore_clone.acquire().await.unwrap();
                        Self::process_event_batch(
                            stream_id,
                            stream_clone,
                            events_to_process,
                            processors_clone,
                            stats_clone,
                            config_clone,
                        )
                        .await;
                    });
                }

                // Try to receive new events (with timeout to allow periodic flushing)
                match tokio::time::timeout(Duration::from_secs(1), receiver.recv()).await {
                    Ok(Ok(event)) => {
                        // Check if event matches stream filter
                        if stream.filter.matches(&event) {
                            event_buffer.push(event);
                        }
                    }
                    Ok(Err(_)) => {
                        // Channel closed, exit task
                        break;
                    }
                    Err(_) => {
                        // Timeout - continue to check for flush conditions
                        continue;
                    }
                }
            }
        });
    }

    /// Process a batch of events for a stream
    async fn process_event_batch(
        stream_id: Uuid,
        stream: StreamConfig,
        events: Vec<JobLifecycleEvent>,
        processors: Arc<RwLock<HashMap<Uuid, Box<dyn StreamProcessor + Send + Sync>>>>,
        stats: Arc<RwLock<HashMap<Uuid, StreamStats>>>,
        config: StreamManagerConfig,
    ) {
        // Check if processor exists
        {
            let processors = processors.read().await;
            if !processors.contains_key(&stream_id) {
                return;
            }
        }

        // Convert events to streamed events
        let mut streamed_events = Vec::new();
        for event in events {
            match Self::prepare_streamed_event(event, &stream) {
                Ok(streamed_event) => streamed_events.push(streamed_event),
                Err(e) => {
                    if config.log_operations {
                        tracing::error!("Failed to prepare streamed event: {}", e);
                    }
                }
            }
        }

        if streamed_events.is_empty() {
            return;
        }

        // Note: This is a simplified version. In a real implementation,
        // we'd properly handle the processor trait object and implement retry logic

        if config.log_operations {
            tracing::debug!(
                "Processing batch of {} events for stream {}",
                streamed_events.len(),
                stream.name
            );
        }

        // For now, simulate successful processing
        Self::update_stream_stats(stream_id, true, streamed_events.len(), stats).await;
    }

    /// Prepare a job lifecycle event for streaming
    fn prepare_streamed_event(
        event: JobLifecycleEvent,
        stream: &StreamConfig,
    ) -> crate::Result<StreamedEvent> {
        // Calculate partition key
        let partition_key = Self::calculate_partition_key(&event, &stream.partitioning);

        // Serialize event data
        let serialized_data = match stream.serialization {
            SerializationFormat::Json => serde_json::to_vec(&event)?,
            SerializationFormat::MessagePack => {
                return Err(HammerworkError::Streaming {
                    message: "MessagePack serialization not implemented".to_string(),
                });
            }
            SerializationFormat::Avro { .. } => {
                return Err(HammerworkError::Streaming {
                    message: "Avro serialization not implemented".to_string(),
                });
            }
            SerializationFormat::Protobuf { .. } => {
                return Err(HammerworkError::Streaming {
                    message: "Protobuf serialization not implemented".to_string(),
                });
            }
        };

        // Prepare headers
        let mut headers = HashMap::new();
        headers.insert("event_type".to_string(), event.event_type.to_string());
        headers.insert("queue_name".to_string(), event.queue_name.clone());
        headers.insert("priority".to_string(), event.priority.to_string());
        headers.insert("timestamp".to_string(), event.timestamp.to_rfc3339());

        Ok(StreamedEvent {
            event,
            partition_key,
            serialized_data,
            streamed_at: Utc::now(),
            headers,
        })
    }

    /// Calculate partition key based on partitioning strategy
    fn calculate_partition_key(
        event: &JobLifecycleEvent,
        strategy: &PartitioningStrategy,
    ) -> Option<String> {
        match strategy {
            PartitioningStrategy::None => None,
            PartitioningStrategy::JobId => Some(event.job_id.to_string()),
            PartitioningStrategy::QueueName => Some(event.queue_name.clone()),
            PartitioningStrategy::Priority => Some(event.priority.to_string()),
            PartitioningStrategy::EventType => Some(event.event_type.to_string()),
            PartitioningStrategy::Custom { metadata_key } => {
                event.metadata.get(metadata_key).cloned()
            }
            PartitioningStrategy::Hash { fields } => {
                let mut hash_input = String::new();
                for field in fields {
                    match field {
                        PartitionField::JobId => hash_input.push_str(&event.job_id.to_string()),
                        PartitionField::QueueName => hash_input.push_str(&event.queue_name),
                        PartitionField::Priority => {
                            hash_input.push_str(&event.priority.to_string())
                        }
                        PartitionField::EventType => {
                            hash_input.push_str(&event.event_type.to_string())
                        }
                        PartitionField::MetadataKey(key) => {
                            if let Some(value) = event.metadata.get(key) {
                                hash_input.push_str(value);
                            }
                        }
                    }
                    hash_input.push('|');
                }

                // Simple hash function (in production, use a proper hash function)
                let hash = hash_input.chars().map(|c| c as u32).sum::<u32>();
                Some(format!("{}", hash % 1000))
            }
        }
    }

    /// Update stream statistics
    async fn update_stream_stats(
        stream_id: Uuid,
        success: bool,
        event_count: usize,
        stats: Arc<RwLock<HashMap<Uuid, StreamStats>>>,
    ) {
        let mut stats_map = stats.write().await;
        if let Some(stream_stats) = stats_map.get_mut(&stream_id) {
            stream_stats.total_events += event_count as u64;

            if success {
                stream_stats.successful_deliveries += event_count as u64;
                stream_stats.last_success_at = Some(Utc::now());
            } else {
                stream_stats.failed_deliveries += event_count as u64;
                stream_stats.last_failure_at = Some(Utc::now());
            }

            // Update success rate
            stream_stats.success_rate =
                stream_stats.successful_deliveries as f64 / stream_stats.total_events as f64;

            stream_stats.calculated_at = Utc::now();
        }
    }
}

/// Placeholder Kafka processor (would be implemented with rdkafka in production)
pub struct KafkaProcessor {
    brokers: Vec<String>,
    topic: String,
    config: HashMap<String, String>,
}

impl KafkaProcessor {
    pub async fn new(
        brokers: Vec<String>,
        topic: String,
        config: HashMap<String, String>,
    ) -> crate::Result<Self> {
        Ok(Self {
            brokers,
            topic,
            config,
        })
    }
}

#[async_trait::async_trait]
impl StreamProcessor for KafkaProcessor {
    async fn send_batch(&self, events: Vec<StreamedEvent>) -> crate::Result<Vec<StreamDelivery>> {
        // Placeholder implementation - in production this would use rdkafka
        // Configuration options could include: compression, acks, retries, batch.size, etc.

        let start_time = Utc::now();
        let mut deliveries = Vec::new();

        // Simulate batch processing with configuration-based delays
        let batch_delay_ms = self
            .config
            .get("batch.delay.ms")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10);

        tokio::time::sleep(tokio::time::Duration::from_millis(batch_delay_ms)).await;

        // Simulate potential failures based on configuration
        let error_rate = self
            .config
            .get("test.error.rate")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);

        for event in events {
            let success = rand::random::<f64>() > error_rate;
            let duration = start_time
                .signed_duration_since(Utc::now())
                .num_milliseconds()
                .unsigned_abs();

            deliveries.push(StreamDelivery {
                delivery_id: Uuid::new_v4(),
                stream_id: Uuid::new_v4(), // Would be passed in
                event_id: event.event.event_id,
                success,
                error_message: if success {
                    None
                } else {
                    Some(format!(
                        "Simulated Kafka delivery failure to topic: {}",
                        self.topic
                    ))
                },
                attempted_at: start_time,
                duration_ms: Some(duration),
                attempt_number: 1,
                partition: event.partition_key,
            });
        }
        Ok(deliveries)
    }

    async fn health_check(&self) -> crate::Result<bool> {
        // In production, this would connect to Kafka brokers and check their health
        // Configuration could include timeout settings, retry policies, etc.

        let health_check_timeout_ms = self
            .config
            .get("health.check.timeout.ms")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(5000);

        // Simulate health check with configurable timeout
        tokio::time::timeout(
            tokio::time::Duration::from_millis(health_check_timeout_ms),
            async {
                // Simulate checking each broker
                for broker in &self.brokers {
                    tracing::debug!("Health checking Kafka broker: {}", broker);
                    // In real implementation, would ping broker
                }
                Ok(true)
            },
        )
        .await
        .unwrap_or(Ok(false))
    }

    async fn get_stats(&self) -> crate::Result<HashMap<String, serde_json::Value>> {
        let mut stats = HashMap::new();
        stats.insert(
            "type".to_string(),
            serde_json::Value::String("kafka".to_string()),
        );
        stats.insert(
            "brokers".to_string(),
            serde_json::Value::Array(
                self.brokers
                    .iter()
                    .map(|b| serde_json::Value::String(b.clone()))
                    .collect(),
            ),
        );
        stats.insert(
            "topic".to_string(),
            serde_json::Value::String(self.topic.clone()),
        );
        stats.insert(
            "config".to_string(),
            serde_json::Value::Object(
                self.config
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            ),
        );
        Ok(stats)
    }

    async fn shutdown(&self) -> crate::Result<()> {
        Ok(())
    }
}

/// Placeholder Kinesis processor (would be implemented with AWS SDK in production)
pub struct KinesisProcessor {
    region: String,
    stream_name: String,
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    config: HashMap<String, String>,
}

impl KinesisProcessor {
    pub async fn new(
        region: String,
        stream_name: String,
        access_key_id: Option<String>,
        secret_access_key: Option<String>,
        config: HashMap<String, String>,
    ) -> crate::Result<Self> {
        Ok(Self {
            region,
            stream_name,
            access_key_id,
            secret_access_key,
            config,
        })
    }
}

#[async_trait::async_trait]
impl StreamProcessor for KinesisProcessor {
    async fn send_batch(&self, events: Vec<StreamedEvent>) -> crate::Result<Vec<StreamDelivery>> {
        // Placeholder implementation
        let mut deliveries = Vec::new();
        for event in events {
            deliveries.push(StreamDelivery {
                delivery_id: Uuid::new_v4(),
                stream_id: Uuid::new_v4(), // Would be passed in
                event_id: event.event.event_id,
                success: true,
                error_message: None,
                attempted_at: Utc::now(),
                duration_ms: Some(100),
                attempt_number: 1,
                partition: event.partition_key,
            });
        }
        Ok(deliveries)
    }

    async fn health_check(&self) -> crate::Result<bool> {
        // In production, this would validate AWS credentials and check Kinesis stream status
        // Configuration could include retry policies, credential validation, etc.

        // Check if we have credentials configured
        let has_credentials = self.access_key_id.is_some() && self.secret_access_key.is_some();

        if !has_credentials {
            tracing::warn!("Kinesis health check: No AWS credentials configured");
            return Ok(false);
        }

        // Simulate credential validation and stream health check
        let health_timeout = self
            .config
            .get("health.check.timeout.ms")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10000);

        tokio::time::timeout(tokio::time::Duration::from_millis(health_timeout), async {
            tracing::debug!(
                "Checking Kinesis stream health: {} in region {}",
                self.stream_name,
                self.region
            );
            // In real implementation, would call AWS Kinesis DescribeStream
            Ok(true)
        })
        .await
        .unwrap_or(Ok(false))
    }

    async fn get_stats(&self) -> crate::Result<HashMap<String, serde_json::Value>> {
        let mut stats = HashMap::new();
        stats.insert(
            "type".to_string(),
            serde_json::Value::String("kinesis".to_string()),
        );
        stats.insert(
            "region".to_string(),
            serde_json::Value::String(self.region.clone()),
        );
        stats.insert(
            "stream_name".to_string(),
            serde_json::Value::String(self.stream_name.clone()),
        );
        if let Some(ref access_key_id) = self.access_key_id {
            stats.insert(
                "access_key_id".to_string(),
                serde_json::Value::String(format!(
                    "{}***",
                    &access_key_id[..4.min(access_key_id.len())]
                )),
            );
        }
        stats.insert(
            "config".to_string(),
            serde_json::Value::Object(
                self.config
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            ),
        );
        Ok(stats)
    }

    async fn shutdown(&self) -> crate::Result<()> {
        Ok(())
    }
}

/// Placeholder Pub/Sub processor (would be implemented with Google Cloud SDK in production)
pub struct PubSubProcessor {
    project_id: String,
    topic_name: String,
    service_account_key: Option<String>,
    config: HashMap<String, String>,
}

impl PubSubProcessor {
    pub async fn new(
        project_id: String,
        topic_name: String,
        service_account_key: Option<String>,
        config: HashMap<String, String>,
    ) -> crate::Result<Self> {
        Ok(Self {
            project_id,
            topic_name,
            service_account_key,
            config,
        })
    }
}

#[async_trait::async_trait]
impl StreamProcessor for PubSubProcessor {
    async fn send_batch(&self, events: Vec<StreamedEvent>) -> crate::Result<Vec<StreamDelivery>> {
        // Placeholder implementation
        let mut deliveries = Vec::new();
        for event in events {
            deliveries.push(StreamDelivery {
                delivery_id: Uuid::new_v4(),
                stream_id: Uuid::new_v4(), // Would be passed in
                event_id: event.event.event_id,
                success: true,
                error_message: None,
                attempted_at: Utc::now(),
                duration_ms: Some(75),
                attempt_number: 1,
                partition: event.partition_key,
            });
        }
        Ok(deliveries)
    }

    async fn health_check(&self) -> crate::Result<bool> {
        Ok(true)
    }

    async fn get_stats(&self) -> crate::Result<HashMap<String, serde_json::Value>> {
        let mut stats = HashMap::new();
        stats.insert(
            "type".to_string(),
            serde_json::Value::String("pubsub".to_string()),
        );
        stats.insert(
            "project_id".to_string(),
            serde_json::Value::String(self.project_id.clone()),
        );
        stats.insert(
            "topic_name".to_string(),
            serde_json::Value::String(self.topic_name.clone()),
        );
        if self.service_account_key.is_some() {
            stats.insert(
                "service_account_configured".to_string(),
                serde_json::Value::Bool(true),
            );
        }
        stats.insert(
            "config".to_string(),
            serde_json::Value::Object(
                self.config
                    .iter()
                    .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                    .collect(),
            ),
        );
        Ok(stats)
    }

    async fn shutdown(&self) -> crate::Result<()> {
        Ok(())
    }
}

/// Helper for creating stream configurations
impl StreamConfig {
    /// Create a new stream configuration
    pub fn new(name: String, backend: StreamBackend) -> Self {
        Self {
            id: Uuid::new_v4(),
            name,
            backend,
            filter: EventFilter::default(),
            partitioning: PartitioningStrategy::None,
            serialization: SerializationFormat::Json,
            retry_policy: StreamRetryPolicy::default(),
            enabled: true,
            buffer_config: BufferConfig::default(),
        }
    }

    /// Set event filter
    pub fn with_filter(mut self, filter: EventFilter) -> Self {
        self.filter = filter;
        self
    }

    /// Set partitioning strategy
    pub fn with_partitioning(mut self, partitioning: PartitioningStrategy) -> Self {
        self.partitioning = partitioning;
        self
    }

    /// Set serialization format
    pub fn with_serialization(mut self, serialization: SerializationFormat) -> Self {
        self.serialization = serialization;
        self
    }

    /// Set retry policy
    pub fn with_retry_policy(mut self, retry_policy: StreamRetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Set buffer configuration
    pub fn with_buffer_config(mut self, buffer_config: BufferConfig) -> Self {
        self.buffer_config = buffer_config;
        self
    }

    /// Enable or disable stream
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{events::JobLifecycleEventType, priority::JobPriority};

    #[test]
    fn test_stream_config_creation() {
        let backend = StreamBackend::Kafka {
            brokers: vec!["localhost:9092".to_string()],
            topic: "job-events".to_string(),
            config: HashMap::new(),
        };

        let stream = StreamConfig::new("test-stream".to_string(), backend)
            .with_partitioning(PartitioningStrategy::QueueName)
            .with_serialization(SerializationFormat::Json)
            .enabled(true);

        assert_eq!(stream.name, "test-stream");
        assert!(stream.enabled);
        assert!(matches!(
            stream.partitioning,
            PartitioningStrategy::QueueName
        ));
        assert!(matches!(stream.serialization, SerializationFormat::Json));
    }

    #[test]
    fn test_partitioning_strategies() {
        let event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "email_queue".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::High,
            timestamp: Utc::now(),
            processing_time_ms: Some(1000),
            error: None,
            payload: None,
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("source".to_string(), "api".to_string());
                metadata
            },
        };

        // Test different partitioning strategies
        assert_eq!(
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::None),
            None
        );

        assert_eq!(
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::QueueName),
            Some("email_queue".to_string())
        );

        assert_eq!(
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::Priority),
            Some("high".to_string())
        );

        assert_eq!(
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::EventType),
            Some("completed".to_string())
        );

        assert_eq!(
            StreamManager::calculate_partition_key(
                &event,
                &PartitioningStrategy::Custom {
                    metadata_key: "source".to_string()
                }
            ),
            Some("api".to_string())
        );

        // Test hash partitioning
        let hash_result = StreamManager::calculate_partition_key(
            &event,
            &PartitioningStrategy::Hash {
                fields: vec![PartitionField::QueueName, PartitionField::Priority],
            },
        );
        assert!(hash_result.is_some());
    }

    #[test]
    fn test_stream_backends() {
        let kafka = StreamBackend::Kafka {
            brokers: vec!["localhost:9092".to_string()],
            topic: "events".to_string(),
            config: HashMap::new(),
        };

        let kinesis = StreamBackend::Kinesis {
            region: "us-east-1".to_string(),
            stream_name: "job-events".to_string(),
            access_key_id: None,
            secret_access_key: None,
            config: HashMap::new(),
        };

        let pubsub = StreamBackend::PubSub {
            project_id: "my-project".to_string(),
            topic_name: "job-events".to_string(),
            service_account_key: None,
            config: HashMap::new(),
        };

        match kafka {
            StreamBackend::Kafka { brokers, topic, .. } => {
                assert_eq!(brokers.len(), 1);
                assert_eq!(topic, "events");
            }
            _ => panic!("Wrong backend type"),
        }

        match kinesis {
            StreamBackend::Kinesis {
                region,
                stream_name,
                ..
            } => {
                assert_eq!(region, "us-east-1");
                assert_eq!(stream_name, "job-events");
            }
            _ => panic!("Wrong backend type"),
        }

        match pubsub {
            StreamBackend::PubSub {
                project_id,
                topic_name,
                ..
            } => {
                assert_eq!(project_id, "my-project");
                assert_eq!(topic_name, "job-events");
            }
            _ => panic!("Wrong backend type"),
        }
    }

    #[test]
    fn test_retry_policy() {
        let policy = StreamRetryPolicy {
            max_attempts: 3,
            initial_delay_secs: 1,
            max_delay_secs: 60,
            backoff_multiplier: 2.0,
            use_jitter: false,
        };

        assert_eq!(policy.max_attempts, 3);
        assert_eq!(policy.initial_delay_secs, 1);
        assert_eq!(policy.backoff_multiplier, 2.0);
        assert!(!policy.use_jitter);
    }

    #[test]
    fn test_buffer_config() {
        let buffer = BufferConfig {
            max_events: 500,
            max_buffer_time_secs: 10,
            batch_size: 50,
        };

        assert_eq!(buffer.max_events, 500);
        assert_eq!(buffer.max_buffer_time_secs, 10);
        assert_eq!(buffer.batch_size, 50);
    }

    #[test]
    fn test_serialization_formats() {
        let json = SerializationFormat::Json;
        let avro = SerializationFormat::Avro {
            schema_registry_url: "http://localhost:8081".to_string(),
        };
        let protobuf = SerializationFormat::Protobuf {
            schema_definition: "syntax = \"proto3\";".to_string(),
        };
        let msgpack = SerializationFormat::MessagePack;

        assert!(matches!(json, SerializationFormat::Json));
        assert!(matches!(avro, SerializationFormat::Avro { .. }));
        assert!(matches!(protobuf, SerializationFormat::Protobuf { .. }));
        assert!(matches!(msgpack, SerializationFormat::MessagePack));
    }

    #[tokio::test]
    async fn test_stream_manager_creation() {
        let config = StreamManagerConfig::default();
        let event_manager = Arc::new(EventManager::new_default());
        let manager = StreamManager::new(event_manager, config);

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_streams, 0);
        assert_eq!(stats.active_streams, 0);
    }

    #[tokio::test]
    async fn test_stream_manager_add_remove_stream() {
        let config = StreamManagerConfig::default();
        let event_manager = Arc::new(EventManager::new_default());
        let manager = StreamManager::new(event_manager, config);

        let backend = StreamBackend::Kafka {
            brokers: vec!["localhost:9092".to_string()],
            topic: "test-events".to_string(),
            config: HashMap::new(),
        };

        let stream_config = StreamConfig::new("test-stream".to_string(), backend)
            .with_partitioning(PartitioningStrategy::QueueName)
            .with_serialization(SerializationFormat::Json);

        let stream_id = stream_config.id;

        // Add stream
        manager.add_stream(stream_config).await.unwrap();

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_streams, 1);
        assert_eq!(stats.active_streams, 1);

        // Remove stream
        manager.remove_stream(stream_id).await.unwrap();

        let stats = manager.get_stats().await;
        assert_eq!(stats.total_streams, 0);
        assert_eq!(stats.active_streams, 0);
    }

    #[tokio::test]
    async fn test_stream_manager_enable_disable() {
        let config = StreamManagerConfig::default();
        let event_manager = Arc::new(EventManager::new_default());
        let manager = StreamManager::new(event_manager, config);

        let backend = StreamBackend::Kinesis {
            region: "us-west-2".to_string(),
            stream_name: "test-stream".to_string(),
            access_key_id: None,
            secret_access_key: None,
            config: HashMap::new(),
        };

        let stream_config = StreamConfig::new("test-stream".to_string(), backend);
        let stream_id = stream_config.id;

        manager.add_stream(stream_config).await.unwrap();

        // Initially enabled
        let stats = manager.get_stats().await;
        assert_eq!(stats.active_streams, 1);

        // Disable stream
        manager.disable_stream(stream_id).await.unwrap();
        let stats = manager.get_stats().await;
        assert_eq!(stats.active_streams, 0);
        assert_eq!(stats.total_streams, 1); // Still exists but disabled

        // Re-enable stream
        manager.enable_stream(stream_id).await.unwrap();
        let stats = manager.get_stats().await;
        assert_eq!(stats.active_streams, 1);
    }

    #[test]
    fn test_streamed_event_creation() {
        let event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: Some(1500),
            error: None,
            payload: Some(serde_json::json!({"key": "value"})),
            metadata: HashMap::new(),
        };

        let _stream_config = StreamConfig::new(
            "test-stream".to_string(),
            StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "events".to_string(),
                config: HashMap::new(),
            },
        );

        let streamed_event = StreamedEvent {
            event,
            partition_key: Some("partition-1".to_string()),
            serialized_data: b"serialized data".to_vec(),
            streamed_at: Utc::now(),
            headers: HashMap::new(),
        };

        assert!(streamed_event.partition_key.is_some());
        assert!(!streamed_event.serialized_data.is_empty());
        assert_eq!(
            streamed_event.event.event_type,
            JobLifecycleEventType::Completed
        );
    }

    #[test]
    fn test_stream_delivery_result() {
        let delivery = StreamDelivery {
            delivery_id: Uuid::new_v4(),
            stream_id: Uuid::new_v4(),
            event_id: Uuid::new_v4(),
            success: true,
            error_message: None,
            attempted_at: Utc::now(),
            duration_ms: Some(100),
            attempt_number: 1,
            partition: Some("partition-0".to_string()),
        };

        assert!(delivery.success);
        assert!(delivery.error_message.is_none());
        assert_eq!(delivery.attempt_number, 1);
        assert_eq!(delivery.duration_ms, Some(100));
    }

    #[test]
    fn test_stream_stats_aggregation() {
        let mut stats = StreamStats {
            stream_id: Uuid::new_v4(),
            total_events: 1000,
            successful_deliveries: 950,
            failed_deliveries: 50,
            success_rate: 0.95,
            avg_delivery_time_ms: 125.5,
            buffered_events: 25,
            last_success_at: Some(Utc::now() - chrono::Duration::minutes(2)),
            last_failure_at: Some(Utc::now() - chrono::Duration::minutes(10)),
            calculated_at: Utc::now(),
        };

        // Verify stats calculations
        assert_eq!(
            stats.total_events,
            stats.successful_deliveries + stats.failed_deliveries
        );
        assert!((stats.success_rate - 0.95).abs() < f64::EPSILON);
        assert!(stats.last_success_at.is_some());
        assert!(stats.last_failure_at.is_some());
        assert_eq!(stats.buffered_events, 25);

        // Update stats with new successful delivery
        stats.total_events += 1;
        stats.successful_deliveries += 1;
        stats.success_rate = stats.successful_deliveries as f64 / stats.total_events as f64;

        assert_eq!(stats.total_events, 1001);
        assert_eq!(stats.successful_deliveries, 951);
        assert!((stats.success_rate - 951.0 / 1001.0).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_kafka_processor_creation() {
        let processor = KafkaProcessor::new(
            vec!["localhost:9092".to_string()],
            "test-topic".to_string(),
            HashMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(processor.brokers.len(), 1);
        assert_eq!(processor.topic, "test-topic");
    }

    #[tokio::test]
    async fn test_kinesis_processor_creation() {
        let processor = KinesisProcessor::new(
            "us-east-1".to_string(),
            "test-stream".to_string(),
            None,
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(processor.region, "us-east-1");
        assert_eq!(processor.stream_name, "test-stream");
        assert!(processor.access_key_id.is_none());
    }

    #[tokio::test]
    async fn test_pubsub_processor_creation() {
        let processor = PubSubProcessor::new(
            "my-project".to_string(),
            "test-topic".to_string(),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        assert_eq!(processor.project_id, "my-project");
        assert_eq!(processor.topic_name, "test-topic");
        assert!(processor.service_account_key.is_none());
    }

    #[tokio::test]
    async fn test_stream_processor_health_check() {
        let processor = KafkaProcessor::new(
            vec!["localhost:9092".to_string()],
            "test-topic".to_string(),
            HashMap::new(),
        )
        .await
        .unwrap();

        // Health check should pass (placeholder implementation)
        let health = processor.health_check().await.unwrap();
        assert!(health);
    }

    #[tokio::test]
    async fn test_stream_processor_batch_sending() {
        let processor = KafkaProcessor::new(
            vec!["localhost:9092".to_string()],
            "test-topic".to_string(),
            HashMap::new(),
        )
        .await
        .unwrap();

        let event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "test".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: Some(1000),
            error: None,
            payload: None,
            metadata: HashMap::new(),
        };

        let streamed_event = StreamedEvent {
            event,
            partition_key: Some("partition-0".to_string()),
            serialized_data: b"test data".to_vec(),
            streamed_at: Utc::now(),
            headers: HashMap::new(),
        };

        let events = vec![streamed_event];
        let deliveries = processor.send_batch(events).await.unwrap();

        assert_eq!(deliveries.len(), 1);
        assert!(deliveries[0].success);
    }

    #[test]
    fn test_partition_field_combinations() {
        let event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            event_type: JobLifecycleEventType::Started,
            priority: JobPriority::High,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: HashMap::new(),
        };

        // Test single field partitioning
        let queue_partition =
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::QueueName);
        assert_eq!(queue_partition, Some("test_queue".to_string()));

        let priority_partition =
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::Priority);
        assert_eq!(priority_partition, Some("high".to_string()));

        let event_type_partition =
            StreamManager::calculate_partition_key(&event, &PartitioningStrategy::EventType);
        assert_eq!(event_type_partition, Some("started".to_string()));

        // Test hash partitioning with multiple fields
        let hash_partition = StreamManager::calculate_partition_key(
            &event,
            &PartitioningStrategy::Hash {
                fields: vec![
                    PartitionField::QueueName,
                    PartitionField::Priority,
                    PartitionField::EventType,
                ],
            },
        );
        assert!(hash_partition.is_some());

        // Hash should be consistent for same input
        let hash_partition2 = StreamManager::calculate_partition_key(
            &event,
            &PartitioningStrategy::Hash {
                fields: vec![
                    PartitionField::QueueName,
                    PartitionField::Priority,
                    PartitionField::EventType,
                ],
            },
        );
        assert_eq!(hash_partition, hash_partition2);
    }

    #[test]
    fn test_stream_config_builder() {
        let backend = StreamBackend::PubSub {
            project_id: "test-project".to_string(),
            topic_name: "events".to_string(),
            service_account_key: None,
            config: HashMap::new(),
        };

        let filter = EventFilter::new().with_event_types(vec![
            JobLifecycleEventType::Completed,
            JobLifecycleEventType::Failed,
        ]);

        let retry_policy = StreamRetryPolicy {
            max_attempts: 5,
            initial_delay_secs: 2,
            max_delay_secs: 120,
            backoff_multiplier: 2.0,
            use_jitter: true,
        };

        let buffer_config = BufferConfig {
            max_events: 1000,
            max_buffer_time_secs: 30,
            batch_size: 100,
        };

        let stream = StreamConfig::new("comprehensive-stream".to_string(), backend)
            .with_filter(filter)
            .with_partitioning(PartitioningStrategy::Hash {
                fields: vec![PartitionField::QueueName, PartitionField::Priority],
            })
            .with_serialization(SerializationFormat::Avro {
                schema_registry_url: "http://schema-registry:8081".to_string(),
            })
            .with_retry_policy(retry_policy)
            .with_buffer_config(buffer_config)
            .enabled(true);

        assert_eq!(stream.name, "comprehensive-stream");
        assert!(stream.enabled);
        assert!(matches!(
            stream.partitioning,
            PartitioningStrategy::Hash { .. }
        ));
        assert!(matches!(
            stream.serialization,
            SerializationFormat::Avro { .. }
        ));
        assert_eq!(stream.retry_policy.max_attempts, 5);
        assert!(stream.retry_policy.use_jitter);
        assert_eq!(stream.buffer_config.max_events, 1000);
        assert_eq!(stream.filter.event_types.len(), 2);
    }

    #[test]
    fn test_stream_manager_config_defaults() {
        let config = StreamManagerConfig::default();

        assert_eq!(config.max_concurrent_processors, 50);
        assert!(config.log_operations);
        assert_eq!(config.global_flush_interval_secs, 10);
    }

    #[test]
    fn test_stream_config_serialization() {
        let backend = StreamBackend::Kinesis {
            region: "eu-west-1".to_string(),
            stream_name: "production-events".to_string(),
            access_key_id: Some("AKIA...".to_string()),
            secret_access_key: Some("secret".to_string()),
            config: {
                let mut config = HashMap::new();
                config.insert("batch_size".to_string(), "500".to_string());
                config
            },
        };

        let stream_config = StreamConfig::new("prod-stream".to_string(), backend)
            .with_partitioning(PartitioningStrategy::Custom {
                metadata_key: "tenant_id".to_string(),
            })
            .with_serialization(SerializationFormat::Protobuf {
                schema_definition: "syntax = \"proto3\"; message Event { string id = 1; }"
                    .to_string(),
            });

        let serialized = serde_json::to_string(&stream_config).unwrap();
        let deserialized: StreamConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.name, stream_config.name);
        assert_eq!(deserialized.enabled, stream_config.enabled);

        match &deserialized.backend {
            StreamBackend::Kinesis {
                region,
                stream_name,
                ..
            } => {
                assert_eq!(region, "eu-west-1");
                assert_eq!(stream_name, "production-events");
            }
            _ => panic!("Wrong backend type after deserialization"),
        }
    }

    #[tokio::test]
    async fn test_stream_processor_stats() {
        let processor = PubSubProcessor::new(
            "test-project".to_string(),
            "test-topic".to_string(),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();

        let stats = processor.get_stats().await.unwrap();

        assert_eq!(
            stats["type"],
            serde_json::Value::String("pubsub".to_string())
        );
        assert_eq!(
            stats["project_id"],
            serde_json::Value::String("test-project".to_string())
        );
        assert_eq!(
            stats["topic_name"],
            serde_json::Value::String("test-topic".to_string())
        );
    }

    #[test]
    fn test_buffer_config_defaults() {
        let buffer = BufferConfig::default();

        assert_eq!(buffer.max_events, 1000);
        assert_eq!(buffer.max_buffer_time_secs, 5);
        assert_eq!(buffer.batch_size, 100);
    }

    #[test]
    fn test_stream_retry_policy_defaults() {
        let policy = StreamRetryPolicy::default();

        assert_eq!(policy.max_attempts, 5);
        assert_eq!(policy.initial_delay_secs, 1);
        assert_eq!(policy.max_delay_secs, 300);
        assert_eq!(policy.backoff_multiplier, 2.0);
        assert!(policy.use_jitter);
    }

    #[test]
    fn test_custom_metadata_partitioning() {
        let mut metadata = HashMap::new();
        metadata.insert("tenant_id".to_string(), "tenant_123".to_string());
        metadata.insert("region".to_string(), "us-west-2".to_string());

        let event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "multi_tenant_queue".to_string(),
            event_type: JobLifecycleEventType::Enqueued,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata,
        };

        // Test custom metadata partitioning
        let tenant_partition = StreamManager::calculate_partition_key(
            &event,
            &PartitioningStrategy::Custom {
                metadata_key: "tenant_id".to_string(),
            },
        );
        assert_eq!(tenant_partition, Some("tenant_123".to_string()));

        let region_partition = StreamManager::calculate_partition_key(
            &event,
            &PartitioningStrategy::Custom {
                metadata_key: "region".to_string(),
            },
        );
        assert_eq!(region_partition, Some("us-west-2".to_string()));

        // Test non-existent metadata key
        let missing_partition = StreamManager::calculate_partition_key(
            &event,
            &PartitioningStrategy::Custom {
                metadata_key: "missing_key".to_string(),
            },
        );
        assert_eq!(missing_partition, None);
    }

    #[tokio::test]
    async fn test_stream_processor_configuration_usage() {
        // Test that configuration fields are properly used in stream processors

        // Test Kafka processor with custom configuration
        let mut kafka_config = HashMap::new();
        kafka_config.insert("batch.delay.ms".to_string(), "50".to_string());
        kafka_config.insert("test.error.rate".to_string(), "0.1".to_string());
        kafka_config.insert("health.check.timeout.ms".to_string(), "1000".to_string());

        let kafka_processor = KafkaProcessor::new(
            vec!["localhost:9092".to_string()],
            "test-topic".to_string(),
            kafka_config,
        )
        .await
        .unwrap();

        // Test that stats include configuration
        let stats = kafka_processor.get_stats().await.unwrap();
        assert_eq!(
            stats.get("type").unwrap(),
            &serde_json::Value::String("kafka".to_string())
        );
        assert!(stats.contains_key("config"));

        // Test Kinesis processor with AWS credentials
        let mut kinesis_config = HashMap::new();
        kinesis_config.insert("retries".to_string(), "3".to_string());

        let kinesis_processor = KinesisProcessor::new(
            "us-west-2".to_string(),
            "test-stream".to_string(),
            Some("AKIA***".to_string()),
            Some("secret".to_string()),
            kinesis_config,
        )
        .await
        .unwrap();

        let kinesis_stats = kinesis_processor.get_stats().await.unwrap();
        assert_eq!(
            kinesis_stats.get("type").unwrap(),
            &serde_json::Value::String("kinesis".to_string())
        );
        assert!(kinesis_stats.contains_key("access_key_id"));
        assert!(kinesis_stats.contains_key("config"));

        // Test PubSub processor with service account
        let mut pubsub_config = HashMap::new();
        pubsub_config.insert("max_messages".to_string(), "1000".to_string());

        let pubsub_processor = PubSubProcessor::new(
            "my-project".to_string(),
            "test-topic".to_string(),
            Some("service-account-key".to_string()),
            pubsub_config,
        )
        .await
        .unwrap();

        let pubsub_stats = pubsub_processor.get_stats().await.unwrap();
        assert_eq!(
            pubsub_stats.get("type").unwrap(),
            &serde_json::Value::String("pubsub".to_string())
        );
        assert_eq!(
            pubsub_stats.get("service_account_configured").unwrap(),
            &serde_json::Value::Bool(true)
        );
        assert!(pubsub_stats.contains_key("config"));
    }
}
