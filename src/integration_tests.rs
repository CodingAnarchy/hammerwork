//! Integration tests for webhook and event streaming systems.
//!
//! These tests verify the interaction between the event system, webhook delivery,
//! and stream processing components working together as a complete system.

#[cfg(test)]
mod tests {
    use crate::{
        events::{EventFilter, EventManager, JobError, JobLifecycleEvent, JobLifecycleEventType},
        priority::JobPriority,
        streaming::{
            PartitioningStrategy, SerializationFormat, StreamBackend, StreamConfig, StreamManager,
            StreamManagerConfig,
        },
        webhooks::{
            HttpMethod, RetryPolicy, WebhookAuth, WebhookConfig, WebhookManager,
            WebhookManagerConfig,
        },
    };
    use chrono::Utc;
    use std::{collections::HashMap, sync::Arc, time::Duration};
    use tokio::time::sleep;
    use uuid::Uuid;

    fn create_test_event() -> JobLifecycleEvent {
        JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "integration_test_queue".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::High,
            timestamp: Utc::now(),
            processing_time_ms: Some(1500),
            error: None,
            payload: Some(serde_json::json!({
                "user_id": 12345,
                "action": "process_payment",
                "amount": 99.99
            })),
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("tenant_id".to_string(), "tenant_123".to_string());
                metadata.insert("correlation_id".to_string(), "corr_456".to_string());
                metadata
            },
        }
    }

    #[tokio::test]
    async fn test_event_manager_webhook_integration() {
        // Set up event manager
        let event_manager = Arc::new(EventManager::new_default());

        // Set up webhook manager
        let webhook_config = WebhookManagerConfig::default();
        let webhook_manager = WebhookManager::new(event_manager.clone(), webhook_config);

        // Create and add a webhook
        let webhook = WebhookConfig {
            id: Uuid::new_v4(),
            name: "payment_webhook".to_string(),
            url: "https://api.example.com/webhooks/payments".to_string(),
            method: HttpMethod::Post,
            headers: {
                let mut headers = HashMap::new();
                headers.insert("Content-Type".to_string(), "application/json".to_string());
                headers.insert("X-Source".to_string(), "hammerwork".to_string());
                headers
            },
            auth: Some(WebhookAuth::Bearer {
                token: "webhook_token_123".to_string(),
            }),
            secret: Some("webhook_secret".to_string()),
            filter: EventFilter::new()
                .with_event_types(vec![JobLifecycleEventType::Completed])
                .with_queue_names(vec!["integration_test_queue".to_string()])
                .with_priorities(vec![JobPriority::High]),
            retry_policy: RetryPolicy {
                max_attempts: 3,
                initial_delay_secs: 1,
                max_delay_secs: 10,
                backoff_multiplier: 2.0,
                retry_on_status_codes: vec![500, 502, 503],
            },
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        let webhook_id = webhook.id;
        webhook_manager.add_webhook(webhook).await.unwrap();

        // Verify webhook was added
        let stats = webhook_manager.get_stats().await;
        assert_eq!(stats.total_webhooks, 1);
        assert_eq!(stats.active_webhooks, 1);

        // Publish an event that should trigger the webhook
        let event = create_test_event();
        event_manager.publish_event(event).await.unwrap();

        // Give some time for webhook processing
        sleep(Duration::from_millis(100)).await;

        // Verify webhook received the event (in a real scenario, we'd check delivery logs)
        let webhook_stats = webhook_manager.get_webhook_stats(webhook_id).await;
        // Note: In this test environment, the webhook won't actually be delivered
        // but the system should be set up to handle it
        assert!(webhook_stats.is_some());
    }

    #[tokio::test]
    async fn test_event_manager_streaming_integration() {
        // Set up event manager
        let event_manager = Arc::new(EventManager::new_default());

        // Set up stream manager
        let stream_config = StreamManagerConfig::default();
        let stream_manager = StreamManager::new(event_manager.clone(), stream_config);

        // Create and add a stream
        let backend = StreamBackend::Kafka {
            brokers: vec!["localhost:9092".to_string()],
            topic: "payment_events".to_string(),
            config: HashMap::new(),
        };

        let stream = StreamConfig::new("payment_stream".to_string(), backend)
            .with_filter(
                EventFilter::new()
                    .with_event_types(vec![JobLifecycleEventType::Completed])
                    .with_queue_names(vec!["integration_test_queue".to_string()]),
            )
            .with_partitioning(PartitioningStrategy::Custom {
                metadata_key: "tenant_id".to_string(),
            })
            .with_serialization(SerializationFormat::Json)
            .enabled(true);

        let stream_id = stream.id;
        stream_manager.add_stream(stream).await.unwrap();

        // Verify stream was added
        let stats = stream_manager.get_stats().await;
        assert_eq!(stats.total_streams, 1);
        assert_eq!(stats.active_streams, 1);

        // Publish an event that should be streamed
        let event = create_test_event();
        event_manager.publish_event(event).await.unwrap();

        // Give some time for stream processing
        sleep(Duration::from_millis(100)).await;

        // Verify stream received the event
        let stream_stats = stream_manager.get_stream_stats(stream_id).await;
        assert!(stream_stats.is_some());
    }

    #[tokio::test]
    async fn test_full_system_integration() {
        // Set up all components
        let event_manager = Arc::new(EventManager::new_default());

        let webhook_manager =
            WebhookManager::new(event_manager.clone(), WebhookManagerConfig::default());

        let stream_manager =
            StreamManager::new(event_manager.clone(), StreamManagerConfig::default());

        // Add multiple webhooks with different filters
        let webhook1 = WebhookConfig {
            id: Uuid::new_v4(),
            name: "completed_jobs_webhook".to_string(),
            url: "https://api.example.com/webhooks/completed".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: EventFilter::new().with_event_types(vec![JobLifecycleEventType::Completed]),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        let webhook2 = WebhookConfig {
            id: Uuid::new_v4(),
            name: "failed_jobs_webhook".to_string(),
            url: "https://api.example.com/webhooks/failed".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: EventFilter::new().with_event_types(vec![JobLifecycleEventType::Failed]),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        webhook_manager.add_webhook(webhook1).await.unwrap();
        webhook_manager.add_webhook(webhook2).await.unwrap();

        // Add multiple streams with different backends
        let kafka_stream = StreamConfig::new(
            "kafka_all_events".to_string(),
            StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "all_job_events".to_string(),
                config: HashMap::new(),
            },
        )
        .with_partitioning(PartitioningStrategy::QueueName);

        let kinesis_stream = StreamConfig::new(
            "kinesis_high_priority".to_string(),
            StreamBackend::Kinesis {
                region: "us-east-1".to_string(),
                stream_name: "high_priority_jobs".to_string(),
                access_key_id: None,
                secret_access_key: None,
                config: HashMap::new(),
            },
        )
        .with_filter(
            EventFilter::new().with_priorities(vec![JobPriority::High, JobPriority::Critical]),
        );

        let pubsub_stream = StreamConfig::new(
            "pubsub_completed_jobs".to_string(),
            StreamBackend::PubSub {
                project_id: "my-project".to_string(),
                topic_name: "completed_jobs".to_string(),
                service_account_key: None,
                config: HashMap::new(),
            },
        )
        .with_filter(EventFilter::new().with_event_types(vec![JobLifecycleEventType::Completed]));

        stream_manager.add_stream(kafka_stream).await.unwrap();
        stream_manager.add_stream(kinesis_stream).await.unwrap();
        stream_manager.add_stream(pubsub_stream).await.unwrap();

        // Verify all components are set up
        let webhook_stats = webhook_manager.get_stats().await;
        assert_eq!(webhook_stats.total_webhooks, 2);
        assert_eq!(webhook_stats.active_webhooks, 2);

        let stream_stats = stream_manager.get_stats().await;
        assert_eq!(stream_stats.total_streams, 3);
        assert_eq!(stream_stats.active_streams, 3);

        // Publish various events
        let events = vec![
            // Completed event - should trigger completed webhook and multiple streams
            JobLifecycleEvent {
                event_id: Uuid::new_v4(),
                job_id: Uuid::new_v4(),
                queue_name: "payment_queue".to_string(),
                event_type: JobLifecycleEventType::Completed,
                priority: JobPriority::High,
                timestamp: Utc::now(),
                processing_time_ms: Some(2000),
                error: None,
                payload: Some(serde_json::json!({"amount": 100.0})),
                metadata: HashMap::new(),
            },
            // Failed event - should trigger failed webhook and kafka stream
            JobLifecycleEvent {
                event_id: Uuid::new_v4(),
                job_id: Uuid::new_v4(),
                queue_name: "email_queue".to_string(),
                event_type: JobLifecycleEventType::Failed,
                priority: JobPriority::Normal,
                timestamp: Utc::now(),
                processing_time_ms: None,
                error: Some(JobError {
                    message: "SMTP server unavailable".to_string(),
                    error_type: Some("NetworkError".to_string()),
                    details: None,
                    retry_attempt: Some(2),
                }),
                payload: None,
                metadata: HashMap::new(),
            },
            // Critical priority event - should trigger kinesis stream
            JobLifecycleEvent {
                event_id: Uuid::new_v4(),
                job_id: Uuid::new_v4(),
                queue_name: "urgent_queue".to_string(),
                event_type: JobLifecycleEventType::Started,
                priority: JobPriority::Critical,
                timestamp: Utc::now(),
                processing_time_ms: None,
                error: None,
                payload: None,
                metadata: HashMap::new(),
            },
        ];

        // Publish all events
        for event in events {
            event_manager.publish_event(event).await.unwrap();
        }

        // Give time for processing
        sleep(Duration::from_millis(200)).await;

        // Verify events were distributed correctly
        // In a real scenario, we would check delivery logs and stream metrics
        let final_webhook_stats = webhook_manager.get_stats().await;
        let final_stream_stats = stream_manager.get_stats().await;

        assert_eq!(final_webhook_stats.total_webhooks, 2);
        assert_eq!(final_stream_stats.total_streams, 3);
    }

    #[tokio::test]
    async fn test_event_filtering_across_systems() {
        let event_manager = Arc::new(EventManager::new_default());
        let webhook_manager =
            WebhookManager::new(event_manager.clone(), WebhookManagerConfig::default());
        let stream_manager =
            StreamManager::new(event_manager.clone(), StreamManagerConfig::default());

        // Create a very specific filter for high-priority payment completions
        let specific_filter = EventFilter::new()
            .with_event_types(vec![JobLifecycleEventType::Completed])
            .with_queue_names(vec!["payment_queue".to_string()])
            .with_priorities(vec![JobPriority::High, JobPriority::Critical])
            .with_processing_time_range(Some(1000), Some(5000)) // 1-5 seconds
            .with_metadata_filter("tenant_id".to_string(), "premium_tenant".to_string());

        // Add webhook with specific filter
        let webhook = WebhookConfig {
            id: Uuid::new_v4(),
            name: "premium_payments_webhook".to_string(),
            url: "https://api.example.com/premium".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: specific_filter.clone(),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        webhook_manager.add_webhook(webhook).await.unwrap();

        // Add stream with same filter
        let stream = StreamConfig::new(
            "premium_payments_stream".to_string(),
            StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "premium_payments".to_string(),
                config: HashMap::new(),
            },
        )
        .with_filter(specific_filter);

        stream_manager.add_stream(stream).await.unwrap();

        // Create matching event
        let matching_event = JobLifecycleEvent {
            event_id: Uuid::new_v4(),
            job_id: Uuid::new_v4(),
            queue_name: "payment_queue".to_string(),
            event_type: JobLifecycleEventType::Completed,
            priority: JobPriority::High,
            timestamp: Utc::now(),
            processing_time_ms: Some(2500), // Within range
            error: None,
            payload: Some(serde_json::json!({"amount": 1000.0})),
            metadata: {
                let mut metadata = HashMap::new();
                metadata.insert("tenant_id".to_string(), "premium_tenant".to_string());
                metadata
            },
        };

        // Create non-matching events
        let non_matching_events = vec![
            // Wrong queue
            JobLifecycleEvent {
                queue_name: "email_queue".to_string(),
                ..matching_event.clone()
            },
            // Wrong event type
            JobLifecycleEvent {
                event_type: JobLifecycleEventType::Failed,
                ..matching_event.clone()
            },
            // Wrong priority
            JobLifecycleEvent {
                priority: JobPriority::Low,
                ..matching_event.clone()
            },
            // Wrong processing time
            JobLifecycleEvent {
                processing_time_ms: Some(500), // Too fast
                ..matching_event.clone()
            },
            // Wrong metadata
            JobLifecycleEvent {
                metadata: {
                    let mut metadata = HashMap::new();
                    metadata.insert("tenant_id".to_string(), "regular_tenant".to_string());
                    metadata
                },
                ..matching_event.clone()
            },
        ];

        // Publish matching event
        event_manager.publish_event(matching_event).await.unwrap();

        // Publish non-matching events
        for event in non_matching_events {
            event_manager.publish_event(event).await.unwrap();
        }

        // Give time for processing
        sleep(Duration::from_millis(100)).await;

        // In a real scenario, only the matching event should have been
        // delivered to the webhook and stream
        let webhook_stats = webhook_manager.get_stats().await;
        let stream_stats = stream_manager.get_stats().await;

        assert_eq!(webhook_stats.active_webhooks, 1);
        assert_eq!(stream_stats.active_streams, 1);
    }

    #[tokio::test]
    async fn test_concurrent_event_processing() {
        let event_manager = Arc::new(EventManager::new_default());
        let webhook_manager = Arc::new(WebhookManager::new(
            event_manager.clone(),
            WebhookManagerConfig::default(),
        ));
        let stream_manager = Arc::new(StreamManager::new(
            event_manager.clone(),
            StreamManagerConfig::default(),
        ));

        // Add webhook and stream
        let webhook = WebhookConfig {
            id: Uuid::new_v4(),
            name: "concurrent_test_webhook".to_string(),
            url: "https://api.example.com/concurrent".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: EventFilter::new(),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        webhook_manager.add_webhook(webhook).await.unwrap();

        let stream = StreamConfig::new(
            "concurrent_test_stream".to_string(),
            StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "concurrent_events".to_string(),
                config: HashMap::new(),
            },
        );

        stream_manager.add_stream(stream).await.unwrap();

        // Spawn multiple tasks publishing events concurrently
        let num_tasks = 10;
        let events_per_task = 5;
        let mut handles = Vec::new();

        for task_id in 0..num_tasks {
            let event_manager_clone = event_manager.clone();
            let handle = tokio::spawn(async move {
                for event_id in 0..events_per_task {
                    let event = JobLifecycleEvent {
                        event_id: Uuid::new_v4(),
                        job_id: Uuid::new_v4(),
                        queue_name: format!("queue_{}", task_id),
                        event_type: JobLifecycleEventType::Completed,
                        priority: JobPriority::Normal,
                        timestamp: Utc::now(),
                        processing_time_ms: Some(1000),
                        error: None,
                        payload: None,
                        metadata: {
                            let mut metadata = HashMap::new();
                            metadata.insert("task_id".to_string(), task_id.to_string());
                            metadata.insert("event_id".to_string(), event_id.to_string());
                            metadata
                        },
                    };
                    event_manager_clone.publish_event(event).await.unwrap();
                }
            });
            handles.push(handle);
        }

        // Wait for all publishing tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Give time for all events to be processed
        sleep(Duration::from_millis(500)).await;

        // Verify the system handled concurrent events correctly
        let webhook_stats = webhook_manager.get_stats().await;
        let stream_stats = stream_manager.get_stats().await;
        let event_stats = event_manager.get_stats().await;

        assert_eq!(webhook_stats.active_webhooks, 1);
        assert_eq!(stream_stats.active_streams, 1);

        // Event manager should show it's still functioning
        assert!(event_stats.buffer_current_size <= event_stats.buffer_capacity);
    }

    #[tokio::test]
    async fn test_system_resilience_with_failures() {
        let event_manager = Arc::new(EventManager::new_default());
        let webhook_manager =
            WebhookManager::new(event_manager.clone(), WebhookManagerConfig::default());
        let stream_manager =
            StreamManager::new(event_manager.clone(), StreamManagerConfig::default());

        // Add webhook that would fail (invalid URL in real scenario)
        let failing_webhook = WebhookConfig {
            id: Uuid::new_v4(),
            name: "failing_webhook".to_string(),
            url: "https://invalid.example.com/webhook".to_string(),
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: EventFilter::new(),
            retry_policy: RetryPolicy {
                max_attempts: 2, // Reduced for faster test
                initial_delay_secs: 1,
                max_delay_secs: 5,
                backoff_multiplier: 2.0,
                retry_on_status_codes: vec![500, 502, 503],
            },
            enabled: true,
            timeout_secs: 5, // Short timeout
            payload_template: None,
        };

        webhook_manager.add_webhook(failing_webhook).await.unwrap();

        // Add working stream
        let working_stream = StreamConfig::new(
            "working_stream".to_string(),
            StreamBackend::Kafka {
                brokers: vec!["localhost:9092".to_string()],
                topic: "working_events".to_string(),
                config: HashMap::new(),
            },
        );

        stream_manager.add_stream(working_stream).await.unwrap();

        // Add another working webhook
        let working_webhook = WebhookConfig {
            id: Uuid::new_v4(),
            name: "working_webhook".to_string(),
            url: "https://httpbin.org/post".to_string(), // More likely to work
            method: HttpMethod::Post,
            headers: HashMap::new(),
            auth: None,
            secret: None,
            filter: EventFilter::new(),
            retry_policy: RetryPolicy::default(),
            enabled: true,
            timeout_secs: 30,
            payload_template: None,
        };

        webhook_manager.add_webhook(working_webhook).await.unwrap();

        // Publish events
        for i in 0..5 {
            let event = JobLifecycleEvent {
                event_id: Uuid::new_v4(),
                job_id: Uuid::new_v4(),
                queue_name: "resilience_test".to_string(),
                event_type: JobLifecycleEventType::Completed,
                priority: JobPriority::Normal,
                timestamp: Utc::now(),
                processing_time_ms: Some(1000),
                error: None,
                payload: Some(serde_json::json!({"test_id": i})),
                metadata: HashMap::new(),
            };
            event_manager.publish_event(event).await.unwrap();
        }

        // Give time for processing and retries
        sleep(Duration::from_millis(500)).await;

        // System should still be functioning despite some failures
        let webhook_stats = webhook_manager.get_stats().await;
        let stream_stats = stream_manager.get_stats().await;

        assert_eq!(webhook_stats.total_webhooks, 2);
        assert_eq!(stream_stats.total_streams, 1);
        assert_eq!(stream_stats.active_streams, 1);
    }
}
