use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::StatusCode,
    routing::{get, post, put},
};
use mongodb::bson::doc;
use serde_json::{Value, json};
use std::net::SocketAddr;

use crate::auth::{AuthUser, hash_password, issue_token, verify_password};
use crate::categories::Category;
use crate::error::AppError;
use crate::links;
use crate::models::{
    CompanyProfile, CompanyRequest, EmailOtpDoc, ForgotPasswordRequest, LoginRequest,
    MagicLinkRequest, MatchProfile, MatchProfileRequest, RegisterRequest, ResendOtpRequest,
    ResetPasswordRequest, UpdateProfileRequest, UserDoc, UserResponse, VerifyEmailRequest,
};
use crate::otp;
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;
use super::subscription;

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        // The web portal has no signup and no password: the bot mints a one-time link and
        // the portal trades it here for a session.
        .route("/auth/magic", post(magic_link))
        // Both are public by necessity: the user cannot authenticate, that is the problem.
        // Neither confirms whether an address has an account.
        .route("/auth/forgot-password", post(forgot_password))
        .route("/auth/reset-password", post(reset_password))
}

/// Both verification routes are AUTHENTICATED, not keyed on an email in the body.
/// Register and login already return a token for an unverified account, so the client
/// always has one, and binding the code to the caller means this cannot be aimed at
/// someone else's mailbox.
pub fn verification_router() -> Router<AppState> {
    Router::new()
        .route("/auth/verify-email", post(verify_email))
        .route("/auth/resend-otp", post(resend_otp))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(me).patch(update_me))
        // The company/vendor profile is its own resource rather than a field on the user:
        // it is the employer side of the account, with its own validation and caps.
        .route("/me/company", axum::routing::patch(update_company))
        // The match profile has its own endpoint rather than riding on PATCH /auth/me: it is
        // the one field the matcher and the alert queue read, so it gets its own validation,
        // its own caps and a verb that says "this replaces the whole profile".
        .route(
            "/me/match-profile",
            put(put_match_profile).delete(delete_match_profile),
        )
}

/// Creates (or replaces) the account's code, stores only its hash, and mails it.
///
/// Replacing rather than accumulating is deliberate: with one row per (user, purpose), an
/// older code stops working the moment a newer one is issued, so a resend cannot leave two
/// live keys. A mail failure is returned but never fatal to the caller's own operation.
async fn issue_otp(
    state: &AppState,
    user_id: &str,
    email: &str,
    name: &str,
    purpose: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now();
    let code = otp::generate_code();
    let key = otp::otp_key(user_id, purpose);
    let record = EmailOtpDoc {
        id: key.clone(),
        user_id: user_id.to_string(),
        purpose: purpose.to_string(),
        email: email.to_string(),
        code_hash: otp::hash_code(&code),
        expires_at: otp::expires_in(now, otp::ttl_minutes(purpose)),
        attempts: 0,
        last_sent_at: now.to_rfc3339(),
        created_at: now.to_rfc3339(),
    };

    state
        .email_otps()
        .replace_one(doc! { "_id": &key }, &record)
        .upsert(true)
        .await?;

    let (subject, body) = match purpose {
        otp::PURPOSE_RESET => crate::mail::password_reset_email(&code, name),
        _ => crate::mail::verification_email(&code, name),
    };
    if let Err(err) = state.mailer.send(email, &subject, &body).await {
        // The code is stored either way, so a transient mail failure is recoverable with
        // "resend" instead of leaving the account in a state it cannot leave.
        tracing::warn!(email = %otp::mask_email(email), purpose, error = %err, "email could not be sent");
        return Err(AppError::Internal(
            "The email could not be sent just now. Please try again in a moment.".to_string(),
        ));
    }
    tracing::info!(email = %otp::mask_email(email), purpose, "code sent");
    Ok(())
}

async fn register(State(state): State<AppState>, Json(body): Json<RegisterRequest>) -> ApiResult {
    otp::validate_password(&body.password).map_err(AppError::BadRequest)?;
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(AppError::BadRequest("Enter your email address".to_string()));
    }
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Enter your name".to_string()));
    }

    if state
        .users()
        .find_one(doc! { "email": &email })
        .await?
        .is_some()
    {
        // 409, not 400: the address is valid, it is already taken.
        return Err(AppError::Conflict(
            "Email is already registered".to_string(),
        ));
    }

    let now = now_iso();
    let id = uuid_id();
    let user = UserDoc {
        id: id.clone(),
        name,
        email: email.clone(),
        password_hash: hash_password(&body.password),
        provider: "password".to_string(),
        email_verified: false,
        phone: None,
        headline: None,
        avatar_url: None,
        role: "user".to_string(),
        location: None,
        match_profile: None,
        alerts: None,
        education_level: None,
        experience_years: None,
        sectors: Vec::new(),
        skills: Vec::new(),
        company: None,
        // A password account has no Discord identity until it is linked.
        discord_id: None,
        created_at: now,
    };

    state.users().insert_one(&user).await?;

    let token = issue_token(
        &state.config.jwt_secret,
        &state.config.jwt_issuer,
        &id,
        "user",
    )?;

    // A failure to mail is reported in the response, not thrown: the account and its token
    // are already valid, and losing them would be worse than an inbox the user has to
    // re-request. The client shows the code screen either way and offers "resend".
    let delivery = match issue_otp(&state, &id, &email, &user.name, otp::PURPOSE_VERIFY).await {
        Ok(()) => json!({
            "sent": true,
            "to": otp::mask_email(&email),
            "expires_in_minutes": otp::OTP_TTL_MINUTES
        }),
        Err(err) => json!({
            "sent": false,
            "to": otp::mask_email(&email),
            "expires_in_minutes": otp::OTP_TTL_MINUTES,
            "error": err.message().to_string()
        }),
    };

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "data": {
                "user": UserResponse::from_doc(&user),
                "token": token,
                "verification": delivery
            },
            "meta": { "email_verification_required": true }
        })),
    ))
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> ApiResult {
    let email = body.email.trim().to_lowercase();
    let user = state
        .users()
        .find_one(doc! { "email": &email })
        .await?
        .ok_or_else(|| AppError::Unauthorized("Invalid email or password".to_string()))?;

    // An account created through an OAuth provider has no password_hash. Comparing against
    // the empty string would fail anyway, but saying so is the difference between a user
    // retrying and a user finding the right button.
    if user.provider != "password" {
        return Err(AppError::Unauthorized(format!(
            "This account signs in with {}. Use that option instead.",
            provider_label(&user.provider)
        )));
    }

    if !verify_password(&body.password, &user.password_hash) {
        return Err(AppError::Unauthorized(
            "Invalid email or password".to_string(),
        ));
    }

    let token = issue_token(
        &state.config.jwt_secret,
        &state.config.jwt_issuer,
        &user.id,
        &user.role,
    )?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "user": UserResponse::from_doc(&user), "token": token }
        })),
    ))
}

// ---------------------------------------------------------------------------
// Portal sign-in (one-time link)
// ---------------------------------------------------------------------------

/// `POST /auth/magic` — the ONLY way into the web portal, and the only session endpoint
/// that is neither a password nor a signup.
///
/// Single use is enforced by the database, not by a read-then-write: the exchange is one
/// `find_one_and_update` whose filter requires `used_at: null` and an unexpired
/// `expires_at`. Two parallel exchanges with the same token therefore cannot both match —
/// the loser sees `None` and gets the same 400 as an unknown token.
///
/// Every failure answers with the SAME message and status. The endpoint is public, so
/// distinguishing "never existed" from "expired" from "already used" would tell an attacker
/// which guesses were once real. Attempts are logged (by IP, never by token) and a coarse
/// per-IP limit keeps one client from walking the space.
async fn magic_link(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<MagicLinkRequest>,
) -> ApiResult {
    let ip = addr.ip().to_string();
    if !state.magic_limiter.check(&ip) {
        tracing::warn!(ip = %ip, "magic link exchange refused: too many attempts");
        return Err(AppError::TooManyRequests(
            "Too many sign-in attempts. Wait a few minutes and try again.".to_string(),
        ));
    }

    let invalid = || AppError::BadRequest("This link has expired or was already used".to_string());

    let raw = body.token.trim();
    if raw.is_empty() {
        tracing::info!(ip = %ip, "magic link exchange with no token");
        return Err(invalid());
    }

    let now = mongodb::bson::DateTime::from_millis(chrono::Utc::now().timestamp_millis());
    let burned = state
        .link_tokens()
        .find_one_and_update(
            doc! {
                "_id": links::token_fingerprint(raw),
                "used_at": null,
                "expires_at": { "$gt": now },
            },
            doc! { "$set": { "used_at": now } },
        )
        .await?;

    let Some(token) = burned else {
        tracing::info!(ip = %ip, "magic link exchange refused: unknown, used or expired");
        return Err(invalid());
    };

    let user = state
        .users()
        .find_one(doc! { "_id": &token.user_id })
        .await?
        .ok_or_else(|| {
            // The token was valid but its account is gone. This is a data problem, not a
            // bad link, so it is reported as an error and not disguised as one.
            tracing::error!(user_id = %token.user_id, "magic link points at a missing account");
            AppError::Internal("This link no longer points at an account".to_string())
        })?;

    let jwt = issue_token(
        &state.config.jwt_secret,
        &state.config.jwt_issuer,
        &user.id,
        &user.role,
    )?;
    let subscription = subscription::subscription_for(&state, &user.id).await?;

    tracing::info!(user_id = %user.id, ip = %ip, "portal session issued from a one-time link");
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "user": UserResponse::from_doc(&user),
                "token": jwt,
                "subscription": subscription::payload(&subscription),
            }
        })),
    ))
}

async fn me(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> ApiResult {
    let user_doc = load_user(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": UserResponse::from_doc(&user_doc) })),
    ))
}

/// PATCH /auth/me — what the Settings screen saves. The account is taken from the token,
/// never the body, so this can only ever edit the caller.
///
/// Absent fields are left alone; a present-but-blank field is cleared. Name is refused when
/// blank because every screen falls back to it, and `""` renders as an empty header.
async fn update_me(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<UpdateProfileRequest>,
) -> ApiResult {
    if body.email.is_some() {
        return Err(AppError::BadRequest(
            "Email cannot be changed here — it is your sign-in address.".to_string(),
        ));
    }

    let mut set = doc! {};
    let mut unset = doc! {};

    if let Some(name) = body.name.as_deref() {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("Name cannot be empty".to_string()));
        }
        if name.chars().count() > 120 {
            return Err(AppError::BadRequest(
                "Name is too long (120 characters maximum)".to_string(),
            ));
        }
        set.insert("name", name.to_string());
    }

    for (field, value) in [
        ("phone", body.phone),
        ("location", body.location),
        ("headline", body.headline),
    ] {
        match value {
            Some(value) if value.trim().is_empty() => {
                unset.insert(field, "");
            }
            Some(value) => {
                set.insert(field, value.trim().to_string());
            }
            None => {}
        }
    }

    if !set.is_empty() || !unset.is_empty() {
        let mut update = doc! {};
        if !set.is_empty() {
            update.insert("$set", set);
        }
        if !unset.is_empty() {
            update.insert("$unset", unset);
        }
        state
            .users()
            .update_one(doc! { "_id": &user.id }, update)
            .await?;
    }

    // Read back rather than echo the request: then the response is the stored document,
    // and a client can trust it as the new state.
    let updated = load_user(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": UserResponse::from_doc(&updated) })),
    ))
}

// ---------------------------------------------------------------------------
// Match profile
// ---------------------------------------------------------------------------

/// Limits on what a profile may hold. Small on purpose: the matcher scores every candidate
/// against every keyword, and each entry is also read on every alert run.
pub const MAX_PROFILE_CATEGORIES: usize = 10;
pub const MAX_PROFILE_KEYWORDS: usize = 20;
pub const MAX_PROFILE_LOCATIONS: usize = 5;
/// A keyword or location is a search term, not a sentence.
pub const MAX_PROFILE_TERM_CHARS: usize = 60;

/// Turns a request into the document shape the matcher already understands, or explains
/// which input it will not take.
///
/// Categories are the matcher's QUERY FILTER, so an unknown slug is refused here by name.
/// Job ingest does the opposite — it nulls an unknown slug and keeps the listing — but that
/// is a scraped board's free text; this is a person typing, and silently dropping half of
/// what they asked for is worse than saying which entry is wrong.
fn validate_match_profile(body: MatchProfileRequest) -> Result<MatchProfile, AppError> {
    let mut categories: Vec<String> = Vec::new();
    for raw in &body.categories {
        let trimmed = raw.trim();
        let Some(category) = Category::from_slug(trimmed) else {
            return Err(AppError::BadRequest(if trimmed.is_empty() {
                "A category cannot be blank.".to_string()
            } else {
                format!(
                    "'{trimmed}' is not a category this app recognises. Pick from the category list."
                )
            }));
        };
        let slug = category.slug().to_string();
        if !categories.contains(&slug) {
            categories.push(slug);
        }
    }
    if categories.len() > MAX_PROFILE_CATEGORIES {
        return Err(AppError::BadRequest(format!(
            "Choose at most {MAX_PROFILE_CATEGORIES} categories — you picked {}.",
            categories.len()
        )));
    }

    let keywords = clean_terms(body.keywords, MAX_PROFILE_KEYWORDS, "keyword")?;
    let locations = clean_terms(body.locations, MAX_PROFILE_LOCATIONS, "location")?;

    // An empty category list is a valid profile that matches nothing: `GET /jobs/match`
    // already answers `meta.reason = "no_match_profile"` for it, which is the same state a
    // brand-new account is in. Refusing it would leave a client unable to clear its own
    // profile except by calling DELETE.
    Ok(MatchProfile {
        categories,
        keywords,
        locations,
        updated_at: now_iso(),
    })
}

/// Trim, drop blanks, collapse case-insensitive duplicates (keeping the first spelling),
/// refuse anything longer than a term should be, and enforce the cap.
fn clean_terms(raw: Vec<String>, cap: usize, noun: &str) -> Result<Vec<String>, AppError> {
    let mut terms: Vec<String> = Vec::new();
    for value in raw {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_PROFILE_TERM_CHARS {
            return Err(AppError::BadRequest(format!(
                "Each {noun} must be {MAX_PROFILE_TERM_CHARS} characters or fewer — '{}' is {}.",
                trimmed,
                trimmed.chars().count()
            )));
        }
        if !terms
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            terms.push(trimmed.to_string());
        }
    }
    if terms.len() > cap {
        return Err(AppError::BadRequest(format!(
            "Add at most {cap} {noun}s — you entered {}.",
            terms.len()
        )));
    }
    Ok(terms)
}

/// PUT /me/match-profile — replaces the caller's profile. The account comes from the token,
/// so this can only ever write the caller's own document.
async fn put_match_profile(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<MatchProfileRequest>,
) -> ApiResult {
    let profile = validate_match_profile(body)?;
    let stored = mongodb::bson::to_bson(&profile)?;

    state
        .users()
        .update_one(
            doc! { "_id": &user.id },
            doc! { "$set": { "match_profile": stored } },
        )
        .await?;

    // Read back rather than echo: the response is then the stored document, and a client can
    // treat it as the new state (same rule as PATCH /auth/me).
    let updated = load_user(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": UserResponse::from_doc(&updated) })),
    ))
}

/// DELETE /me/match-profile — removes the field entirely rather than storing an empty
/// profile, so `GET /auth/me` stops echoing one and the matcher answers `no_match_profile`.
async fn delete_match_profile(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    state
        .users()
        .update_one(
            doc! { "_id": &user.id },
            doc! { "$unset": { "match_profile": "" } },
        )
        .await?;

    let updated = load_user(&state, &user.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": UserResponse::from_doc(&updated) })),
    ))
}

// ---------------------------------------------------------------------------
// Company / vendor profile
// ---------------------------------------------------------------------------

/// A company is a shop window, not a CV: ten services with short labels is the whole
/// shape, and a list that grows without limit stops being readable.
pub const MAX_COMPANY_SERVICES: usize = 10;
pub const MAX_COMPANY_SERVICE_CHARS: usize = 40;
pub const MAX_COMPANY_NAME_CHARS: usize = 120;
pub const MAX_COMPANY_WEBSITE_CHARS: usize = 300;
pub const MAX_COMPANY_DESCRIPTION_CHARS: usize = 1000;

/// Trim, drop blanks, collapse case-insensitive duplicates, and refuse anything over the
/// cap instead of silently keeping ten of twelve: a caller that loses two services without
/// being told will believe they were saved.
fn clean_services(raw: Vec<String>) -> Result<Vec<String>, AppError> {
    let mut services: Vec<String> = Vec::new();
    for value in raw {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() > MAX_COMPANY_SERVICE_CHARS {
            return Err(AppError::BadRequest(format!(
                "Each service must be {MAX_COMPANY_SERVICE_CHARS} characters or fewer — '{}' is {}.",
                trimmed,
                trimmed.chars().count()
            )));
        }
        if !services
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(trimmed))
        {
            services.push(trimmed.to_string());
        }
        if services.len() > MAX_COMPANY_SERVICES {
            return Err(AppError::BadRequest(format!(
                "List at most {MAX_COMPANY_SERVICES} services — you entered {}.",
                services.len()
            )));
        }
    }
    Ok(services)
}

fn normalize_optional(
    value: Option<String>,
    max: usize,
    noun: &str,
) -> Result<Option<String>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > max {
        return Err(AppError::BadRequest(format!(
            "The {noun} must be {max} characters or fewer."
        )));
    }
    Ok(Some(trimmed.to_string()))
}

/// `PATCH /me/company` — the employer/vendor side of an account.
///
/// An empty name CLEARS the company (200, not 400): the portal's company form needs a way
/// to say "I am not a company after all", and a validation error is not that. Every other
/// field is normalized here so the stored document is the canonical one.
async fn update_company(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthUser>,
    Json(body): Json<CompanyRequest>,
) -> ApiResult {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        state
            .users()
            .update_one(
                doc! { "_id": &auth.id },
                doc! { "$unset": { "company": "" } },
            )
            .await?;
        let updated = load_user(&state, &auth.id).await?;
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": UserResponse::from_doc(&updated) })),
        ));
    }

    if name.chars().count() > MAX_COMPANY_NAME_CHARS {
        return Err(AppError::BadRequest(format!(
            "The company name must be {MAX_COMPANY_NAME_CHARS} characters or fewer."
        )));
    }

    let company = CompanyProfile {
        name,
        services: clean_services(body.services)?,
        website: normalize_optional(body.website, MAX_COMPANY_WEBSITE_CHARS, "website")?,
        description: normalize_optional(
            body.description,
            MAX_COMPANY_DESCRIPTION_CHARS,
            "description",
        )?,
    };

    let stored = mongodb::bson::to_bson(&company)?;
    state
        .users()
        .update_one(
            doc! { "_id": &auth.id },
            doc! { "$set": { "company": stored } },
        )
        .await?;

    // Read back rather than echo, same rule as every other write in this file.
    let updated = load_user(&state, &auth.id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": UserResponse::from_doc(&updated) })),
    ))
}

async fn verify_email(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<VerifyEmailRequest>,
) -> ApiResult {
    let code = body.code.trim();
    if code.is_empty() {
        return Err(AppError::BadRequest(
            "Enter the 6-digit code from your email".to_string(),
        ));
    }

    let verify_key = otp::otp_key(&user.id, otp::PURPOSE_VERIFY);
    let stored = state
        .email_otps()
        .find_one(doc! { "_id": &verify_key })
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(
                "No code is waiting for this account. Request a new one.".to_string(),
            )
        })?;

    let now = chrono::Utc::now();

    if stored.attempts >= otp::OTP_MAX_ATTEMPTS {
        return Err(AppError::TooManyRequests(
            "Too many incorrect codes. Request a new one.".to_string(),
        ));
    }
    if otp::is_expired(&stored.expires_at, now) {
        return Err(AppError::BadRequest(
            "That code has expired. Request a new one.".to_string(),
        ));
    }

    if !otp::code_matches(&stored.code_hash, code) {
        // Keyed on the CODE document ("user_id:purpose"). Keying it on the user id alone
        // matches nothing, which leaves the counter at zero forever and makes the attempt
        // limit decorative.
        state
            .email_otps()
            .update_one(
                doc! { "_id": &verify_key },
                doc! { "$inc": { "attempts": 1 } },
            )
            .await?;
        let left = otp::OTP_MAX_ATTEMPTS - (stored.attempts + 1);
        return Err(AppError::BadRequest(if left > 0 {
            format!("That code is not correct. {left} attempt(s) left.")
        } else {
            "That code is not correct, and this code is now used up. Request a new one.".to_string()
        }));
    }

    // The account update comes FIRST: burning the code without flipping the flag leaves
    // the user verified in the response and unverified in the database, with no code left
    // to retry — the exact state this order prevents.
    state
        .users()
        .update_one(
            doc! { "_id": &user.id },
            doc! { "$set": { "email_verified": true } },
        )
        .await?;
    state
        .email_otps()
        .delete_one(doc! { "_id": &verify_key })
        .await?;

    let user_doc = load_user(&state, &user.id).await?;
    tracing::info!(user_id = %user.id, "email verified");
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "user": UserResponse::from_doc(&user_doc), "email_verified": true }
        })),
    ))
}

/// Sends a fresh code. The cooldown prevents this from being turned into a mail bomb
/// aimed at an address the caller does not own.
async fn resend_otp(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(_body): Json<ResendOtpRequest>,
) -> ApiResult {
    let user_doc = load_user(&state, &user.id).await?;

    if user_doc.email_verified {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "data": { "sent": false, "already_verified": true }
            })),
        ));
    }

    let verify_key = otp::otp_key(&user.id, otp::PURPOSE_VERIFY);
    if let Some(existing) = state
        .email_otps()
        .find_one(doc! { "_id": &verify_key })
        .await?
    {
        let wait = otp::resend_wait_seconds(&existing.last_sent_at, chrono::Utc::now());
        if wait > 0 {
            return Err(AppError::TooManyRequests(format!(
                "Please wait {wait} second(s) before requesting another code."
            )));
        }
    }

    issue_otp(
        &state,
        &user.id,
        &user_doc.email,
        &user_doc.name,
        otp::PURPOSE_VERIFY,
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "sent": true,
                "to": otp::mask_email(&user_doc.email),
                "expires_in_minutes": otp::OTP_TTL_MINUTES
            }
        })),
    ))
}

/// Sends a reset code. The response is identical whether or not the address has an
/// account: an endpoint that answers "no such user" is an account-existence oracle, and
/// this one is unauthenticated.
async fn forgot_password(
    State(state): State<AppState>,
    Json(body): Json<ForgotPasswordRequest>,
) -> ApiResult {
    let email = body.email.trim().to_lowercase();
    let generic = "If that email has a Jobify account, a reset code is on its way.";

    let Some(user) = state.users().find_one(doc! { "email": &email }).await? else {
        tracing::info!(email = %otp::mask_email(&email), "password reset requested for an address with no account");
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "message": generic } })),
        ));
    };

    if user.provider != "password" {
        tracing::info!(user_id = %user.id, provider = %user.provider, "password reset requested for a non-password account");
        return Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": { "message": generic } })),
        ));
    }

    // Same cooldown as the signup code, for the same reason: without it this endpoint is a
    // mail bomb aimed at any address a stranger can type.
    let key = otp::otp_key(&user.id, otp::PURPOSE_RESET);
    if let Some(existing) = state.email_otps().find_one(doc! { "_id": &key }).await? {
        let wait = otp::resend_wait_seconds(&existing.last_sent_at, chrono::Utc::now());
        if wait > 0 {
            return Err(AppError::TooManyRequests(format!(
                "Please wait {wait} second(s) before requesting another code."
            )));
        }
    }

    // A mail failure is reported only in the log: telling the caller would reveal that the
    // account exists.
    if let Err(err) = issue_otp(
        &state,
        &user.id,
        &user.email,
        &user.name,
        otp::PURPOSE_RESET,
    )
    .await
    {
        tracing::warn!(user_id = %user.id, error = %err.message(), "reset email could not be sent");
    }

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "message": generic,
                "expires_in_minutes": otp::RESET_TTL_MINUTES
            }
        })),
    ))
}

/// Completes a reset with the mailed code. Public by necessity, so the code is what proves
/// identity — which is why it is single-use, attempt-limited and purpose-scoped.
async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordRequest>,
) -> ApiResult {
    otp::validate_password(&body.new_password).map_err(AppError::BadRequest)?;
    let email = body.email.trim().to_lowercase();

    // One message for "no such account", "no code waiting" and "wrong code": the endpoint
    // is unauthenticated, so anything more specific is a probe.
    let invalid = || {
        AppError::BadRequest(
            "That reset code is not valid or has expired. Request a new one.".to_string(),
        )
    };

    let user = state
        .users()
        .find_one(doc! { "email": &email })
        .await?
        .ok_or_else(invalid)?;

    if user.provider != "password" {
        return Err(invalid());
    }

    let key = otp::otp_key(&user.id, otp::PURPOSE_RESET);
    let stored = state
        .email_otps()
        .find_one(doc! { "_id": &key })
        .await?
        .ok_or_else(invalid)?;

    let now = chrono::Utc::now();
    if stored.attempts >= otp::OTP_MAX_ATTEMPTS || otp::is_expired(&stored.expires_at, now) {
        return Err(AppError::BadRequest(
            "That reset code has expired or is used up. Request a new one.".to_string(),
        ));
    }

    if !otp::code_matches(&stored.code_hash, body.code.trim()) {
        state
            .email_otps()
            .update_one(doc! { "_id": &key }, doc! { "$inc": { "attempts": 1 } })
            .await?;
        let left = otp::OTP_MAX_ATTEMPTS - (stored.attempts + 1);
        return Err(AppError::BadRequest(if left > 0 {
            format!("That reset code is not correct. {left} attempt(s) left.")
        } else {
            "That reset code is not correct, and it is now used up. Request a new one.".to_string()
        }));
    }

    state
        .users()
        .update_one(
            doc! { "_id": &user.id },
            doc! { "$set": { "password_hash": hash_password(&body.new_password) } },
        )
        .await?;
    // Burn it: a used reset code is a standing key to the account otherwise.
    state.email_otps().delete_one(doc! { "_id": &key }).await?;
    // A pending signup code is moot now — the address is proven by the reset mail.
    state
        .email_otps()
        .delete_one(doc! { "_id": otp::otp_key(&user.id, otp::PURPOSE_VERIFY) })
        .await?;

    tracing::info!(user_id = %user.id, "password reset completed");
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": {
                "message": "Your password has been changed. You can sign in with it now.",
                "email": email
            }
        })),
    ))
}

async fn load_user(state: &AppState, user_id: &str) -> Result<UserDoc, AppError> {
    state
        .users()
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))
}

fn provider_label(provider: &str) -> &str {
    match provider {
        "google" => "Google",
        "apple" => "Apple",
        other => other,
    }
}

#[allow(dead_code)]
fn _unused(_: Value) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(categories: &[&str], keywords: &[&str], locations: &[&str]) -> MatchProfileRequest {
        MatchProfileRequest {
            categories: categories.iter().map(|v| v.to_string()).collect(),
            keywords: keywords.iter().map(|v| v.to_string()).collect(),
            locations: locations.iter().map(|v| v.to_string()).collect(),
        }
    }

    #[test]
    fn saves_a_valid_profile_and_stamps_it_with_now() {
        let profile = validate_match_profile(request(
            &["ict", "  wash  ", "ict"],
            &[" Nurse ", "nurse", "M&E"],
            &[" Mogadishu "],
        ))
        .expect("a canonical profile must be accepted");

        // Canonical slugs, in the order given, with the duplicate collapsed.
        assert_eq!(profile.categories, vec!["ict", "wash"]);
        // Trimmed, and the case-insensitive duplicate keeps the first spelling.
        assert_eq!(profile.keywords, vec!["Nurse", "M&E"]);
        assert_eq!(profile.locations, vec!["Mogadishu"]);
        let stamped = chrono::DateTime::parse_from_rfc3339(&profile.updated_at)
            .expect("updated_at must be RFC3339");
        assert!(
            (chrono::Utc::now() - stamped.with_timezone(&chrono::Utc))
                .num_seconds()
                .abs()
                < 5
        );
    }

    #[test]
    fn an_unknown_slug_is_a_400_that_names_the_offender() {
        let err = validate_match_profile(request(&["ict", "computer_tecnology"], &["nurse"], &[]))
            .expect_err("an unknown slug must be refused");
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(err.code(), "bad_request");
        assert!(
            err.message().contains("computer_tecnology"),
            "the message must name the bad slug: {}",
            err.message()
        );
        // Nothing is written when one entry is wrong: this is all-or-nothing, unlike ingest.
        assert!(!err.message().contains("'ict'"));

        // A blank entry is refused rather than dropped, for the same reason.
        let blank = validate_match_profile(request(&[""], &[], &[])).unwrap_err();
        assert_eq!(blank.status_code(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn an_empty_category_list_is_valid_and_means_no_profile() {
        let profile = validate_match_profile(request(&[], &["nurse"], &["Hargeisa"]))
            .expect("clearing the categories must be accepted");
        assert!(profile.categories.is_empty());
        // The keywords are kept: PUT replaces the profile, it does not guess intent. The
        // matcher treats "no categories" as no profile either way.
        assert_eq!(profile.keywords, vec!["nurse"]);
        assert_eq!(profile.locations, vec!["Hargeisa"]);

        let all_empty = validate_match_profile(request(&[], &[], &[])).unwrap();
        assert!(all_empty.categories.is_empty() && all_empty.keywords.is_empty());
    }

    #[test]
    fn the_caps_are_enforced() {
        let slugs = crate::categories::Category::ALL;
        let eleven: Vec<&str> = slugs.iter().take(11).map(|c| c.slug()).collect();
        let err = validate_match_profile(request(&eleven, &[], &[])).unwrap_err();
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert!(err.message().contains("at most 10 categories"));

        // Exactly at the cap is fine.
        let ten: Vec<&str> = slugs.iter().take(10).map(|c| c.slug()).collect();
        assert_eq!(
            validate_match_profile(request(&ten, &[], &[]))
                .unwrap()
                .categories
                .len(),
            10
        );

        let keywords: Vec<String> = (0..21).map(|n| format!("word{n}")).collect();
        let err = validate_match_profile(MatchProfileRequest {
            categories: vec!["ict".to_string()],
            keywords,
            locations: Vec::new(),
        })
        .unwrap_err();
        assert!(err.message().contains("at most 20 keywords"));

        let locations: Vec<String> = (0..6).map(|n| format!("place{n}")).collect();
        let err = validate_match_profile(MatchProfileRequest {
            categories: vec!["ict".to_string()],
            keywords: Vec::new(),
            locations,
        })
        .unwrap_err();
        assert!(err.message().contains("at most 5 locations"));
    }

    #[test]
    fn terms_are_trimmed_blank_entries_dropped_and_length_bounded() {
        let profile = validate_match_profile(request(
            &["health"],
            &["  ", " accountant ", ""],
            &["  Mogadishu  "],
        ))
        .unwrap();
        assert_eq!(profile.keywords, vec!["accountant"]);
        assert_eq!(profile.locations, vec!["Mogadishu"]);

        let long = "a".repeat(MAX_PROFILE_TERM_CHARS + 1);
        let err = validate_match_profile(request(&["health"], &[&long], &[])).unwrap_err();
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert!(err.message().contains("60 characters"));

        // Exactly 60 characters is allowed.
        let at_limit = "b".repeat(MAX_PROFILE_TERM_CHARS);
        assert!(validate_match_profile(request(&["health"], &[&at_limit], &[])).is_ok());
    }

    /// The endpoint is mounted on the Bearer-guarded router; this is the guard it meets
    /// there. A caller with no token, a malformed header or a dead token is answered 401
    /// before any handler runs. The live HTTP status is also checked against the running
    /// server with curl (`PUT /me/match-profile` with no Authorization header → 401).
    #[test]
    fn a_request_without_a_usable_token_is_unauthorized() {
        use crate::middleware::bearer_user;
        use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

        let unauthorized = |headers: &HeaderMap| {
            let err = bearer_user(headers, "test-secret").expect_err("must be refused");
            assert_eq!(err.status_code(), StatusCode::UNAUTHORIZED);
            assert_eq!(err.code(), "unauthorized");
        };

        unauthorized(&HeaderMap::new());

        let mut basic = HeaderMap::new();
        basic.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Basic dXNlcjpwYXNz"),
        );
        unauthorized(&basic);

        let mut garbled = HeaderMap::new();
        garbled.insert(AUTHORIZATION, HeaderValue::from_static("Bearer not.a.jwt"));
        unauthorized(&garbled);

        // ...and the same header with a real token is the caller.
        let token = issue_token("test-secret", "jobify-backend", "user-1", "user").unwrap();
        let mut valid = HeaderMap::new();
        valid.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert_eq!(bearer_user(&valid, "test-secret").unwrap().id, "user-1");
    }

    #[test]
    fn company_services_are_trimmed_deduped_and_capped() {
        let services = clean_services(vec![
            " Solar installation ".to_string(),
            "solar INSTALLATION".to_string(),
            "".to_string(),
            "Maintenance".to_string(),
        ])
        .expect("a valid service list must be accepted");
        assert_eq!(services, vec!["Solar installation", "Maintenance"]);

        let too_many: Vec<String> = (0..11).map(|n| format!("service {n}")).collect();
        let err = clean_services(too_many).expect_err("over the cap must be refused");
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert!(err.message().contains("at most 10 services"));

        let long = vec!["x".repeat(MAX_COMPANY_SERVICE_CHARS + 1)];
        let err = clean_services(long).expect_err("an over-long service must be refused");
        assert!(err.message().contains("40 characters"));
    }

    #[test]
    fn a_blank_company_field_is_stored_as_absent() {
        assert_eq!(
            normalize_optional(Some("   ".to_string()), 300, "website").unwrap(),
            None
        );
        assert_eq!(normalize_optional(None, 300, "website").unwrap(), None);
        assert_eq!(
            normalize_optional(Some("  https://a.so ".to_string()), 300, "website").unwrap(),
            Some("https://a.so".to_string())
        );
        let err = normalize_optional(Some("x".repeat(301)), 300, "website").expect_err("too long");
        assert!(err.message().contains("website"));
    }
}
