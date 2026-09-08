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

pub async fn ensure_credit(state: &AppState, user_id: &str) -> Result<CreditDoc, AppError> {
    let credit = get_or_create_credit(state, user_id).await?;
    if credit.tokens == 0 {
        return Err(AppError::PaymentRequired(
            "Insufficient credit. Please top up your balance to continue.".to_string(),
        ));
    }
    Ok(credit)
}

pub fn credit_json(credit: &CreditDoc) -> serde_json::Value {
    serde_json::json!({
        "balanceUsd": tokens_to_usd(credit.tokens),
        "tokens": credit.tokens,
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
        title: title.to_string(),
        body: body.to_string(),
        read: false,
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
}
