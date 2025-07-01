//! Web server implementation for the Hammerwork dashboard.
//!
//! This module provides the main `WebDashboard` struct for starting and configuring
//! the web server, including database connections, authentication, and route setup.
//!
//! # Examples
//!
//! ## Basic Server Setup
//!
//! ```rust,no_run
//! use hammerwork_web::{WebDashboard, DashboardConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = DashboardConfig::new()
//!         .with_bind_address("127.0.0.1", 8080)
//!         .with_database_url("postgresql://localhost/hammerwork");
//!
//!     let dashboard = WebDashboard::new(config).await?;
//!     dashboard.start().await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Server with Authentication
//!
//! ```rust,no_run
//! use hammerwork_web::{WebDashboard, DashboardConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let config = DashboardConfig::new()
//!         .with_bind_address("0.0.0.0", 9090)
//!         .with_database_url("postgresql://localhost/hammerwork")
//!         .with_auth("admin", "$2b$12$hash...")
//!         .with_cors(true);
//!
//!     let dashboard = WebDashboard::new(config).await?;
//!     dashboard.start().await?;
//!
//!     Ok(())
//! }
//! ```

use crate::{
    Result, api,
    auth::{AuthState, auth_filter, handle_auth_rejection},
    config::DashboardConfig,
    websocket::WebSocketState,
};
use hammerwork::JobQueue;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tracing::{error, info};
use warp::{Filter, Reply};

/// Main web dashboard server.
///
/// The `WebDashboard` provides a complete web interface for monitoring and managing
/// Hammerwork job queues. It includes REST API endpoints, WebSocket support for
/// real-time updates, authentication, and a modern HTML/CSS/JS frontend.
///
/// # Examples
///
/// ```rust,no_run
/// use hammerwork_web::{WebDashboard, DashboardConfig};
/// use std::path::PathBuf;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = DashboardConfig::new()
///         .with_bind_address("127.0.0.1", 8080)
///         .with_database_url("postgresql://localhost/hammerwork")
///         .with_static_dir(PathBuf::from("./assets"))
///         .with_cors(false);
///
///     let dashboard = WebDashboard::new(config).await?;
///     dashboard.start().await?;
///
///     Ok(())
/// }
/// ```
pub struct WebDashboard {
    config: DashboardConfig,
    auth_state: AuthState,
    websocket_state: Arc<RwLock<WebSocketState>>,
}

impl WebDashboard {
    /// Create a new web dashboard instance.
    ///
    /// This initializes the dashboard with the provided configuration but does not
    /// start the web server. Call `start()` to begin serving requests.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use hammerwork_web::{WebDashboard, DashboardConfig};
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<(), Box<dyn std::error::Error>> {
    ///     let config = DashboardConfig::new()
    ///         .with_database_url("postgresql://localhost/hammerwork");
    ///
    ///     let dashboard = WebDashboard::new(config).await?;
    ///     // Dashboard is created but not yet started
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid or if initialization fails.
    pub async fn new(config: DashboardConfig) -> Result<Self> {
        let auth_state = AuthState::new(config.auth.clone());
        let websocket_state = Arc::new(RwLock::new(WebSocketState::new(config.websocket.clone())));

        Ok(Self {
            config,
            auth_state,
            websocket_state,
        })
    }

    /// Start the web server
    pub async fn start(self) -> Result<()> {
        let bind_addr: SocketAddr = self.config.bind_addr().parse()?;

        // Create job queue
        let queue = Arc::new(self.create_job_queue().await?);

        // Create API routes
        let api_routes = Self::create_api_routes_static(queue.clone(), self.auth_state.clone());

        // Create WebSocket routes
        let websocket_routes = Self::create_websocket_routes_static(
            self.websocket_state.clone(),
            self.auth_state.clone(),
        );

        // Create static file routes
        let static_routes = Self::create_static_routes_static(self.config.static_dir.clone())?;

        // Combine all routes
        let routes = api_routes
            .or(websocket_routes)
            .or(static_routes)
            .recover(handle_auth_rejection);

        // Apply CORS if enabled (simplified approach)
        let routes = routes.with(if self.config.enable_cors {
            warp::cors()
                .allow_any_origin()
                .allow_headers(vec!["content-type", "authorization"])
                .allow_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"])
        } else {
            warp::cors().allow_origin("none") // Effectively disable CORS
        });

        info!("Starting web server on {}", bind_addr);

        // Start cleanup task for auth state
        let auth_state_cleanup = self.auth_state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300)); // 5 minutes
            loop {
                interval.tick().await;
                auth_state_cleanup.cleanup_expired_attempts().await;
            }
        });

        // Start WebSocket ping task
        let websocket_state_ping = self.websocket_state.clone();
        let ping_interval = self.config.websocket.ping_interval;
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(ping_interval);
            loop {
                interval.tick().await;
                let state = websocket_state_ping.read().await;
                state.ping_all_connections().await;
            }
        });

        // Start the server
        warp::serve(routes).run(bind_addr).await;

        Ok(())
    }

    /// Create job queue from database URL
    async fn create_job_queue(&self) -> Result<QueueType> {
        // Determine database type from URL and create appropriate queue
        if self.config.database_url.starts_with("postgres") {
            #[cfg(feature = "postgres")]
            {
                let pg_pool = sqlx::PgPool::connect(&self.config.database_url).await?;
                info!(
                    "Connected to PostgreSQL with {} connections",
                    self.config.pool_size
                );
                Ok(JobQueue::new(pg_pool))
            }
            #[cfg(not(feature = "postgres"))]
            {
                return Err(anyhow::anyhow!(
                    "PostgreSQL support not enabled. Rebuild with --features postgres"
                ));
            }
        } else if self.config.database_url.starts_with("mysql") {
            #[cfg(feature = "mysql")]
            {
                #[cfg(all(feature = "mysql", not(feature = "postgres")))]
                {
                    let mysql_pool = sqlx::MySqlPool::connect(&self.config.database_url).await?;
                    info!(
                        "Connected to MySQL with {} connections",
                        self.config.pool_size
                    );
                    Ok(JobQueue::new(mysql_pool))
                }
                #[cfg(all(feature = "postgres", feature = "mysql"))]
                {
                    return Err(anyhow::anyhow!(
                        "MySQL database URL provided but PostgreSQL is the default when both features are enabled"
                    ));
                }
            }
            #[cfg(not(feature = "mysql"))]
            {
                return Err(anyhow::anyhow!(
                    "MySQL support not enabled. Rebuild with --features mysql"
                ));
            }
        } else {
            Err(anyhow::anyhow!("Unsupported database URL format"))
        }
    }

    /// Create API routes with authentication
    fn create_api_routes_static(
        queue: Arc<QueueType>,
        auth_state: AuthState,
    ) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
        // Health check endpoint (no auth required)
        let health = warp::path("health")
            .and(warp::path::end())
            .and(warp::get())
            .map(|| {
                warp::reply::json(&serde_json::json!({
                    "status": "healthy",
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                    "version": env!("CARGO_PKG_VERSION")
                }))
            });

        // API routes (require authentication)
        let api_routes = api::queues::routes(queue.clone())
            .or(api::jobs::routes(queue.clone()))
            .or(api::stats::routes(queue.clone()))
            .or(api::system::routes(queue.clone()))
            .or(api::archive::archive_routes(queue.clone()))
            .or(api::spawn::spawn_routes(queue));

        let authenticated_api = warp::path("api")
            .and(auth_filter(auth_state))
            .untuple_one()
            .and(api_routes);

        health.or(authenticated_api)
    }

    /// Create WebSocket routes with authentication
    fn create_websocket_routes_static(
        websocket_state: Arc<RwLock<WebSocketState>>,
        auth_state: AuthState,
    ) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
        warp::path("ws")
            .and(warp::path::end())
            .and(auth_filter(auth_state))
            .and(warp::ws())
            .and(warp::any().map(move || websocket_state.clone()))
            .map(
                |_: (), ws: warp::ws::Ws, websocket_state: Arc<RwLock<WebSocketState>>| {
                    ws.on_upgrade(move |socket| async move {
                        let mut state = websocket_state.write().await;
                        if let Err(e) = state.handle_connection(socket).await {
                            error!("WebSocket error: {}", e);
                        }
                    })
                },
            )
    }

    /// Create static file serving routes
    fn create_static_routes_static(
        static_dir: std::path::PathBuf,
    ) -> Result<impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone> {
        // Serve static files
        let static_files = warp::path("static").and(warp::fs::dir(static_dir.clone()));

        // Serve index.html at root
        let index = warp::path::end().and(warp::fs::file(static_dir.join("index.html")));

        // Catch-all for SPA routing - serve index.html
        let spa_routes = warp::any().and(warp::fs::file(static_dir.join("index.html")));

        Ok(index.or(static_files).or(spa_routes))
    }
}

#[cfg(all(feature = "postgres", not(feature = "mysql")))]
type QueueType = JobQueue<sqlx::Postgres>;

#[cfg(all(feature = "mysql", not(feature = "postgres")))]
type QueueType = JobQueue<sqlx::MySql>;

#[cfg(all(feature = "postgres", feature = "mysql"))]
type QueueType = JobQueue<sqlx::Postgres>; // Default to PostgreSQL when both are enabled

#[cfg(all(not(feature = "postgres"), not(feature = "mysql")))]
compile_error!("At least one database feature (postgres or mysql) must be enabled");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DashboardConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_dashboard_creation() {
        let temp_dir = tempdir().unwrap();
        let config = DashboardConfig::new().with_static_dir(temp_dir.path().to_path_buf());

        let dashboard = WebDashboard::new(config).await;
        assert!(dashboard.is_ok());
    }

    #[test]
    fn test_cors_configuration() {
        let config = DashboardConfig::new().with_cors(true);
        assert!(config.enable_cors);
    }
}
