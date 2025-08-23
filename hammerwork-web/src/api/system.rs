//! System information and administration API endpoints.
//!
//! This module provides comprehensive system administration and information endpoints
//! for monitoring the web dashboard service, runtime metrics, and maintenance operations.
//!
//! # API Endpoints
//!
//! - `GET /api/system/info` - Complete system information including build and runtime details
//! - `GET /api/system/config` - Current server configuration
//! - `GET /api/system/metrics` - Metrics and monitoring information
//! - `POST /api/system/maintenance` - Perform maintenance operations
//! - `GET /api/version` - Basic version information
//!
//! # Examples
//!
//! ## System Information
//!
//! ```rust
//! use hammerwork_web::api::system::{SystemInfo, BuildInfo, RuntimeInfo, DatabaseInfo};
//! use chrono::Utc;
//!
//! let build_info = BuildInfo {
//!     version: "1.3.0".to_string(),
//!     git_commit: Some("a1b2c3d".to_string()),
//!     build_date: Some("2024-01-15".to_string()),
//!     rust_version: "1.70.0".to_string(),
//!     target_triple: "x86_64-unknown-linux-gnu".to_string(),
//! };
//!
//! let runtime_info = RuntimeInfo {
//!     process_id: 12345,
//!     memory_usage_bytes: Some(134217728), // 128MB
//!     cpu_usage_percent: Some(5.2),
//!     thread_count: Some(8),
//!     gc_collections: None, // Not applicable for Rust
//! };
//!
//! let database_info = DatabaseInfo {
//!     database_type: "PostgreSQL".to_string(),
//!     connection_url: "***masked***".to_string(),
//!     pool_size: 10,
//!     active_connections: Some(3),
//!     connection_health: true,
//!     last_migration: Some("20240101_initial".to_string()),
//! };
//!
//! let system_info = SystemInfo {
//!     version: "1.3.0".to_string(),
//!     build_info,
//!     runtime_info,
//!     database_info,
//!     features: vec!["postgres".to_string(), "auth".to_string()],
//!     uptime_seconds: 86400,
//!     started_at: Utc::now(),
//! };
//!
//! assert_eq!(system_info.version, "1.3.0");
//! assert!(system_info.features.contains(&"postgres".to_string()));
//! assert_eq!(system_info.runtime_info.process_id, 12345);
//! ```
//!
//! ## Maintenance Operations
//!
//! ```rust
//! use hammerwork_web::api::system::MaintenanceRequest;
//! use serde_json::json;
//!
//! let cleanup_request = MaintenanceRequest {
//!     operation: "cleanup".to_string(),
//!     target: Some("old_jobs".to_string()),
//!     dry_run: Some(true),
//! };
//!
//! let vacuum_request = MaintenanceRequest {
//!     operation: "vacuum".to_string(),
//!     target: Some("hammerwork_jobs".to_string()),
//!     dry_run: Some(false),
//! };
//!
//! assert_eq!(cleanup_request.operation, "cleanup");
//! assert_eq!(cleanup_request.dry_run, Some(true));
//! assert_eq!(vacuum_request.operation, "vacuum");
//! ```
//!
//! ## Server Configuration
//!
//! ```rust
//! use hammerwork_web::api::system::ServerConfig;
//!
//! let config = ServerConfig {
//!     bind_address: "0.0.0.0".to_string(),
//!     port: 8080,
//!     authentication_enabled: true,
//!     cors_enabled: false,
//!     websocket_max_connections: 100,
//!     static_assets_path: "/var/www/dashboard".to_string(),
//! };
//!
//! assert_eq!(config.bind_address, "0.0.0.0");
//! assert_eq!(config.port, 8080);
//! assert!(config.authentication_enabled);
//! assert!(!config.cors_enabled);
//! ```
//!
//! ## Metrics Information
//!
//! ```rust
//! use hammerwork_web::api::system::MetricsInfo;
//! use chrono::Utc;
//!
//! let metrics_info = MetricsInfo {
//!     prometheus_enabled: true,
//!     metrics_endpoint: "/metrics".to_string(),
//!     custom_metrics_count: 15,
//!     last_scrape: Some(Utc::now()),
//! };
//!
//! assert!(metrics_info.prometheus_enabled);
//! assert_eq!(metrics_info.metrics_endpoint, "/metrics");
//! assert_eq!(metrics_info.custom_metrics_count, 15);
//! ```

use super::ApiResponse;
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use warp::{Filter, Reply};

/// Shared system state for tracking runtime information
#[derive(Clone)]
pub struct SystemState {
    /// Application start time
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Server configuration
    pub config: crate::DashboardConfig,
    /// Database type (detected at runtime)
    pub database_type: String,
    /// Pool size from actual connection pool
    pub pool_size: u32,
}

impl SystemState {
    pub fn new(config: crate::DashboardConfig, database_type: String, pool_size: u32) -> Self {
        Self {
            started_at: chrono::Utc::now(),
            config,
            database_type,
            pool_size,
        }
    }

    pub fn uptime_seconds(&self) -> i64 {
        (chrono::Utc::now() - self.started_at).num_seconds()
    }
}

/// System information
#[derive(Debug, Serialize)]
pub struct SystemInfo {
    pub version: String,
    pub build_info: BuildInfo,
    pub runtime_info: RuntimeInfo,
    pub database_info: DatabaseInfo,
    pub features: Vec<String>,
    pub uptime_seconds: u64,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

/// Build information
#[derive(Debug, Serialize)]
pub struct BuildInfo {
    pub version: String,
    pub git_commit: Option<String>,
    pub build_date: Option<String>,
    pub rust_version: String,
    pub target_triple: String,
}

/// Runtime information
#[derive(Debug, Serialize)]
pub struct RuntimeInfo {
    pub process_id: u32,
    pub memory_usage_bytes: Option<u64>,
    pub cpu_usage_percent: Option<f64>,
    pub thread_count: Option<usize>,
    pub gc_collections: Option<u64>,
}

/// Database information
#[derive(Debug, Serialize)]
pub struct DatabaseInfo {
    pub database_type: String,
    pub connection_url: String, // Masked for security
    pub pool_size: u32,
    pub active_connections: Option<u32>,
    pub connection_health: bool,
    pub last_migration: Option<String>,
}

/// Server configuration
#[derive(Debug, Serialize)]
pub struct ServerConfig {
    pub bind_address: String,
    pub port: u16,
    pub authentication_enabled: bool,
    pub cors_enabled: bool,
    pub websocket_max_connections: usize,
    pub static_assets_path: String,
}

/// Metrics endpoint information
#[derive(Debug, Serialize)]
pub struct MetricsInfo {
    pub prometheus_enabled: bool,
    pub metrics_endpoint: String,
    pub custom_metrics_count: u32,
    pub last_scrape: Option<chrono::DateTime<chrono::Utc>>,
}

/// Configuration update request
#[derive(Debug, Deserialize)]
pub struct ConfigUpdateRequest {
    pub setting: String,
    pub value: serde_json::Value,
}

/// Maintenance operation request
#[derive(Debug, Deserialize)]
pub struct MaintenanceRequest {
    pub operation: String,      // "vacuum", "reindex", "cleanup", "optimize"
    pub target: Option<String>, // table name or queue name
    pub dry_run: Option<bool>,
}

/// Create system routes
pub fn routes<T>(
    queue: Arc<T>,
    system_state: Arc<RwLock<SystemState>>,
) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
where
    T: DatabaseQueue + Send + Sync + 'static,
{
    let queue_filter = warp::any().map(move || queue.clone());
    let state_filter = warp::any().map(move || system_state.clone());

    let info = warp::path("system")
        .and(warp::path("info"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and(state_filter.clone())
        .and_then(system_info_handler);

    let config = warp::path("system")
        .and(warp::path("config"))
        .and(warp::path::end())
        .and(warp::get())
        .and(state_filter.clone())
        .and_then(system_config_handler);

    let metrics_info = warp::path("system")
        .and(warp::path("metrics"))
        .and(warp::path::end())
        .and(warp::get())
        .and(state_filter.clone())
        .and_then(metrics_info_handler);

    let maintenance = warp::path("system")
        .and(warp::path("maintenance"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter.clone())
        .and(warp::body::json())
        .and_then(maintenance_handler);

    let version = warp::path("version")
        .and(warp::path::end())
        .and(warp::get())
        .and_then(version_handler);

    info.or(config).or(metrics_info).or(maintenance).or(version)
}

/// Handler for system information
async fn system_info_handler<T>(
    queue: Arc<T>,
    system_state: Arc<RwLock<SystemState>>,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // Test database connection
    let database_healthy = queue.get_all_queue_stats().await.is_ok();

    let build_info = BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_commit: option_env!("GIT_COMMIT").map(|s| s.to_string()),
        build_date: option_env!("BUILD_DATE").map(|s| s.to_string()),
        rust_version: get_rust_version(),
        target_triple: get_target_triple(),
    };

    let runtime_info = RuntimeInfo {
        process_id: std::process::id(),
        memory_usage_bytes: get_memory_usage(),
        cpu_usage_percent: None, // Would need process monitoring
        thread_count: None,      // Would need thread monitoring
        gc_collections: None,    // Not applicable for Rust
    };

    let state = system_state.read().await;

    let database_info = DatabaseInfo {
        database_type: state.database_type.clone(),
        connection_url: "***masked***".to_string(),
        pool_size: state.pool_size,
        active_connections: None, // Would need pool access
        connection_health: database_healthy,
        last_migration: None, // Would need migration table query
    };

    let features = vec![
        #[cfg(feature = "postgres")]
        "postgres".to_string(),
        #[cfg(feature = "mysql")]
        "mysql".to_string(),
        #[cfg(feature = "auth")]
        "auth".to_string(),
    ];

    let system_info = SystemInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_info,
        runtime_info,
        database_info,
        features,
        uptime_seconds: state.uptime_seconds() as u64,
        started_at: state.started_at,
    };

    Ok(warp::reply::json(&ApiResponse::success(system_info)))
}

/// Handler for system configuration
async fn system_config_handler(
    system_state: Arc<RwLock<SystemState>>,
) -> Result<impl Reply, warp::Rejection> {
    let state = system_state.read().await;
    let config = ServerConfig {
        bind_address: state.config.bind_address.clone(),
        port: state.config.port,
        authentication_enabled: state.config.auth.enabled,
        cors_enabled: state.config.enable_cors,
        websocket_max_connections: state.config.websocket.max_connections,
        static_assets_path: state.config.static_dir.to_string_lossy().to_string(),
    };

    Ok(warp::reply::json(&ApiResponse::success(config)))
}

/// Handler for metrics information
async fn metrics_info_handler(
    system_state: Arc<RwLock<SystemState>>,
) -> Result<impl Reply, warp::Rejection> {
    let state = system_state.read().await;

    let metrics_info = MetricsInfo {
        prometheus_enabled: cfg!(feature = "metrics"),
        metrics_endpoint: "/metrics".to_string(),
        custom_metrics_count: get_custom_metrics_count(),
        last_scrape: get_last_scrape_time().await,
    };

    Ok(warp::reply::json(&ApiResponse::success(metrics_info)))
}

/// Handler for maintenance operations
async fn maintenance_handler<T>(
    queue: Arc<T>,
    request: MaintenanceRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    let dry_run = request.dry_run.unwrap_or(false);

    match request.operation.as_str() {
        "cleanup" => {
            if dry_run {
                let response = ApiResponse::success(serde_json::json!({
                    "operation": "cleanup",
                    "dry_run": true,
                    "message": "Dry run: Would clean up old completed and dead jobs",
                    "estimated_deletions": 0
                }));
                Ok(warp::reply::json(&response))
            } else {
                // Perform actual cleanup
                let older_than = chrono::Utc::now() - chrono::Duration::days(7); // Remove jobs older than 7 days
                match queue.purge_dead_jobs(older_than).await {
                    Ok(count) => {
                        let response = ApiResponse::success(serde_json::json!({
                            "operation": "cleanup",
                            "dry_run": false,
                            "message": format!("Cleaned up {} dead jobs", count),
                            "deletions": count
                        }));
                        Ok(warp::reply::json(&response))
                    }
                    Err(e) => {
                        let response = ApiResponse::<()>::error(format!("Cleanup failed: {}", e));
                        Ok(warp::reply::json(&response))
                    }
                }
            }
        }
        "vacuum" => {
            // Database vacuum operation (PostgreSQL specific)
            let response =
                ApiResponse::<()>::error("Vacuum operation not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        "reindex" => {
            // Database reindex operation
            let response =
                ApiResponse::<()>::error("Reindex operation not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        "optimize" => {
            // General optimization operation
            let response =
                ApiResponse::<()>::error("Optimize operation not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        _ => {
            let response = ApiResponse::<()>::error(format!(
                "Unknown maintenance operation: {}",
                request.operation
            ));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for version information
async fn version_handler() -> Result<impl Reply, warp::Rejection> {
    let version_info = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "name": env!("CARGO_PKG_NAME"),
        "description": env!("CARGO_PKG_DESCRIPTION"),
        "authors": env!("CARGO_PKG_AUTHORS").split(':').collect::<Vec<_>>(),
        "repository": env!("CARGO_PKG_REPOSITORY"),
        "license": env!("CARGO_PKG_LICENSE"),
        "rust_version": get_rust_version(),
        "build_target": get_target_triple(),
    });

    Ok(warp::reply::json(&ApiResponse::success(version_info)))
}

/// Get Rust compiler version
fn get_rust_version() -> String {
    option_env!("RUSTC_VERSION")
        .unwrap_or("unknown")
        .to_string()
}

/// Get target triple
fn get_target_triple() -> String {
    std::env::consts::ARCH.to_string() + "-" + std::env::consts::OS
}

/// Get memory usage (platform-specific)
fn get_memory_usage() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        use std::fs;
        if let Ok(contents) = fs::read_to_string("/proc/self/status") {
            for line in contents.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(kb) = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>().ok())
                    {
                        return Some(kb * 1024); // Convert KB to bytes
                    }
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS memory usage would require system calls
        // For now, return None
    }

    #[cfg(target_os = "windows")]
    {
        // Windows memory usage would require Windows API
        // For now, return None
    }

    None
}

/// Get count of custom metrics (Prometheus metrics beyond the default ones)
fn get_custom_metrics_count() -> u32 {
    #[cfg(feature = "metrics")]
    {
        // TODO: In a real implementation, you would query the Prometheus registry
        // to count custom metrics. For now, we return a placeholder.
        // This would typically involve accessing the global metrics registry
        // and counting user-defined metrics vs. system metrics.
        0
    }

    #[cfg(not(feature = "metrics"))]
    {
        0
    }
}

/// Get the last time metrics were scraped
async fn get_last_scrape_time() -> Option<chrono::DateTime<chrono::Utc>> {
    #[cfg(feature = "metrics")]
    {
        // TODO: Track actual scrape times in a real implementation
        // This would typically be stored in a shared state or metrics registry
        // For now, we return None as metrics scraping time tracking isn't implemented
        None
    }

    #[cfg(not(feature = "metrics"))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maintenance_request_deserialization() {
        let json = r#"{
            "operation": "cleanup",
            "target": "completed_jobs",
            "dry_run": true
        }"#;

        let request: MaintenanceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.operation, "cleanup");
        assert_eq!(request.target, Some("completed_jobs".to_string()));
        assert_eq!(request.dry_run, Some(true));
    }

    #[test]
    fn test_build_info_creation() {
        let build_info = BuildInfo {
            version: "1.0.0".to_string(),
            git_commit: Some("abc123".to_string()),
            build_date: Some("2024-01-01".to_string()),
            rust_version: "1.70.0".to_string(),
            target_triple: "x86_64-unknown-linux-gnu".to_string(),
        };

        let json = serde_json::to_string(&build_info).unwrap();
        assert!(json.contains("1.0.0"));
        assert!(json.contains("abc123"));
    }

    #[test]
    fn test_get_rust_version() {
        let version = get_rust_version();
        assert!(!version.is_empty());
    }

    #[test]
    fn test_get_target_triple() {
        let target = get_target_triple();
        assert!(!target.is_empty());
        assert!(target.contains("-"));
    }
}
