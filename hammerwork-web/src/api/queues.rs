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
//!     is_paused: false,
//!     paused_at: None,
//!     paused_by: None,
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
//!     is_paused: false,
//!     paused_at: None,
//!     paused_by: None,
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

use super::history::{FinishedKind, JobHistory};
use super::{
    ApiResponse, FilterParams, PaginatedResponse, PaginationMeta, PaginationParams, SortParams,
    with_filters, with_pagination, with_sort,
};
use super::{error_reply, json_reply};
use hammerwork::{JobPriority, queue::DatabaseQueue};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use warp::http::StatusCode;
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
    T: JobHistory + 'static,
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
                let pause_info = try_api!(
                    queue.get_queue_pause_info(&stats.queue_name).await,
                    "Failed to get queue pause info"
                );

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
                    last_job_at: try_api!(
                        get_last_job_time(&queue, &stats.queue_name).await,
                        "Failed to get last job time"
                    ),
                    oldest_pending_job: try_api!(
                        get_oldest_pending_job(&queue, &stats.queue_name).await,
                        "Failed to get oldest pending job"
                    ),
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

            Ok(json_reply(&ApiResponse::success(response)))
        }
        Err(e) => Ok(error_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get queue statistics: {}", e),
        )),
    }
}

/// Handler for getting a specific queue
async fn get_queue_handler<T>(
    queue_name: String,
    queue: Arc<T>,
) -> Result<impl Reply, warp::Rejection>
where
    T: JobHistory,
{
    match queue.get_all_queue_stats().await {
        Ok(all_stats) => {
            if let Some(stats) = all_stats.into_iter().find(|s| s.queue_name == queue_name) {
                // Get additional details for this specific queue
                let priority_breakdown = try_api!(
                    get_priority_breakdown(&queue, &queue_name).await,
                    "Failed to get priority breakdown"
                );
                let status_breakdown = try_api!(
                    get_status_breakdown(&queue, &queue_name).await,
                    "Failed to get status breakdown"
                );
                let hourly_throughput = try_api!(
                    get_hourly_throughput(&*queue, &queue_name).await,
                    "Failed to get hourly throughput"
                );
                let recent_errors = try_api!(
                    get_recent_errors(&queue, &queue_name).await,
                    "Failed to get recent errors"
                );

                // Get pause information for this queue
                let pause_info = try_api!(
                    queue.get_queue_pause_info(&queue_name).await,
                    "Failed to get queue pause info"
                );

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
                    last_job_at: try_api!(
                        get_last_job_time(&queue, &stats.queue_name).await,
                        "Failed to get last job time"
                    ),
                    oldest_pending_job: try_api!(
                        get_oldest_pending_job(&queue, &stats.queue_name).await,
                        "Failed to get oldest pending job"
                    ),
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

                Ok(json_reply(&ApiResponse::success(detailed_stats)))
            } else {
                Ok(error_reply(
                    StatusCode::NOT_FOUND,
                    format!("Queue '{}' not found", queue_name),
                ))
            }
        }
        Err(e) => Ok(error_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get queue statistics: {}", e),
        )),
    }
}

/// Handler for queue actions (pause, resume, clear, etc.)
async fn queue_action_handler<T>(
    queue_name: String,
    queue: Arc<T>,
    action_request: QueueActionRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: JobHistory,
{
    match action_request.action.as_str() {
        "clear_dead" => {
            let older_than = chrono::Utc::now() - chrono::Duration::days(7); // Remove jobs older than 7 days
            match queue
                .delete_jobs(Some(&queue_name), FinishedKind::Dead, Some(older_than))
                .await
            {
                Ok(count) => {
                    let response = ApiResponse::success(serde_json::json!({
                        "message": format!(
                            "Cleared {} dead jobs older than 7 days from queue '{}'",
                            count, queue_name
                        ),
                        "queue": queue_name,
                        "count": count
                    }));
                    Ok(json_reply(&response))
                }
                Err(e) => Ok(error_reply(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to clear dead jobs: {}", e),
                )),
            }
        }
        "clear_completed" => match queue
            .delete_jobs(Some(&queue_name), FinishedKind::Completed, None)
            .await
        {
            Ok(count) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Cleared {} completed jobs from queue '{}'", count, queue_name),
                    "queue": queue_name,
                    "cleared_count": count
                }));
                Ok(json_reply(&response))
            }
            Err(e) => Ok(error_reply(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to clear completed jobs: {}", e),
            )),
        },
        "pause" => match queue.pause_queue(&queue_name, Some("web-ui")).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Queue '{}' has been paused", queue_name),
                    "queue": queue_name,
                    "action": "pause"
                }));
                Ok(json_reply(&response))
            }
            Err(e) => Ok(error_reply(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to pause queue: {}", e),
            )),
        },
        "resume" => match queue.resume_queue(&queue_name, Some("web-ui")).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Queue '{}' has been resumed", queue_name),
                    "queue": queue_name,
                    "action": "resume"
                }));
                Ok(json_reply(&response))
            }
            Err(e) => Ok(error_reply(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to resume queue: {}", e),
            )),
        },
        _ => Ok(error_reply(
            StatusCode::BAD_REQUEST,
            format!("Unknown action: {}", action_request.action),
        )),
    }
}

/// Handler for getting jobs in a specific queue: the jobs listing scoped to the queue.
async fn queue_jobs_handler<T>(
    queue_name: String,
    queue: Arc<T>,
    pagination: PaginationParams,
    mut filters: FilterParams,
    sort: SortParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    filters.queue = Some(queue_name);
    super::jobs::list_jobs_handler(queue, pagination, filters, sort).await
}

/// Helper function to get the last job time for a queue
async fn get_last_job_time<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<Option<chrono::DateTime<chrono::Utc>>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get recent jobs from multiple sources and find the most recent timestamp
    let mut latest_time: Option<chrono::DateTime<chrono::Utc>> = None;

    // Check ready jobs
    let ready_jobs = queue.get_ready_jobs(queue_name, 10).await?;
    for job in ready_jobs {
        if let Some(time) = job.completed_at.or(job.started_at).or(Some(job.created_at)) {
            latest_time = match latest_time {
                Some(current) if time > current => Some(time),
                None => Some(time),
                _ => latest_time,
            };
        }
    }

    // Check dead jobs
    let dead_jobs = queue
        .get_dead_jobs_by_queue(queue_name, Some(10), Some(0))
        .await?;
    for job in dead_jobs {
        if let Some(time) = job
            .failed_at
            .or(job.completed_at)
            .or(job.started_at)
            .or(Some(job.created_at))
        {
            latest_time = match latest_time {
                Some(current) if time > current => Some(time),
                None => Some(time),
                _ => latest_time,
            };
        }
    }

    Ok(latest_time)
}

/// Helper function to get the oldest pending job time for a queue
async fn get_oldest_pending_job<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<Option<chrono::DateTime<chrono::Utc>>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get ready jobs (these are pending jobs) and find the oldest
    let ready_jobs = queue.get_ready_jobs(queue_name, 100).await?;
    Ok(ready_jobs
        .iter()
        .filter(|job| matches!(job.status, hammerwork::job::JobStatus::Pending))
        .map(|job| job.created_at)
        .min())
}

/// Helper function to get priority breakdown for a queue
async fn get_priority_breakdown<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<std::collections::HashMap<String, u64>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Use the new get_priority_stats method
    let priority_stats = queue.get_priority_stats(queue_name).await?;
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
    Ok(breakdown)
}

/// Helper function to get status breakdown for a queue
async fn get_status_breakdown<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<std::collections::HashMap<String, u64>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Use existing job counts method
    let counts = queue.get_job_counts_by_status(queue_name).await?;
    Ok(counts.into_iter().collect())
}

/// Completed/failed counts per hour over the last 24 hours (23 whole hours plus the
/// current one) for a queue.
async fn get_hourly_throughput<T>(
    queue: &T,
    queue_name: &str,
) -> hammerwork::Result<Vec<HourlyThroughput>>
where
    T: JobHistory,
{
    let now = chrono::Utc::now();
    let start = super::history::hour_floor(now) - chrono::Duration::hours(23);
    let buckets = queue.hourly_activity(Some(queue_name), start, now).await?;
    Ok(buckets
        .into_iter()
        .map(|b| HourlyThroughput {
            hour: b.hour,
            completed: b.completed,
            failed: b.failed,
        })
        .collect())
}

/// Helper function to get recent errors for a queue
async fn get_recent_errors<T>(
    queue: &Arc<T>,
    queue_name: &str,
) -> hammerwork::Result<Vec<RecentError>>
where
    T: DatabaseQueue + Send + Sync,
{
    // Get dead jobs which contain failed jobs with error messages
    let dead_jobs = queue
        .get_dead_jobs_by_queue(queue_name, Some(20), Some(0))
        .await?;
    Ok(dead_jobs
        .into_iter()
        .filter_map(|job| {
            job.error_message.map(|error_msg| RecentError {
                job_id: job.id.to_string(),
                error_message: error_msg,
                occurred_at: job.failed_at.unwrap_or(job.created_at),
                attempts: job.attempts,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{body_json, unreachable_queue};

    #[tokio::test]
    async fn test_list_queues_returns_500_when_database_is_down() {
        let response = list_queues_handler(
            unreachable_queue(),
            PaginationParams::default(),
            serde_json::from_value(serde_json::json!({})).unwrap(),
            SortParams {
                sort_by: None,
                sort_order: None,
            },
        )
        .await
        .unwrap()
        .into_response();
        let (status, body) = body_json(response).await;
        assert_eq!(status, 500);
        assert_eq!(body["success"], false);
    }

    #[tokio::test]
    async fn test_clear_actions_report_database_failures_not_success() {
        for action in ["clear_completed", "clear_dead"] {
            let response = queue_action_handler(
                "q".to_string(),
                unreachable_queue(),
                QueueActionRequest {
                    action: action.to_string(),
                    confirm: Some(true),
                },
            )
            .await
            .unwrap()
            .into_response();
            let (status, body) = body_json(response).await;
            assert_eq!(status, 500, "{action}");
            assert_eq!(body["success"], false);
        }
    }

    #[tokio::test]
    async fn test_queue_jobs_delegates_to_job_listing() {
        let response = queue_jobs_handler(
            "q".to_string(),
            unreachable_queue(),
            PaginationParams::default(),
            serde_json::from_value(serde_json::json!({})).unwrap(),
            SortParams {
                sort_by: None,
                sort_order: None,
            },
        )
        .await
        .unwrap()
        .into_response();
        let (status, body) = body_json(response).await;
        // The real listing ran (and failed on the dead database), not a stub message.
        assert_eq!(status, 500);
        assert!(body.get("data").is_none_or(|d| d.is_null()));
    }

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
