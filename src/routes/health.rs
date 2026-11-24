use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/health", get(check))
}

/// A bare 404 at the root reads as "the backend is broken" rather than "there is nothing
/// at the root". This answers with what the service is and where to look next, in the same
/// `{ status, data }` envelope as every other response.
async fn root() -> Json<Value> {
    Json(json!({
        "status": "success",
        "data": {
            "service": "jobify-backend",
            "status": "ok",
            "health": "/health",
            "public": ["/jobs", "/jobs/{id}", "/auth/register", "/auth/login"],
            "authenticated": ["/auth/me", "/jobs/match", "/jobs/saved", "/alerts", "/notifications"],
            "environment": std::env::var("RUST_ENV").unwrap_or_else(|_| "development".to_string()),
            "timestamp": chrono::Utc::now().to_rfc3339()
        }
    }))
}

async fn check() -> Json<Value> {
    Json(json!({
        "status": "success",
        "data": {
            "status": "ok",
            "service": "jobify-backend",
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "environment": std::env::var("RUST_ENV").unwrap_or_else(|_| "development".to_string())
        }
    }))
}
