use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::json;

use crate::auth::AuthUser;
use crate::services::{credit_json, get_or_create_credit};
use crate::state::AppState;

use super::ApiResult;
use super::common::paginate;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/credits", get(balance))
        .route("/credits/usage", get(usage))
}

#[derive(Deserialize)]
pub struct UsageQuery {
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 {
    30
}

async fn balance(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> ApiResult {
    let credit = get_or_create_credit(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": credit_json(&credit) })),
    ))
}

async fn usage(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<UsageQuery>,
) -> ApiResult {
    let limit = query.limit.clamp(1, 200);
    let (data, _total) = paginate(
        &state.token_usage(),
        doc! { "user_id": &user.id },
        1,
        limit,
        doc! { "date": -1 },
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": data })),
    ))
}
