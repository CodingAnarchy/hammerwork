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

/// The event types a client can subscribe to.
pub const EVENT_TYPES: [&str; 4] = [
    "queue_updates",
    "job_updates",
    "system_alerts",
    "archive_events",
];

/// What a connection wants to receive. A new connection receives every event until it
/// sends its first `Subscribe` or `Unsubscribe`.
#[derive(Debug)]
struct Subscription {
    all: bool,
    types: std::collections::HashSet<String>,
}

impl Subscription {
    fn everything() -> Self {
        Self {
            all: true,
            types: std::collections::HashSet::new(),
        }
    }

    fn wants(&self, event_type: &str) -> bool {
        self.all || self.types.contains(event_type)
    }

    fn subscribe(&mut self, event_types: Vec<String>) {
        // The first explicit choice replaces the default of "everything".
        self.all = false;
        self.types.extend(event_types);
    }

    fn unsubscribe(&mut self, event_types: &[String]) {
        if self.all {
            self.all = false;
            self.types = EVENT_TYPES.iter().map(|t| t.to_string()).collect();
        }
        for event_type in event_types {
            self.types.remove(event_type);
        }
    }
}

/// WebSocket connection state manager
#[derive(Debug)]
pub struct WebSocketState {
    config: WebSocketConfig,
    connections: HashMap<Uuid, mpsc::UnboundedSender<Message>>,
    subscriptions: HashMap<Uuid, Subscription>,
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

    /// Serve one WebSocket connection until it closes.
    ///
    /// The shared `state` is only locked for the short moments that register the connection,
    /// handle a client message and unregister it, never while waiting for the client: a
    /// connection that held the lock for its whole lifetime would block every other connection,
    /// the ping task and the broadcast listener.
    pub async fn serve_connection(
        state: Arc<tokio::sync::RwLock<WebSocketState>>,
        websocket: warp::ws::WebSocket,
    ) -> crate::Result<()> {
        let connection_id = Uuid::new_v4();
        let (mut ws_sender, mut ws_receiver) = websocket.split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        {
            let mut guard = state.write().await;
            if guard.connections.len() >= guard.config.max_connections {
                warn!("Maximum WebSocket connections reached, rejecting new connection");
                return Ok(());
            }
            guard.connections.insert(connection_id, tx);
            guard
                .subscriptions
                .insert(connection_id, Subscription::everything());
        }
        info!("WebSocket connection established: {}", connection_id);

        // Spawn task to handle outgoing messages to this client
        let writer = tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if let Err(e) = ws_sender.send(message).await {
                    debug!(
                        "Failed to send WebSocket message to {}: {}",
                        connection_id, e
                    );
                    break;
                }
            }
        });

        // Handle incoming messages from this client
        while let Some(result) = ws_receiver.next().await {
            match result {
                Ok(message) => {
                    let close = message.is_close();
                    let outcome = {
                        let mut guard = state.write().await;
                        guard.handle_client_message(connection_id, message).await
                    };
                    if let Err(e) = outcome {
                        error!(
                            "Error handling client message from {}: {}",
                            connection_id, e
                        );
                        break;
                    }
                    if close {
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
        {
            let mut guard = state.write().await;
            guard.connections.remove(&connection_id);
            guard.subscriptions.remove(&connection_id);
        }
        writer.abort();
        info!("WebSocket connection closed: {}", connection_id);

        Ok(())
    }

    /// Handle a message from a client
    async fn handle_client_message(
        &mut self,
        connection_id: Uuid,
        message: Message,
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
                let pong_msg = Message::pong(message.as_bytes().to_vec());
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
                self.subscriptions
                    .entry(connection_id)
                    .or_insert_with(Subscription::everything)
                    .subscribe(event_types);
            }
            ClientMessage::Unsubscribe { event_types } => {
                info!(
                    "Client {} unsubscribed from events: {:?}",
                    connection_id, event_types
                );
                self.subscriptions
                    .entry(connection_id)
                    .or_insert_with(Subscription::everything)
                    .unsubscribe(&event_types);
            }
            ClientMessage::Ping => {
                // Answer the client that asked, not everyone.
                if let Some(sender) = self.connections.get(&connection_id) {
                    let pong = Message::text(serde_json::to_string(&ServerMessage::Pong)?);
                    let _ = sender.send(pong);
                }
            }
        }

        Ok(())
    }

    /// Broadcast a message to all connected clients
    pub async fn broadcast_to_all(&self, message: ServerMessage) -> crate::Result<()> {
        let json_message = serde_json::to_string(&message)?;
        let ws_message = Message::text(json_message);

        // A closed channel belongs to a connection that is shutting down; its handler
        // removes it, so a failed send is not an error here.
        for sender in self.connections.values() {
            let _ = sender.send(ws_message.clone());
        }

        Ok(())
    }

    /// Broadcast a message to the clients subscribed to `event_type`
    pub async fn broadcast_to_subscribed(
        &self,
        message: ServerMessage,
        event_type: &str,
    ) -> crate::Result<()> {
        let json_message = serde_json::to_string(&message)?;
        let ws_message = Message::text(json_message);

        for (connection_id, sender) in &self.connections {
            let wanted = self
                .subscriptions
                .get(connection_id)
                .is_none_or(|subscription| subscription.wants(event_type));
            if wanted {
                let _ = sender.send(ws_message.clone());
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
        if self.broadcast_sender.send(broadcast_message).is_err() {
            return Err(anyhow::anyhow!(
                "Failed to send archive event to broadcast channel"
            ));
        }

        Ok(())
    }

    /// Send ping to all connections to keep them alive
    pub async fn ping_all_connections(&self) {
        let ping_message = Message::ping(b"ping".to_vec());
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

    use std::time::Duration;
    use tokio::sync::RwLock;
    use warp::Filter;

    type Shared = Arc<RwLock<WebSocketState>>;

    fn ws_route(
        state: Shared,
    ) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
        warp::path("ws")
            .and(warp::ws())
            .and(warp::any().map(move || state.clone()))
            .map(|ws: warp::ws::Ws, state: Shared| {
                ws.on_upgrade(move |socket| async move {
                    let _ = WebSocketState::serve_connection(state, socket).await;
                })
            })
    }

    async fn connect(
        route: &(impl Filter<Extract = (impl warp::Reply + 'static,), Error = warp::Rejection>
              + Clone
              + Send
              + Sync
              + 'static),
    ) -> warp::test::WsClient {
        warp::test::ws()
            .path("/ws")
            .handshake(route.clone())
            .await
            .expect("handshake")
    }

    async fn wait_for_connections(state: &Shared, expected: usize) {
        for _ in 0..200 {
            if state.read().await.connection_count() == expected {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "expected {expected} connections, have {}",
            state.read().await.connection_count()
        );
    }

    /// The next text message as JSON, or `None` if nothing arrives soon.
    async fn next_json(client: &mut warp::test::WsClient) -> Option<serde_json::Value> {
        match tokio::time::timeout(Duration::from_millis(400), client.recv()).await {
            Ok(Ok(message)) if message.is_text() => {
                Some(serde_json::from_str(message.to_str().unwrap()).unwrap())
            }
            Ok(Ok(other)) => panic!("unexpected frame: {other:?}"),
            Ok(Err(_)) | Err(_) => None,
        }
    }

    fn new_state() -> Shared {
        Arc::new(RwLock::new(WebSocketState::new(WebSocketConfig::default())))
    }

    fn alert(message: &str) -> ServerMessage {
        ServerMessage::SystemAlert {
            message: message.to_string(),
            severity: AlertSeverity::Warning,
        }
    }

    #[tokio::test]
    async fn several_clients_connect_at_once_and_all_receive_broadcasts() {
        let state = new_state();
        let route = ws_route(state.clone());
        let mut first = connect(&route).await;
        let mut second = connect(&route).await;
        let mut third = connect(&route).await;
        wait_for_connections(&state, 3).await;

        state
            .read()
            .await
            .broadcast_to_all(alert("hello"))
            .await
            .unwrap();
        for client in [&mut first, &mut second, &mut third] {
            let message = next_json(client).await.expect("broadcast delivered");
            assert_eq!(message["type"], "SystemAlert");
            assert_eq!(message["message"], "hello");
            assert_eq!(message["severity"], "Warning");
        }

        // Closing one connection frees only that connection.
        drop(first);
        wait_for_connections(&state, 2).await;
        state
            .read()
            .await
            .broadcast_to_all(alert("again"))
            .await
            .unwrap();
        assert!(next_json(&mut second).await.is_some());
        assert!(next_json(&mut third).await.is_some());
        drop((second, third));
        wait_for_connections(&state, 0).await;
    }

    #[tokio::test]
    async fn subscriptions_filter_events_and_new_clients_get_everything() {
        let state = new_state();
        let route = ws_route(state.clone());
        let mut everything = connect(&route).await;
        let mut archive_only = connect(&route).await;
        wait_for_connections(&state, 2).await;

        archive_only
            .send_text(r#"{"type": "Subscribe", "event_types": ["archive_events"]}"#)
            .await;
        // Give the server a moment to apply the subscription.
        tokio::time::sleep(Duration::from_millis(100)).await;

        let guard = state.read().await;
        guard
            .broadcast_to_subscribed(alert("a system alert"), "system_alerts")
            .await
            .unwrap();
        guard
            .broadcast_to_subscribed(
                ServerMessage::JobsPurged {
                    count: 3,
                    older_than: Utc::now(),
                },
                "archive_events",
            )
            .await
            .unwrap();
        drop(guard);

        assert_eq!(next_json(&mut everything).await.unwrap()["type"], "SystemAlert");
        assert_eq!(next_json(&mut everything).await.unwrap()["type"], "JobsPurged");
        let only = next_json(&mut archive_only).await.unwrap();
        assert_eq!(only["type"], "JobsPurged", "the alert was filtered out");
        assert_eq!(only["count"], 3);
        assert!(next_json(&mut archive_only).await.is_none());

        // Unsubscribing removes a type again; adding one brings it back.
        archive_only
            .send_text(r#"{"type": "Unsubscribe", "event_types": ["archive_events"]}"#)
            .await;
        archive_only
            .send_text(r#"{"type": "Subscribe", "event_types": ["system_alerts"]}"#)
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let guard = state.read().await;
        guard
            .broadcast_to_subscribed(
                ServerMessage::JobsPurged {
                    count: 1,
                    older_than: Utc::now(),
                },
                "archive_events",
            )
            .await
            .unwrap();
        guard
            .broadcast_to_subscribed(alert("now wanted"), "system_alerts")
            .await
            .unwrap();
        drop(guard);
        let got = next_json(&mut archive_only).await.unwrap();
        assert_eq!(got["message"], "now wanted");
        assert!(next_json(&mut archive_only).await.is_none());

        // Unsubscribing from a type while on the default keeps all the others.
        let mut fresh = connect(&route).await;
        wait_for_connections(&state, 3).await;
        fresh
            .send_text(r#"{"type": "Unsubscribe", "event_types": ["job_updates"]}"#)
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let guard = state.read().await;
        guard
            .broadcast_to_subscribed(alert("kept"), "system_alerts")
            .await
            .unwrap();
        guard
            .broadcast_to_subscribed(alert("dropped"), "job_updates")
            .await
            .unwrap();
        drop(guard);
        assert_eq!(next_json(&mut fresh).await.unwrap()["message"], "kept");
        assert!(next_json(&mut fresh).await.is_none());
    }

    #[tokio::test]
    async fn a_client_ping_is_answered_to_that_client_only() {
        let state = new_state();
        let route = ws_route(state.clone());
        let mut asker = connect(&route).await;
        let mut bystander = connect(&route).await;
        wait_for_connections(&state, 2).await;

        asker.send_text(r#"{"type": "Ping"}"#).await;
        assert_eq!(next_json(&mut asker).await.unwrap()["type"], "Pong");
        assert!(next_json(&mut bystander).await.is_none());

        // A protocol-level ping gets a protocol-level pong.
        asker.send(warp::ws::Message::ping(b"hi".to_vec())).await;
        let reply = tokio::time::timeout(Duration::from_secs(1), asker.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(reply.is_pong());
        assert_eq!(reply.as_bytes(), b"hi");
    }

    #[tokio::test]
    async fn malformed_and_unsupported_messages_are_ignored() {
        let state = new_state();
        let route = ws_route(state.clone());
        let mut client = connect(&route).await;
        wait_for_connections(&state, 1).await;

        client.send_text("not json at all").await;
        client.send_text(r#"{"type": "Dance"}"#).await;
        client.send(warp::ws::Message::binary(vec![1, 2, 3])).await;
        client.send(warp::ws::Message::pong(b"x".to_vec())).await;

        // The connection survives and still works.
        client.send_text(r#"{"type": "Ping"}"#).await;
        assert_eq!(next_json(&mut client).await.unwrap()["type"], "Pong");
        assert_eq!(state.read().await.connection_count(), 1);

        client.send(warp::ws::Message::close()).await;
        wait_for_connections(&state, 0).await;
    }

    #[tokio::test]
    async fn connections_beyond_the_limit_are_turned_away() {
        let state = Arc::new(RwLock::new(WebSocketState::new(WebSocketConfig {
            max_connections: 1,
            ..WebSocketConfig::default()
        })));
        let route = ws_route(state.clone());
        let mut first = connect(&route).await;
        wait_for_connections(&state, 1).await;

        let mut rejected = connect(&route).await;
        // The server drops the extra socket without registering it.
        let end = tokio::time::timeout(Duration::from_secs(1), rejected.recv()).await;
        assert!(matches!(end, Ok(Err(_))) || matches!(end, Ok(Ok(ref m)) if m.is_close()));
        assert_eq!(state.read().await.connection_count(), 1);

        state.read().await.broadcast_to_all(alert("x")).await.unwrap();
        assert!(next_json(&mut first).await.is_some());
    }

    #[tokio::test]
    async fn the_ping_task_reaches_every_connection() {
        let state = new_state();
        let route = ws_route(state.clone());
        let mut client = connect(&route).await;
        wait_for_connections(&state, 1).await;

        state.read().await.ping_all_connections().await;
        let frame = tokio::time::timeout(Duration::from_secs(1), client.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(frame.is_ping());
        assert_eq!(frame.as_bytes(), b"ping");
        drop(client);
        wait_for_connections(&state, 0).await;
        // With nobody left, pinging is a no-op.
        state.read().await.ping_all_connections().await;
    }

    #[tokio::test]
    async fn archive_events_flow_through_the_broadcast_listener() {
        use hammerwork::archive::ArchiveEvent;
        let state = new_state();
        WebSocketState::start_broadcast_listener(state.clone())
            .await
            .unwrap();
        // The receiver can only be taken once.
        WebSocketState::start_broadcast_listener(state.clone())
            .await
            .unwrap();
        let route = ws_route(state.clone());
        let mut client = connect(&route).await;
        wait_for_connections(&state, 1).await;

        let job_id = Uuid::new_v4();
        let older_than = Utc::now();
        let events = vec![
            ArchiveEvent::JobArchived {
                job_id,
                queue: "q".into(),
                reason: ArchivalReason::Manual,
            },
            ArchiveEvent::JobRestored {
                job_id,
                queue: "q".into(),
                restored_by: Some("me".into()),
            },
            ArchiveEvent::BulkArchiveStarted {
                operation_id: "op".into(),
                estimated_jobs: 10,
            },
            ArchiveEvent::BulkArchiveProgress {
                operation_id: "op".into(),
                jobs_processed: 5,
                total: 10,
            },
            ArchiveEvent::BulkArchiveCompleted {
                operation_id: "op".into(),
                stats: ArchivalStats::default(),
            },
            ArchiveEvent::JobsPurged {
                count: 2,
                older_than,
            },
        ];
        for event in events {
            state.read().await.publish_archive_event(event).await.unwrap();
        }

        let mut seen = Vec::new();
        for _ in 0..6 {
            seen.push(next_json(&mut client).await.expect("event delivered"));
        }
        let types: Vec<&str> = seen.iter().map(|m| m["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec![
                "JobArchived",
                "JobRestored",
                "BulkArchiveStarted",
                "BulkArchiveProgress",
                "BulkArchiveCompleted",
                "JobsPurged"
            ]
        );
        assert_eq!(seen[0]["job_id"], job_id.to_string());
        assert_eq!(seen[0]["reason"], "Manual");
        assert_eq!(seen[1]["restored_by"], "me");
        assert_eq!(seen[2]["estimated_jobs"], 10);
        assert_eq!(seen[3]["jobs_processed"], 5);
        assert_eq!(seen[4]["stats"]["jobs_archived"], 0);
        assert_eq!(seen[5]["count"], 2);
    }

    #[tokio::test]
    async fn the_other_broadcast_kinds_are_converted_and_filtered_by_type() {
        let state = new_state();
        WebSocketState::start_broadcast_listener(state.clone())
            .await
            .unwrap();
        let route = ws_route(state.clone());
        let mut queue_only = connect(&route).await;
        wait_for_connections(&state, 1).await;
        queue_only
            .send_text(r#"{"type": "Subscribe", "event_types": ["queue_updates", "job_updates", "system_alerts"]}"#)
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;

        let sender = state.read().await.broadcast_sender.clone();
        let now = Utc::now();
        sender
            .send(BroadcastMessage::QueueUpdate {
                queue_name: "emails".into(),
                stats: QueueStats {
                    pending_count: 1,
                    running_count: 2,
                    completed_count: 3,
                    failed_count: 4,
                    dead_count: 5,
                    throughput_per_minute: 6.0,
                    avg_processing_time_ms: 7.0,
                    error_rate: 0.5,
                    updated_at: now,
                },
            })
            .unwrap();
        sender
            .send(BroadcastMessage::JobUpdate {
                job: JobUpdate {
                    id: "j1".into(),
                    queue_name: "emails".into(),
                    status: "Running".into(),
                    priority: "High".into(),
                    attempts: 1,
                    updated_at: now,
                },
            })
            .unwrap();
        sender
            .send(BroadcastMessage::SystemAlert {
                message: "disk".into(),
                severity: AlertSeverity::Critical,
            })
            .unwrap();
        // Not subscribed to archive events: never delivered.
        sender
            .send(BroadcastMessage::JobsPurged {
                count: 9,
                older_than: now,
            })
            .unwrap();

        let queue = next_json(&mut queue_only).await.unwrap();
        assert_eq!(queue["type"], "QueueUpdate");
        assert_eq!(queue["queue_name"], "emails");
        assert_eq!(queue["stats"]["dead_count"], 5);
        let job = next_json(&mut queue_only).await.unwrap();
        assert_eq!((job["type"].as_str(), job["job"]["id"].as_str()), (Some("JobUpdate"), Some("j1")));
        let alert = next_json(&mut queue_only).await.unwrap();
        assert_eq!(alert["severity"], "Critical");
        assert!(next_json(&mut queue_only).await.is_none());
    }

    #[test]
    fn subscription_rules() {
        let mut sub = Subscription::everything();
        assert!(sub.wants("anything"));
        sub.unsubscribe(&["job_updates".to_string()]);
        assert!(!sub.wants("job_updates") && sub.wants("queue_updates"));
        assert!(!sub.wants("custom"), "after the first change only known types remain");

        let mut sub = Subscription::everything();
        sub.subscribe(vec!["archive_events".to_string()]);
        assert!(sub.wants("archive_events") && !sub.wants("queue_updates"));
        sub.subscribe(vec!["queue_updates".to_string()]);
        assert!(sub.wants("queue_updates") && sub.wants("archive_events"));
    }

    #[test]
    fn every_server_message_is_tagged_with_its_type() {
        let now = Utc::now();
        let messages = vec![
            (ServerMessage::Pong, "Pong"),
            (alert("x"), "SystemAlert"),
            (
                ServerMessage::JobArchived {
                    job_id: "j".into(),
                    queue: "q".into(),
                    reason: ArchivalReason::Automatic,
                },
                "JobArchived",
            ),
            (
                ServerMessage::JobRestored {
                    job_id: "j".into(),
                    queue: "q".into(),
                    restored_by: None,
                },
                "JobRestored",
            ),
            (
                ServerMessage::BulkArchiveStarted {
                    operation_id: "o".into(),
                    estimated_jobs: 1,
                },
                "BulkArchiveStarted",
            ),
            (
                ServerMessage::JobsPurged {
                    count: 1,
                    older_than: now,
                },
                "JobsPurged",
            ),
        ];
        for (message, expected) in messages {
            let json: serde_json::Value =
                serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
            assert_eq!(json["type"], expected);
        }
        for severity in [
            AlertSeverity::Info,
            AlertSeverity::Warning,
            AlertSeverity::Error,
            AlertSeverity::Critical,
        ] {
            assert!(serde_json::to_string(&severity).unwrap().starts_with('"'));
        }
        for text in [
            r#"{"type": "Ping"}"#,
            r#"{"type": "Unsubscribe", "event_types": []}"#,
        ] {
            assert!(serde_json::from_str::<ClientMessage>(text).is_ok());
        }
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type": "Subscribe"}"#).is_err());
    }
}
