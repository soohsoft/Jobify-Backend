//! Internal API for the Discord bot process.
//!
//! The bot is the only signup surface: a member talks to it, and it tells this service who
//! they are, what they are looking for, and when to mint a one-time link into the web
//! portal. Every route here is `x-api-key` only — nothing in this module is reachable by a
//! browser — and identity is always the Discord snowflake, never a name or an email.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::{Document, doc};
use mongodb::options::ReturnDocument;
use serde::Deserialize;
use serde_json::json;

use crate::credits;
use crate::error::AppError;
use crate::links;
use crate::models::{
    CompanyRequest, DiscordAlertsRequest, DiscordCompanyRequest, DiscordLinkDoc,
    DiscordProfileRequest, DiscordUserRequest, EducationEntry, LinkTokenDoc, LinkTokenRequest,
    MatchProfile, UserDoc, UserResponse,
};
use crate::onboarding;
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;
use super::alerts::apply_alert_prefs;
use super::auth::{MAX_PROFILE_KEYWORDS, MAX_PROFILE_TERM_CHARS, apply_company};
use super::jobs;
use super::subscription::subscription_for;

/// A portal link is a convenience, not a credential: 15 minutes by default, and never
/// longer than an hour.
pub const DEFAULT_LINK_TTL_SECS: i64 = 900;
pub const MIN_LINK_TTL_SECS: i64 = 300;
pub const MAX_LINK_TTL_SECS: i64 = 3600;

/// A Discord snowflake is a decimal string of digits. It is the account's immutable
/// identity, so it is validated as a shape rather than trusted as free text.
const SNOWFLAKE_MIN_CHARS: usize = 5;
const SNOWFLAKE_MAX_CHARS: usize = 32;

pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/internal/discord/users", post(upsert_user))
        .route("/internal/discord/profile", post(update_profile))
        .route("/internal/discord/link-token", post(mint_link_token))
        .route("/internal/discord/user/{discord_id}", get(get_user))
        // The member's current matches. The bot has no bearer token, so the Bearer-only
        // `GET /jobs/match` is unreachable from Discord; this delegates to the same matcher.
        .route("/internal/discord/matches/{discord_id}", get(get_matches))
        // The delivery queue carries a user id, so whoever drains it must be able to turn
        // that into a Discord address without holding per-user credentials.
        .route("/internal/discord/link/{user_id}", get(get_link))
        .route("/internal/discord/company", post(set_company))
        .route("/internal/discord/alerts", post(set_alert_prefs))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn snowflake(raw: &str) -> Result<String, AppError> {
    let id = raw.trim();
    if id.len() < SNOWFLAKE_MIN_CHARS
        || id.len() > SNOWFLAKE_MAX_CHARS
        || !id.chars().all(|c| c.is_ascii_digit())
    {
        return Err(AppError::BadRequest(
            "discord_id must be the numeric Discord user id (the snowflake)".to_string(),
        ));
    }
    Ok(id.to_string())
}

/// Truncate on a character boundary rather than a byte offset: a Discord display name can
/// contain anything, and slicing bytes panics mid-codepoint.
fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// "The interview is finished" means the two mandatory answers are stored: a name and at
/// least one education entry.
///
/// Deliberately NOT "the matcher has a category": a member whose only degree is recorded as
/// "Other: Marine biology" has answered every question they were asked, and refusing to call
/// that complete would re-interview them forever. They still get match keywords (the words of
/// that answer and of their last position), just no category filter.
pub fn onboarding_complete(user: &UserDoc) -> bool {
    !user.name.trim().is_empty() && !user.education.is_empty()
}

/// Skills become match keywords. Trimmed, case-insensitively deduped, blanks and
/// over-long entries dropped, capped at the matcher's keyword limit.
///
/// No longer asked by the interview (it asks for the last position instead), but still
/// accepted so a web client that writes skills keeps contributing keywords.
fn clean_skills(raw: &[String]) -> Vec<String> {
    let mut skills: Vec<String> = Vec::new();
    for value in raw {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_PROFILE_TERM_CHARS {
            tracing::warn!(skill = %trimmed, "discord profile: over-long skill dropped");
            continue;
        }
        if !skills.iter().any(|kept| kept.eq_ignore_ascii_case(trimmed)) {
            skills.push(trimmed.to_string());
        }
        if skills.len() >= MAX_PROFILE_KEYWORDS {
            break;
        }
    }
    skills
}

/// What the member asked to be alerted about.
///
/// `None` means "no stored preference", which every reader treats as both kinds — so
/// "both" is stored as absence rather than as a third value that behaves like a missing one.
/// An unknown value is a 400 naming the three accepted answers: this drives a query filter,
/// and "Tender" silently becoming nothing is exactly the kind of quiet bug to avoid.
fn canonical_wants(raw: &str) -> Result<Option<&'static str>, AppError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "jobs" => Ok(Some("jobs")),
        "tenders" => Ok(Some("tenders")),
        "both" | "" => Ok(None),
        _ => Err(AppError::BadRequest(
            "wants must be \"jobs\", \"tenders\" or \"both\"".to_string(),
        )),
    }
}

async fn link_for(state: &AppState, discord_id: &str) -> Result<DiscordLinkDoc, AppError> {
    state
        .discord_links()
        .find_one(doc! { "discord_id": discord_id })
        .await?
        .ok_or_else(|| {
            AppError::NotFound(
                "This Discord account is not registered with Jobify yet.".to_string(),
            )
        })
}

async fn load_user(state: &AppState, user_id: &str) -> Result<UserDoc, AppError> {
    state
        .users()
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))
}

// ---------------------------------------------------------------------------
// POST /internal/discord/users
// ---------------------------------------------------------------------------

/// Registers a Discord member on first contact and refreshes their handle afterwards.
///
/// Safe to call on every command: the second call for the same snowflake returns the same
/// user with `created: false`. The row is created by an **upsert on `discord_id`**, which
/// the unique index makes race-proof — two simultaneous first messages cannot both insert,
/// and the loser reads the winner's row instead of creating a second account.
async fn upsert_user(
    State(state): State<AppState>,
    Json(body): Json<DiscordUserRequest>,
) -> ApiResult {
    let discord_id = snowflake(&body.discord_id)?;
    let now = now_iso();

    let username = truncate(body.username.trim(), 64);
    let global_name = body
        .global_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| truncate(value, 120));

    let mut set = doc! { "username": &username, "updated_at": &now };
    if let Some(name) = &global_name {
        set.insert("global_name", name);
    }
    let mut update = doc! {
        "$set": set,
        "$setOnInsert": {
            "_id": uuid_id(),
            "discord_id": &discord_id,
            "user_id": uuid_id(),
            "created_at": &now,
        },
    };
    // A display name that was cleared in Discord must not linger here as a stale label.
    if global_name.is_none() {
        update.insert("$unset", doc! { "global_name": "" });
    }

    let upsert = state
        .discord_links()
        .find_one_and_update(doc! { "discord_id": &discord_id }, update.clone())
        .upsert(true)
        .return_document(ReturnDocument::After)
        .await;

    // A duplicate-key error means another call inserted the row between our filter and
    // our write. The row exists now, so the same upsert matches and updates it — one retry
    // is enough, and the alternative is failing a signup over a race we created.
    let link = match upsert {
        Ok(Some(link)) => link,
        Ok(None) => {
            return Err(AppError::Internal(
                "The Discord link could not be created".to_string(),
            ));
        }
        Err(_) => state
            .discord_links()
            .find_one_and_update(doc! { "discord_id": &discord_id }, update)
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await?
            .ok_or_else(|| {
                AppError::Internal("The Discord link could not be created".to_string())
            })?,
    };

    let existing = state
        .users()
        .find_one(doc! { "_id": &link.user_id })
        .await?;
    let created = existing.is_none();

    let user = match existing {
        Some(user) => {
            // The mirror field may be missing on a row created before this phase.
            if user.discord_id.as_deref() != Some(discord_id.as_str()) {
                state
                    .users()
                    .update_one(
                        doc! { "_id": &user.id },
                        doc! { "$set": { "discord_id": &discord_id } },
                    )
                    .await?;
                load_user(&state, &user.id).await?
            } else {
                user
            }
        }
        None => {
            let display_name = global_name
                .clone()
                .or_else(|| Some(username.clone()).filter(|name| !name.is_empty()))
                .unwrap_or_else(|| "Discord user".to_string());
            let user = UserDoc {
                id: link.user_id.clone(),
                name: display_name,
                // A Discord account has no address. `email` stays empty, which the partial
                // unique index on `email` explicitly excludes, so many Discord users can
                // coexist while real addresses stay unique.
                email: String::new(),
                password_hash: String::new(),
                provider: "discord".to_string(),
                // Discord has already proven who this is; there is no mailbox to verify.
                email_verified: true,
                phone: None,
                headline: None,
                avatar_url: None,
                role: "user".to_string(),
                location: None,
                match_profile: None,
                alerts: None,
                education_level: None,
                education: Vec::new(),
                last_position: None,
                experience_years: None,
                sectors: Vec::new(),
                skills: Vec::new(),
                wants: None,
                company: None,
                discord_id: Some(discord_id.clone()),
                created_at: now,
            };
            match state.users().insert_one(&user).await {
                Ok(_) => user,
                // Same race as above, one level down: the link row now exists, so read the
                // user the other call created.
                Err(_) => load_user(&state, &link.user_id).await?,
            }
        }
    };

    tracing::info!(user_id = %user.id, created, "discord user registered");
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "user": UserResponse::from_doc(&user),
                "created": created,
                "onboarding_complete": onboarding_complete(&user),
            }
        })),
    ))
}

// ---------------------------------------------------------------------------
// POST /internal/discord/profile
// ---------------------------------------------------------------------------

/// Writes what the bot collected during onboarding and derives `match_profile` from it.
///
/// The matching inputs are DERIVED here rather than sent by the bot, so there is exactly one
/// definition of them:
///   - `categories` = the union of the categories the recorded fields of study map to
///     (`onboarding::categories`) — this is the matcher's query filter;
///   - `keywords` = the words of `last_position` (at most four), the detail behind any
///     "Other" field, and any skills a web client still sends;
///   - `locations` stays EMPTY, always: Jobify covers all of Somalia, so there is no city to
///     rank by and the interview never asks for one.
///
/// An absent field is left alone and a blank one clears it (same rule as `PATCH /auth/me`),
/// because the bot asks one question at a time and a half-filled profile must not wipe the
/// answers already given. The education list is sent whole on every save, so `Some(list)`
/// replaces it and `None` leaves it untouched — that is what keeps a two-degree history
/// intact while the later steps are still being answered.
async fn update_profile(
    State(state): State<AppState>,
    Json(body): Json<DiscordProfileRequest>,
) -> ApiResult {
    let discord_id = snowflake(&body.discord_id)?;
    let link = link_for(&state, &discord_id).await?;
    let user = load_user(&state, &link.user_id).await?;

    if let Some(years) = body.experience_years
        && years > 60
    {
        return Err(AppError::BadRequest(
            "experience_years must be 60 or fewer".to_string(),
        ));
    }

    // Validate the whole list before ANY write: a bad third entry must not leave the first two
    // stored and the request half-applied.
    let education = match &body.education {
        Some(raw) => Some(onboarding::canonical_education(raw)?),
        None => None,
    };

    let mut set = Document::new();
    let mut unset = Document::new();

    if let Some(name) = body.name.as_deref() {
        let name = name.trim();
        // A blank name is ignored rather than stored: every screen falls back to it, and an
        // empty one renders as an empty header. Clearing it is not a useful operation.
        if !name.is_empty() {
            set.insert("name", truncate(name, 120));
        }
    }

    // `None` = the bot did not ask; `Some(None)` = the member cleared it; `Some(Some(v))` =
    // the member answered. That distinction is what keeps a partial payload non-destructive.
    //
    // A location is still accepted and stored for older clients, but it is no longer part of
    // the match profile: the interview does not ask, and matches are Somalia-wide.
    let location = body
        .location
        .as_deref()
        .map(str::trim)
        .map(|value| Some(truncate(value, 120)).filter(|value| !value.is_empty()));
    match &location {
        Some(None) => {
            unset.insert("location", "");
        }
        Some(Some(value)) => {
            set.insert("location", value);
        }
        None => {}
    }

    let last_position = body
        .last_position
        .as_deref()
        .map(str::trim)
        .map(|value| Some(truncate(value, 120)).filter(|value| !value.is_empty()));
    match &last_position {
        Some(None) => {
            unset.insert("last_position", "");
        }
        Some(Some(value)) => {
            set.insert("last_position", value);
        }
        None => {}
    }

    if let Some(years) = body.experience_years {
        set.insert("experience_years", i32::from(years));
    }

    if let Some(raw) = body.wants.as_deref() {
        match canonical_wants(raw)? {
            Some(wants) => {
                set.insert("wants", wants);
            }
            // "both" clears the field: absence is what "both" means everywhere it is read.
            None => {
                unset.insert("wants", "");
            }
        }
    }

    // What the matcher will read after this save.
    let effective_education: Vec<EducationEntry> = match &education {
        Some(list) => list.clone(),
        None => user.education.clone(),
    };
    if let Some(list) = &education {
        set.insert("education", mongodb::bson::to_bson(list)?);
        // Keep the single field the portal renders in step with the list.
        match onboarding::highest_level(list) {
            Some(level) => {
                set.insert("education_level", level);
            }
            None => {
                unset.insert("education_level", "");
            }
        }
    } else if let Some(raw) = body.education_level.as_deref() {
        // An older client sending a bare level: validated through the same list.
        set.insert("education_level", onboarding::canonical_level(raw)?);
    }

    let effective_last_position = match &last_position {
        Some(change) => change.clone(),
        None => user.last_position.clone(),
    };

    let skills = if body.skills.is_empty() {
        user.skills.clone()
    } else {
        clean_skills(&body.skills)
    };

    if !body.sectors.is_empty() {
        // The interview replaced sectors with education history. Honouring a stale client's
        // list as match categories would widen the matcher's filter behind the member's back.
        tracing::debug!(
            user_id = %user.id,
            "discord profile: sectors ignored, categories derive from education"
        );
    }

    let categories = onboarding::categories(&effective_education);
    let keywords = onboarding::match_keywords(
        effective_last_position.as_deref(),
        &effective_education,
        &skills,
    );

    let profile = MatchProfile {
        categories: categories.clone(),
        keywords,
        locations: Vec::new(),
        updated_at: now_iso(),
    };

    // `sectors` mirrors the derived categories so the two lists can never disagree.
    set.insert("sectors", &categories);
    set.insert("skills", &skills);
    set.insert("match_profile", mongodb::bson::to_bson(&profile)?);

    let mut update = doc! { "$set": set };
    if !unset.is_empty() {
        update.insert("$unset", unset);
    }
    state
        .users()
        .update_one(doc! { "_id": &user.id }, update)
        .await?;

    let mut updated = load_user(&state, &user.id).await?;

    // Finishing the interview switches the daily digest on — unless the member explicitly
    // turned alerts off, which `alerts.user_disabled` remembers across re-runs of `/start`.
    if onboarding::should_enable_alerts(onboarding_complete(&updated), updated.alerts.as_ref()) {
        apply_alert_prefs(&state, &updated.id, true, Some("daily".to_string())).await?;
        updated = load_user(&state, &user.id).await?;
    }

    let complete = onboarding_complete(&updated);
    tracing::info!(
        user_id = %updated.id,
        complete,
        education = updated.education.len(),
        categories = profile.categories.len(),
        keywords = updated.match_profile.as_ref().map(|p| p.keywords.len()).unwrap_or(0),
        alerts_enabled = updated.alerts.as_ref().map(|a| a.enabled).unwrap_or(false),
        "discord profile saved"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": UserResponse::from_doc(&updated),
            "meta": {
                "onboarding_complete": complete,
                "alerts_enabled": updated.alerts.as_ref().map(|a| a.enabled).unwrap_or(false),
            }
        })),
    ))
}

// ---------------------------------------------------------------------------
// POST /internal/discord/link-token
// ---------------------------------------------------------------------------

/// Mints a one-time portal link for a registered member.
///
/// The raw token is returned here and nowhere else: only its SHA-256 is stored, so this
/// response is the single moment the value exists outside the bot's message.
async fn mint_link_token(
    State(state): State<AppState>,
    Json(body): Json<LinkTokenRequest>,
) -> ApiResult {
    let discord_id = snowflake(&body.discord_id)?;
    let link = link_for(&state, &discord_id).await?;

    let ttl_seconds = body
        .ttl_seconds
        .unwrap_or(DEFAULT_LINK_TTL_SECS)
        .clamp(MIN_LINK_TTL_SECS, MAX_LINK_TTL_SECS);

    let raw = links::new_token();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let expires_ms = now_ms + ttl_seconds * 1000;

    let record = LinkTokenDoc {
        id: links::token_fingerprint(&raw),
        user_id: link.user_id.clone(),
        discord_id: discord_id.clone(),
        created_at: mongodb::bson::DateTime::from_millis(now_ms),
        expires_at: mongodb::bson::DateTime::from_millis(expires_ms),
        used_at: None,
    };
    state.link_tokens().insert_one(&record).await?;

    let expires_at = chrono::DateTime::from_timestamp_millis(expires_ms)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    tracing::info!(
        user_id = %link.user_id,
        ttl_seconds,
        "portal link minted"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "url": format!("{}/enter?t={}", state.config.app_base_url, raw),
                "token": raw,
                "expires_at": expires_at,
                "expires_in_seconds": ttl_seconds,
            }
        })),
    ))
}

// ---------------------------------------------------------------------------
// GET /internal/discord/user/{discord_id}
// ---------------------------------------------------------------------------

/// Everything the bot needs to answer `/me`, `/alerts` and `/credits` without a second
/// round trip — and without spending one: reading the ledger is not a query.
async fn get_user(State(state): State<AppState>, Path(discord_id): Path<String>) -> ApiResult {
    let discord_id = snowflake(&discord_id)?;
    let link = link_for(&state, &discord_id).await?;
    let user = load_user(&state, &link.user_id).await?;
    let subscription = subscription_for(&state, &user.id).await?;
    let credits = credits::snapshot(&state, &user.id).await?;
    let alerts = user.alerts.clone().unwrap_or_default();
    let alerts_enabled = alerts.enabled;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "user": UserResponse::from_doc(&user),
                "subscription_status": subscription.status,
                "onboarding_complete": onboarding_complete(&user),
                "alerts_enabled": alerts_enabled,
                "alerts": alerts,
                "credits": credits,
            }
        })),
    ))
}

// ---------------------------------------------------------------------------
// GET /internal/discord/matches/{discord_id}
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MatchesQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// The member's current matches, scored and ordered by the SAME matcher the portal's
/// `GET /jobs/match` uses.
///
/// The bot has no bearer token, so the protected route is unreachable from Discord. Rather
/// than mint a portal link just to read a list, this takes the snowflake and delegates to
/// `jobs::match_rows`. `record=false` always: the first digest is a READ, and reading must
/// never consume the alert queue (that is the same rule `/jobs` follows).
///
/// No profile yet is a normal answer — `reason: "no_match_profile"`, an empty list — not a 404.
async fn get_matches(
    State(state): State<AppState>,
    Path(discord_id): Path<String>,
    Query(query): Query<MatchesQuery>,
) -> ApiResult {
    let discord_id = snowflake(&discord_id)?;
    let link = link_for(&state, &discord_id).await?;
    let limit = query.limit.unwrap_or(5).clamp(1, 20);

    let (scored, categories) = jobs::match_rows(&state, &link.user_id, limit).await?;
    let reason = if categories.is_empty() {
        "no_match_profile"
    } else {
        "ok"
    };
    let data: Vec<serde_json::Value> = scored
        .iter()
        .map(|(score, job)| json!({ "job": job, "matchScore": score }))
        .collect();

    tracing::debug!(
        user_id = %link.user_id,
        rows = data.len(),
        categories = categories.len(),
        "discord matches read"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": { "reason": reason, "total": data.len(), "categories": categories }
        })),
    ))
}

// ---------------------------------------------------------------------------
// GET /internal/discord/link/{user_id}
// ---------------------------------------------------------------------------

/// The reverse of the link table: a user id (what the delivery queue carries) to the
/// Discord address that can receive a DM.
///
/// 404 — not an empty object — when there is no link: the sender must be able to tell "this
/// account has no Discord address" from "this address is fine" and record the difference.
async fn get_link(State(state): State<AppState>, Path(user_id): Path<String>) -> ApiResult {
    let user_id = user_id.trim().to_string();
    if user_id.is_empty() {
        return Err(AppError::BadRequest("user_id is required".to_string()));
    }
    let link = state
        .discord_links()
        .find_one(doc! { "user_id": &user_id })
        .await?
        .ok_or_else(|| {
            AppError::NotFound("This account has no Discord identity linked.".to_string())
        })?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "user_id": link.user_id,
                "discord_id": link.discord_id,
                "username": link.username,
                "global_name": link.global_name,
            }
        })),
    ))
}

// ---------------------------------------------------------------------------
// POST /internal/discord/company
// ---------------------------------------------------------------------------

/// The employer side of an account, written from Discord.
///
/// `PATCH /me/company` is Bearer-only and the bot has no bearer token, so this route carries
/// the snowflake instead and delegates to the SAME `apply_company` — one validator, one
/// normalization, one clear-on-empty rule for both surfaces.
async fn set_company(
    State(state): State<AppState>,
    Json(body): Json<DiscordCompanyRequest>,
) -> ApiResult {
    let discord_id = snowflake(&body.discord_id)?;
    let link = link_for(&state, &discord_id).await?;
    let updated = apply_company(
        &state,
        &link.user_id,
        CompanyRequest {
            name: body.name,
            services: body.services,
            website: body.website,
            description: body.description,
        },
    )
    .await?;

    tracing::info!(
        user_id = %updated.id,
        company_set = updated.company.is_some(),
        "discord company profile written"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": UserResponse::from_doc(&updated),
            "meta": { "company_set": updated.company.is_some() }
        })),
    ))
}

// ---------------------------------------------------------------------------
// POST /internal/discord/alerts
// ---------------------------------------------------------------------------

/// The `/alerts` toggle for a Discord member. Delegates to the same writer as `PUT /alerts`.
async fn set_alert_prefs(
    State(state): State<AppState>,
    Json(body): Json<DiscordAlertsRequest>,
) -> ApiResult {
    let discord_id = snowflake(&body.discord_id)?;
    let link = link_for(&state, &discord_id).await?;
    let alerts = apply_alert_prefs(&state, &link.user_id, body.enabled, body.mode).await?;

    tracing::info!(
        user_id = %link.user_id,
        enabled = alerts.enabled,
        mode = %alerts.mode,
        "discord alert preferences written"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "alerts": alerts, "alerts_enabled": alerts.enabled }
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snowflake_is_digits_only_and_bounded() {
        assert_eq!(
            snowflake(" 900000000000000001 ").unwrap(),
            "900000000000000001"
        );
        assert!(snowflake("abc123").is_err());
        assert!(snowflake("12 34").is_err());
        assert!(snowflake("").is_err());
        // A real snowflake is 17-20 digits; anything much longer is not an id.
        assert!(snowflake(&"9".repeat(SNOWFLAKE_MAX_CHARS + 1)).is_err());
    }

    #[test]
    fn skills_are_trimmed_deduped_and_capped() {
        let raw = vec![
            " Rust ".to_string(),
            "rust".to_string(),
            "".to_string(),
            "Postgres".to_string(),
        ];
        assert_eq!(clean_skills(&raw), vec!["Rust", "Postgres"]);

        let many: Vec<String> = (0..40).map(|n| format!("skill{n}")).collect();
        assert_eq!(clean_skills(&many).len(), MAX_PROFILE_KEYWORDS);

        // A term longer than a search term is dropped rather than stored.
        let long = vec!["x".repeat(MAX_PROFILE_TERM_CHARS + 1)];
        assert!(clean_skills(&long).is_empty());
    }

    #[test]
    fn onboarding_needs_a_name_and_at_least_one_education_entry() {
        let mut user = UserDoc {
            id: "u1".to_string(),
            name: "Sahra".to_string(),
            email: String::new(),
            password_hash: String::new(),
            provider: "discord".to_string(),
            email_verified: true,
            phone: None,
            headline: None,
            avatar_url: None,
            role: "user".to_string(),
            location: None,
            match_profile: None,
            alerts: None,
            education_level: None,
            education: Vec::new(),
            last_position: None,
            experience_years: None,
            sectors: Vec::new(),
            skills: Vec::new(),
            wants: None,
            company: None,
            discord_id: Some("900000000000000001".to_string()),
            created_at: String::new(),
        };
        // A name alone is not enough: the interview still owes the education step.
        assert!(!onboarding_complete(&user));

        // The DEFAULT a new Discord row is created with has a display name, so an account
        // that has not been interviewed is still not onboarded.
        user.education = vec![EducationEntry {
            level: "bachelor".to_string(),
            field: "Health".to_string(),
        }];
        assert!(onboarding_complete(&user));

        // An "Other" degree yields no category and is STILL complete: they answered.
        user.education = vec![EducationEntry {
            level: "master".to_string(),
            field: "Other: Marine biology".to_string(),
        }];
        user.match_profile = Some(MatchProfile {
            categories: Vec::new(),
            keywords: vec!["marine".to_string(), "biology".to_string()],
            locations: Vec::new(),
            updated_at: String::new(),
        });
        assert!(onboarding_complete(&user));

        // No name is not onboarding, however complete the education is.
        user.name = "   ".to_string();
        assert!(!onboarding_complete(&user));
    }

    #[test]
    fn wants_accepts_three_answers_and_stores_both_as_absence() {
        assert_eq!(canonical_wants("jobs").unwrap(), Some("jobs"));
        assert_eq!(canonical_wants("Tenders").unwrap(), Some("tenders"));
        assert_eq!(canonical_wants("  BOTH ").unwrap(), None);
        assert_eq!(canonical_wants("").unwrap(), None);
        let err = canonical_wants("cv help").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(err.message().contains("jobs"));
    }

    async fn test_state(db_name: &str) -> Option<AppState> {
        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return None;
        };
        let db = client.database(db_name);
        let _ = db.drop().await;
        Some(AppState::new(db, crate::config::Config::from_env()))
    }

    /// The delivery queue hands out a `userId`, so the drain must be able to turn that into
    /// a Discord address — and must 404, not return an empty object, when there is none.
    #[tokio::test]
    async fn the_reverse_lookup_resolves_a_user_id_and_404s_when_unlinked() {
        let Some(state) = test_state("jobify_test_discord_reverse_link").await else {
            return;
        };
        state
            .discord_links()
            .insert_one(&DiscordLinkDoc {
                id: "link-1".to_string(),
                discord_id: "900000000000000777".to_string(),
                user_id: "user-777".to_string(),
                username: "sahra".to_string(),
                global_name: Some("Sahra A.".to_string()),
                created_at: String::new(),
                updated_at: String::new(),
            })
            .await
            .unwrap();

        let (status, Json(body)) = get_link(State(state.clone()), Path("user-777".to_string()))
            .await
            .expect("a linked user must resolve");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["discord_id"], "900000000000000777");
        assert_eq!(body["data"]["username"], "sahra");
        assert_eq!(body["data"]["global_name"], "Sahra A.");

        let missing = get_link(State(state), Path("user-nobody".to_string()))
            .await
            .unwrap_err();
        assert!(matches!(missing, AppError::NotFound(_)));
    }

    /// A Discord member as the bot creates one: a display name, no interview answers yet.
    fn fresh_discord_user(id: &str, discord_id: &str, name: &str) -> UserDoc {
        UserDoc {
            id: id.to_string(),
            name: name.to_string(),
            email: String::new(),
            password_hash: String::new(),
            provider: "discord".to_string(),
            email_verified: true,
            phone: None,
            headline: None,
            avatar_url: None,
            role: "user".to_string(),
            location: None,
            match_profile: None,
            alerts: None,
            education_level: None,
            education: Vec::new(),
            last_position: None,
            experience_years: None,
            sectors: Vec::new(),
            skills: Vec::new(),
            wants: None,
            company: None,
            discord_id: Some(discord_id.to_string()),
            created_at: String::new(),
        }
    }

    fn link_for_test(discord_id: &str, user_id: &str) -> DiscordLinkDoc {
        DiscordLinkDoc {
            id: format!("link-{user_id}"),
            discord_id: discord_id.to_string(),
            user_id: user_id.to_string(),
            username: "sahra".to_string(),
            global_name: Some("Sahra A.".to_string()),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn profile_request(discord_id: &str) -> DiscordProfileRequest {
        DiscordProfileRequest {
            discord_id: discord_id.to_string(),
            name: Some("Sahra Ali".to_string()),
            education_level: None,
            education: Some(vec![
                EducationEntry {
                    level: "bachelor".to_string(),
                    field: "Health".to_string(),
                },
                EducationEntry {
                    level: "master".to_string(),
                    field: "Business administration".to_string(),
                },
            ]),
            last_position: Some("Senior Programme Officer".to_string()),
            experience_years: Some(3),
            location: None,
            sectors: Vec::new(),
            skills: Vec::new(),
            wants: None,
        }
    }

    /// The whole interview's last save, through the route the bot actually calls: the degrees
    /// are stored in order, the match inputs are derived in Rust, and finishing switches the
    /// daily digest on.
    #[tokio::test]
    async fn a_two_degree_profile_is_stored_and_derives_categories_and_keywords() {
        let Some(state) = test_state("jobify_test_discord_onboarding").await else {
            return;
        };
        let discord_id = "900000000000000888";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-onb"))
            .await
            .unwrap();
        state
            .users()
            .insert_one(fresh_discord_user("user-onb", discord_id, "Discord user"))
            .await
            .unwrap();

        let (status, Json(body)) =
            update_profile(State(state.clone()), Json(profile_request(discord_id)))
                .await
                .expect("a valid profile must be accepted");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["meta"]["onboarding_complete"], true);
        assert_eq!(body["meta"]["alerts_enabled"], true);

        let education = body["data"]["education"].as_array().unwrap();
        assert_eq!(education.len(), 2, "both degrees are kept, in order");
        assert_eq!(education[0]["level"], "bachelor");
        assert_eq!(education[0]["field"], "Health");
        assert_eq!(education[1]["level"], "master");

        let profile = &body["data"]["match_profile"];
        assert_eq!(
            profile["categories"],
            json!(["health", "business_and_finance"]),
            "the union of the two fields' categories, in entry order"
        );
        assert_eq!(
            profile["locations"],
            json!([]),
            "Somalia-wide: no city is ever derived"
        );
        let keywords: Vec<&str> = profile["keywords"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap())
            .collect();
        assert!(keywords.contains(&"programme"), "keywords: {keywords:?}");
        assert!(keywords.contains(&"officer"), "keywords: {keywords:?}");

        let stored = state
            .users()
            .find_one(doc! { "_id": "user-onb" })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.name, "Sahra Ali");
        assert_eq!(stored.education.len(), 2);
        assert_eq!(
            stored.last_position.as_deref(),
            Some("Senior Programme Officer")
        );
        assert_eq!(
            stored.education_level.as_deref(),
            Some("master"),
            "the single level field tracks the highest degree"
        );
        assert_eq!(stored.sectors, vec!["health", "business_and_finance"]);
        let alerts = stored.alerts.expect("finishing onboarding enables alerts");
        assert!(alerts.enabled);
        assert_eq!(alerts.mode, "daily");
        assert!(!alerts.user_disabled);
    }

    /// A rejected field must leave the document exactly as it was: no half-applied education.
    #[tokio::test]
    async fn an_unknown_field_is_refused_and_nothing_is_written() {
        let Some(state) = test_state("jobify_test_discord_onboarding_reject").await else {
            return;
        };
        let discord_id = "900000000000000889";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-reject"))
            .await
            .unwrap();
        state
            .users()
            .insert_one(fresh_discord_user(
                "user-reject",
                discord_id,
                "Discord user",
            ))
            .await
            .unwrap();

        let mut request = profile_request(discord_id);
        request.education = Some(vec![EducationEntry {
            level: "bachelor".to_string(),
            field: "Nursing science".to_string(),
        }]);

        let err = update_profile(State(state.clone()), Json(request))
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
        assert!(err.message().contains("Nursing science"));

        let stored = state
            .users()
            .find_one(doc! { "_id": "user-reject" })
            .await
            .unwrap()
            .unwrap();
        assert!(stored.education.is_empty());
        assert!(stored.match_profile.is_none());
        assert!(stored.alerts.is_none());
    }

    /// `/stop` is a promise: re-running the interview must not switch the digest back on.
    #[tokio::test]
    async fn completing_onboarding_does_not_reenable_alerts_after_stop() {
        let Some(state) = test_state("jobify_test_discord_onboarding_stop").await else {
            return;
        };
        let discord_id = "900000000000000890";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-stop"))
            .await
            .unwrap();
        let mut user = fresh_discord_user("user-stop", discord_id, "Discord user");
        user.alerts = Some(crate::models::AlertPrefs {
            enabled: false,
            mode: "daily".to_string(),
            user_disabled: true,
            ..Default::default()
        });
        state.users().insert_one(&user).await.unwrap();

        let (status, Json(body)) =
            update_profile(State(state.clone()), Json(profile_request(discord_id)))
                .await
                .unwrap();
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["meta"]["onboarding_complete"], true);
        assert_eq!(
            body["meta"]["alerts_enabled"], false,
            "an explicit /stop survives onboarding"
        );

        // ... and an explicit /alerts daily afterwards still works.
        let alerts = apply_alert_prefs(&state, "user-stop", true, Some("daily".to_string()))
            .await
            .unwrap();
        assert!(alerts.enabled);
        assert!(!alerts.user_disabled);
    }
}
