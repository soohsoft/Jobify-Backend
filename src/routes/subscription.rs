//! The $1/month subscription: one row per user, plus the honest "pending" payment row
//! that a future gateway phase will settle.
//!
//! Nothing in this module gates the feed, the alerts or the matcher. Whether a listing or
//! a match requires a live subscription is still an open product decision, so this phase
//! only records and reports state.

use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::doc;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::{
    PaymentDoc, SUBSCRIPTION_ACTIVE, SUBSCRIPTION_AMOUNT_USD, SUBSCRIPTION_EXPIRED, SubscriptionDoc,
};
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/subscription", get(get_subscription))
        .route("/subscription/start", post(start_subscription))
}

/// The caller's subscription, normalized for reading.
///
/// Absent means `none` — not a 404: every account has a subscription *state*, and most
/// accounts have never had a row. A stored `active` whose period has already ended is
/// reported as `expired` here rather than waiting for a job to flip it, so a client never
/// has to do date arithmetic to know whether the month is current.
pub async fn subscription_for(
    state: &AppState,
    user_id: &str,
) -> Result<SubscriptionDoc, AppError> {
    let Some(mut subscription) = state
        .subscriptions()
        .find_one(doc! { "_id": user_id })
        .await?
    else {
        return Ok(SubscriptionDoc::none_for(user_id));
    };

    if subscription.status == SUBSCRIPTION_ACTIVE
        && let Some(end) = subscription.period_end.as_deref()
        && let Ok(end) = chrono::DateTime::parse_from_rfc3339(end)
        && end.with_timezone(&chrono::Utc) <= chrono::Utc::now()
    {
        subscription.status = SUBSCRIPTION_EXPIRED.to_string();
    }

    Ok(subscription)
}

/// The shape both `GET /subscription` and the `POST /auth/magic` session use, so the portal
/// cannot show two different ideas of the same subscription.
pub fn payload(subscription: &SubscriptionDoc) -> Value {
    json!({
        "status": subscription.status,
        "period_start": subscription.period_start,
        "period_end": subscription.period_end,
        "amount_usd": subscription.amount_usd,
        "auto_renew": subscription.auto_renew,
        "provider": subscription.provider,
        "days_left": days_left(subscription.period_end.as_deref()),
    })
}

/// Whole days remaining, rounded up: an hour left is still "1 day", and nothing left is 0
/// rather than a negative number a client would render as "-3 days".
fn days_left(period_end: Option<&str>) -> i64 {
    let Some(end) = period_end.and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
    else {
        return 0;
    };
    let seconds = (end.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
    if seconds <= 0 {
        0
    } else {
        (seconds + 86_399) / 86_400
    }
}

async fn get_subscription(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let subscription = subscription_for(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": payload(&subscription) })),
    ))
}

/// Creates (or returns) the pending $1.00 manual payment for this account.
///
/// There is no gateway in this phase, and this endpoint says so rather than pretending:
/// it records what would be charged, leaves the subscription untouched, and answers 200
/// with `meta.gateway_wired: false`. A 501 would be more literal but a client cannot tell
/// "not built yet" from "broken" — the flag is checkable, the status code is ambiguous.
/// Repeating the call reuses the same pending row instead of stacking duplicates.
async fn start_subscription(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let existing = state
        .payments()
        .find_one(doc! {
            "user_id": &user.id,
            "status": "pending",
            "provider": "manual",
            "amount_usd": SUBSCRIPTION_AMOUNT_USD,
        })
        .await?;

    let payment = match existing {
        Some(payment) => payment,
        None => {
            let now = now_iso();
            let reference = format!(
                "JOBIFY-SUB-{}-{}",
                chrono::Utc::now().timestamp_millis(),
                &uuid_id()[..8]
            );
            let payment = PaymentDoc {
                id: uuid_id(),
                user_id: user.id.clone(),
                amount_usd: SUBSCRIPTION_AMOUNT_USD,
                // A subscription buys access, not tokens: the legacy `tokens` field is
                // present for shape compatibility and zero because nothing is credited.
                tokens: 0,
                status: "pending".to_string(),
                reference: reference.clone(),
                provider_ref: reference,
                provider: Some("manual".to_string()),
                created_at: now.clone(),
                updated_at: now,
            };
            state.payments().insert_one(&payment).await?;
            payment
        }
    };

    let subscription = subscription_for(&state, &user.id).await?;
    tracing::info!(
        user_id = %user.id,
        payment_id = %payment.id,
        "subscription checkout requested with no gateway wired"
    );

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "payment_id": payment.id,
                "amount_usd": payment.amount_usd,
                "currency": "USD",
                "provider": "manual",
                "payment_status": payment.status,
                "checkout_url": Value::Null,
                "subscription": payload(&subscription),
            },
            "meta": {
                "gateway_wired": false,
                "message": "No payment gateway is wired yet. The $1.00 charge is recorded as a pending manual payment; the subscription is not activated by this endpoint."
            }
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_left_rounds_up_and_never_goes_negative() {
        let in_90_minutes = (chrono::Utc::now() + chrono::Duration::minutes(90))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert_eq!(days_left(Some(&in_90_minutes)), 1);

        let in_three_days = (chrono::Utc::now() + chrono::Duration::days(3))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert_eq!(days_left(Some(&in_three_days)), 3);

        let yesterday = (chrono::Utc::now() - chrono::Duration::days(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert_eq!(days_left(Some(&yesterday)), 0);

        assert_eq!(days_left(None), 0);
        assert_eq!(days_left(Some("not a date")), 0);
    }

    #[test]
    fn an_account_with_no_row_reports_none_and_one_dollar() {
        let subscription = SubscriptionDoc::none_for("user-1");
        let value = payload(&subscription);
        assert_eq!(value["status"], "none");
        assert_eq!(value["amount_usd"], 1.0);
        assert_eq!(value["days_left"], 0);
        assert!(value["auto_renew"].as_bool() == Some(false));
    }
}
