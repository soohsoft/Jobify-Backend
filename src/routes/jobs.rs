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
use crate::models::{
    AlertPrefs, JobDoc, JobInput, KIND_JOB, KIND_TENDER, MatchProfile, NotificationDoc,
};
use crate::state::AppState;
use crate::util::{escape_regex, nairobi_days_ago, nairobi_today, now_iso, uuid_id};

use super::ApiResult;
use super::common::{paginate, total_pages};

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list))
        .route("/jobs/{id}", get(get_by_id))
}

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/sources", get(sources))
        .route("/internal/jobs", post(ingest_jobs))
        // The bot has no bearer token, so `GET /jobs/match` is unreachable from Discord.
        // This is the same payload shape addressed by the member's Discord identity, which
        // is what "/jobs for me" means in the product.
        .route("/internal/jobs/for-user/{discord_id}", get(jobs_for_user))
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

/// The single definition of "live" for the feed AND the matcher, so the two can never
/// disagree about what a user is allowed to see.
///
/// A job is live when:
///   - it has a closing date that has not passed, or
///   - it has no closing date and was posted within 14 days — or, when no posting date is
///     known, was first recorded within 14 days.
///
/// This is a VISIBILITY rule and nothing more. It deliberately does not write or derive a
/// closing date: an undated job keeps `deadline: null` in every response, so no client can
/// render a deadline the board never published. Do not turn `UNDATED_JOB_LIFETIME_DAYS`
/// into a synthetic deadline.
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
    /// `q` is the documented search parameter; `search` is accepted as the same thing so
    /// an older client keeps working.
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    organization: Option<String>,
    #[serde(default)]
    category: Option<String>,
    /// `job` or `tender`. Absent means both — which is what the 55 listings stored before
    /// `kind` existed need, since they carry no field at all.
    #[serde(default)]
    kind: Option<String>,
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
        && let Some(user_doc) = state.users().find_one(doc! { "_id": &user.id }).await?
    {
        location = user_doc.location.filter(|s| !s.trim().is_empty());
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
    if let Some(search) = query.q.or(query.search).filter(|s| !s.trim().is_empty()) {
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

    // A `kind` filter is a separate clause rather than another key in `filter`, because
    // `filter` may already hold an `$or` from the search box and a Document cannot hold the
    // same key twice. "job" must also match the legacy rows, which have no `kind` field at
    // all: `null` matches both an explicit null and a missing field.
    let kind_clause = match query.kind.as_deref().map(str::trim) {
        Some("") | None => None,
        Some(KIND_TENDER) => Some(doc! { "kind": KIND_TENDER }),
        Some(KIND_JOB) => Some(doc! { "$or": [ { "kind": KIND_JOB }, { "kind": null } ] }),
        Some(other) => {
            return Err(AppError::BadRequest(format!(
                "kind must be \"{KIND_JOB}\" or \"{KIND_TENDER}\" — got '{other}'"
            )));
        }
    };

    let mut clauses = Vec::new();
    if !filter.is_empty() {
        clauses.push(filter);
    }
    clauses.push(live_job_filter());
    if let Some(kind) = kind_clause {
        clauses.push(kind);
    }
    let filter = if clauses.len() == 1 {
        clauses.remove(0)
    } else {
        doc! { "$and": clauses }
    };

    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.jobs(),
        filter,
        page,
        limit,
        // Secondary sort on _id keeps pagination stable when many jobs share the same
        // created_at second (batch ingest), instead of repeating or skipping rows.
        doc! { "created_at": -1, "_id": -1 },
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "page": page,
                "limit": limit,
                "total": total,
                "totalPages": total_pages(total, limit)
            }
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
/// Deliberately NOT filtered by the live-job rule. That rule exists so nobody is shown a
/// job that closed weeks ago while browsing; a job the user chose to keep is theirs, and
/// dropping it from their own list is silent data loss. Each job carries its own `deadline`
/// untouched, so a client can mark one that has closed.
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
    let mut by_id: HashMap<String, JobDoc> = HashMap::new();
    if !ids.is_empty() {
        let mut cursor = state.jobs().find(doc! { "_id": { "$in": &ids } }).await?;
        while let Some(job) = cursor.try_next().await? {
            by_id.insert(job.id.clone(), job);
        }
    }

    // Walk the save order, not the job order: `$in` returns jobs in index order, which
    // would quietly undo the newest-first sort the page just made.
    let data: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            by_id
                .remove(&row.job_id)
                .map(|job| json!({ "job": job, "savedAt": row.created_at }))
        })
        .collect();

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "page": page,
                "limit": limit,
                "total": total,
                "totalPages": total_pages(total, limit)
            }
        })),
    ))
}

/// Save a job. Idempotent: saving twice is one save, and the response says which of the
/// two happened so a client can report it honestly.
async fn save_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    // A save of something that does not exist is a client bug, not an empty list to be
    // discovered later from a saved row pointing at nothing.
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

/// Remove a saved job. Also idempotent: removing something already gone succeeds, because
/// the caller's intent ("this should not be saved") is satisfied either way.
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

        // A category the taxonomy doesn't know is dropped, not guessed at. The job itself
        // is still good, so this is a warning rather than a failure.
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
        let set_on_insert = doc! { "_id": uuid_id(), "created_at": &now };

        // Only overwrite fields the payload actually carries. The scraper is not guaranteed
        // to send everything every cycle — enrichment can be disabled, a detail page can
        // fail, a board can stop exposing a field — and a full `$set` would blank whatever
        // an earlier, richer run had stored. Keys the payload always provides (identity,
        // title, url) stay unconditional; the tradeoff is that an empty value can no longer
        // clear a stored one.
        let mut set = Document::new();
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
        // An absent, blank, or unrecognized slug is skipped rather than written, so a junk
        // value cannot blank a category a previous run got right.
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
        // Always written, and always a known value: a listing is never rejected over its
        // kind, and the 55 legacy documents are read as "job" through the serde default.
        set.insert("kind", canonical_kind(&input.kind));
        if !input.requirements.is_empty() {
            set.insert("requirements", &input.requirements);
        }
        if !input.qualifications.is_empty() {
            set.insert("qualifications", &input.qualifications);
        }

        let update = doc! { "$set": set, "$setOnInsert": set_on_insert };

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
            Err(err) => {
                tracing::warn!(
                    source = %input.source,
                    external_id = %input.external_id,
                    error = %err,
                    "ingest: job could not be persisted"
                );
                failed += 1;
            }
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

/// Only canonical taxonomy slugs are stored (`src/categories.rs`). Anything else — a
/// board's free text, a mis-cased or hallucinated slug — becomes null instead of polluting
/// `GET /jobs?category=`.
fn canonical_category(raw: &Option<String>) -> Option<&'static str> {
    let slug = raw.as_deref()?.trim();
    crate::categories::Category::from_slug(slug).map(|category| category.slug())
}

/// "tender" is the only alternative to a job. Anything else — absent, blank, mis-cased or
/// unknown — is stored as "job": the label is never a reason to drop a listing, and a
/// tender that stops claiming to be one becomes a job rather than keeping a stale type.
fn canonical_kind(raw: &Option<String>) -> &'static str {
    match raw
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some(KIND_TENDER) => KIND_TENDER,
        _ => KIND_JOB,
    }
}

#[derive(Deserialize)]
pub struct MatchQuery {
    #[serde(default = "default_match_limit")]
    limit: u64,
    /// When true, also queue notifications for what is returned (the automated path). The
    /// manual path leaves it false, so asking twice for "my jobs" never consumes alert state.
    #[serde(default)]
    record: bool,
}

fn default_match_limit() -> u64 {
    5
}

/// Weighted toward what actually predicts fit. The category is already the query's gate, so
/// it scores as the constant evidence "this row is in a decided area"; a subcategory phrase
/// in the title is far stronger evidence than a keyword, because it is a whole job title the
/// backend decided the member can apply for ("Data Analyst" in the title of an ICT row).
/// Location is the strongest single signal and keywords break the remaining ties.
///
/// ONE function for every route: `GET /jobs/match`, the bot's for-user list and the alert
/// refresh all score through `ranked_candidates`, so three lists can never rank differently.
fn score_job(job: &JobDoc, profile: &MatchProfile) -> f64 {
    let mut score = 0.0;

    if let Some(category) = job.category.as_deref()
        && profile
            .categories
            .iter()
            .any(|wanted| wanted.eq_ignore_ascii_case(category))
    {
        score += 1.0;
    }

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

    // A decided role title is the whole phrase, so "data analyst" in "Senior Data Analyst"
    // counts and "data" alone does not.
    for subcategory in &profile.subcategories {
        let phrase = subcategory.trim().to_lowercase();
        if !phrase.is_empty() && title.contains(&phrase) {
            score += 3.0;
        }
    }

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

    // No profile (or no categories yet) is a normal state on the way in, not an error: the
    // caller branches on `reason` instead of parsing a failure.
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

    // Already-queued jobs are excluded only on the automated path. The manual list is a
    // live view and keeps showing what the user has already seen.
    let exclude = if query.record {
        queued_job_ids(&state, &user.id).await?
    } else {
        Vec::new()
    };

    let scored = ranked_candidates(
        &state,
        &profile,
        &exclusions,
        &exclude,
        limit as i64,
        kind_preference_clause("both"),
    )
    .await?;
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
                "subcategories": profile.subcategories,
                "categorySource": profile.category_source,
            }
        })),
    ))
}

#[derive(Deserialize)]
pub struct ForUserQuery {
    #[serde(default = "default_match_limit")]
    limit: u64,
    /// `job` or `tender`; absent (or `both`) means both. `jobs`/`tenders` are accepted too
    /// because that is the vocabulary `wants` uses.
    #[serde(default)]
    kind: Option<String>,
}

/// `GET /internal/jobs/for-user/{discord_id}` — the member's own list, addressed by the
/// identity the bot actually holds.
///
/// The payload is deliberately IDENTICAL to `GET /jobs/match` (`data: [{job, matchScore}]`,
/// the same `meta` keys), so the bot has one shape to render and the portal keeps its own
/// authoritative route. Nothing is queued: this is a read, and `/jobs` must not consume the
/// alert state the daily digest owns.
///
/// An unknown snowflake is a 404. "No profile yet" is NOT: it is
/// `meta.reason: "no_match_profile"` with an empty list, exactly like the portal.
async fn jobs_for_user(
    State(state): State<AppState>,
    Path(discord_id): Path<String>,
    Query(query): Query<ForUserQuery>,
) -> ApiResult {
    let discord_id = discord_id.trim();
    let limit = query.limit.clamp(1, 20);
    let kind = query
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| match value.to_ascii_lowercase().as_str() {
            // `both` is spelled out by a caller that has a stored preference, so accept it.
            "both" | "all" => Ok(None),
            other if other == KIND_JOB || other == "jobs" => Ok(Some(KIND_JOB)),
            other if other == KIND_TENDER || other == "tenders" => Ok(Some(KIND_TENDER)),
            other => Err(AppError::BadRequest(format!(
                "kind must be \"{KIND_JOB}\" or \"{KIND_TENDER}\" — got '{other}'"
            ))),
        })
        .transpose()?
        .flatten();

    // The snowflake is the account's identity, so an unknown one means no account. A
    // malformed id can only be a caller bug and is treated the same way rather than
    // explained back to a client that has no business sending one.
    let Some(link) = state
        .discord_links()
        .find_one(doc! { "discord_id": discord_id })
        .await?
    else {
        return Err(AppError::NotFound(
            "This Discord account is not registered with Jobify yet.".to_string(),
        ));
    };

    let outcome = match_rows(&state, &link.user_id, limit as i64, kind).await?;
    let reason = if outcome.areas.categories.is_empty() {
        "no_match_profile"
    } else {
        "ok"
    };
    let data: Vec<Value> = outcome
        .rows
        .iter()
        .map(|(score, job)| json!({ "job": job, "matchScore": score }))
        .collect();

    tracing::debug!(
        user_id = %link.user_id,
        rows = data.len(),
        categories = outcome.areas.categories.len(),
        subcategories = outcome.areas.subcategories.len(),
        source = %outcome.areas.source,
        "for-user match read"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "reason": reason,
                "total": data.len(),
                "queued": 0,
                "categories": outcome.areas.categories,
                "subcategories": outcome.areas.subcategories,
                "categorySource": outcome.areas.source,
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

/// The job/tender preference as a query clause.
///
/// `None` means "no narrowing": `both`, an absent preference, and every account that
/// predates the field all include tenders and jobs. "jobs" must also match a MISSING `kind`
/// field — the older listings carry none — which is why it is an `$or` rather than an
/// equality. Both the plural (`wants`) and singular (`?kind=`) spellings are accepted so a
/// caller never has to translate between two vocabularies for the same idea.
fn kind_preference_clause(wanted: &str) -> Option<Document> {
    let wanted = wanted.trim().to_ascii_lowercase();
    if wanted == "jobs" || wanted == KIND_JOB {
        Some(doc! { "$or": [ doc! { "kind": KIND_JOB }, doc! { "kind": null } ] })
    } else if wanted == "tenders" || wanted == KIND_TENDER {
        Some(doc! { "kind": KIND_TENDER })
    } else {
        None
    }
}

/// Candidates for a profile, scored and sorted.
///
/// The single place that decides what is eligible: the live-job rule, the profile's
/// categories, the user's own rejections, the kind preference and what has already been
/// queued. The feed and the automated alert path both come through here, so they cannot
/// disagree.
async fn ranked_candidates(
    state: &AppState,
    profile: &MatchProfile,
    exclusions: &AlertPrefs,
    exclude_job_ids: &[String],
    limit: i64,
    kind_clause: Option<Document>,
) -> Result<Vec<(f64, JobDoc)>, AppError> {
    let mut filter = live_job_filter();

    // What the member asked for. Applied as its own `$and` element because `live_job_filter`
    // already owns the top-level `$or`, and a Document cannot hold one key twice; `None`
    // ("both", and every account with no preference stored) adds nothing.
    if let Some(clause) = kind_clause {
        filter.insert("$and", vec![clause]);
    }

    let mut categories = doc! { "$in": &profile.categories };
    // A rejected category is applied to the query rather than filtered after scoring, so
    // it cannot occupy one of the returned slots.
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

    // Recency, then id, as tie-breakers so paging is stable.
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

/// The automated path for one user, with no HTTP caller — the alert refresh runs this per
/// subscriber. Returns how many notifications were newly queued.
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
    // A member who asked for tenders only must not be queued jobs they will never be sent.
    let kind_clause = kind_preference_clause(user.wants.as_deref().unwrap_or("both"));
    let scored =
        ranked_candidates(state, &profile, &exclusions, &exclude, limit, kind_clause).await?;
    queue_notifications(state, user_id, &scored).await
}

/// What one matcher read produced, for a caller that must report the decision as well as
/// the rows. `areas` is the stored decision (possibly empty = no usable profile).
pub(crate) struct MatchOutcome {
    pub rows: Vec<(f64, JobDoc)>,
    pub areas: crate::models::MatchAreas,
}

/// The matcher for a stored account, with no HTTP caller.
///
/// `GET /jobs/match` (bearer), `GET /internal/jobs/for-user/{discord_id}` and the bot's
/// `GET /internal/discord/matches/{discord_id}` all come through here, so the portal and
/// Discord cannot disagree about what matches — the same reason `live_job_filter` exists.
/// It always reads, never queues: the caller decides whether a read should consume alert
/// state, and every current caller says no.
///
/// `kind` is "job"/"tender" to narrow, anything else (including `None`) for both; it is
/// applied to the query rather than after scoring so a member who asked for tenders is not
/// shown jobs that occupied the returned slots.
///
/// An empty category list is the "no usable profile" state, which is normal on the way into
/// onboarding.
pub(crate) async fn match_rows(
    state: &AppState,
    user_id: &str,
    limit: i64,
    kind: Option<&str>,
) -> Result<MatchOutcome, AppError> {
    let empty = MatchOutcome {
        rows: Vec::new(),
        areas: crate::models::MatchAreas::default(),
    };
    let Some(user) = state.users().find_one(doc! { "_id": user_id }).await? else {
        return Ok(empty);
    };
    let exclusions = user.alerts.clone().unwrap_or_default();
    let Some(profile) = user
        .match_profile
        .clone()
        .filter(|profile| !profile.categories.is_empty())
    else {
        return Ok(empty);
    };

    let scored = ranked_candidates(
        state,
        &profile,
        &exclusions,
        &[],
        limit,
        kind_preference_clause(kind.unwrap_or("both")),
    )
    .await?;
    let areas = crate::models::MatchAreas {
        categories: profile.categories.clone(),
        subcategories: profile.subcategories.clone(),
        source: profile.category_source.clone(),
    };
    Ok(MatchOutcome {
        rows: scored,
        areas,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        AppError, Document, JobDoc, Json, MatchProfile, StatusCode, canonical_category,
        canonical_kind, doc,
    };

    fn raw(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    #[test]
    fn a_kind_is_tender_only_when_it_says_so() {
        assert_eq!(canonical_kind(&raw("tender")), "tender");
        assert_eq!(canonical_kind(&raw("  TENDER  ")), "tender");
        // Everything else is a job, and nothing is rejected over the label.
        assert_eq!(canonical_kind(&raw("job")), "job");
        assert_eq!(canonical_kind(&raw("consultancy")), "job");
        assert_eq!(canonical_kind(&raw("")), "job");
        assert_eq!(canonical_kind(&None), "job");
    }

    // The alert path narrows on what the member asked for, and "jobs" has to keep matching
    // the older listings that carry no `kind` field at all — an equality would hide them.
    #[test]
    fn the_wants_preference_narrows_the_query_and_both_adds_no_clause() {
        use super::kind_preference_clause;
        use mongodb::bson::Bson;

        assert!(kind_preference_clause("both").is_none());
        assert!(kind_preference_clause("").is_none());
        assert!(kind_preference_clause("anything else").is_none());

        let tenders = kind_preference_clause("TENDERS").expect("tenders must narrow");
        assert_eq!(tenders.get_str("kind").unwrap(), "tender");
        assert!(tenders.get("$or").is_none());

        let jobs = kind_preference_clause("jobs").expect("jobs must narrow");
        let alternatives = jobs.get_array("$or").expect("jobs is an $or").clone();
        assert_eq!(alternatives.len(), 2);
        assert_eq!(
            alternatives[0]
                .as_document()
                .unwrap()
                .get_str("kind")
                .unwrap(),
            "job"
        );
        assert!(matches!(
            alternatives[1].as_document().unwrap().get("kind"),
            Some(Bson::Null)
        ));
    }

    #[test]
    fn accepts_a_known_slug_after_trimming() {
        assert_eq!(canonical_category(&raw("ict")), Some("ict"));
        assert_eq!(
            canonical_category(&raw("  environment_and_climate  ")),
            Some("environment_and_climate")
        );
        assert_eq!(canonical_category(&raw("wash")), Some("wash"));
    }

    #[test]
    fn rejects_unknown_free_text_and_blank() {
        assert_eq!(canonical_category(&raw("totally_made_up_slug")), None);
        assert_eq!(canonical_category(&raw("Computer Technology")), None);
        assert_eq!(
            canonical_category(&raw("Water, Sanitation & Hygiene")),
            None
        );
        assert_eq!(canonical_category(&None), None);
        assert_eq!(canonical_category(&raw("")), None);
        assert_eq!(canonical_category(&raw("   ")), None);
    }

    // The visibility rule is the one thing standing between a user and a job that closed
    // weeks ago, so it is tested against a real collection rather than by re-stating the
    // filter in Rust. Uses its own throwaway database; the live one is never touched.
    #[tokio::test]
    async fn live_filter_keeps_open_jobs_and_expires_undated_ones_after_fourteen_days() {
        use super::{UNDATED_JOB_LIFETIME_DAYS, live_job_filter};
        use crate::util::{nairobi_days_ago, nairobi_today};
        use futures_util::TryStreamExt;
        use mongodb::bson::doc;

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
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
            let mut document = doc! { "_id": *title, "title": *title };
            document.extend(extra.clone());
            jobs.insert_one(&document).await.unwrap();
        }

        let mut cursor = jobs.find(live_job_filter()).await.unwrap();
        let mut live: Vec<String> = Vec::new();
        while let Some(document) = cursor.try_next().await.unwrap() {
            live.push(document.get_str("title").unwrap().to_string());
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

        let _ = db.drop().await;
    }

    fn job_from(value: serde_json::Value) -> JobDoc {
        serde_json::from_value(value).expect("a minimal job document must deserialize")
    }

    /// A decided role title in the title is worth more than a loose keyword hit anywhere,
    /// and the weights are the ones the matcher documents: category +1, subcategory in the
    /// title +3, keyword in the title +1, keyword in the description +0.5.
    #[test]
    fn a_subcategory_phrase_in_the_title_outranks_a_keyword_hit() {
        use super::score_job;

        let profile = MatchProfile {
            categories: vec!["ict".to_string()],
            subcategories: vec!["Data Analyst".to_string()],
            keywords: vec!["analyst".to_string()],
            locations: Vec::new(),
            category_source: "llm".to_string(),
            facts_key: String::new(),
            updated_at: String::new(),
        };

        let exact = job_from(serde_json::json!({
            "_id": "job-1",
            "title": "Data Analyst",
            "description": "SQL reporting",
            "category": "ict",
        }));
        // No subcategory phrase, but the keyword is in the title.
        let keyword_title = job_from(serde_json::json!({
            "_id": "job-2",
            "title": "Analyst, ICT Unit",
            "description": "",
            "category": "ict",
        }));
        // Keyword only in the description.
        let keyword_description = job_from(serde_json::json!({
            "_id": "job-3",
            "title": "ICT Support Officer",
            "description": "Reporting to the analyst team",
            "category": "ict",
        }));

        let exact_score = score_job(&exact, &profile);
        assert_eq!(
            exact_score, 5.0,
            "category 1 + subcategory 3 + keyword in title 1"
        );
        assert_eq!(score_job(&keyword_title, &profile), 2.0);
        assert_eq!(score_job(&keyword_description, &profile), 1.5);
        assert!(
            exact_score > score_job(&keyword_title, &profile),
            "the decided role title must outrank a plain keyword hit"
        );

        // A member with no subcategories scores exactly as before this feature.
        let rules_only = MatchProfile {
            subcategories: Vec::new(),
            ..profile.clone()
        };
        assert_eq!(score_job(&exact, &profile), score_job(&exact, &profile));
        assert_eq!(score_job(&exact, &rules_only), 2.0);
    }

    /// The bot's one call: same rows and shape as `/jobs/match`, addressed by snowflake, and
    /// a 404 — not an empty list — for an account that does not exist.
    #[tokio::test]
    async fn the_for_user_route_404s_for_an_unknown_snowflake_and_lists_a_known_members_rows() {
        use super::{ForUserQuery, jobs_for_user};
        use crate::config::Config;
        use crate::state::AppState;
        use axum::extract::{Path, Query, State};

        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return;
        };
        let db = client.database("jobify_test_jobs_for_user");
        let _ = db.drop().await;
        let state = AppState::new(db.clone(), Config::from_env());

        let missing = jobs_for_user(
            State(state.clone()),
            Path("900000000000000000".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(missing, AppError::NotFound(_)),
            "an unknown snowflake is an unknown account"
        );

        db.collection::<Document>("discord_links")
            .insert_one(doc! {
                "_id": "link-901",
                "discord_id": "900000000000000901",
                "user_id": "user-901",
                "username": "sahra",
            })
            .await
            .unwrap();
        db.collection::<Document>("users")
            .insert_one(doc! {
                "_id": "user-901",
                "name": "Sahra",
                "education": [],
                "match_profile": {
                    "categories": ["ict"],
                    "subcategories": ["Data Analyst"],
                    "keywords": ["sql"],
                    "locations": [],
                    "category_source": "llm",
                },
            })
            .await
            .unwrap();
        db.collection::<Document>("jobs")
            .insert_one(doc! {
                "_id": "job-901",
                "title": "Data Analyst",
                "organization": "Hormuud",
                "description": "SQL reporting",
                "category": "ict",
                "deadline": crate::util::nairobi_days_ago(-5),
                "created_at": crate::util::now_iso(),
            })
            .await
            .unwrap();

        let (status, Json(body)) = jobs_for_user(
            State(state),
            Path("900000000000000901".to_string()),
            Query(ForUserQuery {
                limit: 5,
                kind: Some("job".to_string()),
            }),
        )
        .await
        .expect("a registered member must get a list");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["meta"]["reason"], "ok");
        assert_eq!(body["meta"]["categories"], serde_json::json!(["ict"]));
        assert_eq!(
            body["meta"]["subcategories"],
            serde_json::json!(["Data Analyst"])
        );
        assert_eq!(body["meta"]["categorySource"], "llm");
        let rows = body["data"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "rows: {body}");
        assert_eq!(rows[0]["job"]["_id"], "job-901");
        assert_eq!(
            rows[0]["matchScore"], 4.5,
            "category 1 + subcategory in title 3 + keyword 'sql' in the description 0.5"
        );
        assert_eq!(body["meta"]["queued"], 0, "a read never queues");

        let _ = db.drop().await;
    }
}
