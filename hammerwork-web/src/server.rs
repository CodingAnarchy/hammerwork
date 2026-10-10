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
//!         .with_cors(true)
//!         .with_allowed_origin("https://ops.example.com");
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
    security::{AllowedOrigins, same_origin, same_origin_writes},
    websocket::WebSocketState,
};
#[cfg(any(feature = "postgres", feature = "mysql"))]
use hammerwork::JobQueue;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tracing::{error, info};
use warp::{Filter, Reply};

/// All routes of the dashboard: the health check, the authenticated JSON API under `/api`,
/// the WebSocket endpoint at `/ws`, and the static single-page app for everything else.
///
/// Requests under `/api` and `/ws` never fall through to the single-page app, so a failed
/// authentication or an unknown API path is answered as such (401, 404) instead of with the
/// HTML page. Rejections are turned into replies by [`handle_auth_rejection`].
///
/// State-changing API requests and WebSocket handshakes that a browser sends from a page on
/// another origin than the dashboard's own or one of `allowed_origins` are refused with 403
/// (see [`crate::security`]).
pub fn app_routes<Q>(
    queue: Arc<Q>,
    auth_state: AuthState,
    system_state: Arc<RwLock<SystemState>>,
    websocket_state: Arc<RwLock<WebSocketState>>,
    static_dir: std::path::PathBuf,
    allowed_origins: AllowedOrigins,
) -> impl Filter<Extract = (impl Reply,), Error = std::convert::Infallible> + Clone
where
    Q: api::history::JobHistory + 'static,
{
    let api_routes = WebDashboard::create_api_routes_static(
        queue,
        auth_state.clone(),
        system_state,
        allowed_origins.clone(),
    );
    let websocket_routes =
        WebDashboard::create_websocket_routes_static(websocket_state, auth_state, allowed_origins);
    let static_routes = WebDashboard::create_static_routes_static(static_dir);

    api_routes
        .or(websocket_routes)
        .or(static_routes)
        .recover(handle_auth_rejection)
}

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
    allowed_origins: AllowedOrigins,
    event_manager: Option<Arc<hammerwork::events::EventManager>>,
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
        config.validate()?;
        let allowed_origins = AllowedOrigins::new(&config.allowed_origins)?;
        let auth_state = AuthState::new(config.auth.clone());
        let websocket_state = Arc::new(RwLock::new(WebSocketState::new(config.websocket.clone())));

        Ok(Self {
            config,
            auth_state,
            websocket_state,
            allowed_origins,
            event_manager: None,
        })
    }

    /// Also forward the job lifecycle events of `event_manager` to connected clients as
    /// they happen.
    ///
    /// For a dashboard embedded in the same process as the workers that publish to
    /// `event_manager`. Without it (and in the standalone binary) the dashboard still
    /// pushes job and queue changes, by polling the database every
    /// `websocket.live_update_interval`; see [`crate::live`].
    pub fn with_event_manager(
        mut self,
        event_manager: Arc<hammerwork::events::EventManager>,
    ) -> Self {
        self.event_manager = Some(event_manager);
        self
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
                let queue = job_queue(pool, &encryption_settings()?).await?;
                self.serve(queue, "PostgreSQL").await
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
                let queue = job_queue(pool, &encryption_settings()?).await?;
                self.serve(queue, "MySQL").await
            }
        }
    }

    /// Serve the dashboard for an already connected job queue.
    async fn serve<Q>(self, queue: Q, database_type: &str) -> Result<()>
    where
        Q: api::history::JobHistory + 'static,
    {
        let bind_addr: SocketAddr = self.config.bind_addr().parse()?;
        let queue = Arc::new(queue);

        // Create system state
        let system_state = Arc::new(RwLock::new(SystemState::new(
            self.config.clone(),
            database_type.to_string(),
            self.config.pool_size,
        )));

        // Push job and queue changes to WebSocket clients
        let live_interval = self.config.websocket.live_update_interval;
        if !live_interval.is_zero() {
            crate::live::LiveUpdates::new(
                queue.clone(),
                self.websocket_state.clone(),
                self.config.websocket.live_update_max_jobs,
            )
            .spawn(live_interval);
        }
        if let Some(event_manager) = &self.event_manager {
            let subscription = event_manager
                .subscribe(hammerwork::events::EventFilter::new())
                .await?;
            crate::live::forward_job_events(self.websocket_state.clone(), subscription);
        }

        // API, WebSocket and static file routes
        let routes = app_routes(
            queue,
            self.auth_state.clone(),
            system_state,
            self.websocket_state.clone(),
            self.config.static_dir.clone(),
            self.allowed_origins.clone(),
        );

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
        // browsers apply their default same-origin policy. With it enabled, only the
        // configured origins are granted access (validated in `new`).
        if self.config.enable_cors {
            let cors = warp::cors()
                .allow_origins(self.allowed_origins.as_slice().iter().map(String::as_str))
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
        allowed_origins: AllowedOrigins,
    ) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
    where
        Q: api::history::JobHistory + 'static,
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
            .or(api::archive::archive_routes(queue));

        let authenticated_api = warp::path("api")
            .and(same_origin_writes(allowed_origins))
            .and(auth_filter(auth_state))
            .untuple_one()
            .and(api_routes);

        health.or(authenticated_api)
    }

    /// Create WebSocket routes with authentication, an origin check (browsers on other
    /// origins are refused, against cross-site WebSocket hijacking) and the configured
    /// message size limit.
    fn create_websocket_routes_static(
        websocket_state: Arc<RwLock<WebSocketState>>,
        auth_state: AuthState,
        allowed_origins: AllowedOrigins,
    ) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
        warp::path("ws")
            .and(warp::path::end())
            .and(same_origin(allowed_origins))
            .and(auth_filter(auth_state))
            .and(warp::ws())
            .and(warp::any().map(move || websocket_state.clone()))
            .and_then(
                |_: (), ws: warp::ws::Ws, websocket_state: Arc<RwLock<WebSocketState>>| async move {
                    let max_message_size = websocket_state.read().await.config().max_message_size;
                    let ws = ws
                        .max_message_size(max_message_size)
                        .max_frame_size(max_message_size);
                    Ok::<_, warp::Rejection>(ws.on_upgrade(move |socket| async move {
                        if let Err(e) =
                            WebSocketState::serve_connection(websocket_state, socket).await
                        {
                            error!("WebSocket error: {}", e);
                        }
                    }))
                },
            )
    }

    /// Create static file serving routes
    fn create_static_routes_static(
        static_dir: std::path::PathBuf,
    ) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone {
        // Serve static files
        let static_files = warp::path("static").and(warp::fs::dir(static_dir.clone()));

        // Serve index.html at root
        let index = warp::path::end().and(warp::fs::file(static_dir.join("index.html")));

        // Catch-all for SPA routing - serve index.html, but never for API or WebSocket paths
        let spa_routes = not_api_path().and(warp::fs::file(static_dir.join("index.html")));

        index.or(static_files).or(spa_routes)
    }
}

/// Passes requests outside `/api` and `/ws`; rejects those, so they are not answered with the
/// single-page app.
fn not_api_path() -> impl Filter<Extract = (), Error = warp::Rejection> + Clone {
    warp::path::full()
        .and_then(|path: warp::path::FullPath| async move {
            let path = path.as_str();
            let reserved = ["/api", "/ws"]
                .iter()
                .any(|prefix| path == *prefix || path.starts_with(&format!("{prefix}/")));
            if reserved {
                Err(warp::reject::not_found())
            } else {
                Ok(())
            }
        })
        .untuple_one()
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

/// The application's payload encryption settings: the `[encryption]` section of the
/// `hammerwork.toml` named by `HAMMERWORK_ENCRYPTION_CONFIG`, with the
/// `HAMMERWORK_ENCRYPTION_*` environment variables applied on top (see
/// `PayloadEncryptionConfig::load`). Disabled when neither is set.
#[cfg(any(feature = "postgres", feature = "mysql"))]
fn encryption_settings() -> Result<hammerwork::config::PayloadEncryptionConfig> {
    let path = std::env::var("HAMMERWORK_ENCRYPTION_CONFIG")
        .ok()
        .filter(|path| !path.is_empty());
    Ok(hammerwork::config::PayloadEncryptionConfig::load(
        path.as_deref().map(std::path::Path::new),
    )?)
}

/// The dashboard's job queue.
///
/// It encrypts like the application (jobs created from the dashboard on the
/// application's `encrypted_queues` are encrypted with its key), and it has the plaintext
/// guard, so it refuses to write a plaintext job to a queue that holds encrypted jobs
/// even without encryption settings. A key that cannot be loaded is an error: the
/// dashboard does not start rather than write plaintext.
#[cfg(any(feature = "postgres", feature = "mysql"))]
async fn job_queue<DB: hammerwork::encryption::KeyManagerBackend>(
    pool: sqlx::Pool<DB>,
    encryption: &hammerwork::config::PayloadEncryptionConfig,
) -> Result<JobQueue<DB>> {
    let queue = JobQueue::new(pool)
        .apply_encryption_config(encryption)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "Cannot set up payload encryption from the application's [encryption] \
                 settings ({}); refusing to start rather than store jobs in plaintext",
                e
            )
        })?;
    if encryption.enabled {
        info!(
            "Payload encryption enabled for queues: {:?}",
            encryption.encrypted_queues
        );
    }
    Ok(queue.with_plaintext_guard(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DashboardConfig;
    use tempfile::tempdir;

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_job_queue_encrypts_like_the_application() {
        use base64::Engine as _;
        let pool = || {
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://nobody:nothing@127.0.0.1:1/none")
                .unwrap()
        };

        // No settings: no engine (the plaintext guard still applies)
        let queue = job_queue(pool(), &Default::default()).await.unwrap();
        assert!(queue.encryption_engine().is_none());

        // The application's settings: its engine, with its key
        let var = format!("HW_WEB_TEST_KEY_{}", uuid::Uuid::new_v4().simple());
        let settings = hammerwork::config::PayloadEncryptionConfig {
            enabled: true,
            key_source: hammerwork::config::KeySourceRef::parse(&format!("env://{var}")).unwrap(),
            key_id: Some("web-key".to_string()),
            encrypted_queues: vec!["payments".to_string()],
            ..Default::default()
        };
        let err = job_queue(pool(), &settings)
            .await
            .err()
            .expect("the key is missing");
        assert!(err.to_string().contains("refusing to start"), "{err}");
        // SAFETY: the variable name is unique to this test.
        unsafe {
            std::env::set_var(
                &var,
                base64::engine::general_purpose::STANDARD.encode([3u8; 32]),
            )
        };
        let queue = job_queue(pool(), &settings).await.unwrap();
        assert_eq!(queue.encryption_engine().unwrap().key_id(), "web-key");
    }

    #[tokio::test]
    async fn test_dashboard_creation() {
        let temp_dir = tempdir().unwrap();
        let mut config = DashboardConfig::new().with_static_dir(temp_dir.path().to_path_buf());
        config.auth.enabled = cfg!(feature = "auth");

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

    #[tokio::test]
    async fn new_rejects_cors_without_origins() {
        let mut config = DashboardConfig::new().with_cors(true);
        config.auth.enabled = false;
        let err = WebDashboard::new(config)
            .await
            .err()
            .expect("CORS without origins is refused");
        assert!(err.to_string().contains("allowed_origins"), "{err}");
    }

    type Router = warp::filters::BoxedFilter<(Box<dyn Reply>,)>;

    /// The full route tree over an unreachable database, without authentication.
    fn routes(websocket: crate::config::WebSocketConfig) -> (Router, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "SPA").unwrap();
        let config = DashboardConfig {
            websocket: websocket.clone(),
            ..DashboardConfig::new()
        };
        let system_state = Arc::new(RwLock::new(SystemState::new(
            config.clone(),
            "PostgreSQL".into(),
            1,
        )));
        let auth = AuthState::new(crate::config::AuthConfig {
            enabled: false,
            ..Default::default()
        });
        let routes = app_routes(
            crate::api::test_support::unreachable_queue(),
            auth,
            system_state,
            Arc::new(RwLock::new(WebSocketState::new(websocket))),
            dir.path().to_path_buf(),
            AllowedOrigins::new(["https://ops.example.com"]).unwrap(),
        )
        .map(|reply| Box::new(reply) as Box<dyn Reply>)
        .boxed();
        (routes, dir)
    }

    fn post_job() -> warp::test::RequestBuilder {
        let body = r#"{"queue_name": "q", "payload": {}}"#;
        warp::test::request()
            .method("POST")
            .path("/api/jobs")
            .header("host", "127.0.0.1:8080")
            .header("content-length", body.len().to_string())
            .body(body)
    }

    /// H8: state-changing requests from other origins, and bodies a cross-site form or
    /// no-cors fetch could send, never reach a handler.
    #[tokio::test]
    async fn cross_site_writes_are_refused() {
        let (routes, _dir) = routes(Default::default());
        let status = |request: warp::test::RequestBuilder| {
            let routes = routes.clone();
            async move { request.reply(&routes).await.status().as_u16() }
        };

        // A type-less body (a no-cors fetch with a Blob) or a form post: 415.
        assert_eq!(status(post_job()).await, 415);
        assert_eq!(
            status(post_job().header("content-type", "text/plain")).await,
            415
        );
        // A cross-site page: 403, whatever the content type.
        for site in ["cross-site", "same-site"] {
            let request = post_job()
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:9999")
                .header("sec-fetch-site", site);
            assert_eq!(status(request).await, 403, "{site}");
        }
        let request = post_job()
            .header("content-type", "application/json")
            .header("origin", "http://evil.example");
        assert_eq!(status(request).await, 403);
        let bulk = warp::test::request()
            .method("POST")
            .path("/api/jobs/bulk")
            .header("origin", "null")
            .json(&serde_json::json!({"job_ids": [], "action": "delete"}));
        assert_eq!(status(bulk).await, 403);
        let purge = warp::test::request()
            .method("DELETE")
            .path("/api/archive/purge")
            .header("sec-fetch-site", "cross-site")
            .json(&serde_json::json!({"older_than": "2020-01-01T00:00:00Z", "dry_run": false}));
        assert_eq!(status(purge).await, 403);

        // The dashboard's own page, an allowed origin and non-browser clients get through
        // (to the unreachable database: 500).
        let own = post_job()
            .header("content-type", "application/json")
            .header("origin", "http://127.0.0.1:8080")
            .header("sec-fetch-site", "same-origin");
        assert_eq!(status(own).await, 500);
        let allowed = post_job()
            .header("content-type", "application/json; charset=utf-8")
            .header("origin", "https://ops.example.com")
            .header("sec-fetch-site", "cross-site");
        assert_eq!(status(allowed).await, 500);
        let curl = post_job().header("content-type", "application/json");
        assert_eq!(status(curl).await, 500);

        // Reads are not affected.
        let read = warp::test::request()
            .path("/api/jobs")
            .header("origin", "http://evil.example")
            .header("sec-fetch-site", "cross-site");
        assert_eq!(status(read).await, 500);
    }

    /// M11: request bodies are bounded.
    #[tokio::test]
    async fn oversized_bodies_are_refused() {
        let (routes, _dir) = routes(Default::default());
        let payload = "x".repeat(crate::security::MAX_JSON_BODY_BYTES as usize);
        let response = warp::test::request()
            .method("POST")
            .path("/api/jobs")
            .json(&serde_json::json!({"queue_name": "q", "payload": payload}))
            .reply(&routes)
            .await;
        assert_eq!(response.status(), 413);

        let ids: Vec<String> = (0..=crate::api::jobs::MAX_BULK_JOB_IDS)
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        let response = warp::test::request()
            .method("POST")
            .path("/api/jobs/bulk")
            .json(&serde_json::json!({"job_ids": ids, "action": "delete"}))
            .reply(&routes)
            .await;
        assert_eq!(response.status(), 400);
        let body = String::from_utf8_lossy(response.body()).to_string();
        assert!(body.contains("Too many job IDs"), "{body}");
    }

    /// M12: the WebSocket handshake checks the origin.
    #[tokio::test]
    async fn websocket_handshakes_from_other_origins_are_refused() {
        let (routes, _dir) = routes(Default::default());
        // (The test client sends the request to a local address of its own choosing, so the
        // origin of "our" page is declared with Sec-Fetch-Site.)
        for (origin, site) in [
            ("http://evil.example", Some("cross-site")),
            ("http://evil.example", None),
            ("null", None),
        ] {
            let mut hijack = warp::test::ws().path("/ws").header("origin", origin);
            if let Some(site) = site {
                hijack = hijack.header("sec-fetch-site", site);
            }
            assert!(
                hijack.handshake(routes.clone()).await.is_err(),
                "cross-site WebSocket hijacking from {origin} is refused"
            );
        }
        let own = warp::test::ws()
            .path("/ws")
            .header("origin", "http://127.0.0.1:8080")
            .header("sec-fetch-site", "same-origin")
            .handshake(routes.clone())
            .await;
        assert!(own.is_ok());
        let allowed = warp::test::ws()
            .path("/ws")
            .header("origin", "https://ops.example.com")
            .handshake(routes)
            .await;
        assert!(allowed.is_ok());
    }

    /// M12: messages above `max_message_size` end the connection instead of being buffered.
    #[tokio::test]
    async fn websocket_messages_are_size_limited() {
        let (routes, _dir) = routes(crate::config::WebSocketConfig {
            max_message_size: 1024,
            ..Default::default()
        });
        let mut client = warp::test::ws()
            .path("/ws")
            .handshake(routes.clone())
            .await
            .expect("handshake");
        client.send_text(r#"{"type": "Ping"}"#).await;
        let reply = tokio::time::timeout(std::time::Duration::from_secs(2), client.recv())
            .await
            .expect("a reply")
            .expect("open socket");
        assert!(reply.to_str().unwrap().contains("Pong"));

        client.send_text("x".repeat(4096)).await;
        let end = tokio::time::timeout(std::time::Duration::from_secs(2), client.recv())
            .await
            .expect("the server reacts");
        assert!(
            end.is_err() || end.as_ref().is_ok_and(|message| message.is_close()),
            "the oversized message closed the connection: {end:?}"
        );
    }
}
