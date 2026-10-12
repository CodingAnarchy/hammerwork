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

use super::history::JobHistory;
use super::{
    ApiResponse, FilterParams, PaginatedResponse, PaginationMeta, PaginationParams, SortParams,
    with_filters, with_pagination, with_sort,
};
use super::{error_reply, json_reply, with_shared};
use hammerwork::queue::DatabaseQueue;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use warp::http::StatusCode;
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
    #[allow(dead_code)] // not reached from the running dashboard; exercised by tests
    pub reason: Option<String>,
}

/// The most job IDs one bulk action may name.
pub const MAX_BULK_JOB_IDS: usize = 1000;

/// Bulk job action request (at most [`MAX_BULK_JOB_IDS`] IDs)
#[derive(Debug, Deserialize)]
pub struct BulkJobActionRequest {
    pub job_ids: Vec<String>,
    pub action: String,
    #[allow(dead_code)] // not reached from the running dashboard; exercised by tests
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
    T: JobHistory + 'static,
{
    let queue_filter = with_shared(queue);

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
        .and(crate::security::json_body())
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
        .and(crate::security::json_body())
        .and_then(job_action_handler);

    let bulk_action = warp::path("jobs")
        .and(warp::path("bulk"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter.clone())
        .and(crate::security::json_body())
        .and_then(bulk_job_action_handler);

    let search_jobs = warp::path("jobs")
        .and(warp::path("search"))
        .and(warp::path::end())
        .and(warp::post())
        .and(queue_filter)
        .and(crate::security::json_body())
        .and(with_pagination())
        .and_then(search_jobs_handler);

    list_jobs
        .or(create_job)
        .or(get_job)
        .or(job_action)
        .or(bulk_action)
        .or(search_jobs)
}

/// The API representation of a job.
fn job_info(job: &hammerwork::Job) -> JobInfo {
    let end = job.completed_at.or(job.failed_at).or(job.timed_out_at);
    JobInfo {
        id: job.id.to_string(),
        queue_name: job.queue_name.clone(),
        status: job.status.as_str().to_string(),
        priority: job.priority.to_string(),
        attempts: job.attempts,
        max_attempts: job.max_attempts,
        payload: job.payload.clone(),
        created_at: job.created_at,
        scheduled_at: job.scheduled_at,
        started_at: job.started_at,
        completed_at: job.completed_at,
        failed_at: job.failed_at,
        error_message: job.error_message.clone(),
        processing_time_ms: job
            .started_at
            .zip(end)
            .map(|(start, end)| (end - start).num_milliseconds()),
        cron_schedule: job.cron_schedule.clone(),
        is_recurring: job.is_recurring(),
        trace_id: job.trace_id.clone(),
        correlation_id: job.correlation_id.clone(),
    }
}

/// The most jobs the listing reads per status (or per queue without a status filter).
const LISTING_LIMIT: u32 = 100;

/// The most jobs a search reads per status and queue.
const SEARCH_LIMIT: u32 = 200;

/// Every status a job can be stored in.
const ALL_STATUSES: [&str; 7] = [
    "Pending",
    "Running",
    "Completed",
    "Failed",
    "Dead",
    "TimedOut",
    "Retrying",
];

/// The stored statuses a lowercase `status` filter selects, or `None` when it is not a
/// plain status (`recurring`, or a name that matches nothing). `failed` covers every
/// unsuccessful terminal status.
fn stored_statuses(filter: &str) -> Option<&'static [&'static str]> {
    Some(match filter {
        "pending" => &["Pending"],
        "running" => &["Running"],
        "completed" => &["Completed"],
        "failed" => &["Failed", "Dead", "TimedOut"],
        "dead" => &["Dead"],
        "timedout" => &["TimedOut"],
        "retrying" => &["Retrying"],
        _ => return None,
    })
}

/// Whether a job satisfies the `status` filter of the listing: `failed` covers every
/// unsuccessful terminal status, `recurring` selects recurring jobs, anything else must equal
/// the job's status (ignoring case).
fn status_matches(filter: &str, job: &JobInfo) -> bool {
    let status = job.status.to_lowercase();
    match filter {
        "failed" => matches!(status.as_str(), "failed" | "dead" | "timedout"),
        "recurring" => job.is_recurring,
        other => status == other,
    }
}

/// Order of priorities for sorting (`background` lowest).
fn priority_rank(priority: &str) -> i32 {
    priority
        .parse::<hammerwork::JobPriority>()
        .map(|p| p.as_i32())
        .unwrap_or(-1)
}

/// The jobs of `queue_name` the API can enumerate: pending jobs ready to run, dead jobs
/// and recurring jobs, without duplicates (a recurring job can also be ready).
async fn collect_jobs<T>(
    queue: &T,
    queue_name: &str,
    include_ready: bool,
    include_dead: bool,
    include_recurring: bool,
    limit: u32,
) -> hammerwork::Result<Vec<hammerwork::Job>>
where
    T: DatabaseQueue + Send + Sync,
{
    let mut jobs = Vec::new();
    if include_ready {
        jobs.extend(queue.get_ready_jobs(queue_name, limit).await?);
    }
    if include_dead {
        jobs.extend(
            queue
                .get_dead_jobs_by_queue(queue_name, Some(limit), Some(0))
                .await?,
        );
    }
    if include_recurring {
        jobs.extend(queue.get_recurring_jobs(queue_name).await?);
    }
    let mut seen = std::collections::HashSet::new();
    jobs.retain(|job| seen.insert(job.id));
    Ok(jobs)
}

/// Page `items` according to `pagination` (default 20 per page, at most 100).
fn paginate<I>(items: Vec<I>, pagination: &PaginationParams) -> PaginatedResponse<I> {
    let limit = pagination.limit.unwrap_or(20).clamp(1, 100);
    let page = pagination.page.unwrap_or(1).max(1);
    let offset = pagination
        .offset
        .unwrap_or_else(|| (page - 1).saturating_mul(limit));
    let total = items.len() as u64;
    let items = items
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .collect();
    // Describe the page that was actually served, not the requested limit.
    let served = PaginationParams {
        page: Some((offset / limit).saturating_add(1)),
        limit: Some(limit),
        offset: Some(offset),
    };
    PaginatedResponse {
        items,
        pagination: PaginationMeta::new(&served, total),
    }
}

/// Handler for listing jobs
pub(crate) async fn list_jobs_handler<T>(
    queue: Arc<T>,
    pagination: PaginationParams,
    filters: FilterParams,
    sort: SortParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: JobHistory,
{
    let status_filter = filters
        .status
        .as_deref()
        .map(str::to_lowercase)
        .map(|status| {
            if status == "timed_out" {
                "timedout".to_string()
            } else {
                status
            }
        });
    let search_term = filters
        .search
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase);

    let mut all_jobs = Vec::new();
    let queue_jobs = if let Some(statuses) = status_filter.as_deref().and_then(stored_statuses) {
        // A plain status filter: read the jobs in those statuses, whenever they are due.
        let mut jobs = Vec::new();
        for status in statuses {
            jobs.extend(try_api!(
                queue
                    .jobs_with_status(filters.queue.as_deref(), status, LISTING_LIMIT)
                    .await,
                "Failed to list jobs"
            ));
        }
        jobs
    } else {
        // No status filter (or `recurring`): the jobs ready to run, dead jobs and
        // recurring jobs of each queue.
        let queue_stats = match queue.get_all_queue_stats().await {
            Ok(stats) => stats,
            Err(e) => {
                return Ok(error_reply(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to get queue stats: {}", e),
                ));
            }
        };
        let target_queues: Vec<String> = if let Some(ref queue_name) = filters.queue {
            vec![queue_name.clone()]
        } else {
            queue_stats.iter().map(|s| s.queue_name.clone()).collect()
        };
        let wants = |names: &[&str]| {
            status_filter
                .as_deref()
                .is_none_or(|status| names.contains(&status))
        };
        let mut jobs = Vec::new();
        for queue_name in &target_queues {
            jobs.extend(try_api!(
                collect_jobs(
                    queue.as_ref(),
                    queue_name,
                    wants(&["pending"]),
                    wants(&["failed", "dead"]),
                    wants(&["recurring"]),
                    LISTING_LIMIT
                )
                .await,
                "Failed to list jobs"
            ));
        }
        jobs
    };

    for job in &queue_jobs {
        if filters
            .created_after
            .is_some_and(|after| job.created_at < after)
            || filters
                .created_before
                .is_some_and(|before| job.created_at > before)
        {
            continue;
        }
        if let Some(ref term) = search_term
            && !job_matches_search(job, term)
        {
            continue;
        }

        let job_info = job_info(job);

        // Apply status filter
        if let Some(ref status) = status_filter
            && !status_matches(status, &job_info)
        {
            continue;
        }

        // Apply priority filter
        if let Some(ref priority) = filters.priority
            && !job_info.priority.eq_ignore_ascii_case(priority)
        {
            continue;
        }

        all_jobs.push(job_info);
    }

    // Sort jobs ascending, then reverse for descending order (the default). Without a
    // known `sort_by` the jobs are sorted by creation time, as `SortParams::get_order_by`
    // describes, still honouring `sort_order`.
    match sort.sort_by.as_deref() {
        Some("scheduled_at") => all_jobs.sort_by_key(|j| j.scheduled_at),
        Some("priority") => all_jobs.sort_by_key(|j| priority_rank(&j.priority)),
        _ => all_jobs.sort_by_key(|j| j.created_at),
    }
    if !sort.is_ascending() {
        all_jobs.reverse();
    }

    Ok(json_reply(&ApiResponse::success(paginate(
        all_jobs,
        &pagination,
    ))))
}

/// Handler for creating a new job
async fn create_job_handler<T>(
    queue: Arc<T>,
    request: CreateJobRequest,
) -> Result<impl Reply, warp::Rejection>
where
    T: DatabaseQueue + Send + Sync,
{
    use hammerwork::{CronSchedule, Job, JobPriority};

    if request.queue_name.trim().is_empty() {
        return Ok(error_reply(
            StatusCode::BAD_REQUEST,
            "queue_name must not be empty",
        ));
    }

    let priority = match request.priority.as_deref() {
        None => JobPriority::Normal,
        Some(name) => match name.parse::<JobPriority>() {
            Ok(priority) => priority,
            Err(_) => {
                return Ok(error_reply(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Invalid priority '{}'. Valid options: background, low, normal, high, critical",
                        name
                    ),
                ));
            }
        },
    };

    if let Some(max_attempts) = request.max_attempts
        && max_attempts < 1
    {
        return Ok(error_reply(
            StatusCode::BAD_REQUEST,
            "max_attempts must be at least 1",
        ));
    }

    let mut job = Job::new(request.queue_name, request.payload).with_priority(priority);

    if let Some(expression) = request.cron_schedule.as_deref() {
        let schedule = match CronSchedule::new(expression) {
            Ok(schedule) => schedule,
            Err(e) => {
                return Ok(error_reply(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid cron schedule: {}", e),
                ));
            }
        };
        job = match job.with_cron(schedule) {
            Ok(job) => job,
            Err(e) => {
                return Ok(error_reply(
                    StatusCode::BAD_REQUEST,
                    format!("Invalid cron schedule: {}", e),
                ));
            }
        };
    } else if let Some(scheduled_at) = request.scheduled_at {
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
            Ok(json_reply(&response))
        }
        Err(e) => Ok(error_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create job: {}", e),
        )),
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
            return Ok(error_reply(
                StatusCode::BAD_REQUEST,
                "Invalid job ID format".to_string(),
            ));
        }
    };

    match queue.get_job(job_uuid).await {
        Ok(Some(job)) => Ok(json_reply(&ApiResponse::success(job_info(&job)))),
        Ok(None) => Ok(error_reply(
            StatusCode::NOT_FOUND,
            format!("Job '{}' not found", job_id),
        )),
        Err(e) => Ok(error_reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get job: {}", e),
        )),
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
            return Ok(error_reply(
                StatusCode::BAD_REQUEST,
                "Invalid job ID format".to_string(),
            ));
        }
    };

    match action_request.action.as_str() {
        "retry" => match retry_job_action(queue.as_ref(), job_uuid).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Job '{}' scheduled for retry", job_id)
                }));
                Ok(json_reply(&response))
            }
            Err(e) => Ok(super::queue_error_reply("Failed to retry job", &e)),
        },
        "cancel" | "delete" => match delete_job_action(queue.as_ref(), job_uuid).await {
            Ok(()) => {
                let response = ApiResponse::success(serde_json::json!({
                    "message": format!("Job '{}' deleted", job_id)
                }));
                Ok(json_reply(&response))
            }
            Err(e) => Ok(super::queue_error_reply("Failed to delete job", &e)),
        },
        _ => Ok(error_reply(
            StatusCode::BAD_REQUEST,
            format!("Unknown action: {}", action_request.action),
        )),
    }
}

/// Delete a job, reporting a missing job as `JobNotFound` instead of silently succeeding.
async fn delete_job_action<T>(queue: &T, job_id: uuid::Uuid) -> hammerwork::Result<()>
where
    T: DatabaseQueue + Send + Sync,
{
    if queue.get_job(job_id).await?.is_none() {
        return Err(hammerwork::HammerworkError::JobNotFound {
            id: job_id.to_string(),
        });
    }
    queue.delete_job(job_id).await
}

/// Re-run a job now: `Dead` and `TimedOut` jobs go through `retry_dead_job` (which also
/// resets their attempts); other retryable statuses through `retry_job`. Statuses that
/// cannot be retried (e.g. `Completed`) return an `InvalidJobTransition` error, and a
/// missing job `JobNotFound`.
async fn retry_job_action<T>(queue: &T, job_id: uuid::Uuid) -> hammerwork::Result<()>
where
    T: DatabaseQueue + Send + Sync,
{
    let job =
        queue
            .get_job(job_id)
            .await?
            .ok_or_else(|| hammerwork::HammerworkError::JobNotFound {
                id: job_id.to_string(),
            })?;
    match job.status {
        hammerwork::JobStatus::Dead | hammerwork::JobStatus::TimedOut => {
            queue.retry_dead_job(job_id).await
        }
        _ => queue.retry_job(job_id, chrono::Utc::now()).await,
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
    if !matches!(request.action.as_str(), "retry" | "delete") {
        return Ok(error_reply(
            StatusCode::BAD_REQUEST,
            format!("Unknown action: {}", request.action),
        ));
    }
    if request.job_ids.len() > MAX_BULK_JOB_IDS {
        return Ok(error_reply(
            StatusCode::BAD_REQUEST,
            format!(
                "Too many job IDs: {} (at most {} per request)",
                request.job_ids.len(),
                MAX_BULK_JOB_IDS
            ),
        ));
    }

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
            "retry" => retry_job_action(queue.as_ref(), job_uuid).await,
            _ => delete_job_action(queue.as_ref(), job_uuid).await,
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

    Ok(json_reply(&response))
}

/// Whether `job` matches the lowercase search `term` (id, queue, payload, error, trace ids).
fn job_matches_search(job: &hammerwork::Job, term: &str) -> bool {
    job.id.to_string().contains(term)
        || job.queue_name.to_lowercase().contains(term)
        || payload_search_text(&job.payload).contains(term)
        || [&job.error_message, &job.trace_id, &job.correlation_id]
            .into_iter()
            .flatten()
            .any(|text| text.to_lowercase().contains(term))
}

/// Handler for searching jobs
async fn search_jobs_handler<T>(
    queue: Arc<T>,
    search_request: JobSearchRequest,
    pagination: PaginationParams,
) -> Result<impl Reply, warp::Rejection>
where
    T: JobHistory,
{
    let search_term = search_request.query.to_lowercase();

    // The stored statuses to read: those requested, or every status when none is given.
    let statuses: Vec<&'static str> = match search_request.statuses {
        Some(ref requested) if !requested.is_empty() => {
            let mut stored: Vec<&'static str> = Vec::new();
            for name in requested {
                let name = name.to_lowercase().replace('_', "");
                for status in stored_statuses(&name).unwrap_or(&[]) {
                    if !stored.contains(status) {
                        stored.push(status);
                    }
                }
            }
            stored
        }
        _ => ALL_STATUSES.to_vec(),
    };
    // `None` reads across every queue.
    let target_queues: Vec<Option<&str>> = match search_request.queues {
        Some(ref names) => names.iter().map(|n| Some(n.as_str())).collect(),
        None => vec![None],
    };

    let mut matching_jobs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for queue_name in target_queues {
        let mut queue_jobs = Vec::new();
        for status in &statuses {
            queue_jobs.extend(try_api!(
                queue
                    .jobs_with_status(queue_name, status, SEARCH_LIMIT)
                    .await,
                "Failed to list jobs"
            ));
        }

        for job in &queue_jobs {
            if !seen.insert(job.id) || !job_matches_search(job, &search_term) {
                continue;
            }

            // Apply priority filter
            if let Some(ref priorities) = search_request.priorities {
                let job_priority = job.priority.to_string();
                if !priorities
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(&job_priority))
                {
                    continue;
                }
            }

            // Apply date filters
            if let Some(ref created_after) = search_request.created_after
                && job.created_at < *created_after
            {
                continue;
            }

            if let Some(ref created_before) = search_request.created_before
                && job.created_at > *created_before
            {
                continue;
            }

            matching_jobs.push(job_info(job));
        }
    }

    // Sort by created_at desc by default
    matching_jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));

    Ok(json_reply(&ApiResponse::success(paginate(
        matching_jobs,
        &pagination,
    ))))
}

/// Lowercased text of a job payload used for substring search.
///
/// `serde_json::Value` always serializes, so this does not lose data; it is
/// kept infallible so a (theoretical) failure cannot make a job unsearchable
/// silently: such a payload falls back to `Value`'s `Display` output.
fn payload_search_text(payload: &serde_json::Value) -> String {
    match serde_json::to_string(payload) {
        Ok(text) => text.to_lowercase(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to serialize job payload for search");
            payload.to_string().to_lowercase()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::test_support::{body_json, unreachable_queue};

    #[tokio::test]
    async fn test_list_jobs_returns_500_when_database_is_down() {
        let response = list_jobs_handler(
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
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("Failed to get queue stats")
        );
    }

    /// Calls `list_jobs_handler` with `query` (filter and sort parameters) and returns
    /// the ids of the listed jobs, in order.
    async fn listed_ids(
        queue: Arc<hammerwork::queue::test::TestQueue>,
        query: serde_json::Value,
    ) -> Vec<String> {
        let response = list_jobs_handler(
            queue,
            PaginationParams::default(),
            serde_json::from_value(query.clone()).unwrap(),
            serde_json::from_value(query).unwrap(),
        )
        .await
        .unwrap()
        .into_response();
        let (status, body) = body_json(response).await;
        assert_eq!(status, 200, "{body}");
        body["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect()
    }

    /// Two pending jobs in queue `q`, the second created a minute after the first.
    async fn two_jobs() -> (
        Arc<hammerwork::queue::test::TestQueue>,
        String,
        String,
        chrono::DateTime<chrono::Utc>,
    ) {
        use hammerwork::{Job, queue::test::TestQueue};
        let queue = Arc::new(TestQueue::new());
        let first = queue
            .enqueue(Job::new("q".into(), serde_json::json!({"name": "alpha"})))
            .await
            .unwrap();
        let between = queue.clock().now() + chrono::Duration::seconds(30);
        queue.clock().advance(chrono::Duration::minutes(1));
        let second = queue
            .enqueue(Job::new("q".into(), serde_json::json!({"name": "beta"})))
            .await
            .unwrap();
        (queue, first.to_string(), second.to_string(), between)
    }

    #[tokio::test]
    async fn test_list_jobs_applies_created_and_search_filters() {
        let (queue, first, second, between) = two_jobs().await;
        let ids = |query| listed_ids(queue.clone(), query);

        assert_eq!(
            ids(serde_json::json!({"queue": "q", "created_after": between})).await,
            vec![second.clone()]
        );
        assert_eq!(
            ids(serde_json::json!({"queue": "q", "created_before": between})).await,
            vec![first.clone()]
        );
        assert_eq!(
            ids(serde_json::json!({"queue": "q", "search": "ALPHA"})).await,
            vec![first]
        );
        assert!(
            ids(serde_json::json!({"queue": "q", "search": "no such job"}))
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_list_jobs_sort_order_applies_to_the_default_sort() {
        let (queue, first, second, _) = two_jobs().await;
        let ids = |query| listed_ids(queue.clone(), query);

        // Newest first by default
        assert_eq!(
            ids(serde_json::json!({"queue": "q"})).await,
            vec![second.clone(), first.clone()]
        );
        // `sort_order` without `sort_by` sorts by creation time in that order
        assert_eq!(
            ids(serde_json::json!({"queue": "q", "sort_order": "asc"})).await,
            vec![first.clone(), second.clone()]
        );
        // An unknown field falls back to creation time, in the requested order
        assert_eq!(
            ids(serde_json::json!({"queue": "q", "sort_by": "attempts", "sort_order": "ASC"}))
                .await,
            vec![first.clone(), second.clone()]
        );
        assert_eq!(
            ids(serde_json::json!({"queue": "q", "sort_by": "created_at", "sort_order": "desc"}))
                .await,
            vec![second, first]
        );
    }

    /// #103: the status filter reaches jobs in every status, not just the pending jobs
    /// ready to run and the dead ones.
    #[tokio::test]
    async fn test_list_jobs_filters_by_any_status() {
        use hammerwork::{Job, queue::test::TestQueue};
        let queue = Arc::new(TestQueue::new());
        let mk = |name: &str| Job::new("q".into(), serde_json::json!({ "name": name }));
        let ready = queue.enqueue(mk("ready")).await.unwrap().to_string();
        let later = queue
            .enqueue(Job::with_delay(
                "q".into(),
                serde_json::json!({"name": "later"}),
                chrono::Duration::hours(1),
            ))
            .await
            .unwrap()
            .to_string();
        let done = queue.enqueue(mk("done")).await.unwrap();
        let running = queue.enqueue(mk("running")).await.unwrap();
        let timed_out = queue.enqueue(mk("timed_out")).await.unwrap();
        let dead = queue.enqueue(mk("dead")).await.unwrap();
        let failed = queue.enqueue(mk("failed")).await.unwrap();
        // Claim the jobs that are due so each can end in the wanted status.
        let mut claimed = Vec::new();
        while let Some(job) = queue.dequeue_one("q").await.unwrap() {
            claimed.push(job.id);
        }
        for id in [done, running, timed_out, dead, failed] {
            assert!(claimed.contains(&id), "{id} was claimed");
        }
        // `ready` was claimed too; put it back to Pending.
        let ready_id: hammerwork::JobId = ready.parse().unwrap();
        queue
            .retry_job(ready_id, queue.clock().now())
            .await
            .unwrap();
        queue.complete_job(done).await.unwrap();
        queue.mark_job_timed_out(timed_out, "slow").await.unwrap();
        queue.mark_job_dead(dead, "boom").await.unwrap();
        queue.fail_job(failed, "bad").await.unwrap();

        let ids =
            |status: &'static str| listed_ids(queue.clone(), serde_json::json!({"status": status}));
        assert_eq!(ids("running").await, vec![running.to_string()]);
        assert_eq!(ids("completed").await, vec![done.to_string()]);
        assert_eq!(ids("dead").await, vec![dead.to_string()]);
        assert_eq!(ids("timedout").await, vec![timed_out.to_string()]);
        assert_eq!(ids("timed_out").await, vec![timed_out.to_string()]);
        let mut every_failure = ids("failed").await;
        every_failure.sort();
        let mut expected = vec![dead.to_string(), timed_out.to_string(), failed.to_string()];
        expected.sort();
        assert_eq!(every_failure, expected);
        // Pending includes the job scheduled for later, as well as the ready one.
        let mut pending = ids("pending").await;
        pending.sort();
        let mut expected = vec![ready, later];
        expected.sort();
        assert_eq!(pending, expected);
        assert!(ids("no_such_status").await.is_empty());
    }

    /// #114: search reaches jobs in every status, including failed and future ones.
    #[tokio::test]
    async fn test_search_reaches_every_status() {
        use hammerwork::{Job, queue::test::TestQueue};
        let queue = Arc::new(TestQueue::new());
        let now_job = queue
            .enqueue(Job::new("q".into(), serde_json::json!({"tag": "needle"})))
            .await
            .unwrap();
        let later = queue
            .enqueue(Job::with_delay(
                "q".into(),
                serde_json::json!({"tag": "needle"}),
                chrono::Duration::hours(1),
            ))
            .await
            .unwrap();
        let claimed = queue.dequeue_one("q").await.unwrap().unwrap();
        assert_eq!(claimed.id, now_job);
        queue.fail_job(now_job, "bad needle").await.unwrap();

        let search = |body: serde_json::Value| {
            let queue = queue.clone();
            async move {
                let response = search_jobs_handler(
                    queue,
                    serde_json::from_value(body).unwrap(),
                    PaginationParams::default(),
                )
                .await
                .unwrap()
                .into_response();
                let (status, body) = body_json(response).await;
                assert_eq!(status, 200, "{body}");
                let mut ids: Vec<String> = body["data"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|i| i["id"].as_str().unwrap().to_string())
                    .collect();
                ids.sort();
                ids
            }
        };
        let mut both = vec![now_job.to_string(), later.to_string()];
        both.sort();
        assert_eq!(search(serde_json::json!({"query": "needle"})).await, both);
        assert_eq!(
            search(serde_json::json!({"query": "needle", "statuses": ["failed"]})).await,
            vec![now_job.to_string()]
        );
        assert_eq!(
            search(
                serde_json::json!({"query": "needle", "statuses": ["Pending"], "queues": ["q"]})
            )
            .await,
            vec![later.to_string()]
        );
    }

    #[tokio::test]
    async fn test_invalid_job_id_is_400() {
        let response = get_job_handler("not-a-uuid".to_string(), unreachable_queue())
            .await
            .unwrap()
            .into_response();
        let (status, _) = body_json(response).await;
        assert_eq!(status, 400);
    }

    #[test]
    fn test_payload_search_text_lowercases_json() {
        let payload = serde_json::json!({"To": "User@Example.com"});
        let text = payload_search_text(&payload);
        assert!(text.contains("user@example.com"));
        assert!(text.contains("\"to\""));
    }

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

/// Examples that used to be doctests (moved in-crate when `hammerwork_web::__private` was removed).
#[cfg(test)]
mod doc_examples {

    #[test]
    fn example_1() {
        use crate::api::jobs::CreateJobRequest;
        use serde_json::json;

        let create_request = CreateJobRequest {
            queue_name: "email_queue".to_string(),
            payload: json!({
                "to": "user@example.com",
                "subject": "Welcome!",
                "template": "welcome_email"
            }),
            priority: Some("high".to_string()),
            scheduled_at: None,
            max_attempts: Some(3),
            cron_schedule: None,
            trace_id: Some("trace-123".to_string()),
            correlation_id: Some("corr-456".to_string()),
        };

        // This would be sent as JSON in a POST request to /api/jobs
        let json_payload = serde_json::to_string(&create_request).unwrap();
        assert!(json_payload.contains("email_queue"));
        assert!(json_payload.contains("high"));
    }

    #[test]
    fn example_2() {
        use crate::api::jobs::JobActionRequest;

        let retry_request = JobActionRequest {
            action: "retry".to_string(),
            reason: Some("Network issue resolved".to_string()),
        };

        let cancel_request = JobActionRequest {
            action: "cancel".to_string(),
            reason: Some("No longer needed".to_string()),
        };

        assert_eq!(retry_request.action, "retry");
        assert_eq!(cancel_request.action, "cancel");
    }

    #[test]
    fn example_3() {
        use crate::api::jobs::BulkJobActionRequest;

        let bulk_delete = BulkJobActionRequest {
            job_ids: vec![
                "550e8400-e29b-41d4-a716-446655440000".to_string(),
                "550e8400-e29b-41d4-a716-446655440001".to_string(),
            ],
            action: "delete".to_string(),
            reason: Some("Cleanup old failed jobs".to_string()),
        };

        assert_eq!(bulk_delete.job_ids.len(), 2);
        assert_eq!(bulk_delete.action, "delete");
    }
}
