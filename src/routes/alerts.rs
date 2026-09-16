use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use futures_util::TryStreamExt;
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::{NotificationDoc, UserDoc};
use crate::state::AppState;
use crate::util::{nairobi_today, now_iso};

use super::ApiResult;
use super::jobs::queue_matches_for_user;

/// How many notifications one delivery page returns, and the hard ceiling a caller may ask
/// for. A page rather than the whole backlog, so a large queue drains steadily.
const DELIVERY_BATCH: i64 = 100;
const DELIVERY_PAGE_MAX: i64 = 200;

/// How many matches one refresh may queue per subscriber.
const MATCHES_PER_REFRESH: i64 = 5;

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/matches/refresh", post(refresh_matches))
        .route("/internal/deliveries", get(deliveries))
        .route("/internal/deliveries/sent", post(deliveries_sent))
        .route("/internal/deliveries/failed", post(deliveries_failed))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/alerts", get(get_alerts).put(set_alerts))
        .route("/jobs/{id}/not-a-fit", post(not_a_fit))
}

// ---------------------------------------------------------------------------
// Delivery queue
//
// Channel-agnostic on purpose. It used to return a Telegram chat id, which tied the queue
// to one transport and — worse — forced delivery to be per-user so the bot could hold that
// user's token. It now returns a user id; whatever channel sends the notification resolves
// that user's own address (web push subscription, email, whatever comes next).
// ---------------------------------------------------------------------------

/// One page of the delivery queue.
#[derive(Deserialize)]
pub struct DeliveriesQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct DeliveryIds {
    pub ids: Vec<String>,
}

#[derive(Deserialize)]
pub struct DeliveryFailures {
    /// id -> reason, so one request can mix a permanent refusal with a transient failure.
    pub failures: Vec<DeliveryFailure>,
}

#[derive(Deserialize)]
pub struct DeliveryFailure {
    pub id: String,
    pub reason: String,
}

/// What should be sent next, across **every** subscriber.
///
/// Called with the internal key, so the sender needs no per-user credentials — which is
/// what makes a restart survivable: nothing about delivery depends on a session being in
/// some other process's memory.
async fn deliveries(
    State(state): State<AppState>,
    Query(query): Query<DeliveriesQuery>,
) -> ApiResult {
    let limit = query
        .limit
        .unwrap_or(DELIVERY_BATCH)
        .clamp(1, DELIVERY_PAGE_MAX);

    // Over-fetch: notifications belonging to users who cannot receive (alerts off) are
    // dropped below, and that must not make the page look empty.
    let mut cursor = state
        .notifications()
        .find(doc! { "sent_at": null })
        .sort(doc! { "created_at": 1 })
        .limit(limit * 3)
        .await?;

    let mut candidates: Vec<NotificationDoc> = Vec::new();
    while let Some(notification) = cursor.try_next().await? {
        candidates.push(notification);
        if candidates.len() as i64 >= limit * 3 {
            break;
        }
    }
    if candidates.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "deliveries": [], "count": 0 } })),
        ));
    }

    // One query for the recipients rather than one per notification.
    let user_ids: Vec<String> = candidates.iter().map(|n| n.user_id.clone()).collect();
    let mut reachable = std::collections::HashSet::new();
    let mut user_cursor = state
        .users()
        .find(doc! { "_id": { "$in": &user_ids } })
        .await?;
    while let Some(user) = user_cursor.try_next().await? {
        let wants_alerts = user.alerts.as_ref().map(|a| a.enabled).unwrap_or(false);
        if wants_alerts {
            reachable.insert(user.id);
        }
    }

    let page: Vec<&NotificationDoc> = candidates
        .iter()
        .filter(|n| reachable.contains(&n.user_id))
        .take(limit as usize)
        .collect();

    // One query for the jobs the cards need.
    let job_ids: Vec<String> = page
        .iter()
        .filter_map(|n| n.job_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    let mut jobs = std::collections::HashMap::new();
    if !job_ids.is_empty() {
        let mut job_cursor = state
            .jobs()
            .find(doc! { "_id": { "$in": &job_ids } })
            .await?;
        while let Some(job) = job_cursor.try_next().await? {
            jobs.insert(job.id.clone(), job);
        }
    }

    let deliveries: Vec<Value> = page
        .iter()
        .map(|n| {
            json!({
                "notificationId": &n.id,
                "userId": &n.user_id,
                "title": &n.title,
                "body": &n.body,
                "jobId": &n.job_id,
                "job": n.job_id.as_ref().and_then(|id| jobs.get(id)),
                "createdAt": &n.created_at,
            })
        })
        .collect();

    let count = deliveries.len();
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "deliveries": deliveries, "count": count } })),
    ))
}

/// Mark a page delivered. Bulk, so a pass costs one request rather than one per message.
async fn deliveries_sent(
    State(state): State<AppState>,
    Json(body): Json<DeliveryIds>,
) -> ApiResult {
    if body.ids.is_empty() {
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "marked": 0 } })),
        ));
    }
    let result = state
        .notifications()
        .update_many(
            doc! { "_id": { "$in": &body.ids } },
            doc! { "$set": { "sent_at": now_iso() } },
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "marked": result.modified_count } })),
    ))
}

/// Mark a page failed, with per-item reasons.
///
/// A terminal reason means the address is no longer valid, so alerts are paused for that
/// user rather than retried forever; a transient reason leaves the row for the next pass.
async fn deliveries_failed(
    State(state): State<AppState>,
    Json(body): Json<DeliveryFailures>,
) -> ApiResult {
    let mut marked = 0u64;
    let mut paused = 0u64;

    for failure in &body.failures {
        let reason = failure.reason.trim().to_lowercase();
        let notification = state
            .notifications()
            .find_one_and_update(
                doc! { "_id": &failure.id },
                doc! { "$set": { "sent_at": now_iso(), "failed_reason": &reason } },
            )
            .await?;

        let Some(notification) = notification else {
            continue;
        };
        marked += 1;

        if is_terminal_failure(&reason)
            && state
                .users()
                .update_one(
                    doc! { "_id": &notification.user_id },
                    doc! { "$set": { "alerts.enabled": false } },
                )
                .await?
                .modified_count
                > 0
        {
            paused += 1;
        }
    }

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": { "marked": marked, "alertsPaused": paused } })),
    ))
}

/// Whether a failure means "this address will never work again".
///
/// The old Telegram reasons are kept alongside the web-push ones because they describe the
/// same situations: a channel that has been withdrawn by the user, or an endpoint that no
/// longer exists.
fn is_terminal_failure(reason: &str) -> bool {
    matches!(
        reason,
        "blocked"
            | "deactivated"
            | "chat_not_found"
            | "unsubscribed"
            | "invalid_subscription"
            | "gone"
    )
}

// ---------------------------------------------------------------------------
// User-facing: alert preferences and rejections
// ---------------------------------------------------------------------------

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
    // Only two modes exist; anything else would silently never fire on the daily path, so
    // an unknown value is rejected rather than stored.
    let mode = body.mode.unwrap_or_else(|| "daily".to_string());
    if mode != "daily" && mode != "instant" {
        return Err(AppError::BadRequest(
            "mode must be \"daily\" or \"instant\"".to_string(),
        ));
    }

    // Enabling clears the marker so today's run can fire; disabling stamps it so a later
    // re-enable does not immediately fire a stale batch.
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

/// Records a rejection. Stored on the user rather than inferred, so the signal is visible,
/// reversible, and applied by the matcher as a query filter.
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

// ---------------------------------------------------------------------------
// Queueing
// ---------------------------------------------------------------------------

/// Queues matches for every subscriber who is due. WHO and WHAT to notify is decided here,
/// in one place, so the daily cap and the dedupe cannot be circumvented by a second caller.
///
/// Channel-agnostic: it no longer requires the user to have a Telegram link, which also
/// means it can run before any delivery transport exists at all. Alerts accumulate as
/// pending rows and go out when something drains them.
async fn refresh_matches(State(state): State<AppState>) -> ApiResult {
    let today = nairobi_today();
    let mut cursor = state.users().find(doc! { "alerts.enabled": true }).await?;

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
