//! REST API endpoints for the Hammerwork web dashboard.
//!
//! This module provides a comprehensive REST API for job queue management,
//! including endpoints for jobs, queues, statistics, and system information.
//! All API responses use a standardized format with proper error handling.
//!
//! # API Response Format
//!
//! All API endpoints return responses in a consistent format:
//!
//! ```rust
//! use hammerwork_web::__private::api::ApiResponse;
//! use serde_json::json;
//!
//! // Success response
//! let success_response = ApiResponse::success(json!({"count": 42}));
//! assert!(success_response.success);
//! assert!(success_response.data.is_some());
//! assert!(success_response.error.is_none());
//!
//! // Error response
//! let error_response: ApiResponse<()> = ApiResponse::error("Something went wrong".to_string());
//! assert!(!error_response.success);
//! assert!(error_response.data.is_none());
//! assert!(error_response.error.is_some());
//! ```
//!
//! # Pagination
//!
//! Many endpoints support pagination using query parameters:
//!
//! ```rust
//! use hammerwork_web::__private::api::{PaginationParams, PaginationMeta};
//!
//! let params = PaginationParams {
//!     page: Some(2),
//!     limit: Some(50),
//!     offset: None,
//! };
//!
//! assert_eq!(params.get_limit(), 50);
//! assert_eq!(params.get_offset(), 50); // (page-1) * limit
//!
//! let meta = PaginationMeta::new(&params, 200); // 200 total items
//! assert_eq!(meta.page, 2);
//! assert_eq!(meta.total_pages, 4);
//! assert!(meta.has_next);
//! assert!(meta.has_prev);
//! ```

/// Unwrap a `Result` inside a warp handler returning `Ok(reply)`; on error log it
/// and return a `500` JSON error response instead of swallowing the failure.
macro_rules! try_api {
    ($expr:expr, $context:expr) => {
        match $expr {
            Ok(value) => value,
            Err(e) => return Ok($crate::api::internal_error($context, &e)),
        }
    };
}

pub mod archive;
pub mod history;
pub mod jobs;
pub mod queues;
pub mod stats;
pub mod system;

use serde::{Deserialize, Serialize};
use warp::Reply;

/// Standard API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            timestamp: chrono::Utc::now(),
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
            timestamp: chrono::Utc::now(),
        }
    }
}

/// Serialize `body` as a `200 OK` JSON response.
pub fn json_reply<T: Serialize>(body: &T) -> warp::reply::Response {
    warp::reply::with_status(warp::reply::json(body), warp::http::StatusCode::OK).into_response()
}

/// Build a JSON [`ApiResponse`] error body with the given HTTP status.
pub fn error_reply(
    status: warp::http::StatusCode,
    message: impl Into<String>,
) -> warp::reply::Response {
    let body = ApiResponse::<()>::error(message.into());
    warp::reply::with_status(warp::reply::json(&body), status).into_response()
}

/// Log a failed backend operation and return it as a `500 Internal Server Error`
/// JSON response (`"{context}: {err}"`).
///
/// Handlers use this instead of swallowing the error, so API callers can tell
/// an outage apart from "no data".
pub fn internal_error(context: &str, err: &dyn std::fmt::Display) -> warp::reply::Response {
    tracing::error!(error = %err, "{}", context);
    error_reply(
        warp::http::StatusCode::INTERNAL_SERVER_ERROR,
        format!("{}: {}", context, err),
    )
}

/// Turn a failed queue operation into a JSON error reply: a missing job is a `404`, a
/// transition the job's status does not allow a `409`, anything else a logged `500`.
pub fn queue_error_reply(
    context: &str,
    err: &hammerwork::HammerworkError,
) -> warp::reply::Response {
    use hammerwork::HammerworkError;
    match err {
        HammerworkError::JobNotFound { .. } => error_reply(
            warp::http::StatusCode::NOT_FOUND,
            format!("{}: {}", context, err),
        ),
        HammerworkError::InvalidJobTransition { .. } => error_reply(
            warp::http::StatusCode::CONFLICT,
            format!("{}: {}", context, err),
        ),
        _ => internal_error(context, err),
    }
}

/// Pagination parameters
#[derive(Debug, Deserialize)]
pub struct PaginationParams {
    pub page: Option<u32>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl Default for PaginationParams {
    fn default() -> Self {
        Self {
            page: Some(1),
            limit: Some(50),
            offset: None,
        }
    }
}

/// The page size when a request does not give one.
pub const DEFAULT_PAGE_SIZE: u32 = 50;

/// The largest page size a request may ask for; larger limits are clamped to it.
pub const MAX_PAGE_SIZE: u32 = 1000;

impl PaginationParams {
    /// The offset of the first item: `offset` if given, else `(page - 1) * limit` computed
    /// with the clamped [`get_limit`](Self::get_limit), saturating instead of overflowing.
    pub fn get_offset(&self) -> u32 {
        if let Some(offset) = self.offset {
            offset
        } else {
            let page = self.page.unwrap_or(1);
            page.saturating_sub(1).saturating_mul(self.get_limit())
        }
    }

    /// The page size: `limit`, clamped to `1..=`[`MAX_PAGE_SIZE`].
    pub fn get_limit(&self) -> u32 {
        self.limit
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE)
    }
}

/// The jobs of a queue that failed: those in status `Failed` (a failed dependency, a
/// fail-fast batch or workflow, `fail_job`), `Dead` (out of retries) and `TimedOut`.
///
/// Every "failed" figure of the dashboard (REST and WebSocket) is this count;
/// `dead_count` reports the `Dead` jobs separately.
pub(crate) fn failed_job_count(stats: &hammerwork::stats::QueueStats) -> u64 {
    stats
        .statistics
        .failed
        .saturating_add(stats.dead_count)
        .saturating_add(stats.timed_out_count)
}

/// Pagination metadata for responses
#[derive(Debug, Serialize)]
pub struct PaginationMeta {
    pub page: u32,
    pub limit: u32,
    pub offset: u32,
    pub total: u64,
    pub total_pages: u32,
    pub has_next: bool,
    pub has_prev: bool,
}

impl PaginationMeta {
    pub fn new(params: &PaginationParams, total: u64) -> Self {
        let limit = params.get_limit();
        let offset = params.get_offset();
        // An explicit offset decides which items were served, whatever `page` says, so
        // the page number and the neighbours are derived from it.
        let page = match params.offset {
            Some(offset) => (offset / limit).saturating_add(1),
            None => params.page.unwrap_or(1),
        };
        let total_pages = u32::try_from(total.div_ceil(u64::from(limit))).unwrap_or(u32::MAX);

        Self {
            page,
            limit,
            offset,
            total,
            total_pages,
            has_next: u64::from(offset) + u64::from(limit) < total,
            has_prev: offset > 0,
        }
    }
}

/// Paginated response wrapper
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub pagination: PaginationMeta,
}

/// Query filter parameters
#[derive(Debug, Deserialize, Default)]
pub struct FilterParams {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub queue: Option<String>,
    pub created_after: Option<chrono::DateTime<chrono::Utc>>,
    pub created_before: Option<chrono::DateTime<chrono::Utc>>,
    pub search: Option<String>,
}

/// Sort parameters
#[derive(Debug, Deserialize, Default)]
pub struct SortParams {
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
}

impl SortParams {
    /// Whether `sort_order` asks for ascending order (`asc`, in any case); anything else
    /// is descending.
    pub fn is_ascending(&self) -> bool {
        self.sort_order
            .as_deref()
            .is_some_and(|order| order.trim().eq_ignore_ascii_case("asc"))
    }

    pub fn get_order_by(&self) -> (String, String) {
        let field = self.sort_by.as_deref().unwrap_or("created_at").to_string();
        let direction = if self.is_ascending() { "ASC" } else { "DESC" }.to_string();
        (field, direction)
    }
}

/// Common error handling for API endpoints
pub async fn handle_api_error(
    err: warp::Rejection,
) -> Result<impl warp::Reply, std::convert::Infallible> {
    let response = if err.is_not_found() {
        ApiResponse::<()>::error("Resource not found".to_string())
    } else if err
        .find::<warp::filters::body::BodyDeserializeError>()
        .is_some()
    {
        ApiResponse::<()>::error("Invalid request body".to_string())
    } else if err.find::<warp::reject::InvalidQuery>().is_some() {
        ApiResponse::<()>::error("Invalid query parameters".to_string())
    } else {
        ApiResponse::<()>::error("Internal server error".to_string())
    };

    let status = if err.is_not_found() {
        warp::http::StatusCode::NOT_FOUND
    } else if err
        .find::<warp::filters::body::BodyDeserializeError>()
        .is_some()
        || err.find::<warp::reject::InvalidQuery>().is_some()
    {
        warp::http::StatusCode::BAD_REQUEST
    } else {
        warp::http::StatusCode::INTERNAL_SERVER_ERROR
    };

    Ok(warp::reply::with_status(
        warp::reply::json(&response),
        status,
    ))
}

/// Hand a clone of shared state (a queue handle, system state, ...) to a handler.
pub fn with_shared<T>(
    value: T,
) -> impl warp::Filter<Extract = (T,), Error = std::convert::Infallible> + Clone
where
    T: Clone + Send,
{
    use warp::Filter;
    warp::any().map(move || value.clone())
}

/// Extract pagination parameters from query string
pub fn with_pagination()
-> impl warp::Filter<Extract = (PaginationParams,), Error = warp::Rejection> + Clone {
    warp::query::<PaginationParams>()
}

/// Extract filter parameters from query string  
pub fn with_filters()
-> impl warp::Filter<Extract = (FilterParams,), Error = warp::Rejection> + Clone {
    warp::query::<FilterParams>()
}

/// Extract sort parameters from query string
pub fn with_sort() -> impl warp::Filter<Extract = (SortParams,), Error = warp::Rejection> + Clone {
    warp::query::<SortParams>()
}

/// Test helpers shared by the handler tests of the API submodules.
#[cfg(test)]
pub(crate) mod test_support {
    use hammerwork::JobQueue;
    use std::sync::Arc;

    /// A queue whose database is unreachable, so every query fails quickly.
    pub fn unreachable_queue() -> Arc<JobQueue<sqlx::Postgres>> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_millis(200))
            .connect_lazy("postgres://nobody:nothing@127.0.0.1:1/none")
            .expect("lazy pool");
        Arc::new(JobQueue::new(pool))
    }

    /// The mock queue as a `JobHistory`, for handler tests that need no database. The
    /// aggregate queries (activity, error groups, finished-job counts, archive listing,
    /// table maintenance) report nothing; the per-job queries read the mock's jobs.
    impl super::history::JobHistory for hammerwork::queue::test::TestQueue {
        async fn hourly_activity(
            &self,
            _queue: Option<&str>,
            _start: chrono::DateTime<chrono::Utc>,
            _end: chrono::DateTime<chrono::Utc>,
        ) -> hammerwork::Result<Vec<super::history::HourBucket>> {
            Ok(Vec::new())
        }

        async fn error_groups(
            &self,
            _since: chrono::DateTime<chrono::Utc>,
            _limit: u32,
        ) -> hammerwork::Result<Vec<super::history::ErrorGroup>> {
            Ok(Vec::new())
        }

        async fn count_jobs(
            &self,
            _queue: Option<&str>,
            _kind: super::history::FinishedKind,
            _older_than: Option<chrono::DateTime<chrono::Utc>>,
        ) -> hammerwork::Result<u64> {
            Ok(0)
        }

        async fn delete_jobs(
            &self,
            _queue: Option<&str>,
            _kind: super::history::FinishedKind,
            _older_than: Option<chrono::DateTime<chrono::Utc>>,
        ) -> hammerwork::Result<u64> {
            Ok(0)
        }

        async fn archived_jobs(
            &self,
            _filter: &super::archive::ArchiveFilterParams,
            _limit: u32,
            _offset: u32,
        ) -> hammerwork::Result<(Vec<hammerwork::archive::ArchivedJob>, u64)> {
            Ok((Vec::new(), 0))
        }

        async fn database_now(&self) -> hammerwork::Result<chrono::DateTime<chrono::Utc>> {
            Ok(self.clock().now())
        }

        async fn recent_job_changes(
            &self,
            _since: chrono::DateTime<chrono::Utc>,
            _limit: u32,
        ) -> hammerwork::Result<Vec<crate::websocket::JobUpdate>> {
            Ok(Vec::new())
        }

        async fn table_maintenance(
            &self,
            _operation: super::history::TableMaintenance,
            _tables: &[&'static str],
            _dry_run: bool,
        ) -> hammerwork::Result<Vec<super::history::MaintenanceStatement>> {
            Ok(Vec::new())
        }

        async fn count_archivable(
            &self,
            queue: Option<&str>,
            policy: &hammerwork::archive::ArchivalPolicy,
        ) -> hammerwork::Result<u64> {
            if !policy.enabled {
                return Ok(0);
            }
            let now = self.clock().now();
            let archivable = self
                .get_all_jobs()
                .await
                .into_iter()
                .filter(|job| queue.is_none_or(|name| job.queue_name == name))
                .filter(|job| {
                    super::history::archivable_statuses(policy).into_iter().any(
                        |(after, status, column)| {
                            let finished_at = match column {
                                "completed_at" => job.completed_at,
                                "timed_out_at" => job.timed_out_at,
                                _ => job.failed_at,
                            };
                            match (after, finished_at) {
                                (Some(after), Some(at)) => {
                                    job.status.as_str() == status && at <= now - after
                                }
                                _ => false,
                            }
                        },
                    )
                })
                .count();
            Ok((archivable as u64).min(policy.batch_size as u64))
        }

        async fn running_jobs(
            &self,
            limit: u32,
        ) -> hammerwork::Result<Vec<crate::websocket::JobUpdate>> {
            let mut running: Vec<_> = self
                .get_all_jobs()
                .await
                .into_iter()
                .filter(|job| job.status == hammerwork::JobStatus::Running)
                .filter_map(|job| job.started_at.map(|started_at| (started_at, job)))
                .collect();
            running.sort_by_key(|(started_at, _)| std::cmp::Reverse(*started_at));
            running.truncate(limit as usize);
            Ok(running
                .into_iter()
                .map(|(started_at, job)| {
                    super::history::job_change(
                        job.id.to_string(),
                        job.queue_name,
                        job.status.as_str(),
                        job.priority.as_i32(),
                        job.attempts,
                        started_at,
                    )
                })
                .collect())
        }

        async fn job_states(
            &self,
            ids: &[String],
        ) -> hammerwork::Result<Vec<crate::websocket::JobUpdate>> {
            use hammerwork::queue::DatabaseQueue;
            let mut states = Vec::with_capacity(ids.len());
            for id in ids.iter().filter_map(|id| id.parse::<uuid::Uuid>().ok()) {
                if let Some(job) = self.get_job(id).await? {
                    let changed_at = [
                        job.started_at,
                        job.completed_at,
                        job.failed_at,
                        job.timed_out_at,
                    ]
                    .into_iter()
                    .flatten()
                    .fold(job.created_at, chrono::DateTime::max);
                    states.push(super::history::job_change(
                        id.to_string(),
                        job.queue_name,
                        job.status.as_str(),
                        job.priority.as_i32(),
                        job.attempts,
                        changed_at,
                    ));
                }
            }
            Ok(states)
        }

        async fn jobs_with_status(
            &self,
            queue: Option<&str>,
            status: &str,
            limit: u32,
        ) -> hammerwork::Result<Vec<hammerwork::Job>> {
            let mut jobs: Vec<_> = self
                .get_all_jobs()
                .await
                .into_iter()
                .filter(|job| job.status.as_str() == status)
                .filter(|job| queue.is_none_or(|name| job.queue_name == name))
                .collect();
            jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
            jobs.truncate(limit as usize);
            Ok(jobs)
        }
    }

    /// Status code and JSON body of a handler reply.
    pub async fn body_json(response: warp::reply::Response) -> (u16, serde_json::Value) {
        use warp::Filter;
        // `warp::test` needs a filter; hand the response out once via a shared slot.
        let slot = Arc::new(std::sync::Mutex::new(Some(response)));
        let filter = warp::any().map(move || slot.lock().unwrap().take().unwrap());
        let reply = warp::test::request().reply(&filter).await;
        (
            reply.status().as_u16(),
            serde_json::from_slice(reply.body()).unwrap(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::body_json;
    use super::*;

    /// The mock queue implements every `JobHistory` method itself (there are no
    /// default bodies any more): running jobs, job states and archivable counts come
    /// from its jobs.
    #[tokio::test]
    async fn test_queue_job_history_reads_its_jobs() {
        use super::history::JobHistory;
        use hammerwork::queue::{DatabaseQueue, test::TestQueue};

        let queue = TestQueue::new();
        let running = queue
            .enqueue(hammerwork::Job::new("h".into(), serde_json::json!({})))
            .await
            .unwrap();
        let done = queue
            .enqueue(hammerwork::Job::new("h".into(), serde_json::json!({})))
            .await
            .unwrap();
        queue.dequeue_one("h").await.unwrap().unwrap();
        queue.dequeue_one("h").await.unwrap().unwrap();
        queue.complete_job(done).await.unwrap();

        let jobs = queue.running_jobs(10).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, running.to_string());
        assert_eq!(jobs[0].status, "Running");

        let states = queue
            .job_states(&[running.to_string(), done.to_string(), "not-a-uuid".into()])
            .await
            .unwrap();
        let statuses: Vec<_> = states.iter().map(|s| s.status.as_str()).collect();
        assert_eq!(statuses, ["Running", "Completed"]);

        let policy = hammerwork::archive::ArchivalPolicy::new()
            .archive_completed_after(chrono::Duration::zero())
            .enabled(true);
        assert_eq!(queue.count_archivable(Some("h"), &policy).await.unwrap(), 1);
        assert_eq!(
            queue.count_archivable(Some("other"), &policy).await.unwrap(),
            0
        );
        assert_eq!(
            queue
                .count_archivable(None, &policy.clone().enabled(false))
                .await
                .unwrap(),
            0
        );
    }

    #[test]
    fn test_api_response_success() {
        let response = ApiResponse::success("test data");
        assert!(response.success);
        assert_eq!(response.data, Some("test data"));
        assert!(response.error.is_none());
    }

    #[test]
    fn test_api_response_error() {
        let response: ApiResponse<()> = ApiResponse::error("Something went wrong".to_string());
        assert!(!response.success);
        assert!(response.data.is_none());
        assert_eq!(response.error, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_pagination_params_defaults() {
        let params = PaginationParams::default();
        assert_eq!(params.get_limit(), 50);
        assert_eq!(params.get_offset(), 0);
    }

    #[test]
    fn test_pagination_params_calculation() {
        let params = PaginationParams {
            page: Some(3),
            limit: Some(20),
            offset: None,
        };
        assert_eq!(params.get_limit(), 20);
        assert_eq!(params.get_offset(), 40); // (3-1) * 20
    }

    #[test]
    fn the_offset_uses_the_clamped_limit_and_never_overflows() {
        // M13: limit=5000&page=2 serves at most 1000 rows, so page 2 starts at row 1000.
        let params = PaginationParams {
            page: Some(2),
            limit: Some(5000),
            offset: None,
        };
        assert_eq!(params.get_limit(), MAX_PAGE_SIZE);
        assert_eq!(params.get_offset(), MAX_PAGE_SIZE);

        let huge = PaginationParams {
            page: Some(u32::MAX),
            limit: Some(u32::MAX),
            offset: None,
        };
        assert_eq!(
            huge.get_offset(),
            u32::MAX,
            "saturates instead of overflowing"
        );

        let zero = PaginationParams {
            page: Some(0),
            limit: Some(0),
            offset: None,
        };
        assert_eq!(zero.get_limit(), 1, "a zero limit is clamped to one");
        assert_eq!(zero.get_offset(), 0);
        let meta = PaginationMeta::new(&zero, 3);
        assert_eq!(meta.total_pages, 3);

        let explicit = PaginationParams {
            page: Some(7),
            limit: Some(10),
            offset: Some(5),
        };
        assert_eq!(explicit.get_offset(), 5, "an explicit offset wins");
    }

    #[test]
    fn test_pagination_meta() {
        let params = PaginationParams {
            page: Some(2),
            limit: Some(10),
            offset: None,
        };
        let meta = PaginationMeta::new(&params, 45);

        assert_eq!(meta.page, 2);
        assert_eq!(meta.limit, 10);
        assert_eq!(meta.total, 45);
        assert_eq!(meta.total_pages, 5);
        assert!(meta.has_next);
        assert!(meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_describes_an_explicit_offset() {
        let last_page = PaginationParams {
            page: None,
            limit: Some(50),
            offset: Some(100),
        };
        let meta = PaginationMeta::new(&last_page, 120);
        assert_eq!(meta.page, 3);
        assert_eq!(meta.offset, 100);
        assert!(!meta.has_next);
        assert!(meta.has_prev);

        // The offset wins over a page that disagrees with it
        let first_items = PaginationParams {
            page: Some(3),
            limit: Some(50),
            offset: Some(0),
        };
        let meta = PaginationMeta::new(&first_items, 120);
        assert_eq!(meta.page, 1);
        assert!(meta.has_next);
        assert!(!meta.has_prev);
    }

    #[test]
    fn test_sort_params_defaults() {
        let params = SortParams {
            sort_by: None,
            sort_order: None,
        };
        let (field, direction) = params.get_order_by();
        assert_eq!(field, "created_at");
        assert_eq!(direction, "DESC");
    }

    /// #94: `asc` selects ascending order in any case (the jobs list only accepted
    /// lowercase); everything else is descending.
    #[test]
    fn sort_order_is_case_insensitive() {
        let order = |order: Option<&str>| SortParams {
            sort_by: Some("created_at".to_string()),
            sort_order: order.map(str::to_string),
        };
        for ascending in ["asc", "ASC", "Asc", " asc "] {
            assert!(order(Some(ascending)).is_ascending(), "{ascending}");
            assert_eq!(order(Some(ascending)).get_order_by().1, "ASC");
        }
        for descending in [None, Some("desc"), Some("DESC"), Some("ascending")] {
            assert!(!order(descending).is_ascending(), "{descending:?}");
            assert_eq!(order(descending).get_order_by().1, "DESC");
        }
    }

    #[test]
    fn test_sort_params_custom() {
        let params = SortParams {
            sort_by: Some("name".to_string()),
            sort_order: Some("asc".to_string()),
        };
        let (field, direction) = params.get_order_by();
        assert_eq!(field, "name");
        assert_eq!(direction, "ASC");
    }

    #[tokio::test]
    async fn test_internal_error_is_500_with_json_error_body() {
        let (status, body) = body_json(internal_error("Failed to list jobs", &"db down")).await;
        assert_eq!(status, 500);
        assert_eq!(body["success"], false);
        assert_eq!(body["error"], "Failed to list jobs: db down");
        assert!(body["data"].is_null());
    }

    #[tokio::test]
    async fn test_error_reply_uses_given_status() {
        let (status, body) =
            body_json(error_reply(warp::http::StatusCode::BAD_REQUEST, "nope")).await;
        assert_eq!(status, 400);
        assert_eq!(body["error"], "nope");
    }

    #[tokio::test]
    async fn test_json_reply_is_200() {
        let (status, body) = body_json(json_reply(&ApiResponse::success(5))).await;
        assert_eq!(status, 200);
        assert_eq!(body["data"], 5);
    }
}

/// #71: the listing endpoints skip rows they cannot decode instead of failing the whole
/// page with a 500; reading the corrupt job itself still fails. The corrupt rows are
/// written with raw SQL and removed at the end.
#[cfg(test)]
mod undecodable_row_tests {
    use super::history::JobHistory;
    use hammerwork::archive::{ArchivalConfig, ArchivalPolicy, ArchivalReason};
    use hammerwork::{Job, JobId};
    use serde_json::{Value, json};
    use std::future::Future;
    use std::sync::Arc;
    use warp::Filter;

    /// The ids listed in a paginated response, sorted.
    fn listed(body: &Value) -> Vec<String> {
        let mut ids: Vec<String> = body["data"]["items"]
            .as_array()
            .unwrap_or_else(|| panic!("no items: {body}"))
            .iter()
            .map(|item| item["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    }

    /// `sql` runs one statement. `corrupt_archive_id` makes the archived row undecodable
    /// by giving it an id that is not a UUID (possible on MySQL, where the id is text).
    async fn listings_skip_undecodable_rows<Q, S, F>(
        queue: Arc<Q>,
        sql: S,
        corrupt_archive_id: bool,
    ) where
        Q: JobHistory + 'static,
        S: Fn(String) -> F,
        F: Future<Output = ()>,
    {
        let routes = super::jobs::routes(queue.clone())
            .or(super::queues::routes(queue.clone()))
            .or(super::archive::archive_routes(queue.clone()));
        let get = |path: String| {
            let routes = routes.clone();
            async move {
                let response = warp::test::request().path(&path).reply(&routes).await;
                let body: Value = serde_json::from_slice(response.body()).unwrap();
                (response.status().as_u16(), body)
            }
        };

        let tag = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("undecodable_{tag}");
        let archive_name = format!("undecodable_archive_{tag}");
        let enqueue = |queue_name: &str| queue.enqueue(Job::new(queue_name.to_string(), json!({})));
        let break_timeout = |id: JobId| {
            sql(format!(
                "UPDATE hammerwork_jobs SET timeout_seconds = -1 WHERE id = '{id}'"
            ))
        };

        let ready = enqueue(&name).await.unwrap();
        let bad_ready = enqueue(&name).await.unwrap();
        break_timeout(bad_ready).await;
        let dead = enqueue(&name).await.unwrap();
        let bad_dead = enqueue(&name).await.unwrap();
        for id in [dead, bad_dead] {
            queue.mark_job_dead(id, "gone").await.unwrap();
        }
        break_timeout(bad_dead).await;
        let mut healthy = vec![ready.to_string(), dead.to_string()];
        healthy.sort();

        // Job list, queue jobs and search: the healthy jobs, without a 500.
        for path in [
            format!("/jobs?queue={name}&limit=100"),
            format!("/queues/{name}/jobs?limit=100"),
        ] {
            let (status, body) = get(path.clone()).await;
            assert_eq!(status, 200, "{path}: {body}");
            assert_eq!(listed(&body), healthy, "{path}");
        }
        let response = warp::test::request()
            .method("POST")
            .path("/jobs/search?limit=100")
            .json(&json!({"query": "", "queues": [name]}))
            .reply(&routes)
            .await;
        let body: Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(response.status(), 200, "{body}");
        assert_eq!(listed(&body), healthy, "search");

        // The corrupt job itself still reports the error.
        let (status, body) = get(format!("/jobs/{bad_ready}")).await;
        assert_eq!(status, 500, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("timeout_seconds"),
            "{body}"
        );

        // Archived jobs
        let archived = enqueue(&archive_name).await.unwrap();
        let bad_archived = enqueue(&archive_name).await.unwrap();
        for id in [archived, bad_archived] {
            queue.mark_job_dead(id, "gone").await.unwrap();
        }
        let policy = ArchivalPolicy::new()
            .archive_dead_after(chrono::Duration::seconds(0))
            .enabled(true);
        queue
            .archive_jobs(
                Some(&archive_name),
                &policy,
                &ArchivalConfig::new(),
                ArchivalReason::Manual,
                None,
            )
            .await
            .unwrap();
        if corrupt_archive_id {
            sql(format!(
                "UPDATE hammerwork_jobs_archive SET id = 'not-a-uuid' WHERE id = '{bad_archived}'"
            ))
            .await;
        }
        let (status, body) = get(format!("/archive/jobs?queue={archive_name}")).await;
        assert_eq!(status, 200, "{body}");
        let ids = listed(&body);
        assert!(ids.contains(&archived.to_string()), "{body}");
        if corrupt_archive_id {
            assert_eq!(ids, [archived.to_string()], "{body}");
        }

        for (table, queue_name) in [
            ("hammerwork_jobs", &name),
            ("hammerwork_jobs_archive", &archive_name),
        ] {
            sql(format!(
                "DELETE FROM {table} WHERE queue_name = '{queue_name}'"
            ))
            .await;
        }
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL (PostgreSQL)"]
    async fn postgres_listings_skip_undecodable_rows() {
        let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let queue = Arc::new(hammerwork::JobQueue::new(pool.clone()));
        let sql = |statement: String| {
            let pool = pool.clone();
            async move {
                sqlx::query(sqlx::AssertSqlSafe(statement))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        listings_skip_undecodable_rows(queue, sql, false).await;
    }

    #[tokio::test]
    #[ignore = "requires MYSQL_DATABASE_URL"]
    async fn mysql_listings_skip_undecodable_rows() {
        let url = std::env::var("MYSQL_DATABASE_URL").expect("MYSQL_DATABASE_URL");
        let pool = sqlx::MySqlPool::connect(&url).await.unwrap();
        let queue = Arc::new(hammerwork::JobQueue::new(pool.clone()));
        let sql = |statement: String| {
            let pool = pool.clone();
            async move {
                sqlx::query(sqlx::AssertSqlSafe(statement))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        };
        listings_skip_undecodable_rows(queue, sql, true).await;
    }
}
