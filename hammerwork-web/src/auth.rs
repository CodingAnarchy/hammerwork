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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use warp::{Filter, Rejection, Reply};
use base64::Engine;

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

    /// Verify password against stored hash
    fn verify_password(&self, password: &str) -> bool {
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
    async fn is_locked_out(&self, username: &str) -> bool {
        let attempts = self.failed_attempts.read().await;
        if let Some((count, last_attempt)) = attempts.get(username) {
            if *count >= self.config.max_failed_attempts {
                let elapsed = last_attempt.elapsed();
                return elapsed < self.config.lockout_duration;
            }
        }
        false
    }

    /// Record a failed login attempt
    async fn record_failed_attempt(&self, username: &str) {
        let mut attempts = self.failed_attempts.write().await;
        let default_entry = (0, std::time::Instant::now());
        let (count, _) = attempts.get(username).unwrap_or(&default_entry);
        let new_count = *count;
        attempts.insert(username.to_string(), (new_count + 1, std::time::Instant::now()));
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
    warp::header::optional::<String>("authorization")
        .and_then(move |auth_header: Option<String>| {
            let auth_state = auth_state.clone();
            async move {
                if !auth_state.is_enabled() {
                    return Ok::<_, Rejection>(());
                }

                let auth_header = auth_header.ok_or_else(|| {
                    warp::reject::custom(AuthError::MissingCredentials)
                })?;

                let (username, password) = extract_basic_auth(&auth_header)
                    .ok_or_else(|| warp::reject::custom(AuthError::InvalidFormat))?;

                if auth_state.verify_credentials(&username, &password).await {
                    Ok(())
                } else {
                    Err(warp::reject::custom(AuthError::InvalidCredentials))
                }
            }
        })
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
pub async fn handle_auth_rejection(err: Rejection) -> Result<Box<dyn Reply>, std::convert::Infallible> {
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
                    warp::http::StatusCode::UNAUTHORIZED,
                )))
            }
        }
    } else {
        // Not an auth error, return generic error
        let error_response = serde_json::json!({"error": "Internal server error"});
        Ok(Box::new(warp::reply::with_status(
            warp::reply::json(&error_response),
            warp::http::StatusCode::INTERNAL_SERVER_ERROR,
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
}