use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch},
};
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::json;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::state::AppState;

use super::ApiResult;
use super::common::paginate;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(list))
        .route("/notifications/unread-count", get(unread_count))
        .route("/notifications/read-all", patch(read_all))
        .route("/notifications/:id/read", patch(read_one))
}

#[derive(Deserialize)]
pub struct NotificationsQuery {
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_page() -> u64 {
    1
}
fn default_limit() -> u64 {
    30
}

async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<NotificationsQuery>,
) -> ApiResult {
    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.notifications(),
        doc! { "user_id": &user.id },
        page,
        limit,
        doc! { "created_at": -1 },
    )
    .await?;

    let total_pages = (total as f64 / limit as f64).ceil() as u64;
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": { "page": page, "limit": limit, "total": total, "totalPages": total_pages }
        })),
    ))
}

async fn unread_count(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let count = state
        .notifications()
        .count_documents(doc! { "user_id": &user.id, "read": false })
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "count": count } })),
    ))
}

async fn read_all(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let result = state
        .notifications()
        .update_many(
            doc! { "user_id": &user.id, "read": false },
            doc! { "$set": { "read": true } },
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "updated": result.modified_count } })),
    ))
}

async fn read_one(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let result = state
        .notifications()
        .update_one(
            doc! { "_id": &id, "user_id": &user.id },
            doc! { "$set": { "read": true } },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Notification not found".to_string()));
    }

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "id": id, "read": true } })),
    ))
}
