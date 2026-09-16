use axum::{
    Extension, Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use futures_util::TryStreamExt;
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::{AuthUser, issue_token};
use crate::error::AppError;
use crate::models::{AlertPrefs, JobDoc, UserDoc};
use crate::services::{add_tokens, credit_json, get_or_create_credit};
use crate::state::AppState;
use crate::util::{nairobi_today, now_iso, uuid_id};

use super::ApiResult;
use super::jobs::queue_matches_for_user;

/// How many notifications one poll returns. A batch rather than the whole backlog,
/// so a long absence does not become one enormous message.
const DELIVERY_BATCH: i64 = 10;

/// How many matches one refresh may queue per subscriber.
const MATCHES_PER_REFRESH: i64 = 5;

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/bot/session", post(session))
        .route("/internal/matches/refresh", post(refresh_matches))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/bot/alerts", get(get_alerts).put(set_alerts))
        .route("/bot/jobs/{id}/not-a-fit", post(not_a_fit))
        .route("/bot/notifications", get(pending_notifications))
        .route("/bot/notifications/{id}/sent", post(mark_sent))
}

#[derive(Deserialize)]
pub struct BotSessionRequest {
    pub telegram_user_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
}

/// Identity for the bot, which keeps no database of its own: it hands over the
/// Telegram user id and gets back a jobify account plus a token.
///
/// Deliberately idempotent, and it returns a fresh token every call — so the bot can
/// re-call it when a token expires instead of persisting credentials anywhere.
async fn session(State(state): State<AppState>, Json(body): Json<BotSessionRequest>) -> ApiResult {
    let telegram_id = body.telegram_user_id.trim().to_string();
    if telegram_id.is_empty() {
        return Err(AppError::BadRequest(
            "telegram_user_id is required".to_string(),
        ));
    }

    let existing = state
        .users()
        .find_one(doc! { "telegram_chat_id": &telegram_id })
        .await?;

    let (user, created) = match existing {
        Some(user) => (user, false),
        None => {
            let name = body
                .name
                .clone()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| body.username.clone())
                .unwrap_or_else(|| format!("Telegram {telegram_id}"));
            let user = UserDoc {
                id: uuid_id(),
                name,
                // A synthetic address: the account exists so there is someone to bill
                // and match for, not so anybody can log in with a password.
                email: format!("tg-{telegram_id}@bot.jobify.local"),
                password_hash: String::new(),
                role: "user".to_string(),
                location: None,
                match_profile: None,
                telegram_chat_id: Some(telegram_id.clone()),
                alerts: Some(AlertPrefs {
                    enabled: false,
                    mode: "daily".to_string(),
                    ..Default::default()
                }),
                created_at: now_iso(),
            };
            state.users().insert_one(&user).await?;
            (user, true)
        }
    };

    // A bot user has no way to pay, so with no grant the first interview is refused
    // with a 402 and the flow dies at step one. Configurable, and applied ONLY on
    // creation: granting on every session call would be an infinite tap.
    let mut granted = 0u64;
    if created && state.config.bot_signup_grant_tokens > 0 {
        granted = state.config.bot_signup_grant_tokens;
        add_tokens(&state, &user.id, granted).await?;
    }

    let token = issue_token(
        &state.config.jwt_secret,
        &state.config.jwt_issuer,
        &user.id,
        &user.role,
    )?;
    let credit = get_or_create_credit(&state, &user.id).await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "token": token,
                "created": created,
                "grantedTokens": granted,
                "user": {
                    "id": user.id,
                    "name": user.name,
                    "role": user.role,
                    "matchProfile": user.match_profile,
                    "alerts": user.alerts,
                },
                "credits": credit_json(&credit),
            }
        })),
    ))
}

#[derive(Deserialize)]
pub struct AlertsRequest {
    pub enabled: bool,
    #[serde(default)]
    pub mode: Option<String>,
}

async fn get_alerts(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let user = state
        .users()
        .find_one(doc! { "_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": user.alerts.unwrap_or_default() })),
    ))
}

async fn set_alerts(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Json(body): Json<AlertsRequest>,
) -> ApiResult {
    // Only two modes exist; anything else would silently never fire on the daily
    // path, so an unknown value is rejected rather than stored.
    let mode = body.mode.unwrap_or_else(|| "daily".to_string());
    if mode != "daily" && mode != "instant" {
        return Err(AppError::BadRequest(
            "mode must be \"daily\" or \"instant\"".to_string(),
        ));
    }

    // Enabling clears the marker so today's run can fire; disabling stamps it so a
    // later re-enable does not immediately fire a stale batch.
    let last_sent_at = if body.enabled {
        String::new()
    } else {
        now_iso()
    };

    state
        .users()
        .update_one(
            doc! { "_id": &auth.id },
            doc! {
                "$set": {
                    "alerts.enabled": body.enabled,
                    "alerts.mode": &mode,
                    "alerts.last_sent_at": &last_sent_at,
                }
            },
        )
        .await?;

    let user = state
        .users()
        .find_one(doc! { "_id": &auth.id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": user.alerts.unwrap_or_default() })),
    ))
}

#[derive(Deserialize)]
pub struct NotAFitRequest {
    /// "employer" (the default) or "category".
    #[serde(default)]
    pub scope: Option<String>,
}

/// Records a rejection. Stored on the user rather than inferred, so the signal is
/// visible, reversible, and applied by the matcher as a query filter.
async fn not_a_fit(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(job_id): Path<String>,
    body: Option<Json<NotAFitRequest>>,
) -> ApiResult {
    let scope = body
        .and_then(|Json(body)| body.scope)
        .unwrap_or_else(|| "employer".to_string());

    let job = state
        .jobs()
        .find_one(doc! { "_id": &job_id })
        .await?
        .ok_or_else(|| AppError::NotFound("Job not found".to_string()))?;

    let (field, value) = match scope.as_str() {
        "category" => ("excluded_categories", job.category.clone()),
        _ => ("excluded_employers", Some(job.organization.clone())),
    };
    let value = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest(format!("This job has no {scope} to exclude")))?;

    state
        .users()
        .update_one(
            doc! { "_id": &auth.id },
            doc! {
                "$addToSet": { format!("alerts.{field}"): &value },
                "$set": { "alerts.enabled": true, "alerts.mode": "daily" },
            },
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "scope": scope, "excluded": value } })),
    ))
}

/// What the bot has not delivered yet. `read` is not this: the user never presses
/// "read" in a chat, so delivery has to be tracked separately or the bot either
/// repeats itself or drops matches.
async fn pending_notifications(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
) -> ApiResult {
    let mut cursor = state
        .notifications()
        .find(doc! { "user_id": &auth.id, "sent_at": null, "job_id": { "$exists": true } })
        .sort(doc! { "created_at": 1 })
        .limit(DELIVERY_BATCH)
        .await?;

    let mut data: Vec<Value> = Vec::new();
    while let Some(notification) = cursor.try_next().await? {
        // The notification carries only a title and body; the card the user sees needs
        // the job itself (location, closing date, category), so it rides along and the
        // bot needs no second round trip.
        let job: Option<JobDoc> = match notification.job_id.as_ref() {
            Some(job_id) => state.jobs().find_one(doc! { "_id": job_id }).await?,
            None => None,
        };
        data.push(json!({ "notification": notification, "job": job }));
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": { "total": data.len(), "batch": DELIVERY_BATCH }
        })),
    ))
}

async fn mark_sent(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let result = state
        .notifications()
        .update_one(
            doc! { "_id": &id, "user_id": &auth.id },
            doc! { "$set": { "sent_at": now_iso(), "read": true } },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Notification not found".to_string()));
    }
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "id": id } })),
    ))
}

/// Queues matches for every subscriber who is due, then the bot delivers what is
/// pending. The split matters: WHO and WHAT to notify is decided here, in one place,
/// so the daily cap and the dedupe cannot be circumvented by a second caller.
async fn refresh_matches(State(state): State<AppState>) -> ApiResult {
    let today = nairobi_today();
    let mut cursor = state
        .users()
        .find(doc! { "alerts.enabled": true, "telegram_chat_id": { "$exists": true } })
        .await?;

    let mut subscribers: Vec<UserDoc> = Vec::new();
    while let Some(user) = cursor.try_next().await? {
        subscribers.push(user);
    }

    let mut queued_total = 0u64;
    let mut results: Vec<Value> = Vec::new();
    for user in &subscribers {
        let prefs = user.alerts.clone().unwrap_or_default();
        // Daily runs at most once per Nairobi day. Instant is bounded by how often the
        // caller polls rather than by a stored timestamp.
        let due = if prefs.mode == "instant" {
            true
        } else {
            !prefs.last_sent_at.starts_with(&today)
        };
        if !due {
            results
                .push(json!({ "userId": user.id, "queued": 0, "skipped": "already sent today" }));
            continue;
        }

        let queued = queue_matches_for_user(&state, &user.id, MATCHES_PER_REFRESH).await?;
        queued_total += queued;
        state
            .users()
            .update_one(
                doc! { "_id": &user.id },
                doc! { "$set": { "alerts.last_sent_at": now_iso() } },
            )
            .await?;
        results.push(json!({ "userId": user.id, "queued": queued }));
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "subscribers": subscribers.len(),
                "queuedTotal": queued_total,
                "users": results,
            }
        })),
    ))
}
