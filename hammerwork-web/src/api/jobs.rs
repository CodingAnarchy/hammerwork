//! Job management API endpoints.
//!
//! This module provides comprehensive REST API endpoints for managing Hammerwork jobs,
//! including creating, listing, searching, and performing actions on jobs.
//!
//! # API Endpoints
//!
//! - `GET /api/jobs` - List jobs with filtering, pagination, and sorting
//! - `POST /api/jobs` - Create a new job
//! - `GET /api/jobs/{id}` - Get details of a specific job
//! - `POST /api/jobs/{id}/actions` - Perform actions on a job (retry, cancel, delete)
//! - `POST /api/jobs/bulk` - Perform bulk actions on multiple jobs
//! - `POST /api/jobs/search` - Search jobs with full-text queries
//!
//! # Examples
//!
//! ## Creating a Job
//!
//! ```rust
//! use hammerwork_web::api::jobs::CreateJobRequest;
//! use serde_json::json;
//!
//! let create_request = CreateJobRequest {
//!     queue_name: "email_queue".to_string(),
//!     payload: json!({
//!         "to": "user@example.com",
//!         "subject": "Welcome!",
//!         "template": "welcome_email"
//!     }),
//!     priority: Some("high".to_string()),
//!     scheduled_at: None,
//!     max_attempts: Some(3),
//!     cron_schedule: None,
//!     trace_id: Some("trace-123".to_string()),
//!     correlation_id: Some("corr-456".to_string()),
//! };
//!
//! // This would be sent as JSON in a POST request to /api/jobs
//! let json_payload = serde_json::to_string(&create_request).unwrap();
//! assert!(json_payload.contains("email_queue"));
//! assert!(json_payload.contains("high"));
//! ```
//!
//! ## Job Actions
//!
//! ```rust
//! use hammerwork_web::api::jobs::JobActionRequest;
//!
//! let retry_request = JobActionRequest {
//!     action: "retry".to_string(),
//!     reason: Some("Network issue resolved".to_string()),
//! };
//!
//! let cancel_request = JobActionRequest {
//!     action: "cancel".to_string(),
//!     reason: Some("No longer needed".to_string()),
//! };
//!
//! assert_eq!(retry_request.action, "retry");
//! assert_eq!(cancel_request.action, "cancel");
//! ```
//!
//! ## Bulk Operations
//!
//! ```rust
//! use hammerwork_web::api::jobs::BulkJobActionRequest;
//!
//! let bulk_delete = BulkJobActionRequest {
//!     job_ids: vec![
//!         "550e8400-e29b-41d4-a716-446655440000".to_string(),
//!         "550e8400-e29b-41d4-a716-446655440001".to_string(),
//!     ],
//!     action: "delete".to_string(),
//!     reason: Some("Cleanup old failed jobs".to_string()),
//! };
//!
//! assert_eq!(bulk_delete.job_ids.len(), 2);
//! assert_eq!(bulk_delete.action, "delete");
//! ```

use super::{
    ApiResponse, FilterParams, PaginatedResponse, PaginationMeta, PaginationParams, SortParams,
    with_filters, with_pagination, with_sort,
};
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use warp::{Filter, Reply};

/// Job information for API responses
#[derive(Debug, Serialize)]
pub struct JobInfo {
    pub id: String,
    pub queue_name: String,
    pub status: String,
    pub priority: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub payload: serde_json::Value,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub scheduled_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub failed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
    pub processing_time_ms: Option<i64>,
    pub cron_schedule: Option<String>,
    pub is_recurring: bool,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
}

/// Job creation request
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateJobRequest {
    pub queue_name: String,
    pub payload: serde_json::Value,
    pub priority: Option<String>,
    pub scheduled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub max_attempts: Option<i32>,
    pub cron_schedule: Option<String>,
    pub trace_id: Option<String>,
    pub correlation_id: Option<String>,
}

/// Job action request
#[derive(Debug, Deserialize)]
pub struct JobActionRequest {
    pub action: String, // "retry", "cancel", "delete"
    pub reason: Option<String>,
}

/// Bulk job action request
#[derive(Debug, Deserialize)]
pub struct BulkJobActionRequest {
    pub job_ids: Vec<String>,
    pub action: String,
    pub reason: Option<String>,
}

/// Job search request
#[derive(Debug, Deserialize)]
pub struct JobSearchRequest {
    pub query: String,
    pub queues: Option<Vec<String>>,
    pub statuses: Option<Vec<String>>,
    pub priorities: Option<Vec<String>>,
}

/// Create job routes
pub fn routes<T>(
    queue: Arc<T>,
) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
where
    T: DatabaseQueue + Send + Sync + 'static,
{
    let queue_filter = warp::any().map(move || queue.clone());

    let list_jobs = warp::path("jobs")
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and(with_pagination())
        .and(with_filters())
        .and(with_sort())
        .and_then(list_jobs_handler);

    let create_job = warp::path("jobs")
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter.clone())
        .and(warp::body::json())
        .and_then(create_job_handler);

    let get_job = warp::path("jobs")
        .and(warp::path::param::<String>())
        .and(warp::path::end())
        .and(warp::get())
        .and(queue_filter.clone())
        .and_then(get_job_handler);

    let job_action = warp::path("jobs")
        .and(warp::path::param::<String>())
        .and(warp::path("actions"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter.clone())
        .and(warp::body::json())
        .and_then(job_action_handler);

    let bulk_action = warp::path("jobs")
        .and(warp::path("bulk"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter.clone())
        .and(warp::body::json())
        .and_then(bulk_job_action_handler);

    let search_jobs = warp::path("jobs")
        .and(warp::path("search"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter)
        .and(warp::body::json())
        .and(with_pagination())
        .and_then(search_jobs_handler);

    list_jobs
        .or(create_job)
        .or(get_job)
        .or(job_action)
        .or(bulk_action)
        .or(search_jobs)
}

/// Handler for listing jobs
async fn list_jobs_handler<T>(
    queue: Arc<T>,
    pagination: PaginationParams,
    filters: FilterParams,
    sort: SortParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // For now, return a placeholder response
    // In a real implementation, this would query the database with filters and pagination
    let _ = (queue, filters, sort);

    let mock_jobs = vec![JobInfo {
        id: "job-1".to_string(),
        queue_name: "default".to_string(),
        status: "pending".to_string(),
        priority: "normal".to_string(),
        attempts: 0,
        max_attempts: 3,
        payload: serde_json::json!({"task": "send_email", "to": "user@example.com"}),
        created_at: chrono::Utc::now(),
        scheduled_at: chrono::Utc::now(),
        started_at: None,
        completed_at: None,
        failed_at: None,
        error_message: None,
        processing_time_ms: None,
        cron_schedule: None,
        is_recurring: false,
        trace_id: None,
        correlation_id: None,
    }];

    let response = PaginatedResponse {
        items: mock_jobs,
        pagination: PaginationMeta::new(&pagination, 1),
    };

    Ok(warp::reply::json(&ApiResponse::success(response)))
}

/// Handler for creating a new job
async fn create_job_handler<T>(
    queue: Arc<T>,
    request: CreateJobRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    use hammerwork::{Job, JobPriority};

    let priority = match request.priority.as_deref() {
        Some("background") => JobPriority::Background,
        Some("low") => JobPriority::Low,
        Some("normal") => JobPriority::Normal,
        Some("high") => JobPriority::High,
        Some("critical") => JobPriority::Critical,
        _ => JobPriority::Normal,
    };

    let mut job = Job::new(request.queue_name, request.payload).with_priority(priority);

    if let Some(scheduled_at) = request.scheduled_at {
        job.scheduled_at = scheduled_at;
    }

    if let Some(max_attempts) = request.max_attempts {
        job = job.with_max_attempts(max_attempts);
    }

    if let Some(trace_id) = request.trace_id {
        job.trace_id = Some(trace_id);
    }

    if let Some(correlation_id) = request.correlation_id {
        job.correlation_id = Some(correlation_id);
    }

    match queue.enqueue(job).await {
        Ok(job_id) => {
            let response = ApiResponse::success(serde_json::json!({
                "message": "Job created successfully",
                "job_id": job_id.to_string()
            }));
            Ok(warp::reply::json(&response))
        }
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to create job: {}", e));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for getting a specific job
async fn get_job_handler<T>(job_id: String, queue: Arc<T>) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    let job_uuid = match uuid::Uuid::parse_str(&job_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            let response = ApiResponse::<()>::error("Invalid job ID format".to_string());
            return Ok(warp::reply::json(&response));
        }
    };

    match queue.get_job(job_uuid).await {
        Ok(Some(job)) => {
            let job_info = JobInfo {
                id: job.id.to_string(),
                queue_name: job.queue_name.clone(),
                status: format!("{:?}", job.status),
                priority: format!("{:?}", job.priority),
                attempts: job.attempts,
                max_attempts: job.max_attempts,
                payload: job.payload.clone(),
                created_at: job.created_at,
                scheduled_at: job.scheduled_at,
                started_at: job.started_at,
                completed_at: job.completed_at,
                failed_at: job.failed_at,
                error_message: job.error_message.clone(),
                processing_time_ms: job.started_at.and_then(|start| {
                    job.completed_at
                        .or(job.failed_at)
                        .or(job.timed_out_at)
                        .map(|end| (end - start).num_milliseconds())
                }),
                cron_schedule: job.cron_schedule.clone(),
                is_recurring: job.is_recurring(),
                trace_id: job.trace_id.clone(),
                correlation_id: job.correlation_id.clone(),
            };

            Ok(warp::reply::json(&ApiResponse::success(job_info)))
        }
        Ok(None) => {
            let response = ApiResponse::<()>::error(format!("Job '{}' not found", job_id));
            Ok(warp::reply::json(&response))
        }
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to get job: {}", e));
            Ok(warp::reply::json(&response))
        }
    }
}

/// Handler for job actions
async fn job_action_handler<T>(
    job_id: String,
    queue: Arc<T>,
    action_request: JobActionRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    let job_uuid = match uuid::Uuid::parse_str(&job_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            let response = ApiResponse::<()>::error("Invalid job ID format".to_string());
            return Ok(warp::reply::json(&response));
        }
    };

    match action_request.action.as_str() {
        "retry" => match queue.retry_job(job_uuid, chrono::Utc::now()).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Job '{}' scheduled for retry", job_id)
                }));
                Ok(warp::reply::json(&response))
            }
            Err(e) => {
                let response = ApiResponse::<()>::error(format!("Failed to retry job: {}", e));
                Ok(warp::reply::json(&response))
            }
        },
        "cancel" | "delete" => match queue.delete_job(job_uuid).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Job '{}' deleted", job_id)
                }));
                Ok(warp::reply::json(&response))
            }
            Err(e) => {
                let response = ApiResponse::<()>::error(format!("Failed to delete job: {}", e));
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

/// Handler for bulk job actions
async fn bulk_job_action_handler<T>(
    queue: Arc<T>,
    request: BulkJobActionRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    let mut successful = 0;
    let mut failed = 0;
    let mut errors = Vec::new();

    for job_id_str in &request.job_ids {
        let job_uuid = match uuid::Uuid::parse_str(job_id_str) {
            Ok(uuid) => uuid,
            Err(_) => {
                failed += 1;
                errors.push(format!("Invalid job ID: {}", job_id_str));
                continue;
            }
        };

        let result = match request.action.as_str() {
            "retry" => queue.retry_job(job_uuid, chrono::Utc::now()).await,
            "delete" => queue.delete_job(job_uuid).await,
            _ => {
                failed += 1;
                errors.push(format!("Unknown action: {}", request.action));
                continue;
            }
        };

        match result {
            Ok(()) => successful += 1,
            Err(e) => {
                failed += 1;
                errors.push(format!("Job {}: {}", job_id_str, e));
            }
        }
    }

    let response = ApiResponse::success(serde_json::json!({
        "successful": successful,
        "failed": failed,
        "errors": errors,
        "message": format!("Bulk {} completed: {} successful, {} failed", request.action, successful, failed)
    }));

    Ok(warp::reply::json(&response))
}

/// Handler for searching jobs
async fn search_jobs_handler<T>(
    queue: Arc<T>,
    search_request: JobSearchRequest,
    pagination: PaginationParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    // For now, return a placeholder response
    // In a real implementation, this would perform full-text search on job payloads
    let _ = (queue, search_request);

    let response = PaginatedResponse {
        items: Vec::<JobInfo>::new(),
        pagination: PaginationMeta::new(&pagination, 0),
    };

    Ok(warp::reply::json(&ApiResponse::success(response)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_job_request_deserialization() {
        let json = r#"{
            "queue_name": "email",
            "payload": {"to": "user@example.com", "subject": "Hello"},
            "priority": "high",
            "max_attempts": 5
        }"#;

        let request: CreateJobRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.queue_name, "email");
        assert_eq!(request.priority, Some("high".to_string()));
        assert_eq!(request.max_attempts, Some(5));
    }

    #[test]
    fn test_job_action_request_deserialization() {
        let json = r#"{"action": "retry", "reason": "Network error resolved"}"#;
        let request: JobActionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.action, "retry");
        assert_eq!(request.reason, Some("Network error resolved".to_string()));
    }

    #[test]
    fn test_bulk_job_action_request() {
        let json = r#"{
            "job_ids": ["job-1", "job-2", "job-3"],
            "action": "delete",
            "reason": "Cleanup old jobs"
        }"#;

        let request: BulkJobActionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.job_ids.len(), 3);
        assert_eq!(request.action, "delete");
    }
}
