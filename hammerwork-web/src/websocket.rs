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
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use warp::ws::Message;

/// WebSocket connection state manager
#[derive(Debug)]
pub struct WebSocketState {
    config: WebSocketConfig,
    connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
    broadcast_sender: mpsc::UnboundedSender<BroadcastMessage>,
    broadcast_receiver: Option<mpsc::UnboundedReceiver<BroadcastMessage>>,
}

impl WebSocketState {
    pub fn new(config: WebSocketConfig) -> Self {
        let (broadcast_sender, broadcast_receiver) = mpsc::unbounded_channel();

        Self {
            config,
            connections: HashMap::new(),
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

        // Clean up connection
        self.connections.remove(&connection_id);
        info!("WebSocket connection closed: {}", connection_id);

        Ok(())
    }

    /// Handle a message from a client
    async fn handle_client_message(
        &self,
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
        &self,
        _connection_id: Uuid,
        message: ClientMessage,
    ) -> crate::Result<()> {
        match message {
            ClientMessage::Subscribe { event_types } => {
                info!("Client subscribed to events: {:?}", event_types);
                // TODO: Store subscription preferences per connection
            }
            ClientMessage::Unsubscribe { event_types } => {
                info!("Client unsubscribed from events: {:?}", event_types);
                // TODO: Update subscription preferences
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
    pub async fn start_broadcast_listener(&mut self) -> crate::Result<()> {
        if let Some(mut receiver) = self.broadcast_receiver.take() {
            tokio::spawn(async move {
                while let Some(broadcast_message) = receiver.recv().await {
                    // Convert broadcast message to server message and send to all clients
                    let server_message = match broadcast_message {
                        BroadcastMessage::QueueUpdate { queue_name, stats } => {
                            ServerMessage::QueueUpdate { queue_name, stats }
                        }
                        BroadcastMessage::JobUpdate { job } => ServerMessage::JobUpdate { job },
                        BroadcastMessage::SystemAlert { message, severity } => {
                            ServerMessage::SystemAlert { message, severity }
                        }
                    };

                    // Note: We'd need access to the WebSocketState here to broadcast
                    // In a real implementation, this would be handled differently
                    debug!("Broadcasting message: {:?}", server_message);
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
