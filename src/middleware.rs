use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, Request, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};

use crate::auth::{AuthUser, decode_token};
use crate::error::AppError;
use crate::state::AppState;

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    let user = user_from_headers(&state, req.headers())?;
    req.extensions_mut().insert(user);
    Ok(next.run(req).await)
}

pub async fn optional_auth(
    State(state): State<AppState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, AppError> {
    if let Ok(user) = user_from_headers(&state, req.headers()) {
        req.extensions_mut().insert(user);
    }
    Ok(next.run(req).await)
}

pub async fn require_internal_api(
    State(state): State<AppState>,
    req: Request<Body>,
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
    let token = bearer_token(headers)
        .ok_or_else(|| AppError::Unauthorized("Sign in to continue.".to_string()))?;
    // Deliberately not AppError::from: jsonwebtoken's own text ("Base64 error:
    // Invalid last symbol 101", "ExpiredSignature") is decoder internals, and it
    // was reaching the sign-in screen verbatim because the client displays the
    // message the server sends. Every rejection here means the same thing to a
    // user — the token is no longer usable — so they all get that one sentence.
    decode_token(&state.config.jwt_secret, token).map_err(|_| {
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
pub fn auth_user_from_extension(req: &Request<Body>) -> Option<AuthUser> {
    req.extensions().get::<AuthUser>().cloned()
}
