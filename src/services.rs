use mongodb::bson::doc;

use crate::error::AppError;
use crate::llm::TokenUsage;
use crate::models::{
    COST_PER_TOKEN, CreditDoc, NotificationDoc, PRICE_PER_TOKEN, tokens_to_usd, usd_to_tokens,
};
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

pub async fn get_or_create_credit(state: &AppState, user_id: &str) -> Result<CreditDoc, AppError> {
    if let Some(credit) = state
        .credits()
        .find_one(doc! { "user_id": user_id })
        .await?
    {
        return Ok(credit);
    }

    let now = now_iso();
    let credit = CreditDoc {
        id: uuid_id(),
        user_id: user_id.to_string(),
        tokens: 0,
        debt_tokens: 0,
        created_at: now.clone(),
        updated_at: now,
    };
    state.credits().insert_one(&credit).await?;
    Ok(credit)
}

pub async fn add_tokens(
    state: &AppState,
    user_id: &str,
    amount: u64,
) -> Result<CreditDoc, AppError> {
    let now = now_iso();
    let _ = state
        .credits()
        .update_one(
            doc! { "user_id": user_id },
            doc! {
                "$inc": { "tokens": amount as i64 },
                "$set": { "updated_at": &now },
                "$setOnInsert": { "_id": uuid_id(), "user_id": user_id, "created_at": &now },
            },
        )
        .upsert(true)
        .await?;

    get_or_create_credit(state, user_id).await
}

pub async fn deduct_tokens(
    state: &AppState,
    user_id: &str,
    amount: u64,
) -> Result<CreditDoc, AppError> {
    let now = now_iso();
    let result = state
        .credits()
        .update_one(
            doc! { "user_id": user_id, "tokens": { "$gte": amount as i64 } },
            doc! {
                "$inc": { "tokens": -(amount as i64) },
                "$set": { "updated_at": &now },
            },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::PaymentRequired(
            "Insufficient credit. Please top up your balance to continue.".to_string(),
        ));
    }

    get_or_create_credit(state, user_id).await
}

/// Hold `amount` tokens against the balance *before* any work is done. Returns 402
/// when the balance cannot cover it, so an unaffordable request never starts.
///
/// Paired with `settle_credit`, which refunds the unused part. Reserving instead
/// of charging up front is what makes the pre-flight check sound: the true cost of
/// a request is unknowable until the provider reports usage, but its worst case is
/// (prompt + output cap), and that is what gets held. The decrement itself is a
/// single guarded `$inc`, so two concurrent requests cannot both spend the same
/// tokens.
pub async fn reserve_credit(
    state: &AppState,
    user_id: &str,
    amount: u64,
) -> Result<CreditDoc, AppError> {
    deduct_tokens(state, user_id, amount).await
}

/// Settle a reservation against the usage the provider actually reported.
///
/// Over-reserving is the normal case, so the remainder is refunded. If the request
/// cost more than was held (the prompt estimate ran short), the difference is
/// charged. When even that cannot be covered, the balance is drained to zero and
/// the shortfall recorded as debt — the user already has the content by then, so
/// refusing the charge would mean giving it away silently.
pub async fn settle_credit(
    state: &AppState,
    user_id: &str,
    reserved: u64,
    actual: u64,
) -> Result<CreditDoc, AppError> {
    if actual < reserved {
        return add_tokens(state, user_id, reserved - actual).await;
    }
    if actual == reserved {
        return get_or_create_credit(state, user_id).await;
    }

    let extra = actual - reserved;
    match deduct_tokens(state, user_id, extra).await {
        Ok(credit) => Ok(credit),
        Err(AppError::PaymentRequired(_)) => {
            let credit = drain_credit(state, user_id).await?;
            tracing::warn!(
                user_id,
                shortfall_tokens = extra,
                "request exceeded its reservation and the balance could not cover it; \
                 balance drained to zero and shortfall recorded as debt"
            );
            Ok(credit)
        }
        Err(err) => Err(err),
    }
}

/// Move whatever balance remains into `debt_tokens`. The balance is an unsigned
/// count, so an overshoot cannot be represented as a negative number; recording it
/// separately keeps the shortfall visible instead of silently discarding it.
async fn drain_credit(state: &AppState, user_id: &str) -> Result<CreditDoc, AppError> {
    let credit = get_or_create_credit(state, user_id).await?;
    if credit.tokens == 0 {
        return Ok(credit);
    }

    let drained = credit.tokens;
    state
        .credits()
        .update_one(
            doc! { "user_id": user_id },
            doc! {
                "$inc": { "tokens": -(drained as i64), "debt_tokens": drained as i64 },
                "$set": { "updated_at": now_iso() },
            },
        )
        .await?;

    get_or_create_credit(state, user_id).await
}

pub fn credit_json(credit: &CreditDoc) -> serde_json::Value {
    serde_json::json!({
        "balanceUsd": tokens_to_usd(credit.tokens),
        "tokens": credit.tokens,
        "debtTokens": credit.debt_tokens,
        "currency": crate::models::CREDIT_CURRENCY,
    })
}

pub async fn record_usage(
    state: &AppState,
    user_id: &str,
    usage: &TokenUsage,
) -> Result<(), AppError> {
    let now = now_iso();
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let cost_usd = usage.total_tokens as f64 * COST_PER_TOKEN;
    let charged_usd = usage.total_tokens as f64 * PRICE_PER_TOKEN;

    state
        .token_usage()
        .update_one(
            doc! { "user_id": user_id, "date": &date },
            doc! {
                "$inc": {
                    "prompt_tokens": usage.prompt_tokens as i64,
                    "completion_tokens": usage.completion_tokens as i64,
                    "total_tokens": usage.total_tokens as i64,
                    // Recorded so the real cost can be reconstructed later: cache
                    // hits bill ~50x cheaper than misses, and reasoning tokens bill
                    // as output. Without these the blended rate is a guess.
                    "prompt_cache_hit_tokens": usage.prompt_cache_hit_tokens as i64,
                    "prompt_cache_miss_tokens": usage.prompt_cache_miss_tokens as i64,
                    "reasoning_tokens": usage.reasoning_tokens as i64,
                    "cost_usd": cost_usd,
                    "charged_usd": charged_usd,
                },
                "$set": { "created_at": &now },
                "$setOnInsert": { "_id": uuid_id(), "user_id": user_id, "date": &date },
            },
        )
        .upsert(true)
        .await?;

    Ok(())
}

pub async fn create_notification(
    state: &AppState,
    user_id: &str,
    notification_type: &str,
    title: &str,
    body: &str,
) -> Result<(), AppError> {
    let notification = NotificationDoc {
        id: uuid_id(),
        user_id: user_id.to_string(),
        notification_type: notification_type.to_string(),
        job_id: None,
        title: title.to_string(),
        body: body.to_string(),
        read: false,
        sent_at: None,
        failed_reason: None,
        created_at: now_iso(),
    };
    state.notifications().insert_one(&notification).await?;
    Ok(())
}

#[allow(dead_code)]
pub fn _unused_tokens_helper(usd: f64) -> u64 {
    usd_to_tokens(usd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const TEST_DB: &str = "jobify_test";

    async fn fresh_state() -> AppState {
        let mut config = Config::from_env();
        config.database_name = TEST_DB.to_string();
        let client = mongodb::Client::with_uri_str(&config.mongodb_uri)
            .await
            .expect("failed to connect to test MongoDB");
        let _ = client.database(TEST_DB).drop().await;
        let db = client.database(TEST_DB);
        AppState::new(db, config)
    }

    #[tokio::test]
    async fn add_tokens_creates_credit_and_adds_balance() {
        let state = fresh_state().await;
        let credit = add_tokens(&state, "user-a", 500).await.unwrap();
        assert_eq!(credit.tokens, 500);
        assert_eq!(credit.user_id, "user-a");
    }

    #[tokio::test]
    async fn deduct_at_zero_balance_is_payment_required() {
        let state = fresh_state().await;
        let result = deduct_tokens(&state, "user-b", 1).await;
        assert!(matches!(result, Err(AppError::PaymentRequired(_))));
        let credit = get_or_create_credit(&state, "user-b").await.unwrap();
        assert_eq!(credit.tokens, 0);
    }

    #[tokio::test]
    async fn deduct_exact_balance_reaches_zero() {
        let state = fresh_state().await;
        add_tokens(&state, "user-c", 100).await.unwrap();
        let credit = deduct_tokens(&state, "user-c", 100).await.unwrap();
        assert_eq!(credit.tokens, 0);
    }

    #[tokio::test]
    async fn overdraw_is_refused_and_balance_unchanged() {
        let state = fresh_state().await;
        add_tokens(&state, "user-d", 100).await.unwrap();
        let result = deduct_tokens(&state, "user-d", 200).await;
        assert!(matches!(result, Err(AppError::PaymentRequired(_))));
        let credit = get_or_create_credit(&state, "user-d").await.unwrap();
        assert_eq!(credit.tokens, 100);
    }

    #[tokio::test]
    async fn get_or_create_credit_is_idempotent() {
        let state = fresh_state().await;
        let first = get_or_create_credit(&state, "user-e").await.unwrap();
        let second = get_or_create_credit(&state, "user-e").await.unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.tokens, 0);
    }

    #[tokio::test]
    async fn reserve_beyond_the_balance_is_refused() {
        let state = fresh_state().await;
        add_tokens(&state, "user-u", 100).await.unwrap();
        let result = reserve_credit(&state, "user-u", 101).await;
        assert!(matches!(result, Err(AppError::PaymentRequired(_))));
    }

    #[tokio::test]
    async fn reserve_then_settle_refunds_the_unused_hold() {
        let state = fresh_state().await;
        add_tokens(&state, "user-r", 1_000).await.unwrap();

        let held = reserve_credit(&state, "user-r", 400).await.unwrap();
        assert_eq!(held.tokens, 600);

        let settled = settle_credit(&state, "user-r", 400, 150).await.unwrap();
        assert_eq!(settled.tokens, 850);
    }

    #[tokio::test]
    async fn settle_charges_an_overshoot() {
        let state = fresh_state().await;
        add_tokens(&state, "user-s", 1_000).await.unwrap();
        reserve_credit(&state, "user-s", 400).await.unwrap();

        let settled = settle_credit(&state, "user-s", 400, 550).await.unwrap();
        assert_eq!(settled.tokens, 450);
    }

    #[tokio::test]
    async fn settle_drains_to_zero_and_records_debt_when_the_overshoot_exceeds_the_balance() {
        let state = fresh_state().await;
        add_tokens(&state, "user-t", 500).await.unwrap();
        reserve_credit(&state, "user-t", 400).await.unwrap();

        // The turn cost 400 more than was held and only 100 remains. The remaining
        // 100 is drained and the rest recorded as debt, rather than refusing the
        // charge on a turn the user has already received.
        let settled = settle_credit(&state, "user-t", 400, 900).await.unwrap();
        assert_eq!(settled.tokens, 0);
        assert_eq!(settled.debt_tokens, 100);
    }
}
