use futures_util::TryStreamExt;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::models::JobInput;
use crate::state::AppState;
use crate::util::{nairobi_today, now_iso, uuid_id};

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

    // Exclude jobs whose deadline is already in the past (computed in Nairobi
    // time). A missing/null/empty deadline is not "in the past", so it stays.
    let deadline_filter = doc! {
        "$or": [
            doc! { "deadline": null },
            doc! { "deadline": "" },
            doc! { "deadline": doc! { "$gte": nairobi_today() } },
        ]
    };
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
            canonical_category(&raw("software_engineering_and_web_development")),
            Some("software_engineering_and_web_development")
        );
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(canonical_category(&raw("  wash  ")), Some("wash"));
    }

    #[test]
    fn rejects_unknown_and_free_text() {
        assert_eq!(canonical_category(&raw("totally_made_up_slug")), None);
        assert_eq!(canonical_category(&raw("Software Engineering")), None);
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
}
