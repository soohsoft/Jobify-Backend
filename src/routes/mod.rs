use axum::http::HeaderValue;
use axum::{Router, middleware as axum_middleware};
use tower_http::cors::CorsLayer;

use crate::middleware;
use crate::state::AppState;

mod alerts;
mod assistant;
// `pub(crate)` for the profile caps: the onboarding translation validates against the SAME
// keyword/term limits the portal's `PUT /me/match-profile` enforces.
pub(crate) mod auth;
mod categories;
mod common;
mod credits;
mod discord;
mod health;
// `pub(crate)` because the assistant reuses the ONE live-job visibility rule and the ONE
// subscription reader rather than keeping copies of either.
pub(crate) mod jobs;
mod notifications;
pub(crate) mod subscription;

pub type ApiResult =
    Result<(axum::http::StatusCode, axum::Json<serde_json::Value>), crate::error::AppError>;

pub fn app(state: AppState) -> Router {
    let cors = cors_layer(&state.config.cors_origin);

    // The taxonomy and the feed are public and identical for everyone signed out, so they sit
    // behind one layer pair: optional auth (a signed-in caller's location is applied) and a
    // few-second cache for the anonymous crowd.
    let feed = Router::new()
        .merge(categories::router())
        .merge(jobs::public_router())
        .route_layer(axum_middleware::from_fn(middleware::cache_public_list))
        .route_layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::optional_auth,
        ));

    let public = Router::new()
        .merge(health::router())
        .merge(auth::public_router())
        .merge(feed);

    let protected = Router::new()
        .merge(auth::verification_router())
        .merge(auth::protected_router())
        .merge(jobs::protected_router())
        .merge(alerts::protected_router())
        .merge(notifications::router())
        .merge(subscription::protected_router())
        .merge(credits::protected_router())
        .merge(assistant::protected_router())
        .route_layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::require_auth,
        ));

    let internal = Router::new()
        .merge(jobs::internal_router())
        .merge(alerts::internal_router())
        .merge(discord::internal_router())
        .merge(credits::internal_router())
        .merge(assistant::internal_router())
        .route_layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::require_internal_api,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .merge(internal)
        .layer(cors)
        .with_state(state)
}

fn cors_layer(origin: &str) -> CorsLayer {
    if origin == "*" {
        CorsLayer::permissive()
    } else {
        let header = origin
            .parse::<HeaderValue>()
            .unwrap_or_else(|_| HeaderValue::from_static("*"));
        CorsLayer::new().allow_origin(header)
    }
}
