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
//! use hammerwork_web::api::ApiResponse;
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
//! use hammerwork_web::api::{PaginationParams, PaginationMeta};
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

impl PaginationParams {
    pub fn get_offset(&self) -> u32 {
        if let Some(offset) = self.offset {
            offset
        } else {
            let page = self.page.unwrap_or(1);
            let limit = self.limit.unwrap_or(50);
            (page.saturating_sub(1)) * limit
        }
    }

    pub fn get_limit(&self) -> u32 {
        self.limit.unwrap_or(50).min(1000) // Cap at 1000 items
    }
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
        let page = params.page.unwrap_or(1);
        let total_pages = ((total as f64) / (limit as f64)).ceil() as u32;

        Self {
            page,
            limit,
            offset,
            total,
            total_pages,
            has_next: page < total_pages,
            has_prev: page > 1,
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
    pub fn get_order_by(&self) -> (String, String) {
        let field = self.sort_by.as_deref().unwrap_or("created_at").to_string();
        let direction = match self.sort_order.as_deref() {
            Some("asc") | Some("ASC") => "ASC".to_string(),
            _ => "DESC".to_string(),
        };
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
    fn test_sort_params_defaults() {
        let params = SortParams {
            sort_by: None,
            sort_order: None,
        };
        let (field, direction) = params.get_order_by();
        assert_eq!(field, "created_at");
        assert_eq!(direction, "DESC");
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
