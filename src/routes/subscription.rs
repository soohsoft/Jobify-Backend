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
use futures_util::TryStreamExt;
use mongodb::bson::doc;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::{
    PaymentDoc, SUBSCRIPTION_ACTIVE, SUBSCRIPTION_AMOUNT_USD, SUBSCRIPTION_EXPIRED,
    SubscriptionDoc, WalletDoc,
};
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/subscription", get(get_subscription))
        .route("/subscription/start", post(start_subscription))
        // One read for the whole account page: plan, credits, the unpaid bill and the
        // ledger behind it. Three round trips for one screen invited three ideas of the
        // same account; this is the single shape the portal renders.
        .route("/billing", get(get_billing))
        .route("/wallet/auto-deduct", post(set_auto_deduct))
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

/// The member's wallet, created empty on first read so the page never shows "unknown".
async fn wallet_for(state: &AppState, user_id: &str) -> Result<WalletDoc, AppError> {
    if let Some(wallet) = state.wallets().find_one(doc! { "_id": user_id }).await? {
        return Ok(wallet);
    }
    let wallet = WalletDoc::empty_for(user_id);
    let _ = state.wallets().insert_one(&wallet).await;
    Ok(wallet)
}

/// The rule the wallet exists for: when the month is due and the balance covers it, renew
/// without asking. Returns the wallet after any charge, and whether a charge happened.
async fn auto_renew_from_wallet(
    state: &AppState,
    user_id: &str,
) -> Result<(WalletDoc, bool), AppError> {
    let mut wallet = wallet_for(state, user_id).await?;
    let subscription = subscription_for(state, user_id).await?;
    if !should_charge_wallet(&subscription.status, wallet.auto_deduct, wallet.balance_usd) {
        return Ok((wallet, false));
    }

    let now = chrono::Utc::now();
    let period_end = now + chrono::Duration::days(30);
    wallet.balance_usd = (wallet.balance_usd - SUBSCRIPTION_AMOUNT_USD).max(0.0);
    wallet.updated_at = now_iso();
    state
        .wallets()
        .update_one(
            doc! { "_id": user_id },
            doc! { "$set": { "balance_usd": wallet.balance_usd, "updated_at": &wallet.updated_at } },
        )
        .await?;

    let payment = PaymentDoc {
        id: uuid_id(),
        user_id: user_id.to_string(),
        amount_usd: SUBSCRIPTION_AMOUNT_USD,
        tokens: 0,
        status: "paid".to_string(),
        reference: format!("JOBIFY-WALLET-{}", &uuid_id()[..8]),
        provider_ref: String::new(),
        provider: Some("wallet".to_string()),
        created_at: now_iso(),
        updated_at: now_iso(),
    };
    state.payments().insert_one(&payment).await?;

    // The month is paid: any bill that was still waiting for the member is no longer owed.
    state
        .payments()
        .update_many(
            doc! { "user_id": user_id, "status": "pending" },
            doc! { "$set": { "status": "void", "updated_at": now_iso() } },
        )
        .await?;

    let mut renewed = subscription;
    renewed.status = SUBSCRIPTION_ACTIVE.to_string();
    renewed.period_start = Some(now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    renewed.period_end = Some(period_end.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    renewed.amount_usd = SUBSCRIPTION_AMOUNT_USD;
    renewed.auto_renew = true;
    renewed.provider = Some("wallet".to_string());
    state
        .subscriptions()
        .replace_one(doc! { "_id": user_id }, &renewed)
        .upsert(true)
        .await?;

    tracing::info!(user_id = %user_id, "subscription renewed from wallet balance");
    Ok((wallet, true))
}

/// The whole auto-deduct rule, as a pure decision: charge only when the month is actually due,
/// the switch is on, and the balance covers it (a cent short is not a payment).
fn should_charge_wallet(status: &str, auto_deduct: bool, balance_usd: f64) -> bool {
    let due = status == SUBSCRIPTION_EXPIRED || status == "none";
    due && auto_deduct && balance_usd + 1e-9 >= SUBSCRIPTION_AMOUNT_USD
}

/// The auto-deduct switch on the account page.
async fn set_auto_deduct(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult {
    let enabled = body
        .get("enabled")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut wallet = wallet_for(&state, &user.id).await?;
    wallet.auto_deduct = enabled;
    wallet.updated_at = now_iso();
    state
        .wallets()
        .replace_one(doc! { "_id": &user.id }, &wallet)
        .upsert(true)
        .await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "auto_deduct": wallet.auto_deduct } })),
    ))
}

/// `GET /billing` — everything the account page shows, in one answer.
///
/// `history` is the payments ledger newest-first: what was charged, its reference and
/// whether it settled. `pending` is the one unpaid row the member can act on, or null.
async fn get_billing(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    // The wallet rule runs on read: a member who left auto-deduct on is renewed the moment
    // they look at the page, not only when a background job happens to run.
    let (wallet, charged) = auto_renew_from_wallet(&state, &user.id).await?;
    let subscription = subscription_for(&state, &user.id).await?;
    let credits = crate::credits::snapshot(&state, &user.id).await?;

    let mut cursor = state
        .payments()
        .find(doc! { "user_id": &user.id })
        .sort(doc! { "created_at": -1 })
        .limit(24)
        .await?;
    let mut history: Vec<Value> = Vec::new();
    let mut pending: Option<Value> = None;
    while let Some(payment) = cursor.try_next().await? {
        // Money OUT: a subscription charge, negative in the ledger.
        let row = json!({
            "id": payment.id,
            "reference": payment.reference,
            "amount_usd": payment.amount_usd,
            "kind": "subscription",
            "status": payment.status,
            "provider": payment.provider,
            "created_at": payment.created_at,
        });
        if pending.is_none() && payment.status == "pending" {
            pending = Some(row.clone());
        }
        history.push(row);
    }

    // Money IN: wallet top-ups, positive in the same ledger.
    let mut txns = state
        .wallet_txns()
        .find(doc! { "user_id": &user.id })
        .sort(doc! { "created_at": -1 })
        .limit(24)
        .await?;
    while let Some(txn) = txns.try_next().await? {
        history.push(json!({
            "id": txn.id,
            "reference": txn.reference,
            "amount_usd": txn.amount_usd,
            "kind": "topup",
            "status": "paid",
            "provider": "waafipay",
            "created_at": txn.created_at,
        }));
    }

    // A bill is only "due" while the month is not covered; an active plan owes nothing.
    if subscription.status == SUBSCRIPTION_ACTIVE {
        pending = None;
    }

    // One timeline, newest first.
    history.sort_by(|a, b| {
        let left = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let right = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        right.cmp(left)
    });
    history.truncate(24);

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "subscription": payload(&subscription),
                "credits": serde_json::to_value(&credits).unwrap_or(Value::Null),
                "pending": pending,
                "history": history,
                "wallet": {
                    "balance_usd": wallet.balance_usd,
                    "auto_deduct": wallet.auto_deduct,
                    "renewed_now": charged,
                },
            },
            "meta": { "gateway_wired": false }
        })),
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
    fn the_wallet_only_pays_when_due_switched_on_and_covered() {
        assert!(should_charge_wallet("expired", true, 1.0));
        assert!(should_charge_wallet("none", true, 5.5));
        // A cent short is not a payment.
        assert!(!should_charge_wallet("expired", true, 0.99));
        // Already active: the month is not due.
        assert!(!should_charge_wallet("active", true, 10.0));
        // Switch off: nothing is taken even with money there.
        assert!(!should_charge_wallet("expired", false, 10.0));
    }

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
