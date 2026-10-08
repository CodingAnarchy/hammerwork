//! Authentication middleware for the web dashboard.
//!
//! This module provides authentication functionality including basic auth verification,
//! rate limiting for failed attempts, and account lockout mechanisms.
//!
//! # Examples
//!
//! ## Basic Authentication Setup
//!
//! ```rust
//! use hammerwork_web::auth::{AuthState, extract_basic_auth};
//! use hammerwork_web::config::AuthConfig;
//!
//! let auth_config = AuthConfig {
//!     enabled: true,
//!     username: "admin".to_string(),
//!     password_hash: "plain_password".to_string(), // Use bcrypt in production
//!     ..Default::default()
//! };
//!
//! let auth_state = AuthState::new(auth_config);
//! assert!(auth_state.is_enabled());
//! ```
//!
//! ## Extracting Basic Auth Credentials
//!
//! ```rust
//! use hammerwork_web::auth::extract_basic_auth;
//!
//! // "admin:password" in base64 is "YWRtaW46cGFzc3dvcmQ="
//! let auth_header = "Basic YWRtaW46cGFzc3dvcmQ=";
//! let (username, password) = extract_basic_auth(auth_header).unwrap();
//!
//! assert_eq!(username, "admin");
//! assert_eq!(password, "password");
//!
//! // Invalid format returns None
//! let result = extract_basic_auth("Bearer token123");
//! assert!(result.is_none());
//! ```

use crate::config::AuthConfig;
use base64::Engine;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use warp::{Filter, Rejection, Reply};

/// Authentication middleware state
#[derive(Clone)]
pub struct AuthState {
    config: AuthConfig,
    failed_attempts: Arc<RwLock<HashMap<String, (u32, std::time::Instant)>>>,
}

impl AuthState {
    pub fn new(config: AuthConfig) -> Self {
        Self {
            config,
            failed_attempts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Check if authentication is enabled
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Verify credentials
    pub async fn verify_credentials(&self, username: &str, password: &str) -> bool {
        if !self.config.enabled {
            return true; // No auth required
        }

        // Check if user is locked out
        if self.is_locked_out(username).await {
            return false;
        }

        let valid = username == self.config.username && self.verify_password(password);

        if !valid {
            self.record_failed_attempt(username).await;
        } else {
            self.clear_failed_attempts(username).await;
        }

        valid
    }

    /// Verify password against stored hash. A configuration without a password hash accepts
    /// no password at all (not even an empty one).
    fn verify_password(&self, password: &str) -> bool {
        if self.config.password_hash.is_empty() {
            return false;
        }
        #[cfg(feature = "auth")]
        {
            bcrypt::verify(password, &self.config.password_hash).unwrap_or(false)
        }
        #[cfg(not(feature = "auth"))]
        {
            // Fallback to plain text comparison (not recommended for production)
            password == self.config.password_hash
        }
    }

    /// Check if user is currently locked out
    pub async fn is_locked_out(&self, username: &str) -> bool {
        let attempts = self.failed_attempts.read().await;
        if let Some((count, last_attempt)) = attempts.get(username)
            && *count >= self.config.max_failed_attempts
        {
            let elapsed = last_attempt.elapsed();
            return elapsed < self.config.lockout_duration;
        }
        false
    }

    /// Record a failed login attempt
    async fn record_failed_attempt(&self, username: &str) {
        let mut attempts = self.failed_attempts.write().await;
        let default_entry = (0, std::time::Instant::now());
        let (count, _) = attempts.get(username).unwrap_or(&default_entry);
        let new_count = *count;
        attempts.insert(
            username.to_string(),
            (new_count + 1, std::time::Instant::now()),
        );
    }

    /// Clear failed attempts for successful login
    async fn clear_failed_attempts(&self, username: &str) {
        let mut attempts = self.failed_attempts.write().await;
        attempts.remove(username);
    }

    /// Clean up old failed attempts periodically
    pub async fn cleanup_expired_attempts(&self) {
        let mut attempts = self.failed_attempts.write().await;
        let now = std::time::Instant::now();
        attempts.retain(|_, (_, last_attempt)| {
            now.duration_since(*last_attempt) < self.config.lockout_duration * 2
        });
    }
}

/// Extract basic auth credentials from request.
///
/// Parses a Basic Authentication header and returns the username and password.
/// The header format should be: `Basic <base64-encoded-credentials>`
/// where credentials are in the format `username:password`.
///
/// # Examples
///
/// ```rust
/// use hammerwork_web::auth::extract_basic_auth;
///
/// // Valid basic auth header
/// let auth_header = "Basic YWRtaW46cGFzc3dvcmQ="; // admin:password
/// let (username, password) = extract_basic_auth(auth_header).unwrap();
/// assert_eq!(username, "admin");
/// assert_eq!(password, "password");
///
/// // Invalid format returns None
/// assert!(extract_basic_auth("Bearer token123").is_none());
/// assert!(extract_basic_auth("Basic invalid_base64").is_none());
/// ```
///
/// # Returns
///
/// - `Some((username, password))` if the header is valid
/// - `None` if the header is malformed or not a Basic auth header
pub fn extract_basic_auth(auth_header: &str) -> Option<(String, String)> {
    if !auth_header.starts_with("Basic ") {
        return None;
    }

    let encoded = &auth_header[6..];
    let decoded = ::base64::prelude::BASE64_STANDARD.decode(encoded).ok()?;
    let decoded_str = String::from_utf8(decoded).ok()?;

    let mut parts = decoded_str.splitn(2, ':');
    let username = parts.next()?.to_string();
    let password = parts.next()?.to_string();

    Some((username, password))
}

/// Authentication filter for Warp
pub fn auth_filter(
    auth_state: AuthState,
) -> impl Filter<Extract = ((),), Error = Rejection> + Clone {
    warp::header::optional::<String>("authorization").and_then(
        move |auth_header: Option<String>| {
            let auth_state = auth_state.clone();
            async move {
                if !auth_state.is_enabled() {
                    return Ok::<_, Rejection>(());
                }

                let auth_header = auth_header
                    .ok_or_else(|| warp::reject::custom(AuthError::MissingCredentials))?;

                let (username, password) = extract_basic_auth(&auth_header)
                    .ok_or_else(|| warp::reject::custom(AuthError::InvalidFormat))?;

                if auth_state.is_locked_out(&username).await {
                    return Err(warp::reject::custom(AuthError::AccountLocked));
                }

                if auth_state.verify_credentials(&username, &password).await {
                    Ok(())
                } else {
                    Err(warp::reject::custom(AuthError::InvalidCredentials))
                }
            }
        },
    )
}

/// Custom authentication errors
#[derive(Debug)]
pub enum AuthError {
    MissingCredentials,
    InvalidFormat,
    InvalidCredentials,
    AccountLocked,
}

impl warp::reject::Reject for AuthError {}

/// Handle authentication rejections
pub async fn handle_auth_rejection(
    err: Rejection,
) -> Result<Box<dyn Reply>, std::convert::Infallible> {
    if let Some(auth_error) = err.find::<AuthError>() {
        match auth_error {
            AuthError::MissingCredentials => {
                let response = warp::reply::with_header(
                    warp::reply::with_status(
                        "Authentication required",
                        warp::http::StatusCode::UNAUTHORIZED,
                    ),
                    "WWW-Authenticate",
                    "Basic realm=\"Hammerwork Dashboard\"",
                );
                Ok(Box::new(response))
            }
            AuthError::InvalidFormat => {
                let error_response = serde_json::json!({"error": "Invalid authentication format"});
                Ok(Box::new(warp::reply::with_status(
                    warp::reply::json(&error_response),
                    warp::http::StatusCode::BAD_REQUEST,
                )))
            }
            AuthError::InvalidCredentials => {
                let error_response = serde_json::json!({"error": "Invalid credentials"});
                Ok(Box::new(warp::reply::with_status(
                    warp::reply::json(&error_response),
                    warp::http::StatusCode::UNAUTHORIZED,
                )))
            }
            AuthError::AccountLocked => {
                let error_response = serde_json::json!({"error": "Account temporarily locked"});
                Ok(Box::new(warp::reply::with_status(
                    warp::reply::json(&error_response),
                    warp::http::StatusCode::TOO_MANY_REQUESTS,
                )))
            }
        }
    } else {
        // Not an auth error: 404, 400 and 405 keep their meaning, anything else is a 500.
        let (message, status) = if err.is_not_found() {
            ("Resource not found", warp::http::StatusCode::NOT_FOUND)
        } else if err
            .find::<warp::filters::body::BodyDeserializeError>()
            .is_some()
        {
            ("Invalid request body", warp::http::StatusCode::BAD_REQUEST)
        } else if err.find::<warp::reject::InvalidQuery>().is_some() {
            (
                "Invalid query parameters",
                warp::http::StatusCode::BAD_REQUEST,
            )
        } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
            (
                "Method not allowed",
                warp::http::StatusCode::METHOD_NOT_ALLOWED,
            )
        } else if err.find::<warp::reject::PayloadTooLarge>().is_some() {
            (
                "Request body too large",
                warp::http::StatusCode::PAYLOAD_TOO_LARGE,
            )
        } else {
            (
                "Internal server error",
                warp::http::StatusCode::INTERNAL_SERVER_ERROR,
            )
        };
        let error_response = serde_json::json!({"error": message});
        Ok(Box::new(warp::reply::with_status(
            warp::reply::json(&error_response),
            status,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_auth_state_creation() {
        let config = AuthConfig {
            enabled: true,
            username: "testuser".to_string(),
            password_hash: "testhash".to_string(),
            ..Default::default()
        };

        let auth_state = AuthState::new(config);
        assert!(auth_state.is_enabled());
    }

    #[tokio::test]
    async fn test_disabled_auth() {
        let config = AuthConfig {
            enabled: false,
            ..Default::default()
        };

        let auth_state = AuthState::new(config);
        assert!(!auth_state.is_enabled());
        assert!(auth_state.verify_credentials("anyone", "anything").await);
    }

    #[tokio::test]
    async fn test_failed_attempts_tracking() {
        let config = AuthConfig {
            enabled: true,
            username: "admin".to_string(),
            password_hash: "wronghash".to_string(),
            max_failed_attempts: 3,
            lockout_duration: Duration::from_secs(60),
            ..Default::default()
        };

        let auth_state = AuthState::new(config);

        // Verify multiple failed attempts
        for _ in 0..3 {
            assert!(!auth_state.verify_credentials("admin", "wrongpass").await);
        }

        // Should be locked out now
        assert!(auth_state.is_locked_out("admin").await);
    }

    #[test]
    fn test_extract_basic_auth() {
        // "admin:password" in base64 is "YWRtaW46cGFzc3dvcmQ="
        let auth_header = "Basic YWRtaW46cGFzc3dvcmQ=";
        let (username, password) = extract_basic_auth(auth_header).unwrap();
        assert_eq!(username, "admin");
        assert_eq!(password, "password");
    }

    #[test]
    fn test_extract_basic_auth_invalid() {
        assert!(extract_basic_auth("Bearer token").is_none());
        assert!(extract_basic_auth("Basic invalid").is_none());
    }

    /// The stored form of `password`: a bcrypt hash with the `auth` feature, else the text.
    fn stored_password(password: &str) -> String {
        #[cfg(feature = "auth")]
        {
            bcrypt::hash(password, 4).unwrap()
        }
        #[cfg(not(feature = "auth"))]
        {
            password.to_string()
        }
    }

    fn auth_config(max_failed_attempts: u32, lockout: Duration) -> AuthConfig {
        AuthConfig {
            enabled: true,
            username: "admin".to_string(),
            password_hash: stored_password("s3cret"),
            max_failed_attempts,
            lockout_duration: lockout,
            ..Default::default()
        }
    }

    fn basic(user: &str, password: &str) -> String {
        format!(
            "Basic {}",
            base64::prelude::BASE64_STANDARD.encode(format!("{user}:{password}"))
        )
    }

    #[tokio::test]
    async fn correct_credentials_pass_and_wrong_ones_do_not() {
        let state = AuthState::new(auth_config(5, Duration::from_secs(60)));
        assert!(state.verify_credentials("admin", "s3cret").await);
        assert!(!state.verify_credentials("admin", "wrong").await);
        assert!(!state.verify_credentials("root", "s3cret").await);
        assert!(!state.verify_credentials("admin", "").await);
        assert!(!state.verify_credentials("admin", "S3CRET").await);
    }

    #[tokio::test]
    async fn an_unset_password_accepts_nothing() {
        // The default configuration has authentication enabled but no password; an empty
        // password must not match the empty hash.
        let state = AuthState::new(AuthConfig::default());
        assert!(state.is_enabled());
        assert!(!state.verify_credentials("admin", "").await);
        assert!(!state.verify_credentials("admin", "anything").await);
    }

    #[tokio::test]
    async fn lockout_blocks_even_the_right_password_until_it_expires() {
        let state = AuthState::new(auth_config(2, Duration::from_millis(150)));
        assert!(!state.verify_credentials("admin", "bad").await);
        assert!(!state.is_locked_out("admin").await, "one failure is not enough");
        assert!(!state.verify_credentials("admin", "bad").await);
        assert!(state.is_locked_out("admin").await);
        assert!(
            !state.verify_credentials("admin", "s3cret").await,
            "locked accounts reject the right password"
        );
        assert!(!state.is_locked_out("other").await, "lockout is per user");

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!state.is_locked_out("admin").await);
        assert!(state.verify_credentials("admin", "s3cret").await);
        assert!(
            !state.is_locked_out("admin").await,
            "a successful login clears the failures"
        );
        assert!(!state.verify_credentials("admin", "bad").await);
        assert!(!state.is_locked_out("admin").await, "the count started over");
    }

    #[tokio::test]
    async fn expired_failure_records_are_cleaned_up() {
        let state = AuthState::new(auth_config(5, Duration::from_millis(20)));
        state.verify_credentials("admin", "bad").await;
        state.verify_credentials("ghost", "bad").await;
        assert_eq!(state.failed_attempts.read().await.len(), 2);
        state.cleanup_expired_attempts().await;
        assert_eq!(state.failed_attempts.read().await.len(), 2, "still recent");
        tokio::time::sleep(Duration::from_millis(60)).await;
        state.cleanup_expired_attempts().await;
        assert!(state.failed_attempts.read().await.is_empty());
    }

    #[test]
    fn basic_auth_parsing_handles_odd_input() {
        // The password may contain ':'.
        let header = basic("admin", "pa:ss:word");
        assert_eq!(
            extract_basic_auth(&header),
            Some(("admin".to_string(), "pa:ss:word".to_string()))
        );
        assert_eq!(
            extract_basic_auth(&basic("", "")),
            Some((String::new(), String::new()))
        );
        // No colon at all, bad base64, non-UTF-8 payload, wrong scheme, wrong case.
        let no_colon = format!("Basic {}", base64::prelude::BASE64_STANDARD.encode("nocolon"));
        assert!(extract_basic_auth(&no_colon).is_none());
        assert!(extract_basic_auth("Basic !!!").is_none());
        let binary = format!("Basic {}", base64::prelude::BASE64_STANDARD.encode([0xff, 0xfe, b':']));
        assert!(extract_basic_auth(&binary).is_none());
        assert!(extract_basic_auth("Digest abc").is_none());
        assert!(extract_basic_auth("basic YWRtaW46cA==").is_none());
        assert!(extract_basic_auth("").is_none());
    }

    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn bcrypt_hashes_are_verified_not_compared() {
        let state = AuthState::new(auth_config(5, Duration::from_secs(60)));
        let hash = state.config.password_hash.clone();
        assert!(hash.starts_with("$2"));
        assert!(state.verify_credentials("admin", "s3cret").await);
        // Presenting the stored hash itself is not the password.
        assert!(!state.verify_credentials("admin", &hash).await);
        // A malformed stored hash accepts nothing.
        let broken = AuthState::new(AuthConfig {
            password_hash: "not-a-bcrypt-hash".into(),
            ..auth_config(5, Duration::from_secs(60))
        });
        assert!(!broken.verify_credentials("admin", "not-a-bcrypt-hash").await);
    }

    async fn request_status(state: &AuthState, header: Option<&str>) -> (u16, String) {
        let filter = warp::path("api")
            .and(auth_filter(state.clone()))
            .untuple_one()
            .and(warp::path("ping"))
            .map(|| "pong")
            .recover(handle_auth_rejection);
        let mut request = warp::test::request().path("/api/ping");
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        let response = request.reply(&filter).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    #[tokio::test]
    async fn the_filter_maps_each_failure_to_its_status() {
        let state = AuthState::new(auth_config(2, Duration::from_secs(60)));

        let filter = warp::path("api")
            .and(auth_filter(state.clone()))
            .untuple_one()
            .and(warp::path("ping"))
            .map(|| "pong")
            .recover(handle_auth_rejection);
        let missing = warp::test::request().path("/api/ping").reply(&filter).await;
        assert_eq!(missing.status(), 401);
        assert!(
            missing
                .headers()
                .get("www-authenticate")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Basic realm=")
        );

        let (status, body) = request_status(&state, Some("Bearer token")).await;
        assert_eq!(status, 400);
        assert!(body.contains("Invalid authentication format"), "{body}");

        let (status, body) = request_status(&state, Some(&basic("admin", "s3cret"))).await;
        assert_eq!((status, body.as_str()), (200, "pong"));

        let (status, body) = request_status(&state, Some(&basic("admin", "wrong"))).await;
        assert_eq!(status, 401);
        assert!(body.contains("Invalid credentials"), "{body}");

        // The second failure locks the account: now even the right password is refused.
        let (status, _) = request_status(&state, Some(&basic("admin", "wrong"))).await;
        assert_eq!(status, 401);
        let (status, body) = request_status(&state, Some(&basic("admin", "s3cret"))).await;
        assert_eq!(status, 429);
        assert!(body.contains("temporarily locked"), "{body}");
    }

    #[tokio::test]
    async fn disabled_auth_lets_everything_through() {
        let state = AuthState::new(AuthConfig {
            enabled: false,
            ..Default::default()
        });
        assert_eq!(request_status(&state, None).await.0, 200);
        assert_eq!(request_status(&state, Some("garbage")).await.0, 200);
    }

    #[tokio::test]
    async fn other_rejections_keep_their_status() {
        let json_route = warp::path("json")
            .and(warp::post())
            .and(warp::body::json::<serde_json::Value>())
            .map(|_| "ok");
        let query_route = warp::path("query")
            .and(warp::query::<std::collections::HashMap<String, u32>>())
            .map(|_| "ok");
        let sized = warp::path("sized")
            .and(warp::post())
            .and(warp::body::content_length_limit(4))
            .and(warp::body::bytes())
            .map(|_| "ok");
        let failing = warp::path("fail").and_then(|| async {
            Err::<String, _>(warp::reject::custom(AuthError::InvalidFormat))
        });
        let filter = json_route
            .or(query_route)
            .or(sized)
            .or(failing)
            .recover(handle_auth_rejection);

        let not_found = warp::test::request().path("/nothing").reply(&filter).await;
        assert_eq!(not_found.status(), 404);
        assert!(String::from_utf8_lossy(not_found.body()).contains("Resource not found"));

        let bad_json = warp::test::request()
            .method("POST")
            .path("/json")
            .body("{nope")
            .reply(&filter)
            .await;
        assert_eq!(bad_json.status(), 400);

        let bad_query = warp::test::request()
            .path("/query?n=abc")
            .reply(&filter)
            .await;
        assert_eq!(bad_query.status(), 400);

        let wrong_method = warp::test::request().path("/json").reply(&filter).await;
        assert_eq!(wrong_method.status(), 405);

        let too_big = warp::test::request()
            .method("POST")
            .path("/sized")
            .header("content-length", "100")
            .body("x".repeat(100))
            .reply(&filter)
            .await;
        assert_eq!(too_big.status(), 413);

        let custom = warp::test::request().path("/fail").reply(&filter).await;
        assert_eq!(custom.status(), 400);
    }
}
