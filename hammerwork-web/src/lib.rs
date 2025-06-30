//! # Hammerwork Web Dashboard
//!
//! A web-based admin dashboard for monitoring and managing Hammerwork job queues.
//!
//! This crate provides a comprehensive web interface for:
//! - Real-time queue monitoring and statistics
//! - Job management (retry, cancel, inspect)
//! - Worker status and utilization
//! - Dead job analysis and bulk operations
//! - System health monitoring
//!
//! ## Usage
//!
//! ### As a Binary
//!
//! ```bash
//! # Install the web dashboard
//! cargo install hammerwork-web --features postgres
//!
//! # Start the dashboard
//! hammerwork-web --database-url postgresql://localhost/hammerwork --bind 0.0.0.0:8080
//! ```
//!
//! ### As a Library
//!
//! ```rust,no_run
//! use hammerwork_web::{WebDashboard, DashboardConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = DashboardConfig {
//!         bind_address: "127.0.0.1".to_string(),
//!         port: 8080,
//!         database_url: "postgresql://localhost/hammerwork".to_string(),
//!         ..Default::default()
//!     };
//!
//!     let dashboard = WebDashboard::new(config).await?;
//!     dashboard.start().await?;
//!
//!     Ok(())
//! }
//! ```

pub mod api;
pub mod auth;
pub mod config;
pub mod server;
pub mod websocket;

pub use config::{DashboardConfig, AuthConfig};
pub use server::WebDashboard;

/// Result type alias for consistent error handling
pub type Result<T> = std::result::Result<T, anyhow::Error>;