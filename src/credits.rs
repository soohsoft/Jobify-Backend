//! The smart-query ledger.
//!
//! Units are QUERIES, not tokens. A smart question costs exactly one, taken from the free
//! monthly allowance first and from the purchased balance second; when both are empty the
//! caller gets a 402 with `out_of_credits`, which is the signal the bot turns into a Buy
//! Credits button.
//!
//! Every read and write is keyed on `user_id`, never on `_id`: the nine legacy documents
//! in the live `credits` collection are keyed by a string uuid, and a row created by a raw
//! upsert would carry an ObjectId. Both shapes answer a `user_id` filter, and this module
//! deliberately never deserializes `_id`, so neither shape can break it.

use serde::Serialize;

use crate::error::AppError;
use crate::models::CreditDoc;
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

/// The allowance window. 30 days, matching `SUBSCRIPTION_MONTHLY_SMART_QUERIES`.
pub const PERIOD_DAYS: i64 = 30;

/// The exact sentence the bot shows with the Buy Credits action. One constant so the
/// client, the bot and the tests cannot drift.
pub const OUT_OF_CREDITS_MESSAGE: &str =
    "You have used your free queries this month. Buy more credits to keep going.";

/// What `GET /credits` returns, and what every assistant answer carries so the bot can
/// render the remaining count without a second call.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    /// Purchased queries. Survives a period rollover.
    pub balance: i64,
    pub monthly_allowance: i64,
    pub used_this_period: i64,
    /// RFC3339, end of the current 30-day allowance period.
    pub period_end: String,
    /// `monthly_allowance - used_this_period`, never negative.
    pub remaining_this_period: i64,
}

/// Which bucket one charge landed in. Carried so a failed request refunds the bucket it
/// actually took from rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    Allowance,
    Balance,
}

impl Charge {
    pub fn bucket(self) -> &'static str {
        match self {
            Charge::Allowance => "allowance",
            Charge::Balance => "balance",
        }
    }
}

/// The 30-day window starting now.
fn period_bounds(now: chrono::DateTime<chrono::Utc>) -> (String, String) {
    let start = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let end = (now + chrono::Duration::days(PERIOD_DAYS))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    (start, end)
}

fn is_future(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return false;
    };
    match chrono::DateTime::parse_from_rfc3339(value) {
        Ok(end) => end.with_timezone(&chrono::Utc) > chrono::Utc::now(),
        Err(_) => false,
    }
}

/// Read the ledger, creating it or rolling the period over as needed.
///
/// A legacy row has neither `period_end` nor `allowance_used`, so "no period yet" and
/// "the period ended" take the same branch: they start a fresh 30-day window. Only
/// `allowance_used` is reset — the purchased `queries` balance and the legacy `tokens`
/// field are left alone.
pub async fn load_or_create(state: &AppState, user_id: &str) -> Result<CreditDoc, AppError> {
    let now = chrono::Utc::now();
    let (start, end) = period_bounds(now);

    if let Some(mut credit) = state
        .credits()
        .find_one(mongodb::bson::doc! { "user_id": user_id })
        .await?
    {
        if !is_future(credit.period_end.as_deref()) {
            state
                .credits()
                .update_one(
                    mongodb::bson::doc! { "user_id": user_id },
                    mongodb::bson::doc! { "$set": {
                        "period_start": &start,
                        "period_end": &end,
                        "allowance_used": 0i64,
                        "updated_at": now_iso(),
                    } },
                )
                .await?;
            credit.period_start = start;
            credit.period_end = Some(end);
            credit.allowance_used = 0;
        }
        return Ok(credit);
    }

    // `$setOnInsert` rather than `insert_one`, so two concurrent first questions cannot
    // both create a row; the filter is `user_id`, the identity both _id shapes share.
    state
        .credits()
        .update_one(
            mongodb::bson::doc! { "user_id": user_id },
            mongodb::bson::doc! { "$setOnInsert": {
                "_id": uuid_id(),
                "user_id": user_id,
                "queries": 0i64,
                "allowance_used": 0i64,
                "period_start": &start,
                "period_end": &end,
                "created_at": now_iso(),
                "updated_at": now_iso(),
            } },
        )
        .upsert(true)
        .await?;

    Ok(CreditDoc {
        user_id: user_id.to_string(),
        queries: 0,
        allowance_used: 0,
        period_start: start,
        period_end: Some(end),
        created_at: String::new(),
        updated_at: String::new(),
    })
}

pub async fn snapshot(state: &AppState, user_id: &str) -> Result<Snapshot, AppError> {
    let credit = load_or_create(state, user_id).await?;
    let allowance = state.config.monthly_smart_queries;
    Ok(Snapshot {
        balance: credit.queries.max(0),
        monthly_allowance: allowance,
        used_this_period: credit.allowance_used.max(0),
        period_end: credit.period_end.unwrap_or_default(),
        remaining_this_period: (allowance - credit.allowance_used).max(0),
    })
}

/// Take one query: the free allowance first, then the balance.
///
/// Both decrements are guarded `$inc`s whose filter carries the precondition, so two
/// concurrent requests cannot spend the same last query — the loser's update matches
/// nothing and falls through to the next bucket (or to the 402).
pub async fn charge(state: &AppState, user_id: &str) -> Result<Charge, AppError> {
    // Ensures the document exists and the period fields are present before a `$lt` filter
    // is used: a missing field is neither less than nor greater than the allowance.
    load_or_create(state, user_id).await?;
    let allowance = state.config.monthly_smart_queries;
    let now = now_iso();

    let from_allowance = state
        .credits()
        .update_one(
            mongodb::bson::doc! { "user_id": user_id, "allowance_used": { "$lt": allowance } },
            mongodb::bson::doc! { "$inc": { "allowance_used": 1i64 }, "$set": { "updated_at": &now } },
        )
        .await?;
    if from_allowance.matched_count > 0 {
        return Ok(Charge::Allowance);
    }

    let from_balance = state
        .credits()
        .update_one(
            mongodb::bson::doc! { "user_id": user_id, "queries": { "$gt": 0 } },
            mongodb::bson::doc! { "$inc": { "queries": -1i64 }, "$set": { "updated_at": &now } },
        )
        .await?;
    if from_balance.matched_count > 0 {
        return Ok(Charge::Balance);
    }

    Err(AppError::OutOfCredits(OUT_OF_CREDITS_MESSAGE.to_string()))
}

/// Put a charge back after a failed request. Refunds the bucket that was actually debited;
/// a refund failure is logged and swallowed, because the caller is already being told the
/// request failed and nothing good comes from turning that into a different error.
pub async fn refund(state: &AppState, user_id: &str, charge: Charge) {
    let result = match charge {
        Charge::Allowance => {
            state
                .credits()
                .update_one(
                    mongodb::bson::doc! { "user_id": user_id, "allowance_used": { "$gt": 0 } },
                    mongodb::bson::doc! { "$inc": { "allowance_used": -1i64 }, "$set": { "updated_at": now_iso() } },
                )
                .await
        }
        Charge::Balance => {
            state
                .credits()
                .update_one(
                    mongodb::bson::doc! { "user_id": user_id },
                    mongodb::bson::doc! { "$inc": { "queries": 1i64 }, "$set": { "updated_at": now_iso() } },
                )
                .await
        }
    };
    if let Err(err) = result {
        tracing::error!(user_id, bucket = charge.bucket(), error = %err, "credit refund failed");
    }
}

/// The manual top-up path until a payment gateway exists. Adds purchased queries; it does
/// not touch the allowance or the period.
pub async fn top_up(state: &AppState, user_id: &str, queries: i64) -> Result<Snapshot, AppError> {
    if queries <= 0 {
        return Err(AppError::BadRequest(
            "queries must be a positive number".to_string(),
        ));
    }
    if queries > 100_000 {
        return Err(AppError::BadRequest(
            "queries must be 100000 or fewer".to_string(),
        ));
    }
    load_or_create(state, user_id).await?;
    state
        .credits()
        .update_one(
            mongodb::bson::doc! { "user_id": user_id },
            mongodb::bson::doc! {
                "$inc": { "queries": queries },
                "$set": { "updated_at": now_iso() },
            },
        )
        .await?;
    snapshot(state, user_id).await
}
