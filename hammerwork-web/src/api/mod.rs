//! REST API endpoints for the Hammerwork web dashboard.

pub mod jobs;
pub mod queues;
pub mod stats;
pub mod system;

use serde::{Deserialize, Serialize};

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
pub async fn handle_api_error(err: warp::Rejection) -> Result<impl warp::Reply, std::convert::Infallible> {
    let response = if err.is_not_found() {
        ApiResponse::<()>::error("Resource not found".to_string())
    } else if let Some(_) = err.find::<warp::filters::body::BodyDeserializeError>() {
        ApiResponse::<()>::error("Invalid request body".to_string())
    } else if let Some(_) = err.find::<warp::reject::InvalidQuery>() {
        ApiResponse::<()>::error("Invalid query parameters".to_string())
    } else {
        ApiResponse::<()>::error("Internal server error".to_string())
    };

    let status = if err.is_not_found() {
        warp::http::StatusCode::NOT_FOUND
    } else if err.find::<warp::filters::body::BodyDeserializeError>().is_some() 
        || err.find::<warp::reject::InvalidQuery>().is_some() {
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
pub fn with_pagination() -> impl warp::Filter<Extract = (PaginationParams,), Error = warp::Rejection> + Clone {
    warp::query::<PaginationParams>()
}

/// Extract filter parameters from query string  
pub fn with_filters() -> impl warp::Filter<Extract = (FilterParams,), Error = warp::Rejection> + Clone {
    warp::query::<FilterParams>()
}

/// Extract sort parameters from query string
pub fn with_sort() -> impl warp::Filter<Extract = (SortParams,), Error = warp::Rejection> + Clone {
    warp::query::<SortParams>()
}

#[cfg(test)]
mod tests {
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
}