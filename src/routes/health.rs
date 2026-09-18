use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(root))
        .route("/health", get(check))
}

/// The base URL is the first thing anyone curls when they wire up a client, and a bare 404
/// with an empty body reads as "the backend is broken" rather than "there is nothing at
/// the root". This answers with what the service is and where to look next, and keeps the
/// house `{ status, data }` envelope so a client can parse it like every other response.
async fn root() -> Json<Value> {
    Json(json!({
        "status": "success",
        "data": {
            "service": "jobify-backend",
            "status": "ok",
            "health": "/health",
            "public": ["/jobs", "/jobs/{id}", "/resumes/templates", "/auth/register", "/auth/login"],
            "authenticated": ["/auth/me", "/chats", "/resumes", "/credits", "/jobs/saved", "/jobs/match", "/alerts", "/me/memory"],
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
