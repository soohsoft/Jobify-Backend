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
