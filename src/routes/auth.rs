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
    AuthResult, LoginRequest, RegisterRequest, UpdateProfileRequest, UserDoc, UserResponse,
};
use crate::services::add_tokens;
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/auth/me", get(me).patch(update_me))
        .route("/me/memory", get(get_memory))
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

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "data": AuthResult { user: to_user_response(&user), token }
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
