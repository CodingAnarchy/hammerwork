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
    api::system::SystemState,
    auth::{AuthState, auth_filter, handle_auth_rejection},
    config::DashboardConfig,
    websocket::WebSocketState,
};
#[cfg(any(feature = "postgres", feature = "mysql"))]
use hammerwork::JobQueue;
use hammerwork::queue::DatabaseQueue;
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

    /// Start the web server.
    ///
    /// The database backend is chosen from the scheme of `database_url`
    /// (`postgres://`/`postgresql://` or `mysql://`), so with both the `postgres` and
    /// `mysql` features enabled the dashboard serves either kind of database.
    pub async fn start(self) -> Result<()> {
        match DatabaseBackend::from_url(&self.config.database_url)? {
            #[cfg(feature = "postgres")]
            DatabaseBackend::Postgres => {
                let pool = sqlx::postgres::PgPoolOptions::new()
                    .max_connections(self.config.pool_size)
                    .connect(&self.config.database_url)
                    .await?;
                info!(
                    "Connected to PostgreSQL with {} connections",
                    self.config.pool_size
                );
                self.serve(JobQueue::new(pool), "PostgreSQL").await
            }
            #[cfg(feature = "mysql")]
            DatabaseBackend::MySql => {
                let pool = sqlx::mysql::MySqlPoolOptions::new()
                    .max_connections(self.config.pool_size)
                    .connect(&self.config.database_url)
                    .await?;
                info!(
                    "Connected to MySQL with {} connections",
                    self.config.pool_size
                );
                self.serve(JobQueue::new(pool), "MySQL").await
            }
        }
    }

    /// Serve the dashboard for an already connected job queue.
    async fn serve<Q>(self, queue: Q, database_type: &str) -> Result<()>
    where
        Q: DatabaseQueue + Send + Sync + 'static,
    {
        let bind_addr: SocketAddr = self.config.bind_addr().parse()?;
        let queue = Arc::new(queue);

        // Create system state
        let system_state = Arc::new(RwLock::new(SystemState::new(
            self.config.clone(),
            database_type.to_string(),
            self.config.pool_size,
        )));

        // Create API routes
        let api_routes = Self::create_api_routes_static(
            queue.clone(),
            self.auth_state.clone(),
            system_state.clone(),
        );

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

        // Start WebSocket broadcast listener
        let websocket_state_broadcast = self.websocket_state.clone();
        WebSocketState::start_broadcast_listener(websocket_state_broadcast).await?;

        // Start the server. With CORS disabled no CORS filter is installed at all, so
        // browsers apply their default same-origin policy.
        if self.config.enable_cors {
            let cors = warp::cors()
                .allow_any_origin()
                .allow_headers(vec!["content-type", "authorization"])
                .allow_methods(vec!["GET", "POST", "PUT", "DELETE", "OPTIONS"]);
            warp::serve(routes.with(cors)).run(bind_addr).await;
        } else {
            warp::serve(routes).run(bind_addr).await;
        }

        Ok(())
    }

    /// Create API routes with authentication
    fn create_api_routes_static<Q>(
        queue: Arc<Q>,
        auth_state: AuthState,
        system_state: Arc<RwLock<SystemState>>,
    ) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
    where
        Q: DatabaseQueue + Send + Sync + 'static,
    {
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
            .or(api::stats::routes(queue.clone(), system_state.clone()))
            .or(api::system::routes(queue.clone(), system_state))
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

/// The database backend a dashboard connects to, chosen from the database URL scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DatabaseBackend {
    #[cfg(feature = "postgres")]
    Postgres,
    #[cfg(feature = "mysql")]
    MySql,
}

impl DatabaseBackend {
    /// Picks the backend for `url` from its scheme, failing for an unknown scheme or a
    /// backend whose feature is not enabled.
    fn from_url(url: &str) -> Result<Self> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            #[cfg(feature = "postgres")]
            return Ok(Self::Postgres);
            #[cfg(not(feature = "postgres"))]
            return Err(anyhow::anyhow!(
                "PostgreSQL support not enabled. Rebuild with --features postgres"
            ));
        }
        if url.starts_with("mysql://") {
            #[cfg(feature = "mysql")]
            return Ok(Self::MySql);
            #[cfg(not(feature = "mysql"))]
            return Err(anyhow::anyhow!(
                "MySQL support not enabled. Rebuild with --features mysql"
            ));
        }
        Err(anyhow::anyhow!(
            "Unsupported database URL (expected postgres://, postgresql:// or mysql://)"
        ))
    }
}

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
    fn test_database_backend_from_url() {
        #[cfg(feature = "postgres")]
        {
            assert_eq!(
                DatabaseBackend::from_url("postgres://localhost/db").unwrap(),
                DatabaseBackend::Postgres
            );
            assert_eq!(
                DatabaseBackend::from_url("postgresql://localhost/db").unwrap(),
                DatabaseBackend::Postgres
            );
        }
        #[cfg(feature = "mysql")]
        assert_eq!(
            DatabaseBackend::from_url("mysql://localhost/db").unwrap(),
            DatabaseBackend::MySql
        );
        assert!(DatabaseBackend::from_url("sqlite://db").is_err());
        assert!(DatabaseBackend::from_url("postgresx://db").is_err());
    }

    #[test]
    fn test_cors_configuration() {
        let config = DashboardConfig::new().with_cors(true);
        assert!(config.enable_cors);
    }
}
