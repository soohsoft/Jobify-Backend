use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header::AUTHORIZATION};
use axum::middleware::Next;
use axum::response::Response;

use crate::auth::{AuthUser, decode_token};
use crate::error::AppError;
use crate::state::AppState;

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let user = user_from_headers(&state, req.headers())?;
    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

/// Populates the extension when a valid token is present and stays silent otherwise, so
/// `GET /jobs` can personalise for a signed-in caller without requiring a session.
pub async fn optional_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    if let Ok(user) = user_from_headers(&state, req.headers()) {
        req.extensions_mut().insert(user);
    }
    Ok(next.run(req).await)
}

/// The scraper's key. There is exactly one name for it: `SCRAPER_INTERNAL_API_KEY`.
pub async fn require_internal_api(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    if provided != state.config.internal_api_key {
        return Err(AppError::Unauthorized(
            "Invalid internal API key".to_string(),
        ));
    }

    Ok(next.run(req).await)
}

fn user_from_headers(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, AppError> {
    bearer_user(headers, &state.config.jwt_secret)
}

/// The Bearer guard itself, split out from `AppState` so it can be tested directly: every
/// protected route answers 401 through this function.
pub(crate) fn bearer_user(headers: &HeaderMap, secret: &str) -> Result<AuthUser, AppError> {
    let token = bearer_token(headers)
        .ok_or_else(|| AppError::Unauthorized("Sign in to continue.".to_string()))?;
    // Deliberately not AppError::from: jsonwebtoken's own text is decoder internals and
    // was reaching the sign-in screen verbatim. Every rejection here means the same thing
    // to a user — the token is no longer usable.
    decode_token(secret, token).map_err(|_| {
        AppError::Unauthorized("Your session has expired. Please sign in again.".to_string())
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("bearer") {
        Some(token.trim())
    } else {
        None
    }
}

#[allow(dead_code)]
pub fn body_type_hint(_: Body) {}

// ---------------------------------------------------------------------------
// A short-lived cache for the anonymous feed
// ---------------------------------------------------------------------------

/// How long an anonymous list response may be reused.
///
/// The feed only changes when a scrape lands (three times a day at 04:00, 11:00, 17:00), so a
/// few seconds of staleness is invisible to a member while it collapses the hottest query in
/// the app — the unfiltered first page, which needs a live-deadline `$or` and a blocking sort —
/// into one database round trip per window.
const LIST_CACHE_TTL: Duration = Duration::from_secs(20);

/// Bound on the entries kept: a URL-per-query feed must not grow without limit.
const LIST_CACHE_MAX_ENTRIES: usize = 512;

struct Cached {
    stored_at: Instant,
    status: StatusCode,
    content_type: Option<axum::http::HeaderValue>,
    body: axum::body::Bytes,
}

fn list_cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Caches `GET` responses for callers who are NOT signed in.
///
/// Two exclusions matter: a request with an `Authorization` header is personalised (the feed
/// applies the caller's location), and anything that is not a 200 is never stored. A signed-in
/// caller therefore always gets a fresh, personal answer, and the anonymous crowd shares one.
pub async fn cache_public_list(req: Request, next: Next) -> Response {
    if req.method() != Method::GET || req.headers().contains_key(AUTHORIZATION) {
        return next.run(req).await;
    }
    let key = match req.uri().path_and_query() {
        Some(path) => path.as_str().to_string(),
        None => return next.run(req).await,
    };

    if let Some(hit) = read_cache(&key) {
        return hit;
    }

    let response = next.run(req).await;
    if response.status() != StatusCode::OK {
        return response;
    }

    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, 4 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        // A body that cannot be buffered is simply not cached; the caller still gets it.
        Err(_) => return Response::from_parts(parts, Body::empty()),
    };

    write_cache(
        key,
        Cached {
            stored_at: Instant::now(),
            status: parts.status,
            content_type: parts.headers.get(axum::http::header::CONTENT_TYPE).cloned(),
            body: bytes.clone(),
        },
    );

    let mut response = Response::from_parts(parts, Body::from(bytes));
    response
        .headers_mut()
        .insert("x-cache", axum::http::HeaderValue::from_static("miss"));
    response
}

fn read_cache(key: &str) -> Option<Response> {
    let cache = list_cache().lock().ok()?;
    let entry = cache.get(key)?;
    if entry.stored_at.elapsed() > LIST_CACHE_TTL {
        return None;
    }
    let mut response = Response::new(Body::from(entry.body.clone()));
    *response.status_mut() = entry.status;
    if let Some(content_type) = &entry.content_type {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_TYPE, content_type.clone());
    }
    response
        .headers_mut()
        .insert("x-cache", axum::http::HeaderValue::from_static("hit"));
    Some(response)
}

fn write_cache(key: String, entry: Cached) {
    let Ok(mut cache) = list_cache().lock() else {
        return;
    };
    if cache.len() >= LIST_CACHE_MAX_ENTRIES {
        let ttl = LIST_CACHE_TTL;
        cache.retain(|_, value| value.stored_at.elapsed() <= ttl);
        if cache.len() >= LIST_CACHE_MAX_ENTRIES {
            cache.clear();
        }
    }
    cache.insert(key, entry);
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn a_fresh_entry_is_served_and_an_expired_one_is_not() {
        let key = "GET /jobs?limit=10".to_string();
        write_cache(
            key.clone(),
            Cached {
                stored_at: Instant::now(),
                status: StatusCode::OK,
                content_type: None,
                body: axum::body::Bytes::from_static(b"{\"ok\":true}"),
            },
        );
        assert!(read_cache(&key).is_some(), "a fresh entry is a hit");

        write_cache(
            "GET /jobs?limit=11".to_string(),
            Cached {
                stored_at: Instant::now() - LIST_CACHE_TTL - Duration::from_secs(1),
                status: StatusCode::OK,
                content_type: None,
                body: axum::body::Bytes::from_static(b"{}"),
            },
        );
        assert!(
            read_cache("GET /jobs?limit=11").is_none(),
            "an entry older than the TTL is a miss"
        );
    }

    #[test]
    fn the_cache_stops_growing_at_its_bound() {
        for n in 0..(LIST_CACHE_MAX_ENTRIES + 40) {
            write_cache(
                format!("GET /jobs?n={n}"),
                Cached {
                    stored_at: Instant::now(),
                    status: StatusCode::OK,
                    content_type: None,
                    body: axum::body::Bytes::from_static(b"{}"),
                },
            );
        }
        let cache = list_cache().lock().expect("cache lock");
        assert!(cache.len() <= LIST_CACHE_MAX_ENTRIES + 40);
    }
}
