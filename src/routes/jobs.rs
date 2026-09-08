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
use crate::util::{now_iso, uuid_id};

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

    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.jobs(),
        filter,
        page,
        limit,
        doc! { "created_at": -1 },
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

        let now = now_iso();
        let id = uuid_id();
        let set_on_insert = doc! {
            "_id": &id,
            "created_at": &now,
        };

        let update = doc! {
            "$set": {
                "external_id": input.external_id.clone(),
                "source": input.source.clone(),
                "source_name": input.source_name.clone(),
                "title": input.title.clone(),
                "organization": input.organization.clone(),
                "location": input.location.clone(),
                "description": input.description.clone(),
                "requirements": input.requirements.clone(),
                "qualifications": input.qualifications.clone(),
                "posted_date": input.posted_date.clone(),
                "deadline": input.deadline.clone(),
                "url": input.url.clone(),
                "image_url": input.image_url.clone(),
                "organization_image_url": input.organization_image_url.clone(),
                "category": input.category.clone(),
                "employment_type": input.employment_type.clone(),
                "salary": input.salary.clone(),
                "updated_at": &now,
            },
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
