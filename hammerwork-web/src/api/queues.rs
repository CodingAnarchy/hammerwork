//! Queue management API endpoints.
//!
//! This module provides REST API endpoints for managing and monitoring Hammerwork job queues,
//! including queue statistics, actions, and job management within specific queues.
//!
//! # API Endpoints
//!
//! - `GET /api/queues` - List all queues with statistics
//! - `GET /api/queues/{name}` - Get detailed statistics for a specific queue
//! - `POST /api/queues/{name}/actions` - Perform actions on a queue (pause, resume, clear)
//! - `GET /api/queues/{name}/jobs` - List jobs in a specific queue
//!
//! # Examples
//!
//! ## Queue Information Structure
//!
//! ```rust
//! use hammerwork_web::api::queues::QueueInfo;
//! use chrono::Utc;
//!
//! let queue_info = QueueInfo {
//!     name: "email_queue".to_string(),
//!     pending_count: 25,
//!     running_count: 3,
//!     completed_count: 1500,
//!     failed_count: 12,
//!     dead_count: 2,
//!     avg_processing_time_ms: 250.5,
//!     throughput_per_minute: 45.0,
//!     error_rate: 0.008,
//!     last_job_at: Some(Utc::now()),
//!     oldest_pending_job: Some(Utc::now()),
//! };
//!
//! assert_eq!(queue_info.name, "email_queue");
//! assert_eq!(queue_info.pending_count, 25);
//! assert_eq!(queue_info.running_count, 3);
//! ```
//!
//! ## Queue Actions
//!
//! ```rust
//! use hammerwork_web::api::queues::QueueActionRequest;
//!
//! let clear_dead_request = QueueActionRequest {
//!     action: "clear_dead".to_string(),
//!     confirm: Some(true),
//! };
//!
//! let pause_request = QueueActionRequest {
//!     action: "pause".to_string(),
//!     confirm: None,
//! };
//!
//! assert_eq!(clear_dead_request.action, "clear_dead");
//! assert_eq!(pause_request.action, "pause");
//! ```
//!
//! ## Detailed Queue Statistics
//!
//! ```rust
//! use hammerwork_web::api::queues::{DetailedQueueStats, QueueInfo, HourlyThroughput, RecentError};
//! use std::collections::HashMap;
//! use chrono::Utc;
//!
//! let queue_info = QueueInfo {
//!     name: "default".to_string(),
//!     pending_count: 10,
//!     running_count: 2,
//!     completed_count: 500,
//!     failed_count: 5,
//!     dead_count: 1,
//!     avg_processing_time_ms: 180.0,
//!     throughput_per_minute: 30.0,
//!     error_rate: 0.01,
//!     last_job_at: None,
//!     oldest_pending_job: None,
//! };
//!
//! let mut priority_breakdown = HashMap::new();
//! priority_breakdown.insert("high".to_string(), 5);
//! priority_breakdown.insert("normal".to_string(), 15);
//!
//! let detailed_stats = DetailedQueueStats {
//!     queue_info,
//!     priority_breakdown,
//!     status_breakdown: HashMap::new(),
//!     hourly_throughput: vec![],
//!     recent_errors: vec![],
//! };
//!
//! assert_eq!(detailed_stats.queue_info.name, "default");
//! assert_eq!(detailed_stats.priority_breakdown.get("high"), Some(&5));
//! ```

use super::{
    with_filters, with_pagination, with_sort, ApiResponse, FilterParams,
    PaginatedResponse, PaginationMeta, PaginationParams, SortParams,
};
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use warp::{Filter, Reply};

/// Queue information for API responses
#[derive(Debug, Serialize, Clone)]
pub struct QueueInfo {
    pub name: String,
    pub pending_count: u64,
    pub running_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub dead_count: u64,
    pub avg_processing_time_ms: f64,
    pub throughput_per_minute: f64,
    pub error_rate: f64,
    pub last_job_at: Option<chrono::DateTime<chrono::Utc>>,
    pub oldest_pending_job: Option<chrono::DateTime<chrono::Utc>>,
}

/// Detailed queue statistics
#[derive(Debug, Serialize)]
pub struct DetailedQueueStats {
    pub queue_info: QueueInfo,
    pub priority_breakdown: std::collections::HashMap<String, u64>,
    pub status_breakdown: std::collections::HashMap<String, u64>,
    pub hourly_throughput: Vec<HourlyThroughput>,
    pub recent_errors: Vec<RecentError>,
}

/// Hourly throughput data point
#[derive(Debug, Serialize)]
pub struct HourlyThroughput {
    pub hour: chrono::DateTime<chrono::Utc>,
    pub completed: u64,
    pub failed: u64,
}

/// Recent error information
#[derive(Debug, Serialize)]
pub struct RecentError {
    pub job_id: String,
    pub error_message: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    pub attempts: i32,
}

/// Queue action request
#[derive(Debug, Deserialize)]
pub struct QueueActionRequest {
    pub action: String, // "pause", "resume", "clear_dead", "clear_completed"
    pub confirm: Option<bool>,
}

/// Create queue routes
pub fn routes<T>(
    queue: Arc<T>,
) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
where
    T: DatabaseQueue + Send + Sync + 'static,
{
    let queue_filter = warp::any().map(move || queue.clone());

    let list_queues = warp::path("queues")
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and(with_pagination())
        .and(with_filters())
        .and(with_sort())
        .and_then(list_queues_handler);

    let get_queue = warp::path("queues")
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and_then(get_queue_handler);

    let queue_action = warp::path("queues")
        .and(warp::path::param::<String>())
        .and(warp::path("actions"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter.clone())
        .and(warp::body::json())
        .and_then(queue_action_handler);

    let queue_jobs = warp::path("queues")
        .and(warp::path::param::<String>())
        .and(warp::path("jobs"))
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter)
        .and(with_pagination())
        .and(with_filters())
        .and(with_sort())
        .and_then(queue_jobs_handler);

    list_queues
        .or(get_queue)
        .or(queue_action)
        .or(queue_jobs)
}

/// Handler for listing all queues
async fn list_queues_handler<T>(
    queue: Arc<T>,
    pagination: PaginationParams,
    _filters: FilterParams,
    _sort: SortParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get all queue statistics
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            let mut queue_infos: Vec<QueueInfo> = Vec::new();

            for stats in all_stats {
                let queue_info = QueueInfo {
                    name: stats.queue_name.clone(),
                    pending_count: stats.pending_count,
                    running_count: stats.running_count,
                    completed_count: stats.completed_count,
                    failed_count: stats.dead_count + stats.timed_out_count,
                    dead_count: stats.dead_count,
                    avg_processing_time_ms: stats.statistics.avg_processing_time_ms,
                    throughput_per_minute: stats.statistics.throughput_per_minute,
                    error_rate: stats.statistics.error_rate,
                    last_job_at: None, // TODO: Get from database
                    oldest_pending_job: None, // TODO: Get from database
                };
                queue_infos.push(queue_info);
            }

            // Apply pagination
            let total = queue_infos.len() as u64;
            let offset = pagination.get_offset() as usize;
            let limit = pagination.get_limit() as usize;
            
            let items = if offset < queue_infos.len() {
                let end = (offset + limit).min(queue_infos.len());
                queue_infos[offset..end].to_vec()
            } else {
                Vec::new()
            };

            let response = PaginatedResponse {
                items,
                pagination: PaginationMeta::new(&pagination, total),
            };

            Ok(warp::reply::json(&ApiResponse::success(response)))
        }
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to get queue statistics: {}", e));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for getting a specific queue
async fn get_queue_handler<T>(
    queue_name: String,
    queue: Arc<T>,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            if let Some(stats) = all_stats.into_iter().find(|s| s.queue_name == queue_name) {
                // Get additional details for this specific queue
                let priority_breakdown = std::collections::HashMap::new(); // TODO: Implement
                let status_breakdown = std::collections::HashMap::new(); // TODO: Implement
                let hourly_throughput = Vec::new(); // TODO: Implement
                let recent_errors = Vec::new(); // TODO: Implement

                let queue_info = QueueInfo {
                    name: stats.queue_name.clone(),
                    pending_count: stats.pending_count,
                    running_count: stats.running_count,
                    completed_count: stats.completed_count,
                    failed_count: stats.dead_count + stats.timed_out_count,
                    dead_count: stats.dead_count,
                    avg_processing_time_ms: stats.statistics.avg_processing_time_ms,
                    throughput_per_minute: stats.statistics.throughput_per_minute,
                    error_rate: stats.statistics.error_rate,
                    last_job_at: None,
                    oldest_pending_job: None,
                };

                let detailed_stats = DetailedQueueStats {
                    queue_info,
                    priority_breakdown,
                    status_breakdown,
                    hourly_throughput,
                    recent_errors,
                };

                Ok(warp::reply::json(&ApiResponse::success(detailed_stats)))
            } else {
                let response = ApiResponse::<()>::error(format!("Queue '{}' not found", queue_name));
                Ok(warp::reply::json(&response))
            }
        }
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to get queue statistics: {}", e));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for queue actions (pause, resume, clear, etc.)
async fn queue_action_handler<T>(
    queue_name: String,
    queue: Arc<T>,
    action_request: QueueActionRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    match action_request.action.as_str() {
        "clear_dead" => {
            let older_than = chrono::Utc::now() - chrono::Duration::days(7); // Remove jobs older than 7 days
            match queue.purge_dead_jobs(older_than).await {
                Ok(count) => {
                    let response = ApiResponse::success(serde_json::json!({
                        "message": format!("Cleared {} dead jobs from queue '{}'", count, queue_name),
                        "count": count
                    }));
                    Ok(warp::reply::json(&response))
                }
                Err(e) => {
                    let response = ApiResponse::<()>::error(format!("Failed to clear dead jobs: {}", e));
                    Ok(warp::reply::json(&response))
                }
            }
        }
        "clear_completed" => {
            // TODO: Implement clear completed jobs
            let response = ApiResponse::<()>::error("Clear completed jobs not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        "pause" => {
            // TODO: Implement queue pause
            let response = ApiResponse::<()>::error("Queue pause not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        "resume" => {
            // TODO: Implement queue resume
            let response = ApiResponse::<()>::error("Queue resume not yet implemented".to_string());
            Ok(warp::reply::json(&response))
        }
        _ => {
            let response = ApiResponse::<()>::error(format!("Unknown action: {}", action_request.action));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for getting jobs in a specific queue
async fn queue_jobs_handler<T>(
    queue_name: String,
    queue: Arc<T>,
    pagination: PaginationParams,
    filters: FilterParams,
    sort: SortParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // This would delegate to the jobs API with queue filter
    // For now, return a simple response
    let _ = (queue, pagination, filters, sort);
    
    let response = ApiResponse::success(serde_json::json!({
        "message": format!("Jobs for queue '{}' - implementation pending", queue_name),
        "queue": queue_name
    }));
    
    Ok(warp::reply::json(&response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_action_request_deserialization() {
        let json = r#"{"action": "clear_dead", "confirm": true}"#;
        let request: QueueActionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.action, "clear_dead");
        assert_eq!(request.confirm, Some(true));
    }

    #[test]
    fn test_queue_info_serialization() {
        let queue_info = QueueInfo {
            name: "test_queue".to_string(),
            pending_count: 42,
            running_count: 3,
            completed_count: 1000,
            failed_count: 5,
            dead_count: 2,
            avg_processing_time_ms: 150.5,
            throughput_per_minute: 25.0,
            error_rate: 0.05,
            last_job_at: None,
            oldest_pending_job: None,
        };

        let json = serde_json::to_string(&queue_info).unwrap();
        assert!(json.contains("test_queue"));
        assert!(json.contains("42"));
    }
}