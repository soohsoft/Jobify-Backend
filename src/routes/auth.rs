use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use mongodb::bson::doc;
use serde_json::{Value, json};

use crate::auth::{AuthUser, hash_password, issue_token, verify_password};
use crate::error::AppError;
use crate::models::{
    AuthResult, EmailOtpDoc, LoginRequest, RegisterRequest, ResendOtpRequest, UpdateProfileRequest,
    UserDoc, UserResponse, VerifyEmailRequest,
};
use crate::otp;
use crate::services::add_tokens;
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
}

/// Both verification routes are AUTHENTICATED, not keyed on an email in the body. Register
/// and login already return a token for an unverified account, so the client always has
/// one, and binding the code to the caller means this cannot be pointed at someone else's
/// mailbox to probe or spam it.
pub fn verification_router() -> Router<AppState> {
    Router::new()
        .route("/auth/verify-email", post(verify_email))
        .route("/auth/resend-otp", post(resend_otp))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(me).patch(update_me))
        .route("/me/memory", get(get_memory))
}

/// Creates (or replaces) the account's code, stores only its hash, and mails it.
///
/// Replacing rather than accumulating is deliberate: with one row per user, an older code
/// stops working the moment a newer one is issued, so a resend cannot leave two live keys.
/// A mail failure is returned but never fatal to the caller's own operation — the account
/// already exists by then, and the user can ask for another code.
async fn issue_otp(
    state: &AppState,
    user_id: &str,
    email: &str,
    name: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now();
    let code = otp::generate_code();
    let record = EmailOtpDoc {
        id: user_id.to_string(),
        user_id: user_id.to_string(),
        email: email.to_string(),
        code_hash: otp::hash_code(&code),
        expires_at: otp::expires_at(now),
        attempts: 0,
        last_sent_at: now.to_rfc3339(),
        created_at: now.to_rfc3339(),
    };

    state
        .email_otps()
        .replace_one(doc! { "_id": user_id }, &record)
        .upsert(true)
        .await?;

    let (subject, body) = crate::mail::verification_email(&code, name);
    if let Err(err) = state.mailer.send(email, &subject, &body).await {
        // The code is stored either way, so a transient mail failure is recoverable with
        // "resend" instead of leaving the account in a state it cannot leave.
        tracing::warn!(email = %otp::mask_email(email), error = %err, "verification email could not be sent");
        return Err(AppError::Internal(
            "Your account was created, but the verification email could not be sent. Use resend in a moment."
                .to_string(),
        ));
    }
    tracing::info!(email = %otp::mask_email(email), "verification code sent");
    Ok(())
}

async fn register(State(state): State<AppState>, Json(body): Json<RegisterRequest>) -> ApiResult {
    let email = body.email.trim().to_lowercase();
    let existing = state.users().find_one(doc! { "email": &email }).await?;

    if existing.is_some() {
        return Err(AppError::Conflict(
            "Email is already registered".to_string(),
        ));
    }

    let now = now_iso();
    let id = uuid_id();
    let user = UserDoc {
        id: id.clone(),
        name: body.name.trim().to_string(),
        email: email.clone(),
        password_hash: hash_password(&body.password),
        provider: "password".to_string(),
        external_id: None,
        email_verified: false,
        avatar_url: None,
        phone: None,
        headline: None,
        role: "user".to_string(),
        location: None,
        match_profile: None,
        alerts: None,
        created_at: now,
    };

    state.users().insert_one(&user).await?;

    // Every new account starts with a working balance. With none, the first chat is
    // refused with a 402 and the product is dead on arrival for anyone who has not paid
    // yet. Applied only here, at creation: re-registering the same address is rejected as
    // a conflict above, so the grant cannot be farmed.
    if state.config.signup_grant_tokens > 0 {
        add_tokens(&state, &user.id, state.config.signup_grant_tokens).await?;
    }
    let token = issue_token(
        &state.config.jwt_secret,
        &state.config.jwt_issuer,
        &id,
        "user",
    )?;

    // A failure to mail is reported in the response, not thrown: the account and its token
    // are already valid, and losing them would be worse than an inbox the user has to
    // re-request. The client shows the code screen either way and offers "resend".
    let delivery = match issue_otp(&state, &id, &email, &user.name).await {
        Ok(()) => {
            json!({ "sent": true, "to": otp::mask_email(&email), "expires_in_minutes": otp::OTP_TTL_MINUTES })
        }
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
                "user": to_user_response(&user),
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

    // An account created through Google/Apple has no password_hash. Comparing
    // against the empty string would fail anyway, but saying so is the
    // difference between a user retrying and a user finding the right button.
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
            "data": AuthResult { user: to_user_response(&user), token }
        })),
    ))
}

async fn me(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> ApiResult {
    let user_doc = state
        .users()
        .find_one(doc! { "_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": to_user_response(&user_doc) })),
    ))
}

/// PATCH /auth/me — what the Settings screen saves. The account is taken from
/// the token, never from the body, so this can only ever edit the caller.
///
/// Absent fields are left alone; a present-but-blank field is cleared. Name is
/// refused when blank because every screen falls back to it, and `""` would
/// render as an empty header rather than a missing one.
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

    // Read back rather than echo the request: the response is then the stored
    // document, so a client can trust it as the new state.
    let updated = state
        .users()
        .find_one(doc! { "_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    // The account is the authority on identity, so an edit here has to reach the memory
    // record too — otherwise the assistant keeps greeting the user by their old name.
    if let Err(reason) = crate::memory::refresh(&state, &user.id, "account", None).await {
        tracing::warn!(error = ?reason, "memory refresh failed after a profile edit");
    }

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": to_user_response(&updated) })),
    ))
}

/// `GET /me/memory` — who the service thinks this user is.
///
/// Exposed because the record is not only prompt fuel: a future consumer (a digest, a
/// bot, a support screen) needs the same answer without re-deriving it. It fills in a
/// record when none exists yet, which is a read-through of data the user already owns.
async fn get_memory(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult {
    let stored = crate::memory::load_stored(&state, &user.id).await?;
    match stored {
        Some(record) => Ok((
            StatusCode::OK,
            Json(json!({ "status": "success", "data": record, "meta": { "source": "stored" } })),
        )),
        None => {
            let record = crate::memory::refresh(&state, &user.id, "account", None).await?;
            Ok((
                StatusCode::OK,
                Json(
                    json!({ "status": "success", "data": record, "meta": { "source": "rebuilt" } }),
                ),
            ))
        }
    }
}

fn provider_label(provider: &str) -> &str {
    match provider {
        "google" => "Google",
        "apple" => "Apple",
        other => other,
    }
}

fn to_user_response(user: &UserDoc) -> UserResponse {
    UserResponse {
        id: user.id.clone(),
        name: user.name.clone(),
        email: user.email.clone(),
        role: user.role.clone(),
        provider: user.provider.clone(),
        email_verified: user.email_verified,
        location: user.location.clone(),
        phone: user.phone.clone(),
        headline: user.headline.clone(),
        avatar_url: user.avatar_url.clone(),
        match_profile: user.match_profile.clone(),
        alerts: user.alerts.clone(),
    }
}

#[allow(dead_code)]
fn _unused_json(_: Value) {}

/// Gate for the routes that spend money. An unverified email is how one person farms the
/// signup grant with disposable addresses: each new address mints a fresh balance, and the
/// token grant is worth real money (~$0.27 at current rates). Requiring a real mailbox
/// before the balance can be spent makes that cost a mailbox per account.
///
/// This is checked against the database rather than a claim in the token, because the
/// token outlives the verification: a user who confirms their code mid-session must be able
/// to keep going without logging in again.
pub async fn require_verified_email(state: &AppState, user_id: &str) -> Result<(), AppError> {
    let user = state
        .users()
        .find_one(doc! { "_id": user_id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
    if user.email_verified {
        return Ok(());
    }
    Err(AppError::Forbidden(
        "Confirm your email address to continue — we sent a 6-digit code to your inbox."
            .to_string(),
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

    let stored = state
        .email_otps()
        .find_one(doc! { "_id": &user.id })
        .await?
        .ok_or_else(|| {
            AppError::BadRequest(
                "No code is waiting for this account. Request a new one.".to_string(),
            )
        })?;

    let now = chrono::Utc::now();

    // Already used: a verified account stays verified, and the old code must not work again.
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
        state
            .email_otps()
            .update_one(doc! { "_id": &user.id }, doc! { "$inc": { "attempts": 1 } })
            .await?;
        let left = otp::OTP_MAX_ATTEMPTS - (stored.attempts + 1);
        return Err(AppError::BadRequest(if left > 0 {
            format!("That code is not correct. {left} attempt(s) left.")
        } else {
            "That code is not correct, and this code is now used up. Request a new one.".to_string()
        }));
    }

    state
        .users()
        .update_one(
            doc! { "_id": &user.id },
            doc! { "$set": { "email_verified": true } },
        )
        .await?;
    // Burn the code: a verified address is the point, and a live code is a live secret.
    state
        .email_otps()
        .delete_one(doc! { "_id": &user.id })
        .await?;

    let user_doc = state
        .users()
        .find_one(doc! { "_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    tracing::info!(user_id = %user.id, "email verified");
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "user": to_user_response(&user_doc), "email_verified": true }
        })),
    ))
}

/// Sends a fresh code. Cooldown applies so this cannot be turned into a mail bomb aimed at
/// an address the caller does not own.
async fn resend_otp(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(_body): Json<ResendOtpRequest>,
) -> ApiResult {
    let user_doc = state
        .users()
        .find_one(doc! { "_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

    if user_doc.email_verified {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "status": "success",
                "data": { "sent": false, "already_verified": true }
            })),
        ));
    }

    if let Some(existing) = state
        .email_otps()
        .find_one(doc! { "_id": &user.id })
        .await?
    {
        let wait = otp::resend_wait_seconds(&existing.last_sent_at, chrono::Utc::now());
        if wait > 0 {
            return Err(AppError::TooManyRequests(format!(
                "Please wait {wait} second(s) before requesting another code."
            )));
        }
    }

    issue_otp(&state, &user.id, &user_doc.email, &user_doc.name).await?;

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
