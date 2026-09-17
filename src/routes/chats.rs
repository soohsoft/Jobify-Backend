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
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::llm::{
    ChatMessage, ChatOptions, LlmClient, TokenUsage, estimate_prompt_tokens, estimate_tokens,
    reserve_estimate,
};
use crate::models::{
    ChatDoc, ChatMessageRequest, ChatTurn, CreateChatRequest, RESUME_TEMPLATES, ResumeDoc,
    SelectTemplateRequest, is_valid_template_id, tokens_to_usd,
};
use crate::prompts::{CHAT_SYSTEM_PROMPT, EXTRACT_SYSTEM_PROMPT, JOB_SEARCH_SYSTEM_PROMPT};
use crate::services::{add_tokens, credit_json, record_usage, reserve_credit, settle_credit};
use crate::state::AppState;
use crate::util::{merge_profile, now_iso, uuid_id};

use super::ApiResult;
use super::common::{SseStream, SseTx, paginate, send_sse, sse_event};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chats", get(list).post(create))
        .route("/chats/{id}", get(get_by_id).patch(rename))
        .route("/chats/{id}/messages", post(send_message))
        .route("/chats/{id}/template", post(select_template))
}

#[derive(Deserialize)]
pub struct RenameChatRequest {
    title: String,
}

async fn rename(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<RenameChatRequest>,
) -> ApiResult {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(AppError::BadRequest("Title cannot be empty".to_string()));
    }
    let title = title.chars().take(80).collect::<String>();
    let chat = state
        .chats()
        .find_one_and_update(
            doc! { "_id": &id, "user_id": &user.id },
            mongodb::bson::doc! {
                "$set": { "title": &title, "updated_at": now_iso() }
            },
        )
        .await?
        .ok_or_else(|| AppError::NotFound("Chat not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": chat })),
    ))
}

#[derive(Deserialize)]
pub struct ChatsQuery {
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

async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<CreateChatRequest>,
) -> ApiResult {
    let now = now_iso();
    let chat = ChatDoc {
        id: uuid_id(),
        user_id: user.id.clone(),
        title: body.title.unwrap_or_else(|| "New resume chat".to_string()),
        // Unknown or absent means the CV flow, which is the original behaviour.
        purpose: match body.purpose.as_deref() {
            Some("job_search") => "job_search".to_string(),
            _ => "cv".to_string(),
        },
        status: "collecting".to_string(),
        turns: Vec::new(),
        profile: json!({}),
        template_id: None,
        tokens_used: 0,
        created_at: now.clone(),
        updated_at: now,
    };

    state.chats().insert_one(&chat).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "status": "success", "data": chat })),
    ))
}

async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(query): Query<ChatsQuery>,
) -> ApiResult {
    let page = query.page.max(1);
    let limit = query.limit.clamp(1, 100);
    let (data, total) = paginate(
        &state.chats(),
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
    let chat = state
        .chats()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Chat not found".to_string()))?;

    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": chat })),
    ))
}

async fn select_template(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<SelectTemplateRequest>,
) -> ApiResult {
    if !is_valid_template_id(&body.template_id) {
        return Err(AppError::BadRequest(
            "Please choose one of the available templates: modern, classic, minimal, or professional."
                .to_string(),
        ));
    }

    let chat = state
        .chats()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Chat not found".to_string()))?;

    let resume = finalize_chat(&state, &user.id, chat, &body.template_id).await?;
    Ok((
        StatusCode::OK,
        Json(json!({ "status": "success", "data": resume })),
    ))
}

async fn send_message(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<ChatMessageRequest>,
) -> Result<SseStream, AppError> {
    let chat = state
        .chats()
        .find_one(doc! { "_id": &id, "user_id": &user.id })
        .await?
        .ok_or_else(|| AppError::NotFound("Chat not found".to_string()))?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Event, Infallible>>();
    let task_state = state.clone();
    let task_user = user.id.clone();

    tokio::spawn(async move {
        let result = match chat.status.as_str() {
            "template_selection" => {
                handle_template_selection(&task_state, &task_user, chat, &body.content, &tx).await
            }
            "completed" => {
                send_sse(
                    &tx,
                    "error",
                    json!({
                        "message": "This chat is already completed. Start a new chat to build another CV."
                    }),
                )
                .await;
                Ok(())
            }
            _ => handle_collecting(&task_state, &task_user, chat, &body.content, &tx).await,
        };

        if let Err(err) = result {
            send_sse(&tx, "error", json!({ "message": err.message() })).await;
        }
    });

    Ok(Sse::new(UnboundedReceiverStream::new(rx)))
}

async fn handle_template_selection(
    state: &AppState,
    user_id: &str,
    chat: ChatDoc,
    content: &str,
    tx: &SseTx,
) -> Result<(), AppError> {
    let content_lower = content.to_lowercase();
    let chosen = RESUME_TEMPLATES
        .iter()
        .find(|template| content_lower.contains(template.id))
        .map(|template| template.id.to_string());

    let Some(template_id) = chosen else {
        send_sse(
            tx,
            "error",
            json!({
                "message": "Please choose one of the available templates: modern, classic, minimal, or professional."
            }),
        )
        .await;
        return Ok(());
    };

    let resume = finalize_chat(state, user_id, chat, &template_id).await?;
    let assistant_content =
        format!("Great choice! I've generated your CV using the {template_id} template.");

    send_sse(tx, "delta", json!({ "content": assistant_content })).await;
    send_sse(
        tx,
        "resume",
        json!({ "resumeId": resume.id, "title": resume.title }),
    )
    .await;
    send_sse(tx, "done", json!({ "status": "completed" })).await;
    Ok(())
}

async fn handle_collecting(
    state: &AppState,
    user_id: &str,
    mut chat: ChatDoc,
    content: &str,
    tx: &SseTx,
) -> Result<(), AppError> {
    // A chat session is many turns, and every turn is two LLM calls for every chat
    // type — CV interview, job-search intake, follow-ups. Capping each request still
    // leaves the session unbounded, so the budget is checked before anything runs.
    if chat.tokens_used >= state.config.chat_session_token_budget {
        return Err(AppError::PaymentRequired(format!(
            "This chat has reached its {}-token session budget. Start a new chat to continue.",
            state.config.chat_session_token_budget
        )));
    }

    let now = now_iso();
    let user_turn = ChatTurn {
        role: "user".to_string(),
        content: content.to_string(),
        created_at: now.clone(),
    };
    chat.turns.push(user_turn);

    let messages: Vec<ChatMessage> = std::iter::once(ChatMessage {
        role: "system".to_string(),
        // Which interview this is. A job seeker and a CV writer need different
        // questions, and reusing the CV prompt asked job seekers for their full name.
        content: if chat.purpose == "job_search" {
            JOB_SEARCH_SYSTEM_PROMPT.to_string()
        } else {
            CHAT_SYSTEM_PROMPT.to_string()
        },
    })
    .chain(chat.turns.iter().map(|turn| ChatMessage {
        role: turn.role.clone(),
        content: turn.content.clone(),
    }))
    .collect();

    // Hold the worst case for BOTH calls of this turn before anything is sent: the
    // reply, and the extraction that runs on the same conversation afterwards. The
    // extraction prompt is the conversation again plus the reply, so counting the
    // chat prompt a second time is a deliberate over-estimate — the unused part is
    // refunded by settle_credit. Holding here is what makes the 402 land before the
    // user sees any output.
    let reserved = reserve_estimate(&messages, state.config.llm_max_tokens_chat).saturating_add(
        reserve_estimate(&messages, state.config.llm_max_tokens_extract),
    );
    reserve_credit(state, user_id, reserved).await?;

    send_sse(
        tx,
        "meta",
        json!({ "chatId": chat.id, "status": chat.status }),
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
                max_tokens: state.config.llm_max_tokens_chat,
            },
            |delta| {
                assistant_content.push_str(delta);
                let _ = delta_tx.send(sse_event("delta", json!({ "content": delta })));
            },
        )
        .await
    };
    let stream_result = match stream_result {
        Ok(result) => result,
        Err(err) => {
            // Nothing was delivered, so the hold is released in full.
            let _ = add_tokens(state, user_id, reserved).await;
            return Err(err);
        }
    };

    let assistant_turn = ChatTurn {
        role: "assistant".to_string(),
        content: assistant_content.clone(),
        created_at: now_iso(),
    };
    chat.turns.push(assistant_turn);

    let conversation: Vec<Value> = chat
        .turns
        .iter()
        .map(|turn| json!({ "role": turn.role, "content": turn.content }))
        .collect();

    let extract_messages = vec![
        ChatMessage {
            role: "system".to_string(),
            // The candidate list rides in the user message rather than the prompt
            // constant, so the slugs stay defined once in categories.rs and the
            // prompt cannot drift away from the taxonomy.
            content: format!(
                "{}\n\nCandidate categories for \"categories\" (use these slugs exactly):\n{}",
                EXTRACT_SYSTEM_PROMPT,
                crate::categories::Category::prompt_list()
            ),
        },
        ChatMessage {
            role: "user".to_string(),
            content: serde_json::to_string(&json!({ "conversation": conversation }))
                .map_err(|err| AppError::BadGateway(err.to_string()))?,
        },
    ];

    let extraction = llm
        .chat(
            &extract_messages,
            ChatOptions {
                temperature: 0.0,
                json_mode: true,
                max_tokens: state.config.llm_max_tokens_extract,
            },
        )
        .await;
    let extraction = match extraction {
        Ok(result) => result,
        Err(err) => {
            // The reply was already streamed, so it is charged for what it cost
            // rather than refunded. Only the extraction is lost.
            let stream_usage = stream_result.usage.unwrap_or_default();
            let _ = record_usage(state, user_id, &stream_usage).await;
            let _ = settle_credit(state, user_id, reserved, stream_usage.total_tokens).await;
            return Err(err);
        }
    };

    let extracted = crate::util::extract_json(&extraction.content)?;
    let incoming_profile = extracted
        .get("profile")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let complete = extracted
        .get("complete")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let missing_sections = extracted
        .get("missingSections")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let merged = merge_profile(&chat.profile, &incoming_profile);
    let next_status = if complete {
        "template_selection"
    } else {
        "collecting"
    };

    // Validate every slug through the taxonomy, so a hallucinated or reformatted
    // category is dropped rather than stored, and cap it at three.
    let categories: Vec<String> = {
        let mut out: Vec<String> = Vec::new();
        if let Some(items) = extracted.get("categories").and_then(Value::as_array) {
            for item in items {
                if let Some(slug) = item
                    .as_str()
                    .and_then(|raw| crate::categories::Category::from_slug(raw.trim()))
                {
                    let slug = slug.slug().to_string();
                    if !out.contains(&slug) {
                        out.push(slug);
                    }
                }
            }
        }
        out.truncate(3);
        out
    };
    let keywords: Vec<String> = extracted
        .get("keywords")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .take(8)
                .collect()
        })
        .unwrap_or_default();

    chat.profile = merged.clone();
    chat.status = next_status.to_string();
    chat.updated_at = now_iso();

    let stream_usage = stream_result.usage.unwrap_or_default();
    let extraction_usage = extraction.usage.unwrap_or_default();
    let mut usage = TokenUsage {
        prompt_tokens: stream_usage.prompt_tokens + extraction_usage.prompt_tokens,
        completion_tokens: stream_usage.completion_tokens + extraction_usage.completion_tokens,
        total_tokens: stream_usage.total_tokens + extraction_usage.total_tokens,
        prompt_cache_hit_tokens: stream_usage.prompt_cache_hit_tokens
            + extraction_usage.prompt_cache_hit_tokens,
        prompt_cache_miss_tokens: stream_usage.prompt_cache_miss_tokens
            + extraction_usage.prompt_cache_miss_tokens,
        reasoning_tokens: stream_usage.reasoning_tokens + extraction_usage.reasoning_tokens,
    };

    if usage.total_tokens == 0 {
        // Nothing came back from the provider, so fall back to an estimate. The
        // cache/reasoning split is unknowable here and stays zero.
        let prompt_tokens = estimate_prompt_tokens(&messages);
        let completion_tokens = estimate_tokens(&assistant_content);
        usage = TokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            ..Default::default()
        };
    }

    record_usage(state, user_id, &usage).await?;

    // Persisted before settling: the user already has the reply, so their data must
    // never be lost to a bookkeeping failure. The reservation is already the ceiling
    // on the charge, so settling second cannot undercharge.
    state
        .chats()
        .update_one(
            doc! { "_id": &chat.id, "user_id": user_id },
            doc! {
                "$set": {
                    "turns": mongodb::bson::to_bson(&chat.turns).map_err(|e| AppError::BadRequest(e.to_string()))?,
                    "profile": mongodb::bson::to_bson(&chat.profile).map_err(|e| AppError::BadRequest(e.to_string()))?,
                    "status": &chat.status,
                    "updated_at": &chat.updated_at,
                },
                "$inc": { "tokens_used": usage.total_tokens as i64 },
            },
        )
        .await?;

    // Refunds the unused hold, so the user is charged the real cost of the turn
    // rather than the worst case that was reserved for it.
    let credit = settle_credit(state, user_id, reserved, usage.total_tokens).await?;
    let balance_usd = tokens_to_usd(credit.tokens);

    // What the matcher will read. Dotted paths, so a turn that yields no
    // categories cannot wipe what an earlier turn established.
    let mut match_set = mongodb::bson::Document::new();
    if !categories.is_empty() {
        match_set.insert("match_profile.categories", &categories);
    }
    if !keywords.is_empty() {
        match_set.insert("match_profile.keywords", &keywords);
    }
    if let Some(location) = merged
        .get("location")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        match_set.insert("match_profile.locations", vec![location.to_string()]);
    }
    if !match_set.is_empty() {
        match_set.insert("match_profile.updated_at", now_iso());
        state
            .users()
            .update_one(doc! { "_id": user_id }, doc! { "$set": match_set })
            .await?;
    }

    send_sse(
        tx,
        "profile",
        json!({
            "profile": merged,
            "complete": complete,
            "missingSections": missing_sections,
            "categories": categories,
            "keywords": keywords,
        }),
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
    send_sse(tx, "done", json!({ "status": next_status })).await;

    let _ = credit_json(&credit);
    Ok(())
}

async fn finalize_chat(
    state: &AppState,
    user_id: &str,
    mut chat: ChatDoc,
    template_id: &str,
) -> Result<ResumeDoc, AppError> {
    let now = now_iso();
    chat.turns.push(ChatTurn {
        role: "user".to_string(),
        content: format!("I choose the {template_id} template."),
        created_at: now.clone(),
    });
    chat.turns.push(ChatTurn {
        role: "assistant".to_string(),
        content: format!("Great choice! I've generated your CV using the {template_id} template."),
        created_at: now.clone(),
    });
    chat.template_id = Some(template_id.to_string());
    chat.status = "completed".to_string();
    chat.updated_at = now.clone();

    let full_name = chat
        .profile
        .get("fullName")
        .and_then(Value::as_str)
        .unwrap_or("Untitled");
    let title = format!("{full_name} — {template_id}");

    let resume = ResumeDoc {
        id: uuid_id(),
        user_id: user_id.to_string(),
        chat_id: Some(chat.id.clone()),
        title,
        profile: chat.profile.clone(),
        template_id: Some(template_id.to_string()),
        created_at: now.clone(),
        updated_at: now,
    };

    state.resumes().insert_one(&resume).await?;
    state
        .chats()
        .update_one(
            doc! { "_id": &chat.id, "user_id": user_id },
            doc! {
                "$set": {
                    "turns": mongodb::bson::to_bson(&chat.turns).map_err(|e| AppError::BadRequest(e.to_string()))?,
                    "profile": mongodb::bson::to_bson(&chat.profile).map_err(|e| AppError::BadRequest(e.to_string()))?,
                    "template_id": &chat.template_id,
                    "status": &chat.status,
                    "updated_at": &chat.updated_at,
                }
            },
        )
        .await?;

    Ok(resume)
}
