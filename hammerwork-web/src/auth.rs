//! Authentication middleware for the web dashboard.
//!
//! This module provides HTTP Basic authentication against a bcrypt password hash, a lockout
//! after repeated failures, and a short-lived cache of successful verifications.
//!
//! - **Password hashes:** the configured `password_hash` is always a bcrypt hash and is only
//!   ever *verified* with bcrypt, never compared as text. Verifying bcrypt needs the `auth`
//!   feature (on by default); a build without it rejects every password, and
//!   [`DashboardConfig::validate`](crate::config::DashboardConfig::validate) refuses to start
//!   such a build with authentication enabled.
//! - **Timing:** the username is compared in constant time through a keyed hash, and bcrypt
//!   runs whether or not the username matched, so response times do not reveal the username.
//! - **Blocking:** bcrypt runs on Tokio's blocking thread pool (at most one verification per
//!   CPU at a time), never on the async worker threads. A successful verification is
//!   remembered for [`VERIFIED_CREDENTIALS_TTL`] (or `session_timeout`, if shorter), so a
//!   dashboard polling several endpoints does not pay for bcrypt on every request.
//! - **Lockout:** failures are counted per client IP address (the connection's address, never
//!   a header) and username, so an attacker cannot lock the administrator out from other
//!   addresses. After `max_failed_attempts` failures the client is refused for
//!   `lockout_duration`; afterwards the count starts over, and a successful login resets it.
//!   At most [`MAX_TRACKED_CLIENTS`] clients are tracked.
//!
//! # Examples
//!
//! ## Basic Authentication Setup
//!
//! ```rust
//! use hammerwork_web::auth::AuthState;
//! use hammerwork_web::config::AuthConfig;
//!
//! let auth_config = AuthConfig {
//!     enabled: true,
//!     username: "admin".to_string(),
//!     // A bcrypt hash, for example from `bcrypt::hash(password, bcrypt::DEFAULT_COST)`.
//!     password_hash: "$2b$12$abcdefghijklmnopqrstuuJ0Y7gZ8z5d0FQqfJb8yX3QZpGQ0lW6e".to_string(),
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
use crate::security::RequestRefused;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use subtle::ConstantTimeEq;
use warp::{Filter, Rejection, Reply};

/// The most clients (IP address and username) whose failed attempts are tracked at once.
///
/// When the table is full, expired records are dropped first, then the least recently
/// failed client, so the table cannot grow without bound.
pub const MAX_TRACKED_CLIENTS: usize = 10_000;

/// How long a successful verification is remembered, unless `session_timeout` is shorter.
pub const VERIFIED_CREDENTIALS_TTL: Duration = Duration::from_secs(60);

/// The most remembered verifications. Only correct credentials are remembered, so in
/// practice there is one entry per configured user.
const MAX_VERIFIED_CREDENTIALS: usize = 64;

/// A keyed SHA-256 digest.
type Digest = [u8; 32];

/// Whose failures are counted together: one client address and whether it named the
/// configured user. Every other username shares one record per address, so made-up usernames
/// cannot add entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct AttemptKey {
    client: Option<IpAddr>,
    configured_user: bool,
}

/// The failures of one [`AttemptKey`].
#[derive(Debug, Clone, Copy)]
struct Failures {
    count: u32,
    last: Instant,
}

/// The outcome of checking a set of credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The credentials are correct (or authentication is disabled).
    Accepted,
    /// The credentials are wrong.
    Rejected,
    /// The client failed too often; nothing is checked until the lockout expires.
    LockedOut,
}

/// Authentication middleware state. Cloning it shares the state.
#[derive(Clone)]
pub struct AuthState {
    inner: Arc<Inner>,
}

struct Inner {
    config: AuthConfig,
    /// Random per-process key for the digests below.
    key: Digest,
    /// Keyed digest of the configured username.
    username_digest: Digest,
    failed_attempts: Mutex<HashMap<AttemptKey, Failures>>,
    /// Keyed digests of recently verified credentials, with their expiry.
    verified: Mutex<HashMap<Digest, Instant>>,
    /// Limits concurrent bcrypt verifications to the number of CPUs.
    #[cfg_attr(not(feature = "auth"), allow(dead_code))]
    bcrypt_slots: tokio::sync::Semaphore,
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("enabled", &self.inner.config.enabled)
            .field("username", &self.inner.config.username)
            .finish_non_exhaustive()
    }
}

/// Locks `mutex`, recovering the data if another thread panicked while holding it (every
/// update below leaves the maps consistent).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// HMAC-SHA256 of the length-prefixed `parts` under `key`.
fn keyed_digest(key: &Digest, parts: &[&[u8]]) -> Digest {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        unreachable!("HMAC accepts keys of any length");
    };
    for part in parts {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The address failures are counted under: IPv4-mapped IPv6 addresses count as IPv4, and
/// IPv6 clients are grouped by their /64 network, the usual allocation for one host.
fn client_key(addr: IpAddr) -> IpAddr {
    match addr.to_canonical() {
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            segments[4..].fill(0);
            IpAddr::V6(Ipv6Addr::from(segments))
        }
        v4 => v4,
    }
}

impl AuthState {
    pub fn new(config: AuthConfig) -> Self {
        let key: Digest = rand::random();
        let username_digest = keyed_digest(&key, &[b"username", config.username.as_bytes()]);
        let slots = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            inner: Arc::new(Inner {
                config,
                key,
                username_digest,
                failed_attempts: Mutex::new(HashMap::new()),
                verified: Mutex::new(HashMap::new()),
                bcrypt_slots: tokio::sync::Semaphore::new(slots),
            }),
        }
    }

    /// Check if authentication is enabled
    pub fn is_enabled(&self) -> bool {
        self.inner.config.enabled
    }

    /// Verify credentials from an unknown client address.
    ///
    /// Equivalent to [`check`](Self::check) with no client address, returning whether the
    /// credentials were accepted.
    pub async fn verify_credentials(&self, username: &str, password: &str) -> bool {
        self.check(None, username, password).await == Verdict::Accepted
    }

    /// Check `username` and `password` for a request from `client`.
    ///
    /// A locked-out client gets [`Verdict::LockedOut`] without its credentials being looked
    /// at. Otherwise a failure is counted against the client, and a success clears its
    /// failures.
    pub async fn check(&self, client: Option<IpAddr>, username: &str, password: &str) -> Verdict {
        if !self.is_enabled() {
            return Verdict::Accepted;
        }

        let key = self.attempt_key(client, username);
        if self.key_locked_out(key) {
            return Verdict::LockedOut;
        }

        let credentials = keyed_digest(
            &self.inner.key,
            &[b"credentials", username.as_bytes(), password.as_bytes()],
        );
        if self.recently_verified(&credentials) {
            self.clear_failed_attempts(key);
            return Verdict::Accepted;
        }

        // bcrypt runs whatever the username, so the time taken does not reveal it.
        let password_ok = self.verify_password(password).await;
        if key.configured_user & password_ok {
            self.remember_verified(credentials);
            self.clear_failed_attempts(key);
            Verdict::Accepted
        } else {
            self.record_failed_attempt(key);
            Verdict::Rejected
        }
    }

    /// Whether `username` is the configured one, compared in constant time.
    fn username_matches(&self, username: &str) -> bool {
        let digest = keyed_digest(&self.inner.key, &[b"username", username.as_bytes()]);
        digest.ct_eq(&self.inner.username_digest).into()
    }

    fn attempt_key(&self, client: Option<IpAddr>, username: &str) -> AttemptKey {
        AttemptKey {
            client: client.map(client_key),
            configured_user: self.username_matches(username),
        }
    }

    /// Verify `password` against the stored bcrypt hash, off the async worker threads. A
    /// configuration without a password hash, or a build without the `auth` feature, accepts
    /// no password at all. The stored hash is never compared to the password as text.
    async fn verify_password(&self, password: &str) -> bool {
        if self.inner.config.password_hash.is_empty() {
            return false;
        }
        #[cfg(feature = "auth")]
        {
            let Ok(_slot) = self.inner.bcrypt_slots.acquire().await else {
                return false;
            };
            let hash = self.inner.config.password_hash.clone();
            let password = password.to_string();
            tokio::task::spawn_blocking(move || bcrypt::verify(password, &hash).unwrap_or(false))
                .await
                .unwrap_or(false)
        }
        #[cfg(not(feature = "auth"))]
        {
            let _ = password;
            false
        }
    }

    /// How long a verification is remembered.
    fn verified_ttl(&self) -> Duration {
        VERIFIED_CREDENTIALS_TTL.min(self.inner.config.session_timeout)
    }

    fn recently_verified(&self, credentials: &Digest) -> bool {
        let mut verified = lock(&self.inner.verified);
        match verified.get(credentials) {
            Some(expires) if *expires > Instant::now() => true,
            Some(_) => {
                verified.remove(credentials);
                false
            }
            None => false,
        }
    }

    fn remember_verified(&self, credentials: Digest) {
        let ttl = self.verified_ttl();
        if ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let mut verified = lock(&self.inner.verified);
        if verified.len() >= MAX_VERIFIED_CREDENTIALS {
            verified.retain(|_, expires| *expires > now);
            if verified.len() >= MAX_VERIFIED_CREDENTIALS {
                verified.clear();
            }
        }
        verified.insert(credentials, now + ttl);
    }

    /// Whether `username` is locked out when connecting from an unknown address.
    pub async fn is_locked_out(&self, username: &str) -> bool {
        self.is_locked_out_from(None, username).await
    }

    /// Whether `username` is locked out when connecting from `client`.
    pub async fn is_locked_out_from(&self, client: Option<IpAddr>, username: &str) -> bool {
        self.key_locked_out(self.attempt_key(client, username))
    }

    fn key_locked_out(&self, key: AttemptKey) -> bool {
        let attempts = lock(&self.inner.failed_attempts);
        attempts.get(&key).is_some_and(|failures| {
            failures.count >= self.inner.config.max_failed_attempts
                && failures.last.elapsed() < self.inner.config.lockout_duration
        })
    }

    /// Count a failure. A record whose last failure is older than the lockout duration
    /// starts over, so a lockout always ends.
    fn record_failed_attempt(&self, key: AttemptKey) {
        let lockout = self.inner.config.lockout_duration;
        let now = Instant::now();
        let mut attempts = lock(&self.inner.failed_attempts);
        if !attempts.contains_key(&key) && attempts.len() >= MAX_TRACKED_CLIENTS {
            attempts.retain(|_, failures| now.duration_since(failures.last) < lockout);
            if attempts.len() >= MAX_TRACKED_CLIENTS
                && let Some(stalest) = attempts
                    .iter()
                    .min_by_key(|(_, failures)| failures.last)
                    .map(|(key, _)| *key)
            {
                attempts.remove(&stalest);
            }
        }
        let failures = attempts.entry(key).or_insert(Failures {
            count: 0,
            last: now,
        });
        if now.duration_since(failures.last) >= lockout {
            failures.count = 0;
        }
        failures.count = failures.count.saturating_add(1);
        failures.last = now;
    }

    fn clear_failed_attempts(&self, key: AttemptKey) {
        lock(&self.inner.failed_attempts).remove(&key);
    }

    /// Drop failure records whose lockout has expired, and expired verifications.
    pub async fn cleanup_expired_attempts(&self) {
        let lockout = self.inner.config.lockout_duration;
        let now = Instant::now();
        lock(&self.inner.failed_attempts)
            .retain(|_, failures| now.duration_since(failures.last) < lockout);
        lock(&self.inner.verified).retain(|_, expires| *expires > now);
    }

    /// The number of clients with failure records (for tests and monitoring).
    pub fn tracked_clients(&self) -> usize {
        lock(&self.inner.failed_attempts).len()
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

/// Authentication filter for Warp.
///
/// Failures are counted per client address, taken from the connection (never from a
/// forwarding header, which a client could forge).
pub fn auth_filter(
    auth_state: AuthState,
) -> impl Filter<Extract = ((),), Error = Rejection> + Clone {
    warp::header::optional::<String>("authorization")
        .and(warp::addr::remote())
        .and_then(
            move |auth_header: Option<String>, remote: Option<SocketAddr>| {
                let auth_state = auth_state.clone();
                async move {
                    if !auth_state.is_enabled() {
                        return Ok::<_, Rejection>(());
                    }

                    let auth_header = auth_header
                        .ok_or_else(|| warp::reject::custom(AuthError::MissingCredentials))?;

                    let (username, password) = extract_basic_auth(&auth_header)
                        .ok_or_else(|| warp::reject::custom(AuthError::InvalidFormat))?;

                    match auth_state
                        .check(remote.map(|addr| addr.ip()), &username, &password)
                        .await
                    {
                        Verdict::Accepted => Ok(()),
                        Verdict::Rejected => {
                            Err(warp::reject::custom(AuthError::InvalidCredentials))
                        }
                        Verdict::LockedOut => Err(warp::reject::custom(AuthError::AccountLocked)),
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
        let (message, status) = if let Some(refused) = err.find::<RequestRefused>() {
            (refused.message(), refused.status())
        } else if err.is_not_found() {
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
        } else if err.find::<warp::reject::PayloadTooLarge>().is_some() {
            (
                "Request body too large",
                warp::http::StatusCode::PAYLOAD_TOO_LARGE,
            )
        } else if err.find::<warp::reject::LengthRequired>().is_some() {
            (
                "A Content-Length header is required",
                warp::http::StatusCode::LENGTH_REQUIRED,
            )
        } else if err.find::<warp::reject::MethodNotAllowed>().is_some() {
            // Checked last: a request that reached its route and failed there also carries
            // the 405s of the sibling routes it did not match.
            (
                "Method not allowed",
                warp::http::StatusCode::METHOD_NOT_ALLOWED,
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
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> Option<IpAddr> {
        Some(IpAddr::V4(Ipv4Addr::new(192, 0, 2, last)))
    }

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
        let debug = format!("{auth_state:?}");
        assert!(debug.contains("testuser"), "{debug}");
        assert!(
            !debug.contains("testhash"),
            "the hash is not printed: {debug}"
        );
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
        assert_eq!(
            auth_state.check(None, "admin", "wrongpass").await,
            Verdict::LockedOut
        );
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

    /// The stored form of `password`: always a bcrypt hash.
    #[cfg(feature = "auth")]
    fn stored_password(password: &str) -> String {
        bcrypt::hash(password, 4).unwrap()
    }

    #[cfg(feature = "auth")]
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

    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn correct_credentials_pass_and_wrong_ones_do_not() {
        let state = AuthState::new(auth_config(50, Duration::from_secs(60)));
        assert!(state.verify_credentials("admin", "s3cret").await);
        assert!(!state.verify_credentials("admin", "wrong").await);
        assert!(!state.verify_credentials("root", "s3cret").await);
        assert!(!state.verify_credentials("admi", "s3cret").await);
        assert!(!state.verify_credentials("admin2", "s3cret").await);
        assert!(!state.verify_credentials("", "s3cret").await);
        assert!(!state.verify_credentials("admin", "").await);
        assert!(!state.verify_credentials("admin", "S3CRET").await);
        // A remembered verification is for these exact credentials only.
        assert!(state.verify_credentials("admin", "s3cret").await);
        assert!(!state.verify_credentials("root", "s3cret").await);
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

    /// H5: the stored hash is never compared to the password as text, in any build.
    #[tokio::test]
    async fn the_stored_hash_is_never_a_password() {
        let state = AuthState::new(AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: "$2b$12$abcdefghijklmnopqrstuuJ0Y7gZ8z5d0FQqfJb8yX3QZpGQ0lW6e".into(),
            ..Default::default()
        });
        let hash = state.inner.config.password_hash.clone();
        assert!(!state.verify_credentials("admin", &hash).await);
        // Nor is a plaintext "hash" accepted as its own password.
        let plain = AuthState::new(AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: "s3cret".into(),
            ..Default::default()
        });
        assert!(!plain.verify_credentials("admin", "s3cret").await);
    }

    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn lockout_blocks_even_the_right_password_until_it_expires() {
        let state = AuthState::new(auth_config(2, Duration::from_millis(150)));
        assert!(!state.verify_credentials("admin", "bad").await);
        assert!(
            !state.is_locked_out("admin").await,
            "one failure is not enough"
        );
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
        assert!(
            !state.is_locked_out("admin").await,
            "the count started over"
        );
    }

    /// H6: a lockout ends, and one failure after it does not lock again.
    #[tokio::test]
    async fn an_expired_lockout_starts_the_count_over() {
        let state = AuthState::new(AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: "unusable".into(),
            max_failed_attempts: 3,
            lockout_duration: Duration::from_millis(100),
            ..Default::default()
        });
        for _ in 0..3 {
            assert_eq!(state.check(ip(1), "admin", "x").await, Verdict::Rejected);
        }
        assert_eq!(state.check(ip(1), "admin", "x").await, Verdict::LockedOut);
        tokio::time::sleep(Duration::from_millis(150)).await;
        // Before the fix the count stayed at 3, so this single failure re-locked the account.
        assert_eq!(state.check(ip(1), "admin", "x").await, Verdict::Rejected);
        assert!(!state.is_locked_out_from(ip(1), "admin").await);
        assert_eq!(state.check(ip(1), "admin", "x").await, Verdict::Rejected);
        assert!(!state.is_locked_out_from(ip(1), "admin").await);
    }

    /// H6: an attacker's failures lock out the attacker's address, not the administrator.
    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn lockout_is_per_client_address() {
        let state = AuthState::new(auth_config(2, Duration::from_secs(60)));
        for _ in 0..2 {
            assert_eq!(
                state.check(ip(66), "admin", "guess").await,
                Verdict::Rejected
            );
        }
        assert_eq!(
            state.check(ip(66), "admin", "s3cret").await,
            Verdict::LockedOut
        );
        assert_eq!(
            state.check(ip(7), "admin", "s3cret").await,
            Verdict::Accepted
        );
        assert!(state.is_locked_out_from(ip(66), "admin").await);
        assert!(!state.is_locked_out_from(ip(7), "admin").await);
    }

    /// H6: made-up usernames share one record per address, and the table has a cap.
    #[tokio::test]
    async fn the_failure_table_is_bounded() {
        let state = AuthState::new(AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: "unusable".into(),
            max_failed_attempts: 1_000_000,
            lockout_duration: Duration::from_secs(60),
            ..Default::default()
        });
        for n in 0..200 {
            state
                .check(ip(1), &format!("user-{n}-{}", "x".repeat(n)), "x")
                .await;
        }
        assert_eq!(state.tracked_clients(), 1, "one record for unknown users");
        state.check(ip(1), "admin", "x").await;
        assert_eq!(state.tracked_clients(), 2, "and one for the real user");

        // Many addresses: never more than the cap; the stalest record is evicted.
        for n in 0..(MAX_TRACKED_CLIENTS as u32 + 50) {
            let addr = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n));
            state.record_failed_attempt(state.attempt_key(Some(addr), "admin"));
        }
        assert_eq!(state.tracked_clients(), MAX_TRACKED_CLIENTS);
    }

    #[test]
    fn ipv6_clients_are_grouped_by_network() {
        let a: IpAddr = "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(client_key(a), client_key(b));
        assert_ne!(client_key(a), client_key(c));
        let mapped: IpAddr = "::ffff:192.0.2.9".parse().unwrap();
        assert_eq!(client_key(mapped), ip(9).unwrap());
        assert_eq!(client_key(ip(9).unwrap()), ip(9).unwrap());
    }

    #[tokio::test]
    async fn expired_failure_records_are_cleaned_up() {
        let state = AuthState::new(AuthConfig {
            enabled: true,
            username: "admin".into(),
            password_hash: "unusable".into(),
            lockout_duration: Duration::from_millis(20),
            ..Default::default()
        });
        state.verify_credentials("admin", "bad").await;
        state.verify_credentials("ghost", "bad").await;
        assert_eq!(state.tracked_clients(), 2);
        state.cleanup_expired_attempts().await;
        assert_eq!(state.tracked_clients(), 2, "still recent");
        tokio::time::sleep(Duration::from_millis(60)).await;
        state.cleanup_expired_attempts().await;
        assert_eq!(state.tracked_clients(), 0);
    }

    /// H7: a successful verification is remembered briefly, so later requests skip bcrypt.
    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn successful_verifications_are_remembered_briefly() {
        let state = AuthState::new(auth_config(5, Duration::from_secs(60)));
        assert!(state.verify_credentials("admin", "s3cret").await);
        assert_eq!(lock(&state.inner.verified).len(), 1);
        // Failures are never remembered.
        assert!(!state.verify_credentials("admin", "nope").await);
        assert_eq!(lock(&state.inner.verified).len(), 1);

        // Expired entries are not used, and are cleaned up.
        let credentials = *lock(&state.inner.verified).keys().next().unwrap();
        lock(&state.inner.verified).insert(credentials, Instant::now());
        assert!(!state.recently_verified(&credentials));
        assert!(lock(&state.inner.verified).is_empty());
        assert!(state.verify_credentials("admin", "s3cret").await);
        lock(&state.inner.verified).insert(credentials, Instant::now());
        state.cleanup_expired_attempts().await;
        assert!(lock(&state.inner.verified).is_empty());

        // The cache is bounded.
        for n in 0..(MAX_VERIFIED_CREDENTIALS + 5) {
            state.remember_verified([n as u8; 32]);
        }
        assert!(lock(&state.inner.verified).len() <= MAX_VERIFIED_CREDENTIALS);

        // A zero session timeout turns the cache off.
        let uncached = AuthState::new(AuthConfig {
            session_timeout: Duration::ZERO,
            ..auth_config(5, Duration::from_secs(60))
        });
        assert!(uncached.verify_credentials("admin", "s3cret").await);
        assert!(lock(&uncached.inner.verified).is_empty());
    }

    /// H7: bcrypt does not block the async runtime: on a single-threaded runtime another task
    /// keeps running while a password is verified.
    #[cfg(feature = "auth")]
    #[tokio::test(flavor = "current_thread")]
    async fn bcrypt_runs_off_the_async_threads() {
        let state = AuthState::new(AuthConfig {
            password_hash: bcrypt::hash("s3cret", 10).unwrap(),
            session_timeout: Duration::ZERO,
            ..auth_config(5, Duration::from_secs(60))
        });
        let ticks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = ticks.clone();
        let ticker = tokio::spawn(async move {
            loop {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        tokio::task::yield_now().await;
        let before = ticks.load(std::sync::atomic::Ordering::Relaxed);
        let started = Instant::now();
        assert!(state.verify_credentials("admin", "s3cret").await);
        let elapsed = started.elapsed();
        let during = ticks.load(std::sync::atomic::Ordering::Relaxed) - before;
        ticker.abort();
        assert!(
            during >= 3,
            "the runtime made no progress during a {elapsed:?} verification ({during} ticks)"
        );
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
        let no_colon = format!(
            "Basic {}",
            base64::prelude::BASE64_STANDARD.encode("nocolon")
        );
        assert!(extract_basic_auth(&no_colon).is_none());
        assert!(extract_basic_auth("Basic !!!").is_none());
        let binary = format!(
            "Basic {}",
            base64::prelude::BASE64_STANDARD.encode([0xff, 0xfe, b':'])
        );
        assert!(extract_basic_auth(&binary).is_none());
        assert!(extract_basic_auth("Digest abc").is_none());
        assert!(extract_basic_auth("basic YWRtaW46cA==").is_none());
        assert!(extract_basic_auth("").is_none());
    }

    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn bcrypt_hashes_are_verified_not_compared() {
        let state = AuthState::new(auth_config(5, Duration::from_secs(60)));
        let hash = state.inner.config.password_hash.clone();
        assert!(hash.starts_with("$2"));
        assert!(state.verify_credentials("admin", "s3cret").await);
        // Presenting the stored hash itself is not the password.
        assert!(!state.verify_credentials("admin", &hash).await);
        // A malformed stored hash accepts nothing.
        let broken = AuthState::new(AuthConfig {
            password_hash: "not-a-bcrypt-hash".into(),
            ..auth_config(5, Duration::from_secs(60))
        });
        assert!(
            !broken
                .verify_credentials("admin", "not-a-bcrypt-hash")
                .await
        );
    }

    fn ping_route(
        state: AuthState,
    ) -> impl Filter<Extract = (impl Reply,), Error = std::convert::Infallible> + Clone {
        warp::path("api")
            .and(auth_filter(state))
            .untuple_one()
            .and(warp::path("ping"))
            .map(|| "pong")
            .recover(handle_auth_rejection)
    }

    async fn request_from(
        state: &AuthState,
        header: Option<&str>,
        from: Option<IpAddr>,
    ) -> (u16, String) {
        let mut request = warp::test::request().path("/api/ping");
        if let Some(header) = header {
            request = request.header("authorization", header);
        }
        if let Some(from) = from {
            request = request.remote_addr(SocketAddr::new(from, 40000));
        }
        let response = request.reply(&ping_route(state.clone())).await;
        (
            response.status().as_u16(),
            String::from_utf8_lossy(response.body()).to_string(),
        )
    }

    async fn request_status(state: &AuthState, header: Option<&str>) -> (u16, String) {
        request_from(state, header, None).await
    }

    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn the_filter_maps_each_failure_to_its_status() {
        let state = AuthState::new(auth_config(2, Duration::from_secs(60)));

        let missing = warp::test::request()
            .path("/api/ping")
            .reply(&ping_route(state.clone()))
            .await;
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

    /// H6: the filter counts failures per connection address.
    #[cfg(feature = "auth")]
    #[tokio::test]
    async fn the_filter_locks_out_the_failing_address_only() {
        let state = AuthState::new(auth_config(2, Duration::from_secs(60)));
        let wrong = basic("admin", "wrong");
        let right = basic("admin", "s3cret");
        for _ in 0..2 {
            assert_eq!(request_from(&state, Some(&wrong), ip(66)).await.0, 401);
        }
        assert_eq!(request_from(&state, Some(&right), ip(66)).await.0, 429);
        assert_eq!(request_from(&state, Some(&right), ip(7)).await.0, 200);
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
