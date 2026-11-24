use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, header::AUTHORIZATION};
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
