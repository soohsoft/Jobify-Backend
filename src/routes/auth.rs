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
use crate::models::{AuthResult, LoginRequest, RegisterRequest, UserDoc, UserResponse};
use crate::state::AppState;
use crate::util::{now_iso, uuid_id};

use super::ApiResult;

pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
}

pub fn protected_router() -> Router<AppState> {
    Router::new().route("/auth/me", get(me))
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
        role: "user".to_string(),
        location: None,
        match_profile: None,
        created_at: now,
    };

    state.users().insert_one(&user).await?;
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

fn to_user_response(user: &UserDoc) -> UserResponse {
    UserResponse {
        id: user.id.clone(),
        name: user.name.clone(),
        email: user.email.clone(),
        role: user.role.clone(),
        location: user.location.clone(),
        match_profile: user.match_profile.clone(),
    }
}

#[allow(dead_code)]
fn _unused_json(_: Value) {}
