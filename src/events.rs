//! Event system for job lifecycle tracking and external integrations.
//!
//! This module provides a comprehensive event system for tracking job lifecycle
//! events and delivering them to external systems via webhooks and streaming.
//! It builds upon the existing JobEvent system in stats.rs but extends it for
//! real-time event delivery and filtering.
//!
//! # Overview
//!
//! The event system consists of several key components:
//!
//! - [`JobLifecycleEvent`] - Represents a single job lifecycle event with metadata
//! - [`EventManager`] - Central hub for publishing and subscribing to events
//! - [`EventFilter`] - Flexible filtering system for event subscriptions
//! - [`EventSubscription`] - Subscription handle for receiving filtered events
//!
//! # Features
//!
//! - **Real-time event delivery** - Events are delivered immediately to subscribers
//! - **Flexible filtering** - Filter by event type, queue, priority, processing time, and metadata
//! - **Broadcast channels** - Efficient delivery to multiple subscribers
//! - **Payload control** - Configure whether to include job payload data
//! - **Statistics tracking** - Monitor event publishing and subscription metrics
//!
//! # Examples
//!
//! ## Basic Event Publishing and Subscription
//!
//! ```rust
//! use hammerwork::events::{EventManager, EventFilter, JobLifecycleEvent, JobLifecycleEventType, JobLifecycleEventBuilder};
//! use hammerwork::priority::JobPriority;
//! use uuid::Uuid;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Create an event manager
//! let event_manager = EventManager::new_default();
//!
//! // Subscribe to completion events
//! let filter = EventFilter::new()
//!     .with_event_types(vec![JobLifecycleEventType::Completed]);
//! let mut subscription = event_manager.subscribe(filter).await?;
//!
//! // Create and publish an event
//! let event = JobLifecycleEvent::completed(
//!     Uuid::new_v4(),
//!     "email_queue".to_string(),
//!     JobPriority::Normal,
//!     1500, // processing time in ms
//! );
//! event_manager.publish_event(event).await?;
//!
//! // The subscription is ready to receive events
//! // In a real application, you would receive events in a loop:
//! // let received_event = subscription.receiver.recv().await?;
//! assert_eq!(subscription.filter.event_types.len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! ## Advanced Filtering
//!
//! ```rust
//! use hammerwork::events::{EventManager, EventFilter, JobLifecycleEventType};
//! use hammerwork::priority::JobPriority;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let event_manager = EventManager::new_default();
//!
//! // Create a complex filter
//! let filter = EventFilter::new()
//!     .with_event_types(vec![JobLifecycleEventType::Completed, JobLifecycleEventType::Failed])
//!     .with_queue_names(vec!["critical_queue".to_string(), "urgent_queue".to_string()])
//!     .with_priorities(vec![JobPriority::High, JobPriority::Critical])
//!     .with_processing_time_range(Some(1000), Some(30000)) // 1-30 seconds
//!     .with_metadata_filter("tenant_id".to_string(), "premium".to_string())
//!     .include_payload();
//!
//! let subscription = event_manager.subscribe(filter).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Event Builder Pattern
//!
//! ```rust
//! use hammerwork::events::{JobLifecycleEvent, JobLifecycleEventBuilder, JobError};
//! use hammerwork::priority::JobPriority;
//! use uuid::Uuid;
//!
//! // Build different types of events
//! let job_id = Uuid::new_v4();
//! let queue_name = "processing_queue".to_string();
//!
//! let started_event = JobLifecycleEvent::started(
//!     job_id,
//!     queue_name.clone(),
//!     JobPriority::High,
//! );
//!
//! let failed_event = JobLifecycleEvent::failed(
//!     job_id,
//!     queue_name,
//!     JobPriority::High,
//!     JobError {
//!         message: "Database connection failed".to_string(),
//!         error_type: Some("ConnectionError".to_string()),
//!         details: None,
//!         retry_attempt: Some(2),
//!     },
//! );
//! ```

use crate::priority::JobPriority;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

/// A job lifecycle event that can be delivered to external systems.
///
/// This struct represents a single event in a job's lifecycle, containing all the metadata
/// necessary for external systems to understand what happened to the job.
///
/// # Examples
///
/// ```rust
/// use hammerwork::events::{JobLifecycleEvent, JobLifecycleEventType};
/// use hammerwork::priority::JobPriority;
/// use uuid::Uuid;
/// use chrono::Utc;
/// use std::collections::HashMap;
///
/// let event = JobLifecycleEvent {
///     event_id: Uuid::new_v4(),
///     job_id: Uuid::new_v4(),
///     queue_name: "email_queue".to_string(),
///     event_type: JobLifecycleEventType::Completed,
///     priority: JobPriority::Normal,
///     timestamp: Utc::now(),
///     processing_time_ms: Some(1500),
///     error: None,
///     payload: Some(serde_json::json!({"recipient": "user@example.com"})),
///     metadata: {
///         let mut meta = HashMap::new();
///         meta.insert("tenant_id".to_string(), "acme_corp".to_string());
///         meta
///     },
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLifecycleEvent {
    /// Unique identifier for this event
    pub event_id: Uuid,
    /// The job that this event relates to
    pub job_id: Uuid,
    /// Name of the queue the job belongs to
    pub queue_name: String,
    /// Type of event that occurred
    pub event_type: JobLifecycleEventType,
    /// Priority of the job
    pub priority: JobPriority,
    /// When this event occurred
    pub timestamp: DateTime<Utc>,
    /// Optional processing time for completion events
    pub processing_time_ms: Option<u64>,
    /// Optional error information for failure events
    pub error: Option<JobError>,
    /// Job payload data (configurable whether to include)
    pub payload: Option<serde_json::Value>,
    /// Additional metadata about the job or event
    pub metadata: HashMap<String, String>,
}

/// Types of job lifecycle events
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobLifecycleEventType {
    /// Job was enqueued and is pending execution
    Enqueued,
    /// Job has started processing
    Started,
    /// Job completed successfully
    Completed,
    /// Job failed but may be retried
    Failed,
    /// Job is being retried after a failure
    Retried,
    /// Job has exceeded maximum retry attempts and is dead
    Dead,
    /// Job execution timed out
    TimedOut,
    /// Job was cancelled before completion
    Cancelled,
    /// Job was archived (moved to archive storage)
    Archived,
    /// Job was restored from archive
    Restored,
}

/// Error information for failed jobs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobError {
    /// Error message
    pub message: String,
    /// Error type or category
    pub error_type: Option<String>,
    /// Full error details/stack trace (optional)
    pub details: Option<String>,
    /// Retry attempt number when this error occurred
    pub retry_attempt: Option<u32>,
}

/// Event filter for subscribing to specific events.
///
/// EventFilter provides a flexible way to subscribe only to events that match specific criteria.
/// All filter conditions are combined with AND logic - an event must match ALL specified criteria.
/// Empty filter arrays mean "match all" for that criterion.
///
/// # Examples
///
/// ```rust
/// use hammerwork::events::{EventFilter, JobLifecycleEventType};
/// use hammerwork::priority::JobPriority;
///
/// // Filter for high-priority completed jobs in specific queues
/// let filter = EventFilter::new()
///     .with_event_types(vec![JobLifecycleEventType::Completed])
///     .with_queue_names(vec!["critical".to_string(), "urgent".to_string()])
///     .with_priorities(vec![JobPriority::High, JobPriority::Critical])
///     .with_processing_time_range(Some(5000), None) // > 5 seconds
///     .with_metadata_filter("tenant_id".to_string(), "premium".to_string())
///     .include_payload();
/// ```
///
/// ```rust
/// use hammerwork::events::{EventFilter, JobLifecycleEventType};
///
/// // Filter for any failure events
/// let failure_filter = EventFilter::new()
///     .with_event_types(vec![
///         JobLifecycleEventType::Failed,
///         JobLifecycleEventType::Dead,
///         JobLifecycleEventType::TimedOut,
///     ]);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventFilter {
    /// Filter by event types (empty means all types)
    pub event_types: Vec<JobLifecycleEventType>,
    /// Filter by queue names (empty means all queues)
    pub queue_names: Vec<String>,
    /// Filter by job priorities (empty means all priorities)
    pub priorities: Vec<JobPriority>,
    /// Filter by minimum processing time (in milliseconds)
    pub min_processing_time_ms: Option<u64>,
    /// Filter by maximum processing time (in milliseconds)
    pub max_processing_time_ms: Option<u64>,
    /// Custom metadata filters (key-value pairs that must match)
    pub metadata_filters: HashMap<String, String>,
    /// Include job payload data in events
    pub include_payload: bool,
}


impl EventFilter {
    /// Create a new event filter
    pub fn new() -> Self {
        Self::default()
    }

    /// Filter by specific event types
    pub fn with_event_types(mut self, types: Vec<JobLifecycleEventType>) -> Self {
        self.event_types = types;
        self
    }

    /// Filter by specific queues
    pub fn with_queues(mut self, queues: Vec<String>) -> Self {
        self.queue_names = queues;
        self
    }

    /// Filter by specific queue names (alias for with_queues)
    pub fn with_queue_names(mut self, queue_names: Vec<String>) -> Self {
        self.queue_names = queue_names;
        self
    }

    /// Filter by job priorities
    pub fn with_priorities(mut self, priorities: Vec<JobPriority>) -> Self {
        self.priorities = priorities;
        self
    }

    /// Filter by processing time range
    pub fn with_processing_time_range(mut self, min_ms: Option<u64>, max_ms: Option<u64>) -> Self {
        self.min_processing_time_ms = min_ms;
        self.max_processing_time_ms = max_ms;
        self
    }

    /// Add metadata filter
    pub fn with_metadata_filter(mut self, key: String, value: String) -> Self {
        self.metadata_filters.insert(key, value);
        self
    }

    /// Include job payload data in events
    pub fn include_payload(mut self) -> Self {
        self.include_payload = true;
        self
    }

    /// Check if an event matches this filter
    pub fn matches(&self, event: &JobLifecycleEvent) -> bool {
        // Check event types
        if !self.event_types.is_empty() && !self.event_types.contains(&event.event_type) {
            return false;
        }

        // Check queue names
        if !self.queue_names.is_empty() && !self.queue_names.contains(&event.queue_name) {
            return false;
        }

        // Check priorities
        if !self.priorities.is_empty() && !self.priorities.contains(&event.priority) {
            return false;
        }

        // Check processing time range
        if let Some(processing_time) = event.processing_time_ms {
            if let Some(min_time) = self.min_processing_time_ms {
                if processing_time < min_time {
                    return false;
                }
            }
            if let Some(max_time) = self.max_processing_time_ms {
                if processing_time > max_time {
                    return false;
                }
            }
        }

        // Check metadata filters
        for (key, expected_value) in &self.metadata_filters {
            if let Some(actual_value) = event.metadata.get(key) {
                if actual_value != expected_value {
                    return false;
                }
            } else {
                return false;
            }
        }

        true
    }
}

/// Event subscription for receiving filtered events
#[derive(Debug)]
pub struct EventSubscription {
    /// Unique identifier for this subscription
    pub id: Uuid,
    /// Filter criteria for this subscription
    pub filter: EventFilter,
    /// Channel receiver for receiving events
    pub receiver: broadcast::Receiver<JobLifecycleEvent>,
}

/// Event manager for publishing and subscribing to job lifecycle events.
///
/// The EventManager is the central hub for the event system. It handles publishing events
/// to all subscribers using an efficient broadcast channel, manages subscriptions, and
/// provides statistics about event publishing activity.
///
/// # Thread Safety
///
/// EventManager is thread-safe and can be shared across multiple tasks using Arc.
/// All operations are async and use non-blocking synchronization primitives.
///
/// # Examples
///
/// ```rust
/// use hammerwork::events::{EventManager, EventConfig, EventFilter, JobLifecycleEvent, JobLifecycleEventType, JobLifecycleEventBuilder};
/// use hammerwork::priority::JobPriority;
/// use uuid::Uuid;
/// use std::sync::Arc;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// // Create a manager with custom configuration
/// let config = EventConfig {
///     max_buffer_size: 5000,
///     include_payload_default: true,
///     max_payload_size_bytes: 128 * 1024, // 128KB
///     log_events: true,
/// };
/// let manager = Arc::new(EventManager::new(config));
///
/// // Subscribe to events
/// let filter = EventFilter::new()
///     .with_event_types(vec![JobLifecycleEventType::Failed])
///     .with_priorities(vec![JobPriority::Low]);
/// let mut subscription = manager.subscribe(filter).await?;
///
/// // Publish an event from another task
/// let manager_clone = Arc::clone(&manager);
/// tokio::spawn(async move {
///     let event = JobLifecycleEvent::failed(
///         Uuid::new_v4(),
///         "background_queue".to_string(),
///         JobPriority::Low,
///         hammerwork::events::JobError {
///             message: "Service unavailable".to_string(),
///             error_type: Some("ServiceError".to_string()),
///             details: None,
///             retry_attempt: Some(1),
///         },
///     );
///     let _ = manager_clone.publish_event(event).await;
/// });
///
/// // The subscription is ready to receive events
/// // In a real application, you would receive events in a loop:
/// // while let Ok(event) = subscription.receiver.recv().await {
/// //     if subscription.filter.matches(&event) {
/// //         println!("Job {} failed: {}", event.job_id,
/// //                  event.error.as_ref().unwrap().message);
/// //         break;
/// //     }
/// // }
/// assert!(subscription.filter.priorities.contains(&JobPriority::Low));
/// # Ok(())
/// # }
/// ```
pub struct EventManager {
    /// Broadcast sender for publishing events to all subscribers
    sender: broadcast::Sender<JobLifecycleEvent>,
    /// Active subscriptions (for management purposes)
    subscriptions: Arc<RwLock<HashMap<Uuid, EventFilter>>>,
    /// Configuration for event publishing
    config: EventConfig,
}

/// Configuration for the event system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventConfig {
    /// Maximum number of events to buffer in broadcast channel
    pub max_buffer_size: usize,
    /// Whether to include job payload data by default
    pub include_payload_default: bool,
    /// Maximum payload size to include (in bytes)
    pub max_payload_size_bytes: usize,
    /// Whether to log event publishing
    pub log_events: bool,
}

impl Default for EventConfig {
    fn default() -> Self {
        Self {
            max_buffer_size: 10_000,
            include_payload_default: false,
            max_payload_size_bytes: 64 * 1024, // 64KB
            log_events: false,
        }
    }
}

impl EventManager {
    /// Create a new event manager
    pub fn new(config: EventConfig) -> Self {
        let (sender, _) = broadcast::channel(config.max_buffer_size);

        Self {
            sender,
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Create a new event manager with default configuration
    pub fn new_default() -> Self {
        Self::new(EventConfig::default())
    }

    /// Publish a job lifecycle event to all subscribers.
    ///
    /// This method publishes an event to all active subscribers. The event is delivered
    /// immediately to all subscribers via broadcast channels. If there are no active
    /// subscribers, the event is still processed successfully but simply has no recipients.
    ///
    /// # Payload Handling
    ///
    /// The method applies payload filtering based on the manager's configuration:
    /// - If `include_payload_default` is false, payload is stripped
    /// - If payload exceeds `max_payload_size_bytes`, it's stripped and a metadata entry is added
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::events::{EventManager, JobLifecycleEvent, JobLifecycleEventBuilder};
    /// use hammerwork::priority::JobPriority;
    /// use uuid::Uuid;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let manager = EventManager::new_default();
    ///
    /// let event = JobLifecycleEvent::completed(
    ///     Uuid::new_v4(),
    ///     "email_queue".to_string(),
    ///     JobPriority::Normal,
    ///     2500, // processing time in ms
    /// );
    ///
    /// manager.publish_event(event).await?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Payload serialization fails (when checking size limits)
    /// - Any other internal serialization errors occur
    pub async fn publish_event(&self, mut event: JobLifecycleEvent) -> crate::Result<()> {
        // Apply payload filtering based on configuration
        if !self.config.include_payload_default {
            event.payload = None;
        } else if let Some(ref payload) = event.payload {
            // Check payload size limit
            let payload_size = serde_json::to_string(payload)?.len();
            if payload_size > self.config.max_payload_size_bytes {
                event.payload = None;
                event.metadata.insert(
                    "payload_truncated".to_string(),
                    format!(
                        "Payload size {} exceeded limit {}",
                        payload_size, self.config.max_payload_size_bytes
                    ),
                );
            }
        }

        // Log event if configured
        if self.config.log_events {
            tracing::debug!(
                "Publishing job lifecycle event: {} for job {} in queue {}",
                event.event_type,
                event.job_id,
                event.queue_name
            );
        }

        // Publish to all subscribers
        match self.sender.send(event) {
            Ok(subscriber_count) => {
                if self.config.log_events && subscriber_count > 0 {
                    tracing::trace!("Event delivered to {} subscribers", subscriber_count);
                }
                Ok(())
            }
            Err(broadcast::error::SendError(_)) => {
                // No active subscribers - this is not an error
                Ok(())
            }
        }
    }

    /// Subscribe to job lifecycle events with a filter.
    ///
    /// Creates a new subscription that will receive events matching the provided filter.
    /// The returned `EventSubscription` contains a broadcast receiver that will receive
    /// all published events. It's the subscriber's responsibility to apply the filter
    /// when processing received events.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use hammerwork::events::{EventManager, EventFilter, JobLifecycleEventType};
    /// use hammerwork::priority::JobPriority;
    ///
    /// # #[tokio::main]
    /// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let manager = EventManager::new_default();
    ///
    /// // Subscribe to high-priority failures
    /// let filter = EventFilter::new()
    ///     .with_event_types(vec![JobLifecycleEventType::Failed, JobLifecycleEventType::Dead])
    ///     .with_priorities(vec![JobPriority::High, JobPriority::Critical]);
    ///
    /// let subscription = manager.subscribe(filter).await?;
    ///
    /// // The subscription is ready to receive events
    /// assert_eq!(subscription.filter.event_types.len(), 2);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Returns
    ///
    /// Returns an `EventSubscription` containing:
    /// - A unique subscription ID for management
    /// - The filter criteria
    /// - A broadcast receiver for receiving events
    pub async fn subscribe(&self, filter: EventFilter) -> crate::Result<EventSubscription> {
        let subscription_id = Uuid::new_v4();
        let receiver = self.sender.subscribe();

        // Store subscription for management
        {
            let mut subscriptions = self.subscriptions.write().await;
            subscriptions.insert(subscription_id, filter.clone());
        }

        Ok(EventSubscription {
            id: subscription_id,
            filter,
            receiver,
        })
    }

    /// Unsubscribe from events
    pub async fn unsubscribe(&self, subscription_id: Uuid) -> crate::Result<()> {
        let mut subscriptions = self.subscriptions.write().await;
        subscriptions.remove(&subscription_id);
        Ok(())
    }

    /// Get count of active subscriptions
    pub async fn subscription_count(&self) -> usize {
        let subscriptions = self.subscriptions.read().await;
        subscriptions.len()
    }

    /// Get event manager statistics
    pub async fn get_stats(&self) -> EventManagerStats {
        let subscriptions = self.subscriptions.read().await;
        EventManagerStats {
            active_subscriptions: subscriptions.len(),
            buffer_capacity: self.config.max_buffer_size,
            buffer_current_size: self.sender.len(),
        }
    }
}

/// Statistics about the event manager
#[derive(Debug, Serialize, Deserialize)]
pub struct EventManagerStats {
    /// Number of active subscriptions
    pub active_subscriptions: usize,
    /// Maximum buffer capacity
    pub buffer_capacity: usize,
    /// Current buffer usage
    pub buffer_current_size: usize,
}

/// Helper trait for creating job lifecycle events
pub trait JobLifecycleEventBuilder {
    /// Create an enqueued event
    fn enqueued(job_id: Uuid, queue_name: String, priority: JobPriority) -> JobLifecycleEvent;

    /// Create a started event
    fn started(job_id: Uuid, queue_name: String, priority: JobPriority) -> JobLifecycleEvent;

    /// Create a completed event
    fn completed(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        processing_time_ms: u64,
    ) -> JobLifecycleEvent;

    /// Create a failed event
    fn failed(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        error: JobError,
    ) -> JobLifecycleEvent;

    /// Create a dead event
    fn dead(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        final_error: JobError,
    ) -> JobLifecycleEvent;

    /// Create a timed out event
    fn timed_out(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        timeout_duration_ms: u64,
    ) -> JobLifecycleEvent;
}

impl JobLifecycleEventBuilder for JobLifecycleEvent {
    fn enqueued(job_id: Uuid, queue_name: String, priority: JobPriority) -> JobLifecycleEvent {
        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id,
            queue_name,
            event_type: JobLifecycleEventType::Enqueued,
            priority,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: HashMap::new(),
        }
    }

    fn started(job_id: Uuid, queue_name: String, priority: JobPriority) -> JobLifecycleEvent {
        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id,
            queue_name,
            event_type: JobLifecycleEventType::Started,
            priority,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: HashMap::new(),
        }
    }

    fn completed(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        processing_time_ms: u64,
    ) -> JobLifecycleEvent {
        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id,
            queue_name,
            event_type: JobLifecycleEventType::Completed,
            priority,
            timestamp: Utc::now(),
            processing_time_ms: Some(processing_time_ms),
            error: None,
            payload: None,
            metadata: HashMap::new(),
        }
    }

    fn failed(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        error: JobError,
    ) -> JobLifecycleEvent {
        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id,
            queue_name,
            event_type: JobLifecycleEventType::Failed,
            priority,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: Some(error),
            payload: None,
            metadata: HashMap::new(),
        }
    }

    fn dead(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        final_error: JobError,
    ) -> JobLifecycleEvent {
        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id,
            queue_name,
            event_type: JobLifecycleEventType::Dead,
            priority,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: Some(final_error),
            payload: None,
            metadata: HashMap::new(),
        }
    }

    fn timed_out(
        job_id: Uuid,
        queue_name: String,
        priority: JobPriority,
        timeout_duration_ms: u64,
    ) -> JobLifecycleEvent {
        let mut metadata = HashMap::new();
        metadata.insert(
            "timeout_duration_ms".to_string(),
            timeout_duration_ms.to_string(),
        );

        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id,
            queue_name,
            event_type: JobLifecycleEventType::TimedOut,
            priority,
            timestamp: Utc::now(),
            processing_time_ms: Some(timeout_duration_ms),
            error: Some(JobError {
                message: format!("Job timed out after {}ms", timeout_duration_ms),
                error_type: Some("timeout".to_string()),
                details: None,
                retry_attempt: None,
            }),
            payload: None,
            metadata,
        }
    }
}

/// Display implementation for JobLifecycleEventType
impl std::fmt::Display for JobLifecycleEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobLifecycleEventType::Enqueued => write!(f, "enqueued"),
            JobLifecycleEventType::Started => write!(f, "started"),
            JobLifecycleEventType::Completed => write!(f, "completed"),
            JobLifecycleEventType::Failed => write!(f, "failed"),
            JobLifecycleEventType::Retried => write!(f, "retried"),
            JobLifecycleEventType::Dead => write!(f, "dead"),
            JobLifecycleEventType::TimedOut => write!(f, "timed_out"),
            JobLifecycleEventType::Cancelled => write!(f, "cancelled"),
            JobLifecycleEventType::Archived => write!(f, "archived"),
            JobLifecycleEventType::Restored => write!(f, "restored"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::timeout;

    #[test]
    fn test_event_filter_creation() {
        let filter = EventFilter::new()
            .with_event_types(vec![
                JobLifecycleEventType::Completed,
                JobLifecycleEventType::Failed,
            ])
            .with_queues(vec!["email".to_string(), "notifications".to_string()])
            .with_priorities(vec![JobPriority::High, JobPriority::Critical])
            .with_processing_time_range(Some(1000), Some(5000))
            .with_metadata_filter("source".to_string(), "api".to_string())
            .include_payload();

        assert_eq!(filter.event_types.len(), 2);
        assert_eq!(filter.queue_names.len(), 2);
        assert_eq!(filter.priorities.len(), 2);
        assert_eq!(filter.min_processing_time_ms, Some(1000));
        assert_eq!(filter.max_processing_time_ms, Some(5000));
        assert!(filter.include_payload);
        assert_eq!(
            filter.metadata_filters.get("source"),
            Some(&"api".to_string())
        );
    }

    #[test]
    fn test_event_filter_matching() {
        let filter = EventFilter::new()
            .with_event_types(vec![JobLifecycleEventType::Completed])
            .with_queues(vec!["email".to_string()]);

        let matching_event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "email".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: Some(1000),
            error: None,
            payload: None,
            metadata: HashMap::new(),
        };

        let non_matching_event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "notifications".to_string(),
            event_type: JobLifecycleEventType::Failed,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: HashMap::new(),
        };

        assert!(filter.matches(&matching_event));
        assert!(!filter.matches(&non_matching_event));
    }

    #[test]
    fn test_job_lifecycle_event_builder() {
        let job_id = Uuid::new_v4();
        let queue_name = "test_queue".to_string();
        let priority = JobPriority::High;

        let enqueued = JobLifecycleEvent::enqueued(job_id, queue_name.clone(), priority);
        assert_eq!(enqueued.event_type, JobLifecycleEventType::Enqueued);
        assert_eq!(enqueued.job_id, job_id);
        assert_eq!(enqueued.queue_name, queue_name);
        assert_eq!(enqueued.priority, priority);

        let completed = JobLifecycleEvent::completed(job_id, queue_name.clone(), priority, 1500);
        assert_eq!(completed.event_type, JobLifecycleEventType::Completed);
        assert_eq!(completed.processing_time_ms, Some(1500));

        let error = JobError {
            message: "Test error".to_string(),
            error_type: Some("validation".to_string()),
            details: None,
            retry_attempt: Some(1),
        };
        let failed = JobLifecycleEvent::failed(job_id, queue_name.clone(), priority, error);
        assert_eq!(failed.event_type, JobLifecycleEventType::Failed);
        assert!(failed.error.is_some());
    }

    #[tokio::test]
    async fn test_event_manager_publish_subscribe() {
        let manager = EventManager::new_default();

        // Subscribe to events
        let filter = EventFilter::new().with_event_types(vec![JobLifecycleEventType::Completed]);
        let mut subscription = manager.subscribe(filter).await.unwrap();

        // Publish a matching event
        let event = JobLifecycleEvent::completed(
            Uuid::new_v4(),
            "test_queue".to_string(),
            JobPriority::Normal,
            1000,
        );
        manager.publish_event(event.clone()).await.unwrap();

        // Receive the event
        let received_event = timeout(Duration::from_millis(100), subscription.receiver.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(received_event.event_type, JobLifecycleEventType::Completed);
        assert_eq!(received_event.job_id, event.job_id);
    }

    #[tokio::test]
    async fn test_event_manager_filtered_subscription() {
        let manager = EventManager::new_default();

        // Subscribe only to completion events
        let filter = EventFilter::new().with_event_types(vec![JobLifecycleEventType::Completed]);
        let mut subscription = manager.subscribe(filter).await.unwrap();

        // Publish a failed event (should not be received)
        let failed_event = JobLifecycleEvent::failed(
            Uuid::new_v4(),
            "test_queue".to_string(),
            JobPriority::Normal,
            JobError {
                message: "Test error".to_string(),
                error_type: None,
                details: None,
                retry_attempt: None,
            },
        );
        manager.publish_event(failed_event).await.unwrap();

        // Publish a completed event (should be received)
        let completed_event = JobLifecycleEvent::completed(
            Uuid::new_v4(),
            "test_queue".to_string(),
            JobPriority::Normal,
            1000,
        );
        manager
            .publish_event(completed_event.clone())
            .await
            .unwrap();

        // Should receive only the completed event
        let mut received_event = None;
        for _ in 0..10 {
            // Try receiving up to 10 events
            match timeout(Duration::from_millis(100), subscription.receiver.recv()).await {
                Ok(Ok(event)) => {
                    if subscription.filter.matches(&event) {
                        received_event = Some(event);
                        break;
                    }
                }
                _ => break,
            }
        }

        let received_event = received_event.expect("Should have received a matching event");
        assert_eq!(received_event.event_type, JobLifecycleEventType::Completed);
        assert_eq!(received_event.job_id, completed_event.job_id);

        // No more events should be available
        assert!(
            timeout(Duration::from_millis(50), subscription.receiver.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_event_manager_stats() {
        let manager = EventManager::new_default();

        let initial_stats = manager.get_stats().await;
        assert_eq!(initial_stats.active_subscriptions, 0);

        // Add subscription
        let filter = EventFilter::new();
        let _subscription = manager.subscribe(filter).await.unwrap();

        let stats_with_subscription = manager.get_stats().await;
        assert_eq!(stats_with_subscription.active_subscriptions, 1);
    }

    #[tokio::test]
    async fn test_event_manager_unsubscribe() {
        let manager = EventManager::new_default();

        let filter = EventFilter::new();
        let subscription = manager.subscribe(filter).await.unwrap();
        let subscription_id = subscription.id;

        assert_eq!(manager.subscription_count().await, 1);

        manager.unsubscribe(subscription_id).await.unwrap();
        assert_eq!(manager.subscription_count().await, 0);
    }

    #[test]
    fn test_event_type_display() {
        assert_eq!(JobLifecycleEventType::Enqueued.to_string(), "enqueued");
        assert_eq!(JobLifecycleEventType::Started.to_string(), "started");
        assert_eq!(JobLifecycleEventType::Completed.to_string(), "completed");
        assert_eq!(JobLifecycleEventType::Failed.to_string(), "failed");
        assert_eq!(JobLifecycleEventType::Dead.to_string(), "dead");
        assert_eq!(JobLifecycleEventType::TimedOut.to_string(), "timed_out");
    }

    #[test]
    fn test_processing_time_filter() {
        let filter = EventFilter::new().with_processing_time_range(Some(1000), Some(3000));

        let fast_event = JobLifecycleEvent::completed(
            Uuid::new_v4(),
            "test".to_string(),
            JobPriority::Normal,
            500,
        );

        let medium_event = JobLifecycleEvent::completed(
            Uuid::new_v4(),
            "test".to_string(),
            JobPriority::Normal,
            2000,
        );

        let slow_event = JobLifecycleEvent::completed(
            Uuid::new_v4(),
            "test".to_string(),
            JobPriority::Normal,
            5000,
        );

        assert!(!filter.matches(&fast_event));
        assert!(filter.matches(&medium_event));
        assert!(!filter.matches(&slow_event));
    }

    #[test]
    fn test_metadata_filter() {
        let filter = EventFilter::new()
            .with_metadata_filter("source".to_string(), "api".to_string())
            .with_metadata_filter("version".to_string(), "v1".to_string());

        let mut matching_metadata = HashMap::new();
        matching_metadata.insert("source".to_string(), "api".to_string());
        matching_metadata.insert("version".to_string(), "v1".to_string());

        let mut partial_metadata = HashMap::new();
        partial_metadata.insert("source".to_string(), "api".to_string());

        let matching_event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "test".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: matching_metadata,
        };

        let partial_event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "test".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::Normal,
            timestamp: Utc::now(),
            processing_time_ms: None,
            error: None,
            payload: None,
            metadata: partial_metadata,
        };

        assert!(filter.matches(&matching_event));
        assert!(!filter.matches(&partial_event));
    }

    #[tokio::test]
    async fn test_concurrent_event_publishing() {
        let manager = Arc::new(EventManager::new_default());
        let filter = EventFilter::new();
        let mut subscription = manager.subscribe(filter).await.unwrap();

        // Spawn multiple tasks publishing events concurrently
        let num_tasks = 10;
        let events_per_task = 5;
        let mut handles = Vec::new();

        for task_id in 0..num_tasks {
            let manager_clone = manager.clone();
            let handle = tokio::spawn(async move {
                for event_id in 0..events_per_task {
                    let mut event = JobLifecycleEvent::completed(
                        Uuid::new_v4(),
                        format!("queue_{}", task_id),
                        JobPriority::Normal,
                        1000,
                    );
                    event
                        .metadata
                        .insert("task_id".to_string(), task_id.to_string());
                    event
                        .metadata
                        .insert("event_id".to_string(), event_id.to_string());
                    manager_clone.publish_event(event).await.unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all publishing tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Collect all events
        let mut received_events = Vec::new();
        for _ in 0..(num_tasks * events_per_task) {
            let event = timeout(Duration::from_millis(100), subscription.receiver.recv())
                .await
                .unwrap()
                .unwrap();
            received_events.push(event);
        }

        assert_eq!(received_events.len(), num_tasks * events_per_task);

        // Verify all events were received
        let mut task_counts = std::collections::HashMap::new();
        for event in received_events {
            let task_id = event
                .metadata
                .get("task_id")
                .unwrap()
                .parse::<usize>()
                .unwrap();
            *task_counts.entry(task_id).or_insert(0) += 1;
        }

        assert_eq!(task_counts.len(), num_tasks);
        for count in task_counts.values() {
            assert_eq!(*count, events_per_task);
        }
    }

    #[tokio::test]
    async fn test_event_manager_buffer_overflow() {
        let mut config = EventConfig::default();
        config.max_buffer_size = 2; // Very small buffer
        let manager = EventManager::new(config);

        // Don't create any subscriptions so events accumulate in buffer

        // Publish more events than buffer can hold
        for _i in 0..5 {
            let event = JobLifecycleEvent::completed(
                Uuid::new_v4(),
                "test".to_string(),
                JobPriority::Normal,
                1000,
            );
            // This should not fail even if buffer overflows
            manager.publish_event(event).await.unwrap();
        }

        let stats = manager.get_stats().await;
        assert_eq!(stats.buffer_capacity, 2);
        // Buffer size should not exceed capacity
        assert!(stats.buffer_current_size <= stats.buffer_capacity);
    }

    #[test]
    fn test_job_error_serialization_roundtrip() {
        let original_error = JobError {
            message: "Database connection failed".to_string(),
            error_type: Some("DatabaseError".to_string()),
            details: Some(
                serde_json::to_string(&serde_json::json!({
                    "host": "localhost",
                    "port": 5432,
                    "database": "hammerwork"
                }))
                .unwrap(),
            ),
            retry_attempt: Some(3),
        };

        let serialized = serde_json::to_string(&original_error).unwrap();
        let deserialized: JobError = serde_json::from_str(&serialized).unwrap();

        assert_eq!(deserialized.message, original_error.message);
        assert_eq!(deserialized.error_type, original_error.error_type);
        assert_eq!(deserialized.retry_attempt, original_error.retry_attempt);

        // Compare details JSON
        let details_json: serde_json::Value =
            serde_json::from_str(deserialized.details.as_ref().unwrap()).unwrap();
        assert_eq!(details_json["host"], "localhost");
    }

    #[test]
    fn test_event_filter_empty_means_all() {
        let empty_filter = EventFilter::new();

        // Empty filter should match all events
        let events = vec![
            JobLifecycleEvent::enqueued(Uuid::new_v4(), "queue1".to_string(), JobPriority::Low),
            JobLifecycleEvent::started(Uuid::new_v4(), "queue2".to_string(), JobPriority::High),
            JobLifecycleEvent::completed(
                Uuid::new_v4(),
                "queue3".to_string(),
                JobPriority::Critical,
                2000,
            ),
        ];

        for event in events {
            assert!(
                empty_filter.matches(&event),
                "Empty filter should match all events"
            );
        }
    }

    #[test]
    fn test_event_config_serialization() {
        let config = EventConfig {
            max_buffer_size: 2000,
            include_payload_default: true,
            max_payload_size_bytes: 128 * 1024, // 128KB
            log_events: true,
        };

        let json_str = serde_json::to_string(&config).unwrap();
        let deserialized: EventConfig = serde_json::from_str(&json_str).unwrap();

        assert_eq!(deserialized.max_buffer_size, 2000);
        assert!(deserialized.include_payload_default);
        assert_eq!(deserialized.max_payload_size_bytes, 128 * 1024);
        assert!(deserialized.log_events);
    }
}
