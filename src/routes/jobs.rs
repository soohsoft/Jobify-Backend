use futures_util::TryStreamExt;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::{Document, doc};
use mongodb::options::FindOptions;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::{AlertPrefs, JobDoc, JobInput, MatchProfile, NotificationDoc};
use crate::state::AppState;
use crate::util::{nairobi_days_ago, nairobi_today, now_iso, uuid_id};

use super::{ApiResult, common::paginate};

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{id}", get(get_by_id))
}

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/sources", get(sources))
        .route("/internal/jobs", post(ingest_jobs))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/jobs/match", get(match_jobs))
        .route("/jobs/saved", get(list_saved))
        .route("/jobs/{id}/save", post(save_job).delete(unsave_job))
}

/// How long a job with no closing date stays visible, counted from `posted_date` and
/// falling back to `created_at` when the board never showed a posting date.
pub const UNDATED_JOB_LIFETIME_DAYS: i64 = 14;

/// The single definition of "live" for the feed and the matcher, so the two can never
/// disagree about what a user is allowed to see.
///
/// A job is live when:
///   - it has a closing date that has not passed, or
///   - it has no closing date and was posted within 14 days — or, when no posting date
///     is known, was first recorded within 14 days.
///
/// This is a VISIBILITY rule and nothing more. It deliberately does not write or
/// derive a closing date: an undated job keeps `deadline: null` in every response, so
/// no client can render a deadline the board never published. Do not turn
/// `UNDATED_JOB_LIFETIME_DAYS` into a synthetic deadline — that is exactly the sort of
/// thing that misleads a user into thinking an application window exists.
pub fn live_job_filter() -> Document {
    let today = nairobi_today();
    let cutoff = nairobi_days_ago(UNDATED_JOB_LIFETIME_DAYS);
    doc! {
        "$or": [
            doc! { "deadline": { "$gte": &today } },
            // `null` matches both an explicit null and a missing field.
            doc! { "deadline": null, "$or": [ doc! { "posted_date": { "$gte": &cutoff } }, doc! { "posted_date": null, "created_at": { "$gte": &cutoff } } ] },
            doc! { "deadline": "", "$or": [ doc! { "posted_date": { "$gte": &cutoff } }, doc! { "posted_date": null, "created_at": { "$gte": &cutoff } } ] },
        ]
    }
}

#[derive(Deserialize)]
pub struct JobsQuery {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_page() -> u64 {
    1
}
fn default_limit() -> u64 {
    20
}

async fn list(
    State(state): State<AppState>,
    extension: Option<Extension<AuthUser>>,
    Query(query): Query<JobsQuery>,
) -> ApiResult {
    let mut location = query.location.clone().filter(|s| !s.trim().is_empty());

    if location.is_none()
        && let Some(Extension(user)) = extension
        && let Some(doc) = state.users().find_one(doc! { "_id": &user.id }).await?
    {
        location = doc.location.filter(|s| !s.trim().is_empty());
    }

    let mut filter = doc! {};
    if let Some(location) = location {
        filter.insert(
            "location",
            doc! { "$regex": escape_regex(&location), "$options": "i" },
        );
    }
    if let Some(organization) = query.organization.filter(|s| !s.trim().is_empty()) {
        filter.insert(
            "organization",
            doc! { "$regex": escape_regex(&organization), "$options": "i" },
        );
    }
    if let Some(category) = query.category.filter(|s| !s.trim().is_empty()) {
        filter.insert(
            "category",
            doc! { "$regex": escape_regex(&category), "$options": "i" },
        );
    }
    if let Some(search) = query.search.filter(|s| !s.trim().is_empty()) {
        let pattern = doc! { "$regex": escape_regex(&search), "$options": "i" };
        filter.insert(
            "$or",
            vec![
                doc! { "title": pattern.clone() },
                doc! { "organization": pattern.clone() },
                doc! { "description": pattern },
            ],
        );
    }

    // What counts as live (see `live_job_filter`).
    let deadline_filter = live_job_filter();
    let filter = if filter.is_empty() {
        deadline_filter
    } else {
        doc! { "$and": [filter, deadline_filter] }
    };

    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.jobs(),
        filter,
        page,
        limit,
        // Secondary sort on _id keeps pagination stable when many jobs share
        // the same created_at second (batch ingest), instead of leaking or
        // repeating rows across pages.
        doc! { "created_at": -1, "_id": -1 },
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

async fn get_by_id(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let job = state
        .jobs()
        .find_one(doc! { "_id": &id })
        .await?
        .ok_or_else(|| AppError::NotFound("Job not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": job })),
    ))
}

#[derive(Deserialize)]
pub struct SavedQuery {
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_saved_limit")]
    limit: u64,
}

fn default_saved_limit() -> u64 {
    30
}

/// The user's saved jobs, newest save first.
///
/// Deliberately NOT filtered by the live-job rule. That rule exists so nobody is
/// shown a job that closed weeks ago while browsing; a job the user chose to keep
/// is theirs, and dropping it from their own list is silent data loss — the one
/// failure mode worse than showing an old listing. Each job carries its own
/// `deadline` untouched (never derived), so a client can mark one that has closed.
async fn list_saved(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<SavedQuery>,
) -> ApiResult {
    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (rows, total) = paginate(
        &state.saved_jobs(),
        doc! { "user_id": &user.id },
        page,
        limit,
        doc! { "created_at": -1, "_id": -1 },
    )
    .await?;

    let ids: Vec<String> = rows.iter().map(|row| row.job_id.clone()).collect();
    let mut cursor = state.jobs().find(doc! { "_id": { "$in": &ids } }).await?;
    let mut by_id: HashMap<String, JobDoc> = HashMap::new();
    while let Some(job) = cursor.try_next().await? {
        by_id.insert(job.id.clone(), job);
    }

    // Walk the save order, not the job order: `$in` returns jobs in index order,
    // which would quietly undo the newest-first sort the page just made.
    let data: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            by_id
                .remove(&row.job_id)
                .map(|job| json!({ "job": job, "savedAt": row.created_at }))
        })
        .collect();

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

/// Save a job. Idempotent: saving twice is one save, and the response says which
/// of the two happened so a client can report it honestly.
async fn save_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    // A save of something that does not exist is a client bug, not an empty list
    // to be discovered later from a saved row pointing at nothing.
    if state.jobs().find_one(doc! { "_id": &id }).await?.is_none() {
        return Err(AppError::NotFound("Job not found".to_string()));
    }

    let result = state
        .saved_jobs()
        .update_one(
            doc! { "user_id": &user.id, "job_id": &id },
            doc! { "$setOnInsert": {
                "_id": uuid_id(),
                "user_id": &user.id,
                "job_id": &id,
                "created_at": now_iso(),
            } },
        )
        .upsert(true)
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "saved": true, "alreadySaved": result.upserted_id.is_none(), "job_id": id }
        })),
    ))
}

/// Remove a saved job. Also idempotent: removing something already gone succeeds,
/// because the caller's intent ("this should not be saved") is satisfied either way.
async fn unsave_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let result = state
        .saved_jobs()
        .delete_one(doc! { "user_id": &user.id, "job_id": &id })
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "saved": false, "removed": result.deleted_count > 0, "job_id": id }
        })),
    ))
}

async fn sources(State(state): State<AppState>) -> ApiResult {
    let mut cursor = state.sources().find(doc! {}).await?;
    let mut data = Vec::new();
    while let Some(source) = cursor.try_next().await? {
        data.push(source);
    }

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": data })),
    ))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum IngestPayload {
    Single(Box<JobInput>),
    Many(Vec<JobInput>),
}

async fn ingest_jobs(
    State(state): State<AppState>,
    Json(payload): Json<IngestPayload>,
) -> ApiResult {
    let jobs = match payload {
        IngestPayload::Single(job) => vec![*job],
        IngestPayload::Many(jobs) => jobs,
    };

    if jobs.is_empty() {
        return Err(AppError::BadRequest("No jobs provided".to_string()));
    }

    let received = jobs.len() as u64;
    let mut persisted = 0u64;
    let mut failed = 0u64;

    for input in jobs {
        if input.external_id.trim().is_empty() || input.source.trim().is_empty() {
            failed += 1;
            continue;
        }

        // A category the taxonomy doesn't know is dropped, not guessed at. The
        // job itself is still good, so this is a warning rather than a failure.
        if let Some(raw) = input
            .category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            && canonical_category(&input.category).is_none()
        {
            tracing::warn!(
                source = %input.source,
                external_id = %input.external_id,
                category = %raw,
                "ingest: unrecognized category slug, storing null"
            );
        }

        let now = now_iso();
        let id = uuid_id();
        let set_on_insert = doc! {
            "_id": &id,
            "created_at": &now,
        };

        // Only overwrite fields the payload actually carries. The scraper is not
        // guaranteed to send everything every cycle — enrichment can be disabled,
        // a detail page can fail, a board can stop exposing a field — and a full
        // `$set` would blank whatever an earlier, richer run had stored. Keys the
        // payload always provides (identity, title, url) stay unconditional; the
        // tradeoff is that an empty value can no longer clear a stored one.
        let mut set = mongodb::bson::Document::new();
        set.insert("external_id", &input.external_id);
        set.insert("source", &input.source);
        set.insert("title", &input.title);
        set.insert("url", &input.url);
        set.insert("updated_at", &now);

        if !input.source_name.trim().is_empty() {
            set.insert("source_name", &input.source_name);
        }
        if !input.organization.trim().is_empty() {
            set.insert("organization", &input.organization);
        }
        if !input.description.trim().is_empty() {
            set.insert("description", &input.description);
        }
        if let Some(value) = input.location.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("location", value);
        }
        if let Some(value) = input
            .posted_date
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            set.insert("posted_date", value);
        }
        if let Some(value) = input.deadline.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("deadline", value);
        }
        if let Some(value) = input.image_url.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("image_url", value);
        }
        if let Some(value) = input
            .organization_image_url
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            set.insert("organization_image_url", value);
        }
        // An absent, blank, or unrecognized slug is skipped rather than written,
        // so a junk value cannot blank a category a previous run got right.
        if let Some(canonical) = canonical_category(&input.category) {
            set.insert("category", canonical);
        }
        if let Some(value) = input
            .employment_type
            .as_deref()
            .filter(|s| !s.trim().is_empty())
        {
            set.insert("employment_type", value);
        }
        if let Some(value) = input.salary.as_deref().filter(|s| !s.trim().is_empty()) {
            set.insert("salary", value);
        }
        if !input.requirements.is_empty() {
            set.insert("requirements", &input.requirements);
        }
        if !input.qualifications.is_empty() {
            set.insert("qualifications", &input.qualifications);
        }

        let update = doc! {
            "$set": set,
            "$setOnInsert": set_on_insert,
        };

        match state
            .jobs()
            .update_one(
                doc! { "external_id": &input.external_id, "source": &input.source },
                update,
            )
            .upsert(true)
            .await
        {
            Ok(_) => persisted += 1,
            Err(_) => failed += 1,
        }
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "received": received, "persisted": persisted, "failed": failed }
        })),
    ))
}

/// Only canonical taxonomy slugs are stored (`src/categories.rs`). Anything
/// else — a board's free text, a mis-cased or hallucinated slug from the
/// scraper's LLM — becomes null instead of polluting `GET /jobs?category=`.
fn canonical_category(raw: &Option<String>) -> Option<&'static str> {
    let slug = raw.as_deref()?.trim();
    crate::categories::Category::from_slug(slug).map(|category| category.slug())
}

#[derive(Deserialize)]
pub struct MatchQuery {
    #[serde(default = "default_match_limit")]
    limit: u64,
    /// When true, also queue notifications for what is returned (the automated
    /// path). The manual path leaves it false, so asking twice for "my jobs"
    /// never consumes alert state.
    #[serde(default)]
    record: bool,
}

fn default_match_limit() -> u64 {
    5
}

// Weighted toward what actually predicts fit. The category is the gate and is
// already applied by the query filter, so it is scored as a constant; location
// is the strongest remaining signal and keywords break ties.
fn score_job(job: &JobDoc, profile: &MatchProfile) -> f64 {
    let mut score = 3.0;

    let location = job.location.clone().unwrap_or_default().to_lowercase();
    if !location.is_empty()
        && profile.locations.iter().any(|want| {
            let want = want.trim().to_lowercase();
            !want.is_empty() && location.contains(&want)
        })
    {
        score += 2.0;
    }

    let title = job.title.to_lowercase();
    let description = job.description.to_lowercase();
    for keyword in &profile.keywords {
        let keyword = keyword.trim().to_lowercase();
        if keyword.is_empty() {
            continue;
        }
        if title.contains(&keyword) {
            score += 1.0;
        } else if description.contains(&keyword) {
            score += 0.5;
        }
    }

    score
}

async fn match_jobs(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<MatchQuery>,
) -> ApiResult {
    let limit = query.limit.clamp(1, 20);

    let user_doc = state.users().find_one(doc! { "_id": &user.id }).await?;
    let profile = user_doc.as_ref().and_then(|doc| doc.match_profile.clone());
    let exclusions = user_doc
        .as_ref()
        .and_then(|doc| doc.alerts.clone())
        .unwrap_or_default();

    // No profile (or no categories yet) is a normal state on the way in, not an
    // error: the caller branches on `reason` instead of parsing a failure.
    let Some(profile) = profile.filter(|p| !p.categories.is_empty()) else {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "data": [],
                "meta": { "reason": "no_match_profile", "total": 0, "queued": 0 }
            })),
        ));
    };

    // Already-queued jobs are excluded only on the automated path. The manual list
    // is a live view and keeps showing what the user has already seen.
    let exclude = if query.record {
        queued_job_ids(&state, &user.id).await?
    } else {
        Vec::new()
    };

    let scored = ranked_candidates(&state, &profile, &exclusions, &exclude, limit as i64).await?;
    let queued_count = if query.record {
        queue_notifications(&state, &user.id, &scored).await?
    } else {
        0
    };

    let data: Vec<Value> = scored
        .iter()
        .map(|(score, job)| json!({ "job": job, "matchScore": score }))
        .collect();

    let total = data.len();
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "reason": "ok",
                "total": total,
                "queued": queued_count,
                "categories": profile.categories,
            }
        })),
    ))
}

/// Job ids already queued for a user, so the automated path never repeats one.
async fn queued_job_ids(state: &AppState, user_id: &str) -> Result<Vec<String>, AppError> {
    let mut cursor = state
        .notifications()
        .find(doc! { "user_id": user_id, "job_id": { "$exists": true } })
        .await?;
    let mut ids: Vec<String> = Vec::new();
    while let Some(notification) = cursor.try_next().await? {
        if let Some(job_id) = notification.job_id {
            ids.push(job_id);
        }
    }
    Ok(ids)
}

/// Candidates for a profile, scored and sorted.
///
/// The single place that decides what is eligible: the live-job rule, the profile's
/// categories, the user's own rejections, and what has already been queued. The feed
/// and the automated alert path both come through here, so they cannot disagree.
pub(crate) async fn ranked_candidates(
    state: &AppState,
    profile: &MatchProfile,
    exclusions: &AlertPrefs,
    exclude_job_ids: &[String],
    limit: i64,
) -> Result<Vec<(f64, JobDoc)>, AppError> {
    let mut filter = live_job_filter();

    let mut categories = doc! { "$in": &profile.categories };
    // A rejected category is applied to the query rather than filtered after
    // scoring, so it cannot occupy one of the returned slots.
    if !exclusions.excluded_categories.is_empty() {
        categories.insert("$nin", &exclusions.excluded_categories);
    }
    filter.insert("category", categories);

    if !exclude_job_ids.is_empty() {
        filter.insert("_id", doc! { "$nin": exclude_job_ids });
    }

    // "Not a fit" on an employer, case-insensitive, built from escaped literals so a
    // company name containing regex metacharacters cannot corrupt the pattern.
    if !exclusions.excluded_employers.is_empty() {
        let names: Vec<String> = exclusions
            .excluded_employers
            .iter()
            .map(|name| escape_regex(name))
            .collect();
        filter.insert(
            "organization",
            doc! { "$not": { "$regex": format!("^({})$", names.join("|")), "$options": "i" } },
        );
    }

    let options = FindOptions::builder().limit(300).build();
    let mut cursor = state.jobs().find(filter).with_options(options).await?;
    let mut scored: Vec<(f64, JobDoc)> = Vec::new();
    while let Some(job) = cursor.try_next().await? {
        scored.push((score_job(&job, profile), job));
    }

    // Recency, then id, as the tie-breakers so paging is stable.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.1.created_at.cmp(&a.1.created_at))
            .then_with(|| b.1.id.cmp(&a.1.id))
    });
    scored.truncate(limit as usize);
    Ok(scored)
}

/// Queue one job_match notification per job. A duplicate-key error means the partial
/// unique index caught a repeat, which is the intended outcome rather than a failure.
async fn queue_notifications(
    state: &AppState,
    user_id: &str,
    scored: &[(f64, JobDoc)],
) -> Result<u64, AppError> {
    let now = now_iso();
    let mut queued = 0u64;
    for (_score, job) in scored {
        let notification = NotificationDoc {
            id: uuid_id(),
            user_id: user_id.to_string(),
            notification_type: "job_match".to_string(),
            job_id: Some(job.id.clone()),
            title: job.title.clone(),
            body: job.description.clone(),
            read: false,
            sent_at: None,
            failed_reason: None,
            created_at: now.clone(),
        };
        if state
            .notifications()
            .insert_one(&notification)
            .await
            .is_ok()
        {
            queued += 1;
        }
    }
    Ok(queued)
}

/// The automated path for one user, with no HTTP caller — the alert refresh runs this
/// per subscriber. Returns how many notifications were newly queued.
pub async fn queue_matches_for_user(
    state: &AppState,
    user_id: &str,
    limit: i64,
) -> Result<u64, AppError> {
    let Some(user) = state.users().find_one(doc! { "_id": user_id }).await? else {
        return Ok(0);
    };
    // No profile means nothing to match on yet: a normal state, not an error.
    let Some(profile) = user
        .match_profile
        .clone()
        .filter(|p| !p.categories.is_empty())
    else {
        return Ok(0);
    };
    let exclusions = user.alerts.clone().unwrap_or_default();
    let exclude = queued_job_ids(state, user_id).await?;
    let scored = ranked_candidates(state, &profile, &exclusions, &exclude, limit).await?;
    queue_notifications(state, user_id, &scored).await
}

fn escape_regex(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() * 2);
    for c in value.chars() {
        if ".^$*+?()[]{}|\\".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[allow(dead_code)]
fn _unused_json(_: Value) {}

#[cfg(test)]
mod tests {
    use super::canonical_category;

    fn raw(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    #[test]
    fn accepts_a_known_slug() {
        assert_eq!(
            canonical_category(&raw("computer_technology")),
            Some("computer_technology")
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            canonical_category(&raw("  environment_water_and_sanitation  ")),
            Some("environment_water_and_sanitation")
        );
        // An old, finer slug must NOT resolve: it has to be dropped rather than absorbed into
        // whichever broad category happens to be nearby.
        assert_eq!(canonical_category(&raw("wash")), None);
    }

    #[test]
    fn rejects_unknown_and_free_text() {
        assert_eq!(canonical_category(&raw("totally_made_up_slug")), None);
        assert_eq!(canonical_category(&raw("Computer Technology")), None);
        assert_eq!(canonical_category(&raw("IT & Networking")), None);
    }

    #[test]
    fn rejects_absent_and_blank() {
        assert_eq!(canonical_category(&None), None);
        assert_eq!(canonical_category(&raw("")), None);
        assert_eq!(canonical_category(&raw("   ")), None);
    }

    #[test]
    fn slugs_are_strictly_lowercase() {
        assert_eq!(canonical_category(&raw("WASH")), None);
    }

    // The visibility rule is the one thing standing between a user and a job that
    // closed weeks ago, so it is tested against a real collection rather than by
    // re-stating the filter in Rust.
    #[tokio::test]
    async fn live_filter_keeps_open_jobs_and_expires_undated_ones_after_fourteen_days() {
        use super::{UNDATED_JOB_LIFETIME_DAYS, live_job_filter};
        use crate::util::{nairobi_days_ago, nairobi_today};
        use futures_util::TryStreamExt;
        use mongodb::bson::doc;

        let config = crate::config::Config::from_env();
        let client = mongodb::Client::with_uri_str(&config.mongodb_uri)
            .await
            .expect("failed to connect to test MongoDB");
        let db = client.database("jobify_test_live_filter");
        let _ = db.drop().await;
        let jobs = db.collection::<mongodb::bson::Document>("jobs");

        let day = |n: i64| nairobi_days_ago(n);
        let stamp = |n: i64| format!("{}T09:00:00Z", nairobi_days_ago(n));
        let cases: Vec<(&str, mongodb::bson::Document)> = vec![
            ("open", doc! { "deadline": nairobi_days_ago(-5) }),
            ("closed", doc! { "deadline": day(1) }),
            ("closes-today", doc! { "deadline": nairobi_today() }),
            (
                "undated-posted-recently",
                doc! { "deadline": null, "posted_date": day(3) },
            ),
            (
                "undated-posted-too-long-ago",
                doc! { "deadline": null, "posted_date": day(UNDATED_JOB_LIFETIME_DAYS + 1) },
            ),
            (
                "undated-blank-deadline-recent",
                doc! { "deadline": "", "posted_date": day(3) },
            ),
            (
                "undated-no-posting-date-recorded-recently",
                doc! { "deadline": null, "created_at": stamp(3) },
            ),
            (
                "undated-no-posting-date-recorded-long-ago",
                doc! { "deadline": null, "created_at": stamp(UNDATED_JOB_LIFETIME_DAYS + 1) },
            ),
        ];
        for (title, extra) in &cases {
            let mut doc = doc! { "_id": *title, "title": *title };
            doc.extend(extra.clone());
            jobs.insert_one(&doc).await.unwrap();
        }

        let mut cursor = jobs.find(live_job_filter()).await.unwrap();
        let mut live: Vec<String> = Vec::new();
        while let Some(doc) = cursor.try_next().await.unwrap() {
            live.push(doc.get_str("title").unwrap().to_string());
        }
        live.sort();

        let mut expected = vec![
            "closes-today",
            "open",
            "undated-blank-deadline-recent",
            "undated-no-posting-date-recorded-recently",
            "undated-posted-recently",
        ];
        expected.sort();
        assert_eq!(live, expected);
    }
}
