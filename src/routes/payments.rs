use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::{
    InitiatePaymentRequest, MIN_TOP_UP_USD, PaymentDoc, WaafiCallback, usd_to_tokens,
};
use crate::services::{add_tokens, create_notification};
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;
use super::common::paginate;

pub fn public_router() -> Router<AppState> {
    Router::new().route("/payments/waafipay/callback", post(callback))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/payments", post(initiate).get(list))
        .route("/payments/:id", get(get_by_id))
}

#[derive(Deserialize)]
pub struct PaymentsQuery {
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

async fn initiate(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<InitiatePaymentRequest>,
) -> ApiResult {
    if body.amount_usd < MIN_TOP_UP_USD {
        return Err(AppError::BadRequest(format!(
            "Minimum top-up amount is ${MIN_TOP_UP_USD:.2}"
        )));
    }

    let amount_usd = body.amount_usd;
    let tokens = usd_to_tokens(amount_usd);
    let now = now_iso();
    let internal_ref = format!(
        "JOBIFY-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        &uuid::Uuid::new_v4().to_string()[..8]
    );

    let mut payment = PaymentDoc {
        id: uuid_id(),
        user_id: user.id.clone(),
        amount_usd,
        tokens,
        status: "pending".to_string(),
        reference: internal_ref.clone(),
        provider_ref: internal_ref.clone(),
        created_at: now.clone(),
        updated_at: now,
    };

    state.payments().insert_one(&payment).await?;

    let mut checkout_url: Option<String> = None;
    if state.config.waafi_configured() {
        let provider = call_waafi_create(&state, amount_usd, &internal_ref).await?;
        let provider_ref = ["reference", "payment_id", "id"]
            .iter()
            .find_map(|key| provider.get(*key).and_then(Value::as_str))
            .unwrap_or(&internal_ref)
            .to_string();
        checkout_url = ["checkout_url", "payment_url"]
            .iter()
            .find_map(|key| provider.get(*key).and_then(Value::as_str))
            .map(String::from);

        payment.provider_ref = provider_ref.clone();
        state
            .payments()
            .update_one(
                doc! { "_id": &payment.id },
                doc! { "$set": { "provider_ref": provider_ref, "updated_at": now_iso() } },
            )
            .await?;
    }

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "data": { "payment": payment, "checkoutUrl": checkout_url }
        })),
    ))
}

async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<PaymentsQuery>,
) -> ApiResult {
    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.payments(),
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

async fn get_by_id(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let payment = state
        .payments()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Payment not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": payment })),
    ))
}

async fn callback(State(state): State<AppState>, Json(body): Json<WaafiCallback>) -> ApiResult {
    let reference = body
        .reference
        .clone()
        .ok_or_else(|| AppError::BadRequest("reference is required".to_string()))?;

    let mut payment = state
        .payments()
        .find_one(doc! { "provider_ref": &reference })
        .await?;
    if payment.is_none() {
        payment = state
            .payments()
            .find_one(doc! { "reference": &reference })
            .await?;
    }
    let payment = payment.ok_or_else(|| AppError::NotFound("Payment not found".to_string()))?;

    if payment.status == "success" {
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "id": payment.id, "status": "success" } })),
        ));
    }

    let successful = if state.config.waafi_configured() {
        let provider_status = verify_waafi_payment(&state, &reference).await?;
        is_success_status(provider_status.as_str())
    } else {
        let callback_status = body.status.clone().unwrap_or_default();
        is_success_status(&callback_status)
    };

    let now = now_iso();
    if successful {
        state
            .payments()
            .update_one(
                doc! { "_id": &payment.id },
                doc! { "$set": { "status": "success", "updated_at": &now } },
            )
            .await?;
        add_tokens(&state, &payment.user_id, payment.tokens).await?;
        create_notification(
            &state,
            &payment.user_id,
            "payment",
            "Payment successful",
            &format!(
                "Your top-up of ${:.2} has been added to your balance.",
                payment.amount_usd
            ),
        )
        .await?;

        Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "id": payment.id, "status": "success" } })),
        ))
    } else {
        state
            .payments()
            .update_one(
                doc! { "_id": &payment.id },
                doc! { "$set": { "status": "failed", "updated_at": &now } },
            )
            .await?;

        Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "id": payment.id, "status": "failed" } })),
        ))
    }
}

async fn call_waafi_create(
    state: &AppState,
    amount_usd: f64,
    reference: &str,
) -> Result<Value, AppError> {
    let url = format!(
        "{}/v1/payments",
        state.config.waafi_api_url.trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "application/json")
        .header(
            "Authorization",
            format!("Bearer {}", state.config.waafi_api_key),
        )
        .header("X-Account-Id", &state.config.waafi_account_id)
        .json(&json!({
            "amount": amount_usd,
            "currency": "USD",
            "reference": reference,
            "callback_url": state.config.waafi_callback_url,
        }))
        .send()
        .await?;

    let status = response.status();
    let body: Value = response.json().await?;
    if !status.is_success() {
        return Err(AppError::BadGateway(format!(
            "WaafiPay payment initiation failed ({status}): {}",
            body.to_string().chars().take(300).collect::<String>()
        )));
    }

    Ok(body)
}

async fn verify_waafi_payment(state: &AppState, reference: &str) -> Result<String, AppError> {
    let url = state.config.waafi_verify_url.trim_end_matches('/');
    let response = reqwest::Client::new()
        .post(url)
        .header("Content-Type", "application/json")
        .header(
            "Authorization",
            format!("Bearer {}", state.config.waafi_api_key),
        )
        .header("X-Account-Id", &state.config.waafi_account_id)
        .json(&json!({ "reference": reference }))
        .send()
        .await?;

    let body: Value = response.json().await?;
    Ok(body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn is_success_status(status: &str) -> bool {
    ["success", "completed", "paid"]
        .iter()
        .any(|candidate| status.eq_ignore_ascii_case(candidate))
}
