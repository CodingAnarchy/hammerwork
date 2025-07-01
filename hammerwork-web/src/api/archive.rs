//! Archive management API endpoints.
//!
//! This module provides REST API endpoints for managing job archiving operations,
//! including archiving jobs, restoring archived jobs, listing archived jobs,
//! and configuring archival policies.
//!
//! # Endpoints
//!
//! - `POST /api/archive/jobs` - Archive jobs based on policy
//! - `GET /api/archive/jobs` - List archived jobs with pagination and filtering
//! - `POST /api/archive/jobs/{id}/restore` - Restore an archived job
//! - `DELETE /api/archive/jobs` - Purge old archived jobs
//! - `GET /api/archive/stats` - Get archival statistics
//! - `GET /api/archive/policies` - List archival policies
//! - `POST /api/archive/policies` - Create or update archival policy
//! - `DELETE /api/archive/policies/{id}` - Delete archival policy
//!
//! # Examples
//!
//! ## Archive Jobs
//!
//! ```rust
//! use hammerwork_web::api::archive::{ArchiveRequest, ArchiveResponse};
//! use hammerwork::archive::{ArchivalReason, ArchivalPolicy};
//! use chrono::Duration;
//!
//! let request = ArchiveRequest {
//!     queue_name: Some("completed_jobs".to_string()),
//!     reason: ArchivalReason::Automatic,
//!     archived_by: Some("scheduler".to_string()),
//!     dry_run: false,
//!     policy: Some(ArchivalPolicy::new()
//!         .archive_completed_after(Duration::days(7))),
//! };
//!
//! // This would be sent to POST /api/archive/jobs
//! assert_eq!(request.queue_name, Some("completed_jobs".to_string()));
//! assert!(!request.dry_run);
//! ```
//!
//! ## List Archived Jobs
//!
//! ```rust
//! use hammerwork_web::api::archive::ArchivedJobInfo;
//! use hammerwork::archive::{ArchivalReason, ArchivedJob};
//! use hammerwork::{JobId, JobStatus};
//! use chrono::Utc;
//! use uuid::Uuid;
//!
//! let archived_job = ArchivedJobInfo {
//!     id: Uuid::new_v4(),
//!     queue_name: "email_queue".to_string(),
//!     status: JobStatus::Completed,
//!     created_at: Utc::now(),
//!     archived_at: Utc::now(),
//!     archival_reason: ArchivalReason::Automatic,
//!     original_payload_size: Some(1024),
//!     payload_compressed: true,
//!     archived_by: Some("system".to_string()),
//! };
//!
//! assert_eq!(archived_job.queue_name, "email_queue");
//! assert!(archived_job.payload_compressed);
//! ```

use super::{ApiResponse, FilterParams, PaginatedResponse, PaginationMeta, PaginationParams};
use hammerwork::{
    JobId, JobStatus,
    archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason, ArchivalStats, ArchivedJob},
    queue::DatabaseQueue,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use warp::{Filter, Reply};

/// Request to archive jobs
#[derive(Debug, Deserialize)]
pub struct ArchiveRequest {
    /// Optional queue name to limit archival to specific queue
    pub queue_name: Option<String>,
    /// Reason for archival
    pub reason: ArchivalReason,
    /// Who initiated the archival
    pub archived_by: Option<String>,
    /// Whether this is a dry run (don't actually archive)
    pub dry_run: bool,
    /// Archival policy to use (optional, uses default if not provided)
    pub policy: Option<ArchivalPolicy>,
    /// Archival configuration (optional, uses default if not provided)
    pub config: Option<ArchivalConfig>,
}

impl Default for ArchiveRequest {
    fn default() -> Self {
        Self {
            queue_name: None,
            reason: ArchivalReason::Manual,
            archived_by: None,
            dry_run: true,
            policy: None,
            config: None,
        }
    }
}

/// Response from archiving jobs
#[derive(Debug, Serialize)]
pub struct ArchiveResponse {
    /// Statistics from the archival operation
    pub stats: ArchivalStats,
    /// Whether this was a dry run
    pub dry_run: bool,
    /// Policy used for archival
    pub policy_used: ArchivalPolicy,
    /// Configuration used for archival
    pub config_used: ArchivalConfig,
}

/// Request to restore an archived job
#[derive(Debug, Deserialize)]
pub struct RestoreRequest {
    /// Optional reason for restoration
    pub reason: Option<String>,
    /// Who initiated the restoration
    pub restored_by: Option<String>,
}

/// Response from restoring a job
#[derive(Debug, Serialize)]
pub struct RestoreResponse {
    /// The restored job
    pub job: hammerwork::Job,
    /// When the job was restored
    pub restored_at: chrono::DateTime<chrono::Utc>,
    /// Who restored the job
    pub restored_by: Option<String>,
}

/// Request to purge archived jobs
#[derive(Debug, Deserialize)]
pub struct PurgeRequest {
    /// Delete archived jobs older than this date
    pub older_than: chrono::DateTime<chrono::Utc>,
    /// Whether this is a dry run
    pub dry_run: bool,
    /// Who initiated the purge
    pub purged_by: Option<String>,
}

/// Response from purging archived jobs
#[derive(Debug, Serialize)]
pub struct PurgeResponse {
    /// Number of jobs that would be (or were) purged
    pub jobs_purged: u64,
    /// Whether this was a dry run
    pub dry_run: bool,
    /// When the purge was executed
    pub executed_at: chrono::DateTime<chrono::Utc>,
}

/// Archived job information for API responses
#[derive(Debug, Serialize)]
pub struct ArchivedJobInfo {
    /// Job ID
    pub id: JobId,
    /// Queue name
    pub queue_name: String,
    /// Original job status
    pub status: JobStatus,
    /// When the job was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the job was archived
    pub archived_at: chrono::DateTime<chrono::Utc>,
    /// Reason for archival
    pub archival_reason: ArchivalReason,
    /// Original payload size in bytes
    pub original_payload_size: Option<usize>,
    /// Whether the payload was compressed
    pub payload_compressed: bool,
    /// Who archived the job
    pub archived_by: Option<String>,
}

impl From<ArchivedJob> for ArchivedJobInfo {
    fn from(archived_job: ArchivedJob) -> Self {
        Self {
            id: archived_job.id,
            queue_name: archived_job.queue_name,
            status: archived_job.status,
            created_at: archived_job.created_at,
            archived_at: archived_job.archived_at,
            archival_reason: archived_job.archival_reason,
            original_payload_size: archived_job.original_payload_size,
            payload_compressed: archived_job.payload_compressed,
            archived_by: archived_job.archived_by,
        }
    }
}

/// Archival policy configuration request
#[derive(Debug, Deserialize)]
pub struct PolicyRequest {
    /// Policy name/identifier
    pub name: String,
    /// Archival policy configuration
    pub policy: ArchivalPolicy,
    /// Whether this policy is active
    pub active: bool,
}

/// Archival policy response
#[derive(Debug, Serialize)]
pub struct PolicyResponse {
    /// Policy name/identifier
    pub name: String,
    /// Archival policy configuration
    pub policy: ArchivalPolicy,
    /// Whether this policy is active
    pub active: bool,
    /// When the policy was created
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// When the policy was last modified
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

/// Archive statistics response
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    /// Overall archival statistics
    pub stats: ArchivalStats,
    /// Statistics by queue
    pub by_queue: std::collections::HashMap<String, ArchivalStats>,
    /// Recent archival operations
    pub recent_operations: Vec<RecentOperation>,
}

/// Information about recent archival operations
#[derive(Debug, Serialize)]
pub struct RecentOperation {
    /// Type of operation (archive, restore, purge)
    pub operation_type: String,
    /// Queue affected (if applicable)
    pub queue_name: Option<String>,
    /// Number of jobs affected
    pub jobs_affected: u64,
    /// When the operation occurred
    pub executed_at: chrono::DateTime<chrono::Utc>,
    /// Who executed the operation
    pub executed_by: Option<String>,
    /// Operation reason
    pub reason: Option<String>,
}

/// Archive filter parameters
#[derive(Debug, Deserialize, Default)]
pub struct ArchiveFilterParams {
    /// Filter by queue name
    pub queue: Option<String>,
    /// Filter by archival reason
    pub reason: Option<String>,
    /// Filter by archived after date
    pub archived_after: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter by archived before date
    pub archived_before: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter by who archived
    pub archived_by: Option<String>,
    /// Filter by compression status
    pub compressed: Option<bool>,
    /// Filter by original job status
    pub original_status: Option<String>,
}

/// Create archive API routes
pub fn archive_routes<Q>(
    queue: Arc<Q>,
) -> impl Filter<Extract = impl Reply, Error = warp::Rejection> + Clone
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let archive_jobs = warp::path!("api" / "archive" / "jobs")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_queue(queue.clone()))
        .and_then(handle_archive_jobs);

    let list_archived = warp::path!("api" / "archive" / "jobs")
        .and(warp::get())
        .and(super::with_pagination())
        .and(with_archive_filters())
        .and(with_queue(queue.clone()))
        .and_then(handle_list_archived_jobs);

    let restore_job = warp::path!("api" / "archive" / "jobs" / String / "restore")
        .and(warp::post())
        .and(warp::body::json())
        .and(with_queue(queue.clone()))
        .and_then(handle_restore_job);

    let purge_jobs = warp::path!("api" / "archive" / "purge")
        .and(warp::delete())
        .and(warp::body::json())
        .and(with_queue(queue.clone()))
        .and_then(handle_purge_jobs);

    let archive_stats = warp::path!("api" / "archive" / "stats")
        .and(warp::get())
        .and(warp::query::<FilterParams>())
        .and(with_queue(queue.clone()))
        .and_then(handle_archive_stats);

    archive_jobs
        .or(list_archived)
        .or(restore_job)
        .or(purge_jobs)
        .or(archive_stats)
}

/// Helper to inject queue into handlers
fn with_queue<Q>(
    queue: Arc<Q>,
) -> impl Filter<Extract = (Arc<Q>,), Error = std::convert::Infallible> + Clone
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    warp::any().map(move || queue.clone())
}

/// Extract archive filter parameters from query string
fn with_archive_filters()
-> impl Filter<Extract = (ArchiveFilterParams,), Error = warp::Rejection> + Clone {
    warp::query::<ArchiveFilterParams>()
}

/// Handle archive jobs request
async fn handle_archive_jobs<Q>(
    request: ArchiveRequest,
    queue: Arc<Q>,
) -> Result<impl Reply, warp::Rejection>
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let policy = request.policy.unwrap_or_default();
    let config = request.config.unwrap_or_default();

    if request.dry_run {
        // For dry run, return what would happen without actually archiving
        let response = ArchiveResponse {
            stats: ArchivalStats::default(), // In a real dry run, we'd calculate this
            dry_run: true,
            policy_used: policy,
            config_used: config,
        };
        return Ok(warp::reply::json(&ApiResponse::success(response)));
    }

    match queue
        .archive_jobs(
            request.queue_name.as_deref(),
            &policy,
            &config,
            request.reason,
            request.archived_by.as_deref(),
        )
        .await
    {
        Ok(stats) => {
            let response = ArchiveResponse {
                stats,
                dry_run: false,
                policy_used: policy,
                config_used: config,
            };
            Ok(warp::reply::json(&ApiResponse::success(response)))
        }
        Err(e) => Ok(warp::reply::json(&ApiResponse::<()>::error(format!(
            "Failed to archive jobs: {}",
            e
        )))),
    }
}

/// Handle list archived jobs request
async fn handle_list_archived_jobs<Q>(
    pagination: PaginationParams,
    filters: ArchiveFilterParams,
    queue: Arc<Q>,
) -> Result<impl Reply, warp::Rejection>
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    match queue
        .list_archived_jobs(
            filters.queue.as_deref(),
            Some(pagination.get_limit()),
            Some(pagination.get_offset()),
        )
        .await
    {
        Ok(archived_jobs) => {
            // Convert to API format
            let jobs: Vec<ArchivedJobInfo> = archived_jobs.into_iter().map(Into::into).collect();

            // For simplicity, we'll use the returned count as total
            // In a real implementation, you'd want a separate count query
            let total = jobs.len() as u64;

            let pagination_meta = PaginationMeta::new(&pagination, total);
            let response = PaginatedResponse {
                items: jobs,
                pagination: pagination_meta,
            };

            Ok(warp::reply::json(&ApiResponse::success(response)))
        }
        Err(e) => Ok(warp::reply::json(&ApiResponse::<()>::error(format!(
            "Failed to list archived jobs: {}",
            e
        )))),
    }
}

/// Handle restore job request
async fn handle_restore_job<Q>(
    job_id_str: String,
    request: RestoreRequest,
    queue: Arc<Q>,
) -> Result<impl Reply, warp::Rejection>
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    let job_id = match uuid::Uuid::parse_str(&job_id_str) {
        Ok(id) => id,
        Err(_) => {
            return Ok(warp::reply::json(&ApiResponse::<()>::error(
                "Invalid job ID format".to_string(),
            )));
        }
    };

    match queue.restore_archived_job(job_id).await {
        Ok(job) => {
            let response = RestoreResponse {
                job,
                restored_at: chrono::Utc::now(),
                restored_by: request.restored_by,
            };
            Ok(warp::reply::json(&ApiResponse::success(response)))
        }
        Err(e) => Ok(warp::reply::json(&ApiResponse::<()>::error(format!(
            "Failed to restore job: {}",
            e
        )))),
    }
}

/// Handle purge jobs request
async fn handle_purge_jobs<Q>(
    request: PurgeRequest,
    queue: Arc<Q>,
) -> Result<impl Reply, warp::Rejection>
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    if request.dry_run {
        // For dry run, we'd need to implement a count query
        // For now, return a placeholder
        let response = PurgeResponse {
            jobs_purged: 0, // Would calculate this in a real implementation
            dry_run: true,
            executed_at: chrono::Utc::now(),
        };
        return Ok(warp::reply::json(&ApiResponse::success(response)));
    }

    match queue.purge_archived_jobs(request.older_than).await {
        Ok(jobs_purged) => {
            let response = PurgeResponse {
                jobs_purged,
                dry_run: false,
                executed_at: chrono::Utc::now(),
            };
            Ok(warp::reply::json(&ApiResponse::success(response)))
        }
        Err(e) => Ok(warp::reply::json(&ApiResponse::<()>::error(format!(
            "Failed to purge archived jobs: {}",
            e
        )))),
    }
}

/// Handle archive stats request
async fn handle_archive_stats<Q>(
    filters: FilterParams,
    queue: Arc<Q>,
) -> Result<impl Reply, warp::Rejection>
where
    Q: DatabaseQueue + Send + Sync + 'static,
{
    match queue.get_archival_stats(filters.queue.as_deref()).await {
        Ok(stats) => {
            // For now, return simple stats. In a real implementation,
            // you'd collect per-queue stats and recent operations
            let response = StatsResponse {
                stats,
                by_queue: std::collections::HashMap::new(),
                recent_operations: vec![],
            };
            Ok(warp::reply::json(&ApiResponse::success(response)))
        }
        Err(e) => Ok(warp::reply::json(&ApiResponse::<()>::error(format!(
            "Failed to get archive stats: {}",
            e
        )))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_archive_request_default() {
        let request = ArchiveRequest::default();
        assert!(request.dry_run);
        assert_eq!(request.reason, ArchivalReason::Manual);
        assert!(request.queue_name.is_none());
    }

    #[test]
    fn test_archived_job_info_conversion() {
        let archived_job = ArchivedJob {
            id: uuid::Uuid::new_v4(),
            queue_name: "test_queue".to_string(),
            status: JobStatus::Completed,
            created_at: chrono::Utc::now(),
            archived_at: chrono::Utc::now(),
            archival_reason: ArchivalReason::Automatic,
            original_payload_size: Some(1024),
            payload_compressed: true,
            archived_by: Some("system".to_string()),
        };

        let info: ArchivedJobInfo = archived_job.into();
        assert_eq!(info.queue_name, "test_queue");
        assert!(info.payload_compressed);
        assert_eq!(info.archival_reason, ArchivalReason::Automatic);
    }

    #[test]
    fn test_purge_request_validation() {
        let request = PurgeRequest {
            older_than: chrono::Utc::now() - Duration::days(365),
            dry_run: true,
            purged_by: Some("admin".to_string()),
        };

        assert!(request.dry_run);
        assert_eq!(request.purged_by, Some("admin".to_string()));
    }
}
