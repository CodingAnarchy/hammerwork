//! Request checks that protect the dashboard from cross-site requests (CSRF) and oversized
//! request bodies.
//!
//! A web page the operator visits can make the browser send requests to the dashboard, and
//! the browser attaches cached Basic credentials to them. Two independent checks stop that:
//!
//! - [`same_origin_writes`] refuses state-changing requests (every method except `GET`,
//!   `HEAD` and `OPTIONS`) that a browser sent from another origin, judged by the
//!   `Sec-Fetch-Site` header or, without it, by comparing `Origin` with the `Host` the
//!   request was sent to. [`same_origin`] applies the same rule to every method, for the
//!   WebSocket handshake. Requests without either header (curl, scripts) are not from a
//!   browser page and pass. Origins listed in [`AllowedOrigins`] pass as well.
//! - [`json_body`] only accepts bodies sent as `Content-Type: application/json`, which a page
//!   on another origin cannot send without a CORS preflight, and at most
//!   [`MAX_JSON_BODY_BYTES`] of them.
//!
//! ```rust
//! use hammerwork_web::security::{AllowedOrigins, normalize_origin};
//!
//! assert_eq!(
//!     normalize_origin("https://Ops.Example.com/").as_deref(),
//!     Some("https://ops.example.com")
//! );
//! assert!(normalize_origin("https://ops.example.com/path").is_none());
//!
//! let allowed = AllowedOrigins::new(["https://ops.example.com"]).unwrap();
//! assert!(allowed.contains("https://ops.example.com"));
//! assert!(!allowed.contains("https://evil.example"));
//! ```

use serde::de::DeserializeOwned;
use std::sync::Arc;
use warp::http::{Method, StatusCode};
use warp::{Filter, Rejection};

/// The largest JSON request body the API accepts, in bytes.
pub const MAX_JSON_BODY_BYTES: u64 = 1024 * 1024;

/// Why a request was refused before reaching its handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestRefused {
    /// A browser sent the request from a page on another origin.
    CrossOrigin,
    /// The request body is not declared as `application/json`.
    UnsupportedMediaType,
}

impl warp::reject::Reject for RequestRefused {}

impl RequestRefused {
    /// The error message of the JSON reply.
    pub fn message(&self) -> &'static str {
        match self {
            Self::CrossOrigin => "Cross-origin request refused",
            Self::UnsupportedMediaType => "Content-Type must be application/json",
        }
    }

    /// The HTTP status of the reply.
    pub fn status(&self) -> StatusCode {
        match self {
            Self::CrossOrigin => StatusCode::FORBIDDEN,
            Self::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        }
    }
}

/// The canonical form of a web origin (`scheme://host[:port]`, lowercase, without a trailing
/// slash), or `None` if `origin` is not one: the scheme must be `http` or `https`, the host
/// must be present, a port must be a number, and there must be no path, query or user info.
pub fn normalize_origin(origin: &str) -> Option<String> {
    let origin = origin.trim().to_ascii_lowercase();
    let origin = origin.strip_suffix('/').unwrap_or(&origin);
    let (scheme, authority) = origin.split_once("://")?;
    if !matches!(scheme, "http" | "https") || authority.is_empty() {
        return None;
    }
    if authority.contains(['/', '?', '#', '@', ' ']) {
        return None;
    }
    let host = match authority.rsplit_once(':') {
        // An IPv6 literal without a port ("[::1]") also contains ':'.
        Some((host, port)) if !port.ends_with(']') => {
            if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            port.parse::<u16>().ok()?;
            host
        }
        _ => authority,
    };
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

/// Origins other than the dashboard's own that may send state-changing requests (and, with
/// CORS enabled, read API responses).
#[derive(Debug, Clone, Default)]
pub struct AllowedOrigins(Arc<Vec<String>>);

impl AllowedOrigins {
    /// The normalized `origins`, or an error naming the first one that is not a valid origin.
    pub fn new<I, S>(origins: I) -> crate::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let origins = origins
            .into_iter()
            .map(|origin| {
                let origin = origin.as_ref();
                normalize_origin(origin).ok_or_else(|| {
                    anyhow::anyhow!(
                        "invalid origin {origin:?}: expected scheme://host[:port], for example \
                         https://ops.example.com"
                    )
                })
            })
            .collect::<crate::Result<Vec<_>>>()?;
        Ok(Self(Arc::new(origins)))
    }

    /// Whether `origin` (as a browser sends it) is listed.
    pub fn contains(&self, origin: &str) -> bool {
        normalize_origin(origin).is_some_and(|origin| self.0.contains(&origin))
    }

    /// The normalized origins.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }
}

/// `authority` without the default port of `scheme`, lowercase.
fn without_default_port(scheme: &str, authority: &str) -> String {
    let authority = authority.to_ascii_lowercase();
    let default = match scheme {
        "https" => ":443",
        _ => ":80",
    };
    authority
        .strip_suffix(default)
        .map(str::to_string)
        .unwrap_or(authority)
}

/// Whether a request with these headers may proceed.
///
/// `Sec-Fetch-Site` is set by browsers only and cannot be set by page scripts, so it is
/// trusted when present. Without it, `Origin` must name the host the request was sent to.
/// A request with neither header did not come from a browser page.
pub fn request_allowed(
    origin: Option<&str>,
    fetch_site: Option<&str>,
    host: Option<&str>,
    allowed: &AllowedOrigins,
) -> bool {
    if origin.is_some_and(|origin| allowed.contains(origin)) {
        return true;
    }
    if let Some(site) = fetch_site {
        return matches!(
            site.trim().to_ascii_lowercase().as_str(),
            "same-origin" | "none"
        );
    }
    let Some(origin) = origin else {
        return true;
    };
    let Some(normalized) = normalize_origin(origin) else {
        // "null" (sandboxed frames, file: pages) or garbage.
        return false;
    };
    let Some((scheme, authority)) = normalized.split_once("://") else {
        return false;
    };
    host.is_some_and(|host| {
        without_default_port(scheme, host) == without_default_port(scheme, authority)
    })
}

/// The headers [`request_allowed`] looks at, plus the method.
fn origin_headers()
-> impl Filter<Extract = (Method, Option<String>, Option<String>, Option<String>), Error = Rejection>
+ Clone {
    warp::method()
        .and(warp::header::optional::<String>("origin"))
        .and(warp::header::optional::<String>("sec-fetch-site"))
        .and(
            warp::host::optional().map(|authority: Option<warp::host::Authority>| {
                authority.map(|authority| authority.as_str().to_string())
            }),
        )
}

fn guard(
    allowed: AllowedOrigins,
    writes_only: bool,
) -> impl Filter<Extract = (), Error = Rejection> + Clone {
    origin_headers()
        .and_then(
            move |method: Method,
                  origin: Option<String>,
                  fetch_site: Option<String>,
                  host: Option<String>| {
                let allowed = allowed.clone();
                async move {
                    let safe = matches!(method, Method::GET | Method::HEAD | Method::OPTIONS);
                    if (writes_only && safe)
                        || request_allowed(
                            origin.as_deref(),
                            fetch_site.as_deref(),
                            host.as_deref(),
                            &allowed,
                        )
                    {
                        Ok(())
                    } else {
                        tracing::warn!(
                            origin = origin.as_deref().unwrap_or("-"),
                            sec_fetch_site = fetch_site.as_deref().unwrap_or("-"),
                            %method,
                            "Refused a cross-origin request"
                        );
                        Err(warp::reject::custom(RequestRefused::CrossOrigin))
                    }
                }
            },
        )
        .untuple_one()
}

/// Refuses state-changing requests (any method but `GET`, `HEAD` and `OPTIONS`) that a
/// browser sent from another origin. See [`request_allowed`].
pub fn same_origin_writes(
    allowed: AllowedOrigins,
) -> impl Filter<Extract = (), Error = Rejection> + Clone {
    guard(allowed, true)
}

/// Refuses every request a browser sent from another origin, whatever its method. Used for
/// the WebSocket handshake, which is a `GET`.
pub fn same_origin(
    allowed: AllowedOrigins,
) -> impl Filter<Extract = (), Error = Rejection> + Clone {
    guard(allowed, false)
}

/// Whether a `Content-Type` value is JSON (`application/json`, with optional parameters).
fn is_json(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

/// A JSON request body: requires `Content-Type: application/json` and a `Content-Length` of
/// at most [`MAX_JSON_BODY_BYTES`], then deserializes the body.
pub fn json_body<T>() -> impl Filter<Extract = (T,), Error = Rejection> + Clone
where
    T: DeserializeOwned + Send,
{
    warp::header::optional::<String>("content-type")
        .and_then(|content_type: Option<String>| async move {
            if content_type.as_deref().is_some_and(is_json) {
                Ok(())
            } else {
                Err(warp::reject::custom(RequestRefused::UnsupportedMediaType))
            }
        })
        .untuple_one()
        .and(warp::body::content_length_limit(MAX_JSON_BODY_BYTES))
        .and(warp::body::json())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed(origins: &[&str]) -> AllowedOrigins {
        AllowedOrigins::new(origins).unwrap()
    }

    #[test]
    fn origins_are_normalized_and_validated() {
        for (input, expected) in [
            ("http://localhost:8080", Some("http://localhost:8080")),
            ("HTTPS://Ops.Example.COM/", Some("https://ops.example.com")),
            ("http://[::1]", Some("http://[::1]")),
            ("http://[::1]:9000", Some("http://[::1]:9000")),
            (" https://a.b ", Some("https://a.b")),
            ("*", None),
            ("null", None),
            ("ftp://example.com", None),
            ("https://", None),
            ("https://example.com/path", None),
            ("https://example.com?x", None),
            ("https://user@example.com", None),
            ("https://example.com:", None),
            ("https://example.com:http", None),
            ("https://example.com:99999", None),
            ("https://:8080", None),
            ("example.com", None),
        ] {
            assert_eq!(normalize_origin(input).as_deref(), expected, "{input}");
        }
        let err = AllowedOrigins::new(["https://ok.example", "*"]).unwrap_err();
        assert!(err.to_string().contains("invalid origin \"*\""), "{err}");
        let list = allowed(&["https://Ops.Example.com/"]);
        assert_eq!(list.as_slice(), ["https://ops.example.com"]);
        assert!(list.contains("https://ops.example.com"));
        assert!(!list.contains("null"));
        assert!(AllowedOrigins::default().as_slice().is_empty());
    }

    #[test]
    fn browsers_on_other_origins_are_refused() {
        let none = AllowedOrigins::default();
        let host = Some("127.0.0.1:8080");
        // Not a browser: no Origin and no Sec-Fetch-Site.
        assert!(request_allowed(None, None, host, &none));
        assert!(request_allowed(None, None, None, &none));
        // Sec-Fetch-Site decides when present.
        assert!(request_allowed(None, Some("same-origin"), host, &none));
        assert!(request_allowed(None, Some("none"), host, &none));
        assert!(!request_allowed(None, Some("cross-site"), host, &none));
        assert!(
            !request_allowed(
                Some("http://127.0.0.1:9999"),
                Some("same-site"),
                host,
                &none
            ),
            "another port on the same host is another origin"
        );
        assert!(
            request_allowed(
                Some("https://public.example"),
                Some("same-origin"),
                Some("10.0.0.5:8080"),
                &none
            ),
            "behind a proxy the browser's same-origin verdict wins"
        );
        // Without it, Origin must match Host.
        assert!(request_allowed(
            Some("http://127.0.0.1:8080"),
            None,
            host,
            &none
        ));
        assert!(request_allowed(
            Some("http://Dash.Example"),
            None,
            Some("dash.example:80"),
            &none
        ));
        assert!(request_allowed(
            Some("https://dash.example"),
            None,
            Some("dash.example:443"),
            &none
        ));
        assert!(!request_allowed(
            Some("http://evil.example"),
            None,
            host,
            &none
        ));
        assert!(!request_allowed(Some("null"), None, host, &none));
        assert!(!request_allowed(
            Some("http://127.0.0.1:8080"),
            None,
            None,
            &none
        ));
        // Listed origins pass even cross-site.
        let ops = allowed(&["https://ops.example"]);
        assert!(request_allowed(
            Some("https://ops.example"),
            Some("cross-site"),
            host,
            &ops
        ));
        assert!(!request_allowed(
            Some("https://evil.example"),
            Some("cross-site"),
            host,
            &ops
        ));
    }

    #[test]
    fn content_types() {
        assert!(is_json("application/json"));
        assert!(is_json("Application/JSON; charset=utf-8"));
        assert!(!is_json("text/plain"));
        assert!(!is_json("application/x-www-form-urlencoded"));
        assert!(!is_json("multipart/form-data; boundary=x"));
        assert!(!is_json(""));
    }

    fn write_route() -> impl Filter<Extract = (String,), Error = Rejection> + Clone {
        same_origin_writes(AllowedOrigins::default())
            .and(json_body::<serde_json::Value>())
            .map(|body: serde_json::Value| body.to_string())
    }

    async fn status(request: warp::test::RequestBuilder) -> u16 {
        let route = write_route().recover(crate::auth::handle_auth_rejection);
        request.reply(&route).await.status().as_u16()
    }

    fn post(body: &str) -> warp::test::RequestBuilder {
        warp::test::request()
            .method("POST")
            .path("/")
            .header("host", "127.0.0.1:8080")
            .header("content-length", body.len().to_string())
            .body(body)
    }

    #[tokio::test]
    async fn json_bodies_need_the_json_content_type_and_a_bounded_length() {
        let ok = post("{}").header("content-type", "application/json");
        assert_eq!(status(ok).await, 200);
        // A form or a type-less body (what a cross-site page can send without a preflight).
        assert_eq!(status(post("{}")).await, 415);
        let form = post("{}").header("content-type", "text/plain");
        assert_eq!(status(form).await, 415);
        // Too large.
        let big = format!("\"{}\"", "x".repeat(MAX_JSON_BODY_BYTES as usize));
        let big = post(&big).header("content-type", "application/json");
        assert_eq!(status(big).await, 413);
        // No Content-Length at all.
        let chunked = warp::test::request()
            .method("POST")
            .path("/")
            .header("content-type", "application/json");
        assert_eq!(status(chunked).await, 411);
    }

    #[tokio::test]
    async fn cross_origin_writes_are_refused_and_reads_are_not() {
        let cross = post("{}")
            .header("content-type", "application/json")
            .header("origin", "http://evil.example")
            .header("sec-fetch-site", "cross-site");
        assert_eq!(status(cross).await, 403);
        let same = post("{}")
            .header("content-type", "application/json")
            .header("origin", "http://127.0.0.1:8080");
        assert_eq!(status(same).await, 200);

        let read = same_origin_writes(AllowedOrigins::default()).map(|| "read");
        let get = warp::test::request()
            .path("/")
            .header("origin", "http://evil.example")
            .reply(&read)
            .await;
        assert_eq!(
            get.status(),
            200,
            "reads are left to the same-origin policy"
        );

        let any = same_origin(AllowedOrigins::default())
            .map(|| "ws")
            .recover(crate::auth::handle_auth_rejection);
        let get = warp::test::request()
            .path("/")
            .header("origin", "http://evil.example")
            .reply(&any)
            .await;
        assert_eq!(get.status(), 403, "same_origin checks every method");
    }
}
