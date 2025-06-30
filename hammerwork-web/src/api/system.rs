//! System information and administration API endpoints.

use super::ApiResponse;
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use warp::{Filter, Reply};

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
    pub operation: String, // "vacuum", "reindex", "cleanup", "optimize"
    pub target: Option<String>, // table name or queue name
    pub dry_run: Option<bool>,
}

/// Create system routes
pub fn routes<T>(
    queue: Arc<T>,
) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
where
    T: DatabaseQueue + Send + Sync + 'static,
{
    let queue_filter = warp::any().map(move || queue.clone());

    let info = warp::path("system")
        .and(warp::path("info"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and_then(system_info_handler);

    let config = warp::path("system")
        .and(warp::path("config"))
        .and(warp::path::end())
        .and(warp::get())
        .and_then(system_config_handler);

    let metrics_info = warp::path("system")
        .and(warp::path("metrics"))
        .and(warp::path::end())
        .and(warp::get())
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

    info.or(config)
        .or(metrics_info)
        .or(maintenance)
        .or(version)
}

/// Handler for system information
async fn system_info_handler<T>(queue: Arc<T>) -> Result<impl Reply, warp::Rejection>
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

    let database_info = DatabaseInfo {
        database_type: "PostgreSQL/MySQL".to_string(), // TODO: Detect actual type
        connection_url: "***masked***".to_string(),
        pool_size: 5, // TODO: Get from actual config
        active_connections: None, // TODO: Get from pool
        connection_health: database_healthy,
        last_migration: None, // TODO: Get from migration table
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
        uptime_seconds: 0, // TODO: Track actual uptime
        started_at: chrono::Utc::now(), // TODO: Track actual start time
    };

    Ok(warp::reply::json(&ApiResponse::success(system_info)))
}

/// Handler for system configuration
async fn system_config_handler() -> Result<impl Reply, warp::Rejection> {
    let config = ServerConfig {
        bind_address: "127.0.0.1".to_string(), // TODO: Get from actual config
        port: 8080,                            // TODO: Get from actual config
        authentication_enabled: false,        // TODO: Get from actual config
        cors_enabled: false,                   // TODO: Get from actual config
        websocket_max_connections: 100,       // TODO: Get from actual config
        static_assets_path: "./assets".to_string(), // TODO: Get from actual config
    };

    Ok(warp::reply::json(&ApiResponse::success(config)))
}

/// Handler for metrics information
async fn metrics_info_handler() -> Result<impl Reply, warp::Rejection> {
    let metrics_info = MetricsInfo {
        prometheus_enabled: true, // TODO: Detect if metrics feature is enabled
        metrics_endpoint: "/metrics".to_string(),
        custom_metrics_count: 0,  // TODO: Count actual custom metrics
        last_scrape: None,        // TODO: Track last scrape time
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
            let response = ApiResponse::<()>::error("Vacuum operation not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        "reindex" => {
            // Database reindex operation
            let response = ApiResponse::<()>::error("Reindex operation not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        "optimize" => {
            // General optimization operation
            let response = ApiResponse::<()>::error("Optimize operation not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        _ => {
            let response = ApiResponse::<()>::error(format!("Unknown maintenance operation: {}", request.operation));
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
                    if let Ok(kb) = line
                        .split_whitespace()
                        .nth(1)
                        .and_then(|s| s.parse::<u64>())
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