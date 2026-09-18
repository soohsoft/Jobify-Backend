use axum::http::HeaderValue;
use axum::{Router, middleware as axum_middleware};
use tower_http::cors::CorsLayer;

use crate::middleware;
use crate::state::AppState;

mod alerts;
mod auth;
mod chats;
mod common;
mod credits;
mod health;
mod jobs;
mod notifications;
mod payments;
mod resumes;

pub type ApiResult =
    Result<(axum::http::StatusCode, axum::Json<serde_json::Value>), crate::error::AppError>;

pub fn app(state: AppState) -> Router {
    let cors = cors_layer(&state.config.cors_origin);

    let public = Router::new()
        .merge(health::router())
        .merge(auth::public_router())
        .merge(
            jobs::public_router().route_layer(axum_middleware::from_fn_with_state(
                state.clone(),
                middleware::optional_auth,
            )),
        )
        .merge(resumes::public_router())
        .merge(payments::public_router());

    let protected = Router::new()
        .merge(auth::verification_router())
        .merge(auth::protected_router())
        .merge(chats::router())
        .merge(resumes::protected_router())
        .merge(credits::router())
        .merge(payments::protected_router())
        .merge(notifications::router())
        .merge(jobs::protected_router())
        .merge(alerts::protected_router())
        .route_layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::require_auth,
        ));

    let internal = Router::new()
        .merge(jobs::internal_router())
        .merge(alerts::internal_router())
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
