use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(check))
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
