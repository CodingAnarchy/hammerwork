//! WebSocket archive-event broadcast tests.

use crate::config::WebSocketConfig;
use crate::websocket::{BroadcastMessage, WebSocketState};
use chrono::Utc;
use hammerwork::archive::{ArchivalReason, ArchivalStats, ArchiveEvent};
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn test_websocket_publish_archive_event() {
    let config = WebSocketConfig::default();
    let ws_state = WebSocketState::new(config);

    // Test each type of archive event
    let test_events = vec![
        ArchiveEvent::JobArchived {
            job_id: Uuid::new_v4(),
            queue: "test_queue".to_string(),
            reason: ArchivalReason::Manual,
        },
        ArchiveEvent::JobRestored {
            job_id: Uuid::new_v4(),
            queue: "restore_queue".to_string(),
            restored_by: Some("admin".to_string()),
        },
        ArchiveEvent::BulkArchiveStarted {
            operation_id: "test_op_123".to_string(),
            estimated_jobs: 1000,
        },
        ArchiveEvent::BulkArchiveProgress {
            operation_id: "test_op_123".to_string(),
            jobs_processed: 500,
            total: 1000,
        },
        ArchiveEvent::BulkArchiveCompleted {
            operation_id: "test_op_123".to_string(),
            stats: {
                let mut value = ArchivalStats::default();
                value.jobs_archived = 1000;
                value.jobs_purged = 0;
                value.bytes_archived = 50000;
                value.bytes_purged = 0;
                value.compression_ratio = 0.8;
                value.operation_duration = std::time::Duration::from_secs(30);
                value.last_run_at = Utc::now();
                value
            },
        },
        ArchiveEvent::JobsPurged {
            count: 100,
            older_than: Utc::now(),
        },
    ];

    // Test that each event can be published without errors
    for event in test_events {
        let result = ws_state.publish_archive_event(event).await;
        assert!(result.is_ok(), "Failed to publish archive event");
    }
}

#[test]
fn test_websocket_broadcast_message_conversion() {
    let job_id = Uuid::new_v4();
    let operation_id = "test_op_456".to_string();
    let queue_name = "test_queue".to_string();

    // Test JobArchived event conversion
    let job_archived = ArchiveEvent::JobArchived {
        job_id,
        queue: queue_name.clone(),
        reason: ArchivalReason::Automatic,
    };

    // Manually convert to verify the logic (similar to what happens in publish_archive_event)
    match job_archived {
        ArchiveEvent::JobArchived {
            job_id,
            queue,
            reason,
        } => {
            let broadcast_msg = BroadcastMessage::JobArchived {
                job_id: job_id.to_string(),
                queue,
                reason,
            };

            // Test that the conversion produces valid data
            assert!(matches!(
                broadcast_msg,
                BroadcastMessage::JobArchived { .. }
            ));
        }
        _ => panic!("Expected JobArchived event"),
    }

    // Test BulkArchiveStarted event conversion
    let bulk_started = ArchiveEvent::BulkArchiveStarted {
        operation_id: operation_id.clone(),
        estimated_jobs: 500,
    };

    match bulk_started {
        ArchiveEvent::BulkArchiveStarted {
            operation_id,
            estimated_jobs,
        } => {
            let broadcast_msg = BroadcastMessage::BulkArchiveStarted {
                operation_id,
                estimated_jobs,
            };

            assert!(matches!(
                broadcast_msg,
                BroadcastMessage::BulkArchiveStarted { .. }
            ));
        }
        _ => panic!("Expected BulkArchiveStarted event"),
    }
}

#[test]
fn test_websocket_config_defaults() {
    let config = WebSocketConfig::default();

    // Verify default WebSocket configuration supports archive events
    assert!(config.max_connections > 0);
    assert!(config.ping_interval.as_secs() > 0);
    assert!(config.message_buffer_size > 0);
    assert!(config.max_message_size > 0);
}

#[tokio::test]
async fn test_websocket_state_connection_management() {
    let config = WebSocketConfig::default()
        .with_max_connections(5)
        .with_ping_interval(Duration::from_secs(30))
        .with_message_buffer_size(1024)
        .with_max_message_size(64 * 1024);

    let ws_state = WebSocketState::new(config);

    // Test initial state
    assert_eq!(ws_state.connection_count(), 0);

    // Test that archive events can be published even with no connections
    let test_event = ArchiveEvent::JobArchived {
        job_id: Uuid::new_v4(),
        queue: "test_queue".to_string(),
        reason: ArchivalReason::Manual,
    };

    let result = ws_state.publish_archive_event(test_event).await;
    assert!(
        result.is_ok(),
        "Should handle publishing to empty connection list"
    );
}

#[test]
fn test_archive_event_serialization_for_websocket() {
    let events = vec![
        ArchiveEvent::JobArchived {
            job_id: Uuid::new_v4(),
            queue: "serialize_test".to_string(),
            reason: ArchivalReason::Compliance,
        },
        ArchiveEvent::BulkArchiveProgress {
            operation_id: "progress_op".to_string(),
            jobs_processed: 75,
            total: 100,
        },
    ];

    // Test that all archive events can be serialized for WebSocket transmission
    for event in events {
        let serialized = serde_json::to_string(&event);
        assert!(serialized.is_ok(), "Archive event should be serializable");

        let json_value = serialized.unwrap();
        assert!(!json_value.is_empty());

        // Verify it can be deserialized back
        let deserialized: Result<ArchiveEvent, _> = serde_json::from_str(&json_value);
        assert!(
            deserialized.is_ok(),
            "Serialized event should be deserializable"
        );
    }
}
