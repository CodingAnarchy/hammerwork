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
    ApiResponse, FilterParams, PaginatedResponse, PaginationMeta, PaginationParams, SortParams,
    with_filters, with_pagination, with_sort,
};
use hammerwork::{queue::DatabaseQueue, JobPriority};
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
    pub is_paused: bool,
    pub paused_at: Option<chrono::DateTime<chrono::Utc>>,
    pub paused_by: Option<String>,
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

    list_queues.or(get_queue).or(queue_action).or(queue_jobs)
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
                // Get pause information for this queue
                let pause_info = queue
                    .get_queue_pause_info(&stats.queue_name)
                    .await
                    .unwrap_or(None);

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
                    last_job_at: get_last_job_time(&queue, &stats.queue_name).await,
                    oldest_pending_job: get_oldest_pending_job(&queue, &stats.queue_name).await,
                    is_paused: pause_info.is_some(),
                    paused_at: pause_info.as_ref().map(|p| p.paused_at),
                    paused_by: pause_info.as_ref().and_then(|p| p.paused_by.clone()),
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
            let response =
                ApiResponse::<()>::error(format!("Failed to get queue statistics: {}", e));
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
                let priority_breakdown = get_priority_breakdown(&queue, &queue_name).await;
                let status_breakdown = get_status_breakdown(&queue, &queue_name).await;
                let hourly_throughput = get_hourly_throughput(&queue, &queue_name).await;
                let recent_errors = get_recent_errors(&queue, &queue_name).await;

                // Get pause information for this queue
                let pause_info = queue
                    .get_queue_pause_info(&queue_name)
                    .await
                    .unwrap_or(None);

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
                    is_paused: pause_info.is_some(),
                    paused_at: pause_info.as_ref().map(|p| p.paused_at),
                    paused_by: pause_info.as_ref().and_then(|p| p.paused_by.clone()),
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
                let response =
                    ApiResponse::<()>::error(format!("Queue '{}' not found", queue_name));
                Ok(warp::reply::json(&response))
            }
        }
        Err(e) => {
            let response =
                ApiResponse::<()>::error(format!("Failed to get queue statistics: {}", e));
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
                    let response =
                        ApiResponse::<()>::error(format!("Failed to clear dead jobs: {}", e));
                    Ok(warp::reply::json(&response))
                }
            }
        }
        "clear_completed" => {
            match clear_completed_jobs(&queue, &queue_name).await {
                Ok(count) => {
                    let response = ApiResponse::success(serde_json::json!({
                        "message": format!("Cleared {} completed jobs from queue '{}'", count, queue_name),
                        "queue": queue_name,
                        "cleared_count": count
                    }));
                    Ok(warp::reply::json(&response))
                }
                Err(e) => {
                    let response = ApiResponse::<()>::error(format!("Failed to clear completed jobs: {}", e));
                    Ok(warp::reply::json(&response))
                }
            }
        }
        "pause" => match queue.pause_queue(&queue_name, Some("web-ui")).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Queue '{}' has been paused", queue_name),
                    "queue": queue_name,
                    "action": "pause"
                }));
                Ok(warp::reply::json(&response))
            }
            Err(e) => {
                let response = ApiResponse::<()>::error(format!("Failed to pause queue: {}", e));
                Ok(warp::reply::json(&response))
            }
        },
        "resume" => match queue.resume_queue(&queue_name, Some("web-ui")).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Queue '{}' has been resumed", queue_name),
                    "queue": queue_name,
                    "action": "resume"
                }));
                Ok(warp::reply::json(&response))
            }
            Err(e) => {
                let response = ApiResponse::<()>::error(format!("Failed to resume queue: {}", e));
                Ok(warp::reply::json(&response))
            }
        },
        _ => {
            let response =
                ApiResponse::<()>::error(format!("Unknown action: {}", action_request.action));
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

/// Helper function to get the last job time for a queue
async fn get_last_job_time<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> Option<chrono::DateTime<chrono::Utc>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get recent jobs from multiple sources and find the most recent timestamp
    let mut latest_time: Option<chrono::DateTime<chrono::Utc>> = None;
    
    // Check ready jobs
    if let Ok(ready_jobs) = queue.get_ready_jobs(queue_name, 10).await {
        for job in ready_jobs {
            if let Some(time) = job.completed_at.or(job.started_at).or(Some(job.created_at)) {
                latest_time = match latest_time {
                    Some(current) if time > current => Some(time),
                    None => Some(time),
                    _ => latest_time,
                };
            }
        }
    }
    
    // Check dead jobs
    if let Ok(dead_jobs) = queue.get_dead_jobs_by_queue(queue_name, Some(10), Some(0)).await {
        for job in dead_jobs {
            if let Some(time) = job.failed_at.or(job.completed_at).or(job.started_at).or(Some(job.created_at)) {
                latest_time = match latest_time {
                    Some(current) if time > current => Some(time),
                    None => Some(time),
                    _ => latest_time,
                };
            }
        }
    }
    
    latest_time
}

/// Helper function to get the oldest pending job time for a queue
async fn get_oldest_pending_job<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> Option<chrono::DateTime<chrono::Utc>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get ready jobs (these are pending jobs) and find the oldest
    if let Ok(ready_jobs) = queue.get_ready_jobs(queue_name, 100).await {
        ready_jobs
            .iter()
            .filter(|job| matches!(job.status, hammerwork::job::JobStatus::Pending))
            .map(|job| job.created_at)
            .min()
    } else {
        None
    }
}

/// Helper function to get priority breakdown for a queue
async fn get_priority_breakdown<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> std::collections::HashMap<String, u64>
where
    T: DatabaseQueue + Send + Sync,
{
    // Use the new get_priority_stats method
    if let Ok(priority_stats) = queue.get_priority_stats(queue_name).await {
        let mut breakdown = std::collections::HashMap::new();
        for (priority, count) in priority_stats.job_counts {
            let priority_name = match priority {
                JobPriority::Background => "background",
                JobPriority::Low => "low",
                JobPriority::Normal => "normal", 
                JobPriority::High => "high",
                JobPriority::Critical => "critical",
            };
            breakdown.insert(priority_name.to_string(), count);
        }
        breakdown
    } else {
        std::collections::HashMap::new()
    }
}

/// Helper function to get status breakdown for a queue
async fn get_status_breakdown<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> std::collections::HashMap<String, u64>
where
    T: DatabaseQueue + Send + Sync,
{
    // Use existing job counts method
    if let Ok(counts) = queue.get_job_counts_by_status(queue_name).await {
        counts.into_iter().collect()
    } else {
        std::collections::HashMap::new()
    }
}

/// Helper function to get hourly throughput data for a queue
async fn get_hourly_throughput<T>(
    _queue: &Arc<T>,
    _queue_name: &str,
) -> Vec<HourlyThroughput>
where
    T: DatabaseQueue + Send + Sync,
{
    // This would require tracking hourly statistics
    // For now, return empty as it's not implemented in the DatabaseQueue trait
    Vec::new()
}

/// Helper function to get recent errors for a queue
async fn get_recent_errors<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> Vec<RecentError>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get dead jobs which contain failed jobs with error messages
    if let Ok(dead_jobs) = queue.get_dead_jobs_by_queue(queue_name, Some(20), Some(0)).await {
        dead_jobs
            .into_iter()
            .filter_map(|job| {
                job.error_message.map(|error_msg| RecentError {
                    job_id: job.id.to_string(),
                    error_message: error_msg,
                    occurred_at: job.failed_at.unwrap_or(job.created_at),
                    attempts: job.attempts as i32,
                })
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// Helper function to clear completed jobs from a queue
async fn clear_completed_jobs<T>(
    _queue: &Arc<T>,
    _queue_name: &str,
) -> Result<u64, String>
where
    T: DatabaseQueue + Send + Sync,
{
    // To implement this properly, we would need:
    // 1. A method to query jobs by status (get_jobs_by_status)
    // 2. Filter for completed jobs
    // 3. Delete them using the existing delete_job method
    // 
    // Since DatabaseQueue trait doesn't provide a way to query jobs by status,
    // we cannot implement this functionality without extending the trait.
    Err("Clear completed jobs requires additional DatabaseQueue methods not yet available".to_string())
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
            is_paused: false,
            paused_at: None,
            paused_by: None,
        };

        let json = serde_json::to_string(&queue_info).unwrap();
        assert!(json.contains("test_queue"));
        assert!(json.contains("42"));
        assert!(json.contains("is_paused"));
    }
}
