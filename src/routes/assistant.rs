//! `POST /assistant/query` (Bearer) and its internal twin for the Discord bot.
//!
//! Both routes are thin: identity in, `assistant::answer()` out. The engine owns routing,
//! grounding, charging and refunding, so the two callers cannot drift apart.

use axum::{Extension, Json, Router, extract::State, http::StatusCode, routing::post};
use serde_json::{Value, json};

use crate::assistant;
use crate::assistant::Answer;
use crate::auth::AuthUser;
use crate::models::{AssistantQueryRequest, DiscordAssistantQueryRequest};
use crate::state::AppState;

use super::ApiResult;

pub fn protected_router() -> Router<AppState> {
    Router::new().route("/assistant/query", post(query_self))
}

pub fn internal_router() -> Router<AppState> {
    Router::new().route("/internal/assistant/query", post(query_for_discord))
}

fn payload(answer: &Answer) -> Value {
    json!({
        "intent": answer.intent.as_str(),
        "reply": answer.reply,
        "jobs": answer.rows.iter().map(|row| row.to_json()).collect::<Vec<_>>(),
        "credits": serde_json::to_value(&answer.credits).unwrap_or(Value::Null),
    })
}

/// The web portal's variant: the member is identified by their token.
async fn query_self(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<AssistantQueryRequest>,
) -> ApiResult {
    let answer = assistant::answer(&state, &user.id, &body.message).await?;
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": payload(&answer),
            "meta": { "charged_queries": answer.charged_queries }
        })),
    ))
}

/// The bot's variant: a Discord member has no bearer token, so the snowflake is the
/// identity. An unknown snowflake is a 404 — the bot registers the member first.
async fn query_for_discord(
    State(state): State<AppState>,
    Json(body): Json<DiscordAssistantQueryRequest>,
) -> ApiResult {
    let user_id = assistant::user_for_discord(&state, &body.discord_id).await?;
    let answer = assistant::answer(&state, &user_id, &body.message).await?;
    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": payload(&answer),
            "meta": { "charged_queries": answer.charged_queries, "user_id": user_id }
        })),
    ))
}
