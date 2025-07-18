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
    pub created_after: Option<chrono::DateTime<chrono::Utc>>,
    pub created_before: Option<chrono::DateTime<chrono::Utc>>,
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
    // Since DatabaseQueue doesn't provide direct list methods with filters,
    // we'll use the available methods to gather jobs
    let mut all_jobs = Vec::new();
    
    // Get queue stats to find available queues
    let queue_stats = match queue.get_all_queue_stats().await {
        Ok(stats) => stats,
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to get queue stats: {}", e));
            return Ok(warp::reply::json(&response));
        }
    };
    
    // Filter by queue if specified
    let target_queues: Vec<String> = if let Some(ref queue_name) = filters.queue {
        vec![queue_name.clone()]
    } else {
        queue_stats.iter().map(|s| s.queue_name.clone()).collect()
    };
    
    // For each queue, get jobs from different sources based on status filter
    for queue_name in &target_queues {
        let mut queue_jobs = Vec::new();
        
        // Collect jobs based on status filter or get all types if no filter
        if filters.status.is_none() || filters.status.as_ref().unwrap().to_lowercase() == "pending" {
            // Get ready jobs (pending jobs ready to be processed)
            if let Ok(ready_jobs) = queue.get_ready_jobs(&queue_name, 100).await {
                queue_jobs.extend(ready_jobs);
            }
        }
        
        if filters.status.is_none() || filters.status.as_ref().unwrap().to_lowercase() == "failed" || filters.status.as_ref().unwrap().to_lowercase() == "dead" {
            // Get dead jobs
            if let Ok(dead_jobs) = queue.get_dead_jobs_by_queue(&queue_name, Some(100), Some(0)).await {
                queue_jobs.extend(dead_jobs);
            }
        }
        
        if filters.status.is_none() || filters.status.as_ref().unwrap().to_lowercase() == "recurring" {
            // Get recurring jobs
            if let Ok(recurring_jobs) = queue.get_recurring_jobs(&queue_name).await {
                queue_jobs.extend(recurring_jobs);
            }
        }
        
        for job in queue_jobs {
            let processing_time_ms = match (job.started_at, job.completed_at) {
                (Some(started), Some(completed)) => Some((completed - started).num_milliseconds()),
                _ => None,
            };
            
            let job_info = JobInfo {
                id: job.id.to_string(),
                queue_name: job.queue_name.clone(),
                status: job.status.as_str().to_string(),
                priority: format!("{:?}", job.priority),
                attempts: job.attempts as i32,
                max_attempts: job.max_attempts as i32,
                payload: job.payload.clone(),
                created_at: job.created_at,
                scheduled_at: job.scheduled_at,
                started_at: job.started_at,
                completed_at: job.completed_at,
                failed_at: job.failed_at,
                error_message: job.error_message.clone(),
                processing_time_ms,
                cron_schedule: job.cron_schedule.as_ref().map(|c| c.to_string()),
                is_recurring: job.is_recurring(),
                trace_id: job.trace_id.clone(),
                correlation_id: job.correlation_id.clone(),
            };
            
            // Apply status filter
            if let Some(ref status) = filters.status {
                if job_info.status.to_lowercase() != status.to_lowercase() {
                    continue;
                }
            }
            
            // Apply priority filter
            if let Some(ref priority) = filters.priority {
                if job_info.priority.to_lowercase() != priority.to_lowercase() {
                    continue;
                }
            }
            
            all_jobs.push(job_info);
        }
    }
    
    // Sort jobs
    match sort.sort_by.as_deref() {
        Some("created_at") => {
            all_jobs.sort_by(|a, b| {
                if sort.sort_order.as_deref() == Some("asc") {
                    a.created_at.cmp(&b.created_at)
                } else {
                    b.created_at.cmp(&a.created_at)
                }
            });
        }
        Some("scheduled_at") => {
            all_jobs.sort_by(|a, b| {
                if sort.sort_order.as_deref() == Some("asc") {
                    a.scheduled_at.cmp(&b.scheduled_at)
                } else {
                    b.scheduled_at.cmp(&a.scheduled_at)
                }
            });
        }
        Some("priority") => {
            all_jobs.sort_by(|a, b| {
                if sort.sort_order.as_deref() == Some("asc") {
                    a.priority.cmp(&b.priority)
                } else {
                    b.priority.cmp(&a.priority)
                }
            });
        }
        _ => {
            // Default sort by created_at desc
            all_jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        }
    }
    
    // Apply pagination
    let total_count = all_jobs.len() as u64;
    let page_size = pagination.limit.unwrap_or(20).min(100) as usize;
    let page = pagination.page.unwrap_or(1).max(1) as usize;
    let offset = (page - 1) * page_size;
    
    let paginated_jobs: Vec<JobInfo> = all_jobs
        .into_iter()
        .skip(offset)
        .take(page_size)
        .collect();
    
    let response = PaginatedResponse {
        items: paginated_jobs,
        pagination: PaginationMeta::new(&pagination, total_count),
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
    // Since we don't have direct search methods, we'll gather jobs and filter in memory
    let mut matching_jobs = Vec::new();
    let search_term = search_request.query.to_lowercase();
    
    // Get queue stats to find available queues
    let queue_stats = match queue.get_all_queue_stats().await {
        Ok(stats) => stats,
        Err(e) => {
            let response = ApiResponse::<()>::error(format!("Failed to get queue stats: {}", e));
            return Ok(warp::reply::json(&response));
        }
    };
    
    // Filter by specified queues or use all
    let target_queues: Vec<String> = if let Some(ref queue_names) = search_request.queues {
        queue_names.clone()
    } else {
        queue_stats.iter().map(|s| s.queue_name.clone()).collect()
    };
    
    for queue_name in &target_queues {
        let mut queue_jobs = Vec::new();
        
        // Collect jobs from all sources for comprehensive search
        if let Ok(ready_jobs) = queue.get_ready_jobs(&queue_name, 200).await {
            queue_jobs.extend(ready_jobs);
        }
        
        if let Ok(dead_jobs) = queue.get_dead_jobs_by_queue(&queue_name, Some(200), Some(0)).await {
            queue_jobs.extend(dead_jobs);
        }
        
        if let Ok(recurring_jobs) = queue.get_recurring_jobs(&queue_name).await {
            queue_jobs.extend(recurring_jobs);
        }
        
        for job in queue_jobs {
            // Check if job matches search criteria
            let payload_str = serde_json::to_string(&job.payload).unwrap_or_default().to_lowercase();
            let matches_search = job.id.to_string().contains(&search_term) ||
                job.queue_name.to_lowercase().contains(&search_term) ||
                payload_str.contains(&search_term) ||
                job.error_message.as_ref().map(|e| e.to_lowercase().contains(&search_term)).unwrap_or(false) ||
                job.trace_id.as_ref().map(|t| t.to_lowercase().contains(&search_term)).unwrap_or(false) ||
                job.correlation_id.as_ref().map(|c| c.to_lowercase().contains(&search_term)).unwrap_or(false);
            
            if !matches_search {
                continue;
            }
            
            // Apply status filter
            if let Some(ref statuses) = search_request.statuses {
                if !statuses.iter().any(|s| s.eq_ignore_ascii_case(job.status.as_str())) {
                    continue;
                }
            }
            
            // Apply priority filter
            if let Some(ref priorities) = search_request.priorities {
                let job_priority_str = format!("{:?}", job.priority);
                if !priorities.iter().any(|p| p.eq_ignore_ascii_case(&job_priority_str)) {
                    continue;
                }
            }
            
            // Apply date filters
            if let Some(ref created_after) = search_request.created_after {
                if job.created_at < *created_after {
                    continue;
                }
            }
            
            if let Some(ref created_before) = search_request.created_before {
                if job.created_at > *created_before {
                    continue;
                }
            }
            
            let processing_time_ms = match (job.started_at, job.completed_at) {
                (Some(started), Some(completed)) => Some((completed - started).num_milliseconds()),
                _ => None,
            };
            
            matching_jobs.push(JobInfo {
                id: job.id.to_string(),
                queue_name: job.queue_name.clone(),
                status: job.status.as_str().to_string(),
                priority: format!("{:?}", job.priority),
                attempts: job.attempts as i32,
                max_attempts: job.max_attempts as i32,
                payload: job.payload.clone(),
                created_at: job.created_at,
                scheduled_at: job.scheduled_at,
                started_at: job.started_at,
                completed_at: job.completed_at,
                failed_at: job.failed_at,
                error_message: job.error_message.clone(),
                processing_time_ms,
                cron_schedule: job.cron_schedule.as_ref().map(|c| c.to_string()),
                is_recurring: job.is_recurring(),
                trace_id: job.trace_id.clone(),
                correlation_id: job.correlation_id.clone(),
            });
        }
        
        // Also search recurring jobs
        let recurring_jobs = match queue.get_recurring_jobs(&queue_name).await {
            Ok(jobs) => jobs,
            Err(e) => {
                eprintln!("Failed to get recurring jobs for queue {}: {}", queue_name, e);
                continue;
            }
        };
            
        for job in recurring_jobs {
            // Check if job matches search criteria
            let payload_str = serde_json::to_string(&job.payload).unwrap_or_default().to_lowercase();
            let matches_search = job.id.to_string().contains(&search_term) ||
                job.queue_name.to_lowercase().contains(&search_term) ||
                payload_str.contains(&search_term) ||
                job.error_message.as_ref().map(|e| e.to_lowercase().contains(&search_term)).unwrap_or(false) ||
                job.trace_id.as_ref().map(|t| t.to_lowercase().contains(&search_term)).unwrap_or(false) ||
                job.correlation_id.as_ref().map(|c| c.to_lowercase().contains(&search_term)).unwrap_or(false);
            
            if !matches_search {
                continue;
            }
            
            // Apply additional filters
            if let Some(ref statuses) = search_request.statuses {
                if !statuses.iter().any(|s| s.eq_ignore_ascii_case(job.status.as_str())) {
                    continue;
                }
            }
            
            if let Some(ref priorities) = search_request.priorities {
                let job_priority_str = format!("{:?}", job.priority);
                if !priorities.iter().any(|p| p.eq_ignore_ascii_case(&job_priority_str)) {
                    continue;
                }
            }
            
            if let Some(ref created_after) = search_request.created_after {
                if job.created_at < *created_after {
                    continue;
                }
            }
            
            if let Some(ref created_before) = search_request.created_before {
                if job.created_at > *created_before {
                    continue;
                }
            }
            
            let processing_time_ms = match (job.started_at, job.completed_at) {
                (Some(started), Some(completed)) => Some((completed - started).num_milliseconds()),
                _ => None,
            };
            
            matching_jobs.push(JobInfo {
                id: job.id.to_string(),
                queue_name: job.queue_name.clone(),
                status: job.status.as_str().to_string(),
                priority: format!("{:?}", job.priority),
                attempts: job.attempts as i32,
                max_attempts: job.max_attempts as i32,
                payload: job.payload.clone(),
                created_at: job.created_at,
                scheduled_at: job.scheduled_at,
                started_at: job.started_at,
                completed_at: job.completed_at,
                failed_at: job.failed_at,
                error_message: job.error_message.clone(),
                processing_time_ms,
                cron_schedule: job.cron_schedule.as_ref().map(|c| c.to_string()),
                is_recurring: job.is_recurring(),
                trace_id: job.trace_id.clone(),
                correlation_id: job.correlation_id.clone(),
            });
        }
    }
    
    // Sort by created_at desc by default
    matching_jobs.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    
    // Apply pagination
    let total_count = matching_jobs.len() as u64;
    let page_size = pagination.limit.unwrap_or(20).min(100) as usize;
    let page = pagination.page.unwrap_or(1).max(1) as usize;
    let offset = (page - 1) * page_size;
    
    let paginated_jobs: Vec<JobInfo> = matching_jobs
        .into_iter()
        .skip(offset)
        .take(page_size)
        .collect();
    
    let response = PaginatedResponse {
        items: paginated_jobs,
        pagination: PaginationMeta::new(&pagination, total_count),
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
