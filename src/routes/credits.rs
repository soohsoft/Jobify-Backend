//! Smart-query credits.
//!
//! `GET /credits` is the member's own ledger. `POST /credits/topup` is the MANUAL path
//! until the WaafiPay gateway lands: it is `x-api-key` only and takes no card details, no
//! prices and no payment reference — it adds queries to an account and says so. When a
//! gateway exists it will call the same `credits::top_up`, so nothing downstream changes.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::doc;
use serde_json::{Value, json};

use crate::assistant;
use crate::auth::AuthUser;
use crate::credits;
use crate::error::AppError;
use crate::models::CreditTopupRequest;
use crate::state::AppState;

use super::ApiResult;

pub fn protected_router() -> Router<AppState> {
    Router::new().route("/credits", get(get_credits))
}

pub fn internal_router() -> Router<AppState> {
    Router::new().route("/credits/topup", post(top_up))
}

fn snapshot_json(snapshot: &credits::Snapshot) -> Value {
    serde_json::to_value(snapshot).unwrap_or(Value::Null)
}

/// The four numbers a member needs and nothing else: what is left, what the free allowance
/// is, how much of it is spent, and when it resets.
async fn get_credits(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let snapshot = credits::snapshot(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": snapshot_json(&snapshot) })),
    ))
}

/// The manual top-up. `discord_id` is what the bot knows; `user_id` is what an operator
/// sees in the database. Exactly one identifies the account.
async fn top_up(State(state): State<AppState>, Json(body): Json<CreditTopupRequest>) -> ApiResult {
    let user_id = match body
        .user_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(user_id) => {
            if state
                .users()
                .find_one(doc! { "_id": user_id })
                .await?
                .is_none()
            {
                return Err(AppError::NotFound("User not found".to_string()));
            }
            user_id.to_string()
        }
        None => match body
            .discord_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(discord_id) => assistant::user_for_discord(&state, discord_id).await?,
            None => {
                return Err(AppError::BadRequest(
                    "send either discord_id or user_id".to_string(),
                ));
            }
        },
    };

    let snapshot = credits::top_up(&state, &user_id, body.queries).await?;
    tracing::info!(
        user_id = %user_id,
        queries = body.queries,
        balance = snapshot.balance,
        "manual credit top-up (no payment provider)"
    );

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "user_id": user_id,
                "added": body.queries,
                "balance": snapshot.balance,
                "monthly_allowance": snapshot.monthly_allowance,
                "used_this_period": snapshot.used_this_period,
                "period_end": snapshot.period_end,
                "remaining_this_period": snapshot.remaining_this_period,
            },
            "meta": {
                "payment_provider": "manual",
                "message": "Added manually — no card was charged. This is the internal path until the WaafiPay gateway is wired."
            }
        })),
    ))
}
