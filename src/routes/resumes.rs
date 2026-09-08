use std::convert::Infallible;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    routing::{get, post},
};
use mongodb::bson::doc;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::llm::{
    ChatMessage, ChatOptions, LlmClient, TokenUsage, estimate_prompt_tokens, estimate_tokens,
};
use crate::models::{
    PatchResumeRequest, RESUME_TEMPLATES, ResumeChatRequest, ResumeDoc, is_valid_template_id,
    tokens_to_usd,
};
use crate::prompts::{EDIT_EXTRACT_SYSTEM_PROMPT, RESUME_EDIT_SYSTEM_PROMPT};
use crate::services::{deduct_tokens, ensure_credit, record_usage};
use crate::state::AppState;
use crate::util::now_iso;

use super::ApiResult;
use super::common::{SseStream, SseTx, paginate, send_sse, sse_event};

pub fn public_router() -> Router<AppState> {
    Router::new().route("/resumes/templates", get(templates))
}

pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/resumes", get(list))
        .route("/resumes/{id}", get(get_by_id).patch(update).delete(remove))
        .route("/resumes/{id}/chat", post(edit_chat))
}

#[derive(Deserialize)]
pub struct ResumesQuery {
    #[serde(default = "default_page")]
    page: u64,
    #[serde(default = "default_limit")]
    limit: u64,
}

fn default_page() -> u64 {
    1
}
fn default_limit() -> u64 {
    30
}

async fn templates() -> Json<Value> {
    Json(json!({ "status": "success", "data": RESUME_TEMPLATES }))
}

async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ResumesQuery>,
) -> ApiResult {
    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.resumes(),
        doc! { "user_id": &user.id },
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

async fn get_by_id(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let resume = state
        .resumes()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Resume not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": resume })),
    ))
}

async fn update(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<PatchResumeRequest>,
) -> ApiResult {
    let mut set = doc! {};
    if let Some(title) = body.title {
        set.insert("title", title);
    }
    if let Some(profile) = body.profile {
        set.insert(
            "profile",
            mongodb::bson::to_bson(&profile).map_err(|e| AppError::BadRequest(e.to_string()))?,
        );
    }
    if let Some(template_id) = body.template_id {
        if !is_valid_template_id(&template_id) {
            return Err(AppError::BadRequest(
                "Please choose one of the available templates: modern, classic, minimal, or professional."
                    .to_string(),
            ));
        }
        set.insert("template_id", template_id);
    }

    if set.is_empty() {
        return Err(AppError::BadRequest(
            "Nothing to update. Provide title, profile, or template_id.".to_string(),
        ));
    }

    set.insert("updated_at", now_iso());
    let result = state
        .resumes()
        .update_one(
            doc! { "_id": &id, "user_id": &user.id },
            doc! { "$set": set },
        )
        .await?;

    if result.matched_count == 0 {
        return Err(AppError::NotFound("Resume not found".to_string()));
    }

    let resume = state
        .resumes()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Resume not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": resume })),
    ))
}

async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    let result = state
        .resumes()
        .delete_one(doc! { "_id": &id, "user_id": &user.id })
        .await?;

    if result.deleted_count == 0 {
        return Err(AppError::NotFound("Resume not found".to_string()));
    }

    Ok((StatusCode::NO_CONTENT, Json(json!({ "status": "success" }))))
}

async fn edit_chat(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ResumeChatRequest>,
) -> Result<SseStream, AppError> {
    let resume = state
        .resumes()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Resume not found".to_string()))?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    let task_state = state.clone();
    let task_user = user.id.clone();

    tokio::spawn(async move {
        if let Err(err) = run_resume_edit(&task_state, &task_user, resume, &body.content, &tx).await
        {
            send_sse(&tx, "error", json!({ "message": err.message() })).await;
        }
    });

    Ok(Sse::new(ReceiverStream::new(rx)))
}

async fn run_resume_edit(
    state: &AppState,
    user_id: &str,
    resume: ResumeDoc,
    content: &str,
    tx: &SseTx,
) -> Result<(), AppError> {
    ensure_credit(state, user_id).await?;

    let mut turns: Vec<ChatMessage> = Vec::new();
    if let Some(chat_id) = resume.chat_id.as_ref()
        && let Some(chat) = state
            .chats()
            .find_one(doc! { "_id": chat_id, "user_id": user_id })
            .await?
    {
        turns.extend(chat.turns.iter().map(|turn| ChatMessage {
            role: turn.role.clone(),
            content: turn.content.clone(),
        }));
    }

    turns.push(ChatMessage {
        role: "user".to_string(),
        content: content.to_string(),
    });

    let system_content = format!(
        "{}\n\nCurrent CV profile JSON:\n{}",
        RESUME_EDIT_SYSTEM_PROMPT,
        serde_json::to_string_pretty(&resume.profile)
            .map_err(|err| AppError::BadGateway(err.to_string()))?
    );

    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system_content,
    }];
    messages.extend(turns.iter().cloned());

    send_sse(
        tx,
        "meta",
        json!({ "resumeId": resume.id, "status": "editing" }),
    )
    .await;

    let llm = LlmClient::new(state.config.clone());
    let mut assistant_content = String::new();
    let stream_result = {
        let delta_tx = tx.clone();
        llm.stream(
            &messages,
            ChatOptions {
                temperature: 0.4,
                json_mode: false,
            },
            |delta| {
                assistant_content.push_str(delta);
                let _ = delta_tx.blocking_send(sse_event("delta", json!({ "content": delta })));
            },
        )
        .await?
    };

    let assistant_message = ChatMessage {
        role: "assistant".to_string(),
        content: assistant_content.clone(),
    };

    let mut conversation = turns;
    conversation.push(assistant_message);
    let conversation_json: Vec<Value> = conversation
        .iter()
        .map(|message| json!({ "role": message.role, "content": message.content }))
        .collect();

    let extract_messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: EDIT_EXTRACT_SYSTEM_PROMPT.to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: serde_json::to_string(&json!({ "conversation": conversation_json }))
                .map_err(|err| AppError::BadGateway(err.to_string()))?,
        },
    ];

    let extraction = llm
        .chat(
            &extract_messages,
            ChatOptions {
                temperature: 0.0,
                json_mode: true,
            },
        )
        .await?;

    let extracted = crate::util::extract_json(&extraction.content)?;
    let updated_profile = extracted
        .get("profile")
        .cloned()
        .unwrap_or_else(|| resume.profile.clone());

    let stream_usage = stream_result.usage.unwrap_or_default();
    let extraction_usage = extraction.usage.unwrap_or_default();
    let mut usage = TokenUsage {
        prompt_tokens: stream_usage.prompt_tokens + extraction_usage.prompt_tokens,
        completion_tokens: stream_usage.completion_tokens + extraction_usage.completion_tokens,
        total_tokens: stream_usage.total_tokens + extraction_usage.total_tokens,
    };

    if usage.total_tokens == 0 {
        let prompt_tokens = estimate_prompt_tokens(&messages);
        let completion_tokens = estimate_tokens(&assistant_content);
        usage = TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        };
    }

    record_usage(state, user_id, &usage).await?;
    let credit = deduct_tokens(state, user_id, usage.total_tokens).await?;
    let balance_usd = tokens_to_usd(credit.tokens);

    let updated_at = now_iso();
    state
        .resumes()
        .update_one(
            doc! { "_id": &resume.id, "user_id": user_id },
            doc! { "$set": { "profile": mongodb::bson::to_bson(&updated_profile).map_err(|e| AppError::BadRequest(e.to_string()))?, "updated_at": &updated_at } },
        )
        .await?;

    send_sse(tx, "profile", json!({ "profile": updated_profile })).await;
    send_sse(
        tx,
        "resume",
        json!({ "resumeId": resume.id, "title": resume.title, "updatedAt": updated_at }),
    )
    .await;
    send_sse(
        tx,
        "usage",
        json!({
            "promptTokens": usage.prompt_tokens,
            "completionTokens": usage.completion_tokens,
            "totalTokens": usage.total_tokens,
            "balanceUsd": balance_usd,
        }),
    )
    .await;
    send_sse(tx, "done", json!({ "status": "editing" })).await;

    Ok(())
}
