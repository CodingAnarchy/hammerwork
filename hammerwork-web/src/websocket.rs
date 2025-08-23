//! WebSocket implementation for real-time dashboard updates.
//!
//! This module provides WebSocket functionality for real-time communication between
//! the dashboard frontend and backend. It supports connection management, message
//! broadcasting, and automatic ping/pong for connection health.
//!
//! # Message Types
//!
//! The WebSocket API supports several message types for different events:
//!
//! ```rust
//! use hammerwork_web::websocket::{ClientMessage, ServerMessage, AlertSeverity};
//! use serde_json::json;
//!
//! // Client messages (sent from browser to server)
//! let subscribe_msg = ClientMessage::Subscribe {
//!     event_types: vec!["queue_updates".to_string(), "job_updates".to_string()],
//! };
//!
//! let ping_msg = ClientMessage::Ping;
//!
//! // Server messages (sent from server to browser)
//! let alert_msg = ServerMessage::SystemAlert {
//!     message: "High error rate detected".to_string(),
//!     severity: AlertSeverity::Warning,
//! };
//!
//! let pong_msg = ServerMessage::Pong;
//! ```
//!
//! # Connection Management
//!
//! ```rust
//! use hammerwork_web::websocket::WebSocketState;
//! use hammerwork_web::config::WebSocketConfig;
//!
//! let config = WebSocketConfig::default();
//! let ws_state = WebSocketState::new(config);
//!
//! assert_eq!(ws_state.connection_count(), 0);
//! ```

use crate::config::WebSocketConfig;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, StreamExt};
pub use hammerwork::archive::{ArchivalReason, ArchivalStats};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use warp::ws::Message;

/// WebSocket connection state manager
#[derive(Debug)]
pub struct WebSocketState {
    config: WebSocketConfig,
    connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
    subscriptions: HashMap<Uuid, std::collections::HashSet<String>>,
    broadcast_sender: mpsc::UnboundedSender<BroadcastMessage>,
    broadcast_receiver: Option<mpsc::UnboundedReceiver<BroadcastMessage>>,
}

impl WebSocketState {
    pub fn new(config: WebSocketConfig) -> Self {
        let (broadcast_sender, broadcast_receiver) = mpsc::unbounded_channel();

        Self {
            config,
            connections: HashMap::new(),
            subscriptions: HashMap::new(),
            broadcast_sender,
            broadcast_receiver: Some(broadcast_receiver),
        }
    }

    /// Handle a new WebSocket connection
    pub async fn handle_connection(&mut self, websocket: warp::ws::WebSocket) -> crate::Result<()> {
        let connection_id = Uuid::new_v4();

        if self.connections.len() >= self.config.max_connections {
            warn!("Maximum WebSocket connections reached, rejecting new connection");
            return Ok(());
        }

        let (mut ws_sender, mut ws_receiver) = websocket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        // Store the connection
        self.connections.insert(connection_id, tx);
        info!("WebSocket connection established: {}", connection_id);

        // Spawn task to handle outgoing messages to this client
        let connection_id_clone = connection_id;
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if let Err(e) = ws_sender.send(message).await {
                    debug!(
                        "Failed to send WebSocket message to {}: {}",
                        connection_id_clone, e
                    );
                    break;
                }
            }
        });

        // Handle incoming messages from this client
        let broadcast_sender = self.broadcast_sender.clone();

        while let Some(result) = ws_receiver.next().await {
            match result {
                Ok(message) => {
                    if let Err(e) = self
                        .handle_client_message(connection_id, message, &broadcast_sender)
                        .await
                    {
                        error!(
                            "Error handling client message from {}: {}",
                            connection_id, e
                        );
                        break;
                    }
                }
                Err(e) => {
                    debug!("WebSocket error for connection {}: {}", connection_id, e);
                    break;
                }
            }
        }

        // Clean up connection and subscriptions
        self.connections.remove(&connection_id);
        self.subscriptions.remove(&connection_id);
        info!("WebSocket connection closed: {}", connection_id);

        Ok(())
    }

    /// Handle a message from a client
    async fn handle_client_message(
        &mut self,
        connection_id: Uuid,
        message: Message,
        _broadcast_sender: &mpsc::UnboundedSender<BroadcastMessage>,
    ) -> crate::Result<()> {
        if message.is_text() {
            if let Ok(text) = message.to_str() {
                if let Ok(client_message) = serde_json::from_str::<ClientMessage>(text) {
                    debug!(
                        "Received message from {}: {:?}",
                        connection_id, client_message
                    );
                    self.handle_client_action(connection_id, client_message)
                        .await?;
                } else {
                    warn!("Invalid message format from {}: {}", connection_id, text);
                }
            }
        } else if message.is_ping() {
            // Send pong response
            if let Some(sender) = self.connections.get(&connection_id) {
                let pong_msg = Message::pong(message.as_bytes());
                let _ = sender.send(pong_msg);
            }
        } else if message.is_pong() {
            // Pong received - connection is alive
            debug!("Pong received from {}", connection_id);
        } else if message.is_close() {
            debug!("Close message received from {}", connection_id);
        } else if message.is_binary() {
            warn!("Binary message not supported from {}", connection_id);
        }

        Ok(())
    }

    /// Handle a client action
    async fn handle_client_action(
        &mut self,
        connection_id: Uuid,
        message: ClientMessage,
    ) -> crate::Result<()> {
        match message {
            ClientMessage::Subscribe { event_types } => {
                info!(
                    "Client {} subscribed to events: {:?}",
                    connection_id, event_types
                );
                // Store subscription preferences per connection
                let subscription_set = self
                    .subscriptions
                    .entry(connection_id)
                    .or_insert_with(std::collections::HashSet::new);
                for event_type in event_types {
                    subscription_set.insert(event_type);
                }
            }
            ClientMessage::Unsubscribe { event_types } => {
                info!(
                    "Client {} unsubscribed from events: {:?}",
                    connection_id, event_types
                );
                // Update subscription preferences
                if let Some(subscription_set) = self.subscriptions.get_mut(&connection_id) {
                    for event_type in event_types {
                        subscription_set.remove(&event_type);
                    }
                    // Remove empty subscription sets
                    if subscription_set.is_empty() {
                        self.subscriptions.remove(&connection_id);
                    }
                }
            }
            ClientMessage::Ping => {
                // Client ping - we'll send a pong back via broadcast
                self.broadcast_to_all(ServerMessage::Pong).await?;
            }
        }

        Ok(())
    }

    /// Broadcast a message to all connected clients
    pub async fn broadcast_to_all(&self, message: ServerMessage) -> crate::Result<()> {
        let json_message = serde_json::to_string(&message)?;
        let ws_message = Message::text(json_message);

        let mut disconnected = Vec::new();

        for (&connection_id, sender) in &self.connections {
            if sender.send(ws_message.clone()).is_err() {
                disconnected.push(connection_id);
            }
        }

        // Clean up disconnected clients
        // Note: In a real implementation, we'd need mutable access to self.connections
        // This would be handled by the connection cleanup in handle_connection

        Ok(())
    }

    /// Broadcast a message to subscribed clients only
    pub async fn broadcast_to_subscribed(
        &self,
        message: ServerMessage,
        event_type: &str,
    ) -> crate::Result<()> {
        let json_message = serde_json::to_string(&message)?;
        let ws_message = Message::text(json_message);

        let mut disconnected = Vec::new();

        for (&connection_id, sender) in &self.connections {
            // Check if this connection is subscribed to this event type
            if let Some(subscription_set) = self.subscriptions.get(&connection_id) {
                if subscription_set.contains(event_type) {
                    if sender.send(ws_message.clone()).is_err() {
                        disconnected.push(connection_id);
                    }
                }
            }
        }

        Ok(())
    }

    /// Publish an archive event to all connected clients
    pub async fn publish_archive_event(
        &self,
        event: hammerwork::archive::ArchiveEvent,
    ) -> crate::Result<()> {
        let broadcast_message = match event {
            hammerwork::archive::ArchiveEvent::JobArchived {
                job_id,
                queue,
                reason,
            } => BroadcastMessage::JobArchived {
                job_id: job_id.to_string(),
                queue,
                reason,
            },
            hammerwork::archive::ArchiveEvent::JobRestored {
                job_id,
                queue,
                restored_by,
            } => BroadcastMessage::JobRestored {
                job_id: job_id.to_string(),
                queue,
                restored_by,
            },
            hammerwork::archive::ArchiveEvent::BulkArchiveStarted {
                operation_id,
                estimated_jobs,
            } => BroadcastMessage::BulkArchiveStarted {
                operation_id,
                estimated_jobs,
            },
            hammerwork::archive::ArchiveEvent::BulkArchiveProgress {
                operation_id,
                jobs_processed,
                total,
            } => BroadcastMessage::BulkArchiveProgress {
                operation_id,
                jobs_processed,
                total,
            },
            hammerwork::archive::ArchiveEvent::BulkArchiveCompleted {
                operation_id,
                stats,
            } => BroadcastMessage::BulkArchiveCompleted {
                operation_id,
                stats,
            },
            hammerwork::archive::ArchiveEvent::JobsPurged { count, older_than } => {
                BroadcastMessage::JobsPurged { count, older_than }
            }
        };

        // Send to the broadcast channel
        if let Err(_) = self.broadcast_sender.send(broadcast_message) {
            return Err(anyhow::anyhow!(
                "Failed to send archive event to broadcast channel"
            ));
        }

        Ok(())
    }

    /// Send ping to all connections to keep them alive
    pub async fn ping_all_connections(&self) {
        let ping_message = Message::ping(b"ping");
        let mut disconnected = Vec::new();

        for (&connection_id, sender) in &self.connections {
            if sender.send(ping_message.clone()).is_err() {
                disconnected.push(connection_id);
            }
        }

        if !disconnected.is_empty() {
            debug!(
                "Detected {} disconnected WebSocket clients during ping",
                disconnected.len()
            );
        }
    }

    /// Get current connection count
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Start the broadcast listener task
    pub async fn start_broadcast_listener(
        state: Arc<tokio::sync::RwLock<WebSocketState>>,
    ) -> crate::Result<()> {
        let mut state_guard = state.write().await;
        if let Some(mut receiver) = state_guard.broadcast_receiver.take() {
            drop(state_guard); // Release the lock before spawning the task

            tokio::spawn(async move {
                while let Some(broadcast_message) = receiver.recv().await {
                    // Determine the event type for subscription filtering
                    let event_type = match &broadcast_message {
                        BroadcastMessage::QueueUpdate { .. } => "queue_updates",
                        BroadcastMessage::JobUpdate { .. } => "job_updates",
                        BroadcastMessage::SystemAlert { .. } => "system_alerts",
                        BroadcastMessage::JobArchived { .. } => "archive_events",
                        BroadcastMessage::JobRestored { .. } => "archive_events",
                        BroadcastMessage::BulkArchiveStarted { .. } => "archive_events",
                        BroadcastMessage::BulkArchiveProgress { .. } => "archive_events",
                        BroadcastMessage::BulkArchiveCompleted { .. } => "archive_events",
                        BroadcastMessage::JobsPurged { .. } => "archive_events",
                    };

                    // Convert broadcast message to server message
                    let server_message = match broadcast_message {
                        BroadcastMessage::QueueUpdate { queue_name, stats } => {
                            ServerMessage::QueueUpdate { queue_name, stats }
                        }
                        BroadcastMessage::JobUpdate { job } => ServerMessage::JobUpdate { job },
                        BroadcastMessage::SystemAlert { message, severity } => {
                            ServerMessage::SystemAlert { message, severity }
                        }
                        BroadcastMessage::JobArchived {
                            job_id,
                            queue,
                            reason,
                        } => ServerMessage::JobArchived {
                            job_id,
                            queue,
                            reason,
                        },
                        BroadcastMessage::JobRestored {
                            job_id,
                            queue,
                            restored_by,
                        } => ServerMessage::JobRestored {
                            job_id,
                            queue,
                            restored_by,
                        },
                        BroadcastMessage::BulkArchiveStarted {
                            operation_id,
                            estimated_jobs,
                        } => ServerMessage::BulkArchiveStarted {
                            operation_id,
                            estimated_jobs,
                        },
                        BroadcastMessage::BulkArchiveProgress {
                            operation_id,
                            jobs_processed,
                            total,
                        } => ServerMessage::BulkArchiveProgress {
                            operation_id,
                            jobs_processed,
                            total,
                        },
                        BroadcastMessage::BulkArchiveCompleted {
                            operation_id,
                            stats,
                        } => ServerMessage::BulkArchiveCompleted {
                            operation_id,
                            stats,
                        },
                        BroadcastMessage::JobsPurged { count, older_than } => {
                            ServerMessage::JobsPurged { count, older_than }
                        }
                    };

                    // Actually broadcast the message to subscribed clients
                    let state_read = state.read().await;
                    if let Err(e) = state_read
                        .broadcast_to_subscribed(server_message, event_type)
                        .await
                    {
                        error!("Failed to broadcast message: {}", e);
                    }
                }
            });
        }
        Ok(())
    }
}

/// Messages sent from client to server
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMessage {
    Subscribe { event_types: Vec<String> },
    Unsubscribe { event_types: Vec<String> },
    Ping,
}

/// Messages sent from server to client
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ServerMessage {
    QueueUpdate {
        queue_name: String,
        stats: QueueStats,
    },
    JobUpdate {
        job: JobUpdate,
    },
    SystemAlert {
        message: String,
        severity: AlertSeverity,
    },
    JobArchived {
        job_id: String,
        queue: String,
        reason: ArchivalReason,
    },
    JobRestored {
        job_id: String,
        queue: String,
        restored_by: Option<String>,
    },
    BulkArchiveStarted {
        operation_id: String,
        estimated_jobs: u64,
    },
    BulkArchiveProgress {
        operation_id: String,
        jobs_processed: u64,
        total: u64,
    },
    BulkArchiveCompleted {
        operation_id: String,
        stats: ArchivalStats,
    },
    JobsPurged {
        count: u64,
        older_than: DateTime<Utc>,
    },
    Pong,
}

/// Internal broadcast messages
#[derive(Debug)]
pub enum BroadcastMessage {
    QueueUpdate {
        queue_name: String,
        stats: QueueStats,
    },
    JobUpdate {
        job: JobUpdate,
    },
    SystemAlert {
        message: String,
        severity: AlertSeverity,
    },
    JobArchived {
        job_id: String,
        queue: String,
        reason: ArchivalReason,
    },
    JobRestored {
        job_id: String,
        queue: String,
        restored_by: Option<String>,
    },
    BulkArchiveStarted {
        operation_id: String,
        estimated_jobs: u64,
    },
    BulkArchiveProgress {
        operation_id: String,
        jobs_processed: u64,
        total: u64,
    },
    BulkArchiveCompleted {
        operation_id: String,
        stats: ArchivalStats,
    },
    JobsPurged {
        count: u64,
        older_than: DateTime<Utc>,
    },
}

/// Queue statistics for WebSocket updates
#[derive(Debug, Serialize)]
pub struct QueueStats {
    pub pending_count: u64,
    pub running_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub dead_count: u64,
    pub throughput_per_minute: f64,
    pub avg_processing_time_ms: f64,
    pub error_rate: f64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Job update information
#[derive(Debug, Serialize)]
pub struct JobUpdate {
    pub id: String,
    pub queue_name: String,
    pub status: String,
    pub priority: String,
    pub attempts: i32,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Alert severity levels
#[derive(Debug, Serialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebSocketConfig;

    #[test]
    fn test_websocket_state_creation() {
        let config = WebSocketConfig::default();
        let state = WebSocketState::new(config);
        assert_eq!(state.connection_count(), 0);
    }

    #[test]
    fn test_client_message_deserialization() {
        let json = r#"{"type": "Subscribe", "event_types": ["queue_updates", "job_updates"]}"#;
        let message: ClientMessage = serde_json::from_str(json).unwrap();

        match message {
            ClientMessage::Subscribe { event_types } => {
                assert_eq!(event_types.len(), 2);
                assert!(event_types.contains(&"queue_updates".to_string()));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_server_message_serialization() {
        let message = ServerMessage::SystemAlert {
            message: "High error rate detected".to_string(),
            severity: AlertSeverity::Warning,
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("type"));
        assert!(json.contains("SystemAlert"));
        assert!(json.contains("High error rate detected"));
    }

    #[tokio::test]
    async fn test_broadcast_to_all() {
        let config = WebSocketConfig::default();
        let state = WebSocketState::new(config);

        let message = ServerMessage::Pong;
        let result = state.broadcast_to_all(message).await;
        assert!(result.is_ok());
    }
}
