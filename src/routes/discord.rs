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
use serde_json::{Value, json};

use crate::credits;
use crate::error::AppError;
use crate::links;
use crate::models::{
    CompanyRequest, DiscordAlertsRequest, DiscordCompanyRequest, DiscordLinkDoc,
    DiscordProfileRequest, DiscordUserRequest, EducationEntry, LinkTokenDoc, LinkTokenRequest,
    UserDoc, UserResponse,
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
/// Ten minutes. Long enough to open the link on a phone, short enough that a forwarded
/// message stops working quickly.
pub const DEFAULT_LINK_TTL_SECS: i64 = 600;
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
        // Credits a member's wallet. Operator-only for now: WaafiPay will call the same
        // bookkeeping once its callback lands, so the ledger has one shape from day one.
        .route("/internal/wallet/topup", post(wallet_topup))
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

    // The facts the completion-time derivation reads. Everything below is decided from
    // these, never from what the request happened to carry.
    let effective_years = body.experience_years.or(user.experience_years);
    let effective_name = set
        .get_str("name")
        .map(str::to_string)
        .unwrap_or_else(|_| user.name.clone());
    let facts = crate::profile_llm::Facts {
        education: &effective_education,
        last_position: effective_last_position.as_deref(),
        experience_years: effective_years,
        skills: &skills,
    };
    let facts_key = crate::profile_llm::facts_key(&facts);
    let complete_now = !effective_name.trim().is_empty() && !effective_education.is_empty();

    // Reuse is decided by the answers, not by how the last decision was produced: if the
    // facts the derivation read are unchanged, the stored profile IS the decision for them,
    // and paying for the same call again on every partial save is waste. `category_source`
    // must still be set, which is what distinguishes a decision this backend made (or a
    // fallback it recorded) from a legacy document or a portal-authored profile.
    let reusable = user
        .match_profile
        .as_ref()
        .filter(|stored| {
            complete_now && !stored.category_source.is_empty() && stored.facts_key == facts_key
        })
        .cloned();

    let (categories, keywords, category_source) = if let Some(stored) = reusable {
        tracing::debug!(
            user_id = %user.id,
            "discord profile: facts unchanged, reusing the stored derivation"
        );
        (stored.categories, stored.keywords, stored.category_source)
    } else if complete_now {
        // One cheap call, and only now: a partial save never spends a query, and the member
        // must never end up unmatched because the model was down.
        match crate::profile_llm::derive(state.llm.as_ref(), &facts).await {
            Ok(derived) if derived.is_usable() => {
                let keywords = if derived.keywords.is_empty() {
                    // A model that returned areas but no terms must not leave the matcher
                    // with nothing to rank by.
                    keywords.clone()
                } else {
                    derived.keywords
                };
                tracing::info!(
                    user_id = %user.id,
                    categories = derived.categories.len(),
                    keywords = keywords.len(),
                    "discord profile: categories decided by the model"
                );
                (
                    derived.categories,
                    keywords,
                    crate::profile_llm::SOURCE_LLM.to_string(),
                )
            }
            Ok(derived) => {
                tracing::warn!(
                    user_id = %user.id,
                    categories = derived.categories.len(),
                    "discord profile: derivation returned no usable category, keeping the rules mapping"
                );
                (
                    categories,
                    keywords,
                    crate::profile_llm::SOURCE_RULES.to_string(),
                )
            }
            Err(detail) => {
                tracing::warn!(
                    user_id = %user.id,
                    error = %detail,
                    "discord profile: derivation unavailable, keeping the rules mapping"
                );
                (
                    categories,
                    keywords,
                    crate::profile_llm::SOURCE_RULES.to_string(),
                )
            }
        }
    } else {
        // Mid-interview: the deterministic map, so a half-answered profile already has
        // something to match on. No model call — the answers are still arriving.
        (
            categories,
            keywords,
            crate::profile_llm::SOURCE_RULES.to_string(),
        )
    };

    let mut profile = crate::profile_llm::profile(
        categories.clone(),
        keywords,
        &category_source,
        facts_key,
        now_iso(),
    );

    // The two preference lists the interview collects after the last job. They are preferences,
    // not derived facts, so an absent list keeps what is on file and an empty one clears it.
    let previous_profile = user.match_profile.clone().unwrap_or_default();
    profile.employer_types = match body.employer_types.as_ref() {
        Some(list) => onboarding::clean_employer_types(list)?,
        None => previous_profile.employer_types,
    };
    profile.positions = match body.positions.as_ref() {
        Some(list) => onboarding::clean_positions(list)?,
        None => previous_profile.positions,
    };

    // A member who skips the sector screen would otherwise finish the interview with an empty
    // `positions` list — and then score 40 on rows whose category they already qualify for,
    // never reaching the cut-off, i.e. "no jobs found" for a perfectly answered profile. The
    // derived categories are the honest default: they are what the matcher was going to query
    // anyway, and the member can override them on the sector screen at any time.
    if complete_now && profile.positions.is_empty() {
        // The derivation already caps categories at three, so this is at most three sectors.
        profile.positions = categories.clone();
    }

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
/// `POST /internal/wallet/topup` — adds money to a member's wallet and records it as a
/// top-up transaction, so the account page's ledger shows money in as well as money out.
async fn wallet_topup(State(state): State<AppState>, Json(body): Json<Value>) -> ApiResult {
    let discord_id = snowflake(
        body.get("discord_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default(),
    )?;
    let amount = body
        .get("amount_usd")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    if !(amount.is_finite() && amount > 0.0) {
        return Err(AppError::BadRequest(
            "amount_usd must be a positive number".to_string(),
        ));
    }

    let link = link_for(&state, &discord_id).await?;
    let now = now_iso();

    state
        .wallets()
        .update_one(
            doc! { "_id": &link.user_id },
            doc! {
                "$inc": { "balance_usd": amount },
                "$set": { "updated_at": &now },
                "$setOnInsert": { "auto_deduct": false },
            },
        )
        .upsert(true)
        .await?;

    let reference = format!("JOBIFY-TOPUP-{}", &uuid_id()[..8]);
    let txn = crate::models::WalletTxnDoc {
        id: uuid_id(),
        user_id: link.user_id.clone(),
        amount_usd: amount,
        kind: "topup".to_string(),
        reference: reference.clone(),
        created_at: now,
    };
    state.wallet_txns().insert_one(&txn).await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "amount_usd": amount, "reference": reference, "user_id": link.user_id }
        })),
    ))
}

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

    let outcome = jobs::match_rows(
        &state,
        &link.user_id,
        limit,
        jobs::FeedScope::Both,
        None,
        None,
    )
    .await?;
    let data: Vec<serde_json::Value> = outcome
        .rows
        .iter()
        .map(|(score, job)| json!({ "job": job, "matchScore": score }))
        .collect();

    tracing::debug!(
        user_id = %link.user_id,
        rows = data.len(),
        considered = outcome.considered,
        threshold = outcome.threshold,
        reason = outcome.reason(),
        categories = outcome.areas.categories.len(),
        "discord matches read"
    );
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": data,
            "meta": {
                "reason": outcome.reason(),
                "threshold": outcome.threshold,
                "total": data.len(),
                "considered": outcome.considered,
                "qualified": outcome.qualified,
                "categories": outcome.areas.categories,
                "categorySource": outcome.areas.source,
            }
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
            location: body.location,
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
    use crate::models::MatchProfile;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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
            category_source: "rules".to_string(),
            facts_key: String::new(),
            updated_at: String::new(),
            ..Default::default()
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
        test_state_with_llm(db_name, Arc::new(ScriptedLlm::new(vec![]))).await
    }

    /// The same throwaway database with an injected provider, so no test ever reaches the
    /// network — the derivation is one of the two places a request can leave the process.
    async fn test_state_with_llm(db_name: &str, llm: Arc<ScriptedLlm>) -> Option<AppState> {
        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return None;
        };
        let db = client.database(db_name);
        let _ = db.drop().await;
        Some(AppState::with_llm(
            db,
            crate::config::Config::from_env(),
            llm,
        ))
    }

    /// Answers in order, and counts the calls so "no second call when the facts did not
    /// change" can be asserted rather than assumed.
    struct ScriptedLlm {
        replies: Mutex<VecDeque<Result<crate::llm::LlmReply, crate::llm::LlmError>>>,
        calls: AtomicUsize,
    }

    impl ScriptedLlm {
        fn new(replies: Vec<Result<crate::llm::LlmReply, crate::llm::LlmError>>) -> Self {
            Self {
                replies: Mutex::new(replies.into_iter().collect()),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl crate::llm::LlmClient for ScriptedLlm {
        fn complete<'a>(
            &'a self,
            _request: crate::llm::LlmRequest,
        ) -> futures_util::future::BoxFuture<'a, Result<crate::llm::LlmReply, crate::llm::LlmError>>
        {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(crate::llm::LlmError::Provider(500)));
            Box::pin(std::future::ready(next))
        }
    }

    fn answer(content: &str) -> Result<crate::llm::LlmReply, crate::llm::LlmError> {
        Ok(crate::llm::LlmReply {
            content: content.to_string(),
            usage: crate::llm::LlmUsage::default(),
            finish_reason: Some("stop".to_string()),
        })
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
            employer_types: None,
            positions: None,
            skills: Vec::new(),
            wants: None,
        }
    }

    /// The whole interview's last save, through the route the bot actually calls: the degrees
    /// are stored in order, the model's decision is what gets stored, and finishing switches
    /// the daily digest on.
    #[tokio::test]
    async fn a_two_degree_profile_is_stored_and_the_model_decides_the_areas() {
        let llm = Arc::new(ScriptedLlm::new(vec![answer(
            r#"{"categories":["health","business_and_finance"],"keywords":["programme","officer","health"]}"#,
        )]));
        let Some(state) = test_state_with_llm("jobify_test_discord_onboarding", llm.clone()).await
        else {
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
        assert_eq!(llm.calls(), 1, "completion is one derivation call");

        let education = body["data"]["education"].as_array().unwrap();
        assert_eq!(education.len(), 2, "both degrees are kept, in order");
        assert_eq!(education[0]["level"], "bachelor");
        assert_eq!(education[0]["field"], "Health");
        assert_eq!(education[1]["level"], "master");

        let profile = &body["data"]["match_profile"];
        assert_eq!(
            profile["categories"],
            json!(["health", "business_and_finance"]),
            "the model's validated slugs, in the order it gave them"
        );
        assert_eq!(profile["category_source"], "llm");
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

        let stored_profile = stored.match_profile.expect("a complete profile has one");
        assert_eq!(stored_profile.category_source, "llm");
        assert_eq!(
            stored_profile.facts_key.len(),
            64,
            "a fingerprint of the answers is kept so the next save can skip the model"
        );
    }

    /// A provider that is down must not cost the member their matches: the deterministic
    /// field map is kept, the source says so, and a re-save with the SAME answers reuses the
    /// stored decision instead of paying for the model again.
    #[tokio::test]
    async fn a_failed_derivation_falls_back_to_rules_and_an_unchanged_resave_does_not_call_again() {
        // The first call is a provider failure; the scripted stub then answers correctly for
        // the second save, which is what proves the reuse path did NOT call it.
        let llm = Arc::new(ScriptedLlm::new(vec![
            Err(crate::llm::LlmError::Provider(503)),
            answer(r#"{"categories":["health","business_and_finance"],"keywords":[]}"#),
        ]));
        let Some(state) =
            test_state_with_llm("jobify_test_discord_onboarding_fallback", llm.clone()).await
        else {
            return;
        };
        let discord_id = "900000000000000891";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-fallback"))
            .await
            .unwrap();
        state
            .users()
            .insert_one(fresh_discord_user(
                "user-fallback",
                discord_id,
                "Discord user",
            ))
            .await
            .unwrap();

        let request = profile_request(discord_id);
        let (_, Json(body)) = update_profile(State(state.clone()), Json(request.clone()))
            .await
            .expect("a provider outage must not fail the save");
        assert_eq!(llm.calls(), 1);

        let profile = &body["data"]["match_profile"];
        assert_eq!(
            profile["category_source"], "rules",
            "the fallback says where the areas came from"
        );
        assert_eq!(
            profile["categories"],
            json!(["health", "business_and_finance"]),
            "the deterministic map still gives the matcher a category filter"
        );
        assert_eq!(
            body["meta"]["onboarding_complete"], true,
            "the interview is still finished"
        );

        // Same answers again: reuse, no second call, still "rules" (a rules profile is only
        // retried once the answers change).
        let (_, Json(again)) = update_profile(State(state.clone()), Json(request))
            .await
            .unwrap();
        assert_eq!(
            llm.calls(),
            1,
            "an unchanged re-save must not spend another derivation"
        );
        assert_eq!(again["data"]["match_profile"]["category_source"], "rules");
        assert!(
            !again["data"]["match_profile"]["categories"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    /// An answer with no usable category is the same failure as an outage: keep the rules.
    #[tokio::test]
    async fn an_answer_with_no_known_category_is_not_stored_as_a_decision() {
        let llm = Arc::new(ScriptedLlm::new(vec![answer(
            r#"{"categories":["computer_technology"],"keywords":["sql"]}"#,
        )]));
        let Some(state) = test_state_with_llm("jobify_test_discord_onboarding_bad_slug", llm).await
        else {
            return;
        };
        let discord_id = "900000000000000892";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-bad-slug"))
            .await
            .unwrap();
        state
            .users()
            .insert_one(fresh_discord_user(
                "user-bad-slug",
                discord_id,
                "Discord user",
            ))
            .await
            .unwrap();

        let (_, Json(body)) =
            update_profile(State(state.clone()), Json(profile_request(discord_id)))
                .await
                .unwrap();
        let profile = &body["data"]["match_profile"];
        assert_eq!(profile["category_source"], "rules");
        assert_eq!(
            profile["categories"],
            json!(["health", "business_and_finance"])
        );
    }

    /// A complete profile whose answers did not change reuses the stored model decision.
    #[tokio::test]
    async fn an_unchanged_resave_reuses_the_model_decision() {
        let first = answer(r#"{"categories":["health"],"keywords":["programme"]}"#);
        // A second reply is queued but must never be consumed.
        let second = answer(r#"{"categories":["education"],"keywords":[]}"#);
        let llm = Arc::new(ScriptedLlm::new(vec![first, second]));
        let Some(state) =
            test_state_with_llm("jobify_test_discord_onboarding_reuse", llm.clone()).await
        else {
            return;
        };
        let discord_id = "900000000000000893";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-reuse"))
            .await
            .unwrap();
        state
            .users()
            .insert_one(fresh_discord_user("user-reuse", discord_id, "Discord user"))
            .await
            .unwrap();

        let request = profile_request(discord_id);
        let _ = update_profile(State(state.clone()), Json(request.clone()))
            .await
            .unwrap();
        let (_, Json(again)) = update_profile(State(state.clone()), Json(request))
            .await
            .unwrap();
        assert_eq!(llm.calls(), 1, "the second save must reuse, not re-derive");
        assert_eq!(
            again["data"]["match_profile"]["categories"],
            json!(["health"])
        );
        assert_eq!(again["data"]["match_profile"]["category_source"], "llm");
    }

    /// A partial save never spends a derivation: the answers are still arriving.
    #[tokio::test]
    async fn a_partial_save_never_calls_the_model() {
        let llm = Arc::new(ScriptedLlm::new(vec![answer(
            r#"{"categories":["ict"],"keywords":[]}"#,
        )]));
        let Some(state) =
            test_state_with_llm("jobify_test_discord_onboarding_partial", llm.clone()).await
        else {
            return;
        };
        let discord_id = "900000000000000894";
        state
            .discord_links()
            .insert_one(link_for_test(discord_id, "user-partial"))
            .await
            .unwrap();
        state
            .users()
            .insert_one(fresh_discord_user(
                "user-partial",
                discord_id,
                "Discord user",
            ))
            .await
            .unwrap();

        // Education not answered yet: the interview is not finished.
        let mut request = profile_request(discord_id);
        request.education = None;
        let (_, Json(body)) = update_profile(State(state.clone()), Json(request))
            .await
            .unwrap();
        assert_eq!(body["meta"]["onboarding_complete"], false);
        assert_eq!(llm.calls(), 0, "a partial save must not call the model");
        assert_eq!(body["data"]["match_profile"]["category_source"], "rules");
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
