use axum::{
    Extension, Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::json;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::{PaymentDoc, tokens_to_usd, usd_to_tokens};
use crate::services::{add_tokens, credit_json, get_or_create_credit};
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;
use super::common::paginate;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/credits", get(balance))
        .route("/credits/usage", get(usage))
        .route("/credits/topup", post(topup))
}

#[derive(Deserialize)]
pub struct UsageQuery {
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_limit() -> u64 {
    30
}

#[derive(Deserialize)]
pub struct TopupRequest {
    /// Credit this many tokens...
    #[serde(default)]
    pub tokens: Option<u64>,
    /// ...or this many dollars, converted at the billing rate. Tokens win if both are sent.
    #[serde(default)]
    pub usd: Option<f64>,
    /// Credit another account (handing a tester a balance). Omitted means the caller's own.
    #[serde(default)]
    pub email: Option<String>,
}

/// `POST /credits/topup` — add credit without a payment provider.
///
/// WaafiPay is not wired up, so without this there is no way to refill a balance at all: the
/// chat 402s and the only remedy is editing the database by hand. This is that remedy, with a
/// door on it.
///
/// Off unless `DEV_TOPUP_KEY` is set, and the key must arrive in `X-Topup-Key`. An unset key
/// makes the route 404 rather than open, so a deployment that forgets the variable is closed
/// instead of giving away balance.
async fn topup(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    headers: HeaderMap,
    Json(body): Json<TopupRequest>,
) -> ApiResult {
    let Some(expected) = state.config.dev_topup_key.as_deref() else {
        return Err(AppError::NotFound(
            "Top-up without a payment provider is not enabled on this server.".to_string(),
        ));
    };
    let given = headers
        .get("x-topup-key")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if given != expected {
        return Err(AppError::Forbidden("Invalid top-up key.".to_string()));
    }

    let tokens = match (body.tokens, body.usd) {
        (Some(tokens), _) if tokens > 0 => tokens,
        (_, Some(usd)) if usd > 0.0 => usd_to_tokens(usd),
        _ => {
            return Err(AppError::BadRequest(
                "Send a positive `tokens` or `usd` amount.".to_string(),
            ));
        }
    };

    let target = match body
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty())
    {
        Some(email) => {
            state
                .users()
                .find_one(doc! { "email": email.to_lowercase() })
                .await?
                .ok_or_else(|| AppError::NotFound(format!("No account with the email {email}.")))?
                .id
        }
        None => user.id.clone(),
    };

    let credit = add_tokens(&state, &target, tokens).await?;

    // A ledger row, so a granted balance shows up in the payment history instead of appearing
    // out of nowhere. `provider_ref: "manual"` is what marks it as not-a-payment.
    let now = now_iso();
    state
        .payments()
        .insert_one(&PaymentDoc {
            id: uuid_id(),
            user_id: target,
            amount_usd: tokens_to_usd(tokens),
            tokens,
            status: "succeeded".to_string(),
            reference: "MANUAL-TOPUP".to_string(),
            provider_ref: "manual".to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": credit_json(&credit) })),
    ))
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
