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

use futures_util::TryStreamExt;

use crate::auth::AuthUser;
use crate::error::AppError;
use crate::llm::{
    ChatMessage, ChatOptions, LlmClient, TokenUsage, estimate_prompt_tokens, estimate_tokens,
    reserve_estimate,
};
use crate::memory;
use crate::models::{
    ChatDoc, ChatMessageRequest, ChatTurn, CreateChatRequest, RESUME_TEMPLATES, ResumeDoc,
    SelectTemplateRequest, is_valid_template_id, tokens_to_usd,
};
use crate::prompts::{
    ASSISTANT_SYSTEM_PROMPT, CHAT_SYSTEM_PROMPT, EXTRACT_SYSTEM_PROMPT, JOB_SEARCH_SYSTEM_PROMPT,
};
use crate::services::{add_tokens, credit_json, record_usage, reserve_credit, settle_credit};
use crate::state::AppState;
use crate::util::{merge_profile, now_iso, uuid_id};

use super::ApiResult;
use super::common::{SseStream, SseTx, paginate, send_sse, sse_event};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/chats", get(list).post(create))
        .route("/chats/{id}", get(get_by_id).patch(rename).delete(remove))
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

/// Delete a conversation.
///
/// The CV the conversation produced is a separate document and is KEPT: a user who
/// deletes a chat has thrown away the interview, not the CV that came out of it.
/// The link is cleared instead (`chat_id` removed) so no resume dangles at a chat
/// that no longer exists, and the count of detached CVs comes back in the response
/// so the client can say what happened.
///
/// Consequence worth knowing: the CV editor counts its edits against its linked
/// chat's session budget. A detached CV is therefore no longer capped by that
/// budget — the per-request token cap and the credit gate still apply, so nothing
/// becomes free, but a very long editing session is no longer bounded.
async fn remove(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> ApiResult {
    // Owner-checked in the filter: another user's id deletes nothing and is a 404,
    // never a silent success.
    let deleted = state
        .chats()
        .delete_one(doc! { "_id": &id, "user_id": &user.id })
        .await?;

    if deleted.deleted_count == 0 {
        return Err(AppError::NotFound("Chat not found".to_string()));
    }

    let detached = state
        .resumes()
        .update_many(
            doc! { "chat_id": &id, "user_id": &user.id },
            doc! { "$unset": { "chat_id": "" } },
        )
        .await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "data": { "deleted": true, "id": id, "detachedResumes": detached.modified_count }
        })),
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
    super::auth::require_verified_email(&state, &user.id).await?;
    let now = now_iso();
    // Unknown or absent means the SEQUENCED assistant — the flow the product is built
    // around. It used to default to "cv", which is why every conversation behaved like a CV
    // interview whatever the user wanted: the client never sends a purpose, so the default
    // decided everything. "cv" is now opt-in, for the CV studio when it reopens.
    let purpose = match body.purpose.as_deref() {
        Some("job_search") => "job_search".to_string(),
        Some("cv") => "cv".to_string(),
        _ => "assistant".to_string(),
    };

    // A CV interview starts from what the account knows, so a returning user is not
    // asked for their name and contact details again. Seeded at creation rather than
    // in the prompt alone: the profile is the artifact, and a fact the assistant was
    // told but the extractor never hears would be missing from the CV.
    //
    // Not done for job_search: that flow's profile is categories and keywords, not a
    // person's contact details.
    let memory = if purpose == "cv" {
        memory::load(&state, &user.id).await?
    } else {
        memory::UserMemory::default()
    };

    // The language is remembered on the account, so a returning user is not asked again.
    let account = state.users().find_one(doc! { "_id": &user.id }).await?;
    // Priority: what the language tab sent, then what the account already chose, then English.
    let language = body
        .language
        .as_deref()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value == "en" || value == "so")
        .map(String::from)
        .or_else(|| {
            account
                .as_ref()
                .and_then(|doc| doc.preferred_language.clone())
                .filter(|value| value == "en" || value == "so")
        });
    // A tab choice is a standing preference, not just a property of this chat.
    if let Some(chosen) = language.as_deref() {
        state
            .users()
            .update_one(
                doc! { "_id": &user.id },
                doc! { "$set": { "preferred_language": chosen } },
            )
            .await?;
    }

    let chat = ChatDoc {
        id: uuid_id(),
        user_id: user.id.clone(),
        title: body.title.unwrap_or_else(|| "New resume chat".to_string()),
        purpose,
        language,
        status: "collecting".to_string(),
        turns: Vec::new(),
        profile: memory.fields.clone(),
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
    // Checked before the stream opens, so the refusal arrives as an HTTP status the client
    // can branch on rather than an error inside an SSE body.
    super::auth::require_verified_email(&state, &user.id).await?;
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

    // What the service already knows about this user, appended to the system prompt.
    // Loaded per turn rather than cached on the chat: the account can be edited in
    // between (Settings writes a name), and a stale memory is how the assistant
    // insists on an old job title. Empty for a user with nothing known, in which case
    // nothing is appended.
    let memory = memory::load(state, user_id).await?;

    let mut system = match chat.purpose.as_str() {
        // The sequenced conversation: language, then intent, then jobs. Unknown purposes land
        // here too, so a chat created before this existed behaves like a new one.
        "job_search" => JOB_SEARCH_SYSTEM_PROMPT.to_string(),
        "cv" => CHAT_SYSTEM_PROMPT.to_string(),
        _ => ASSISTANT_SYSTEM_PROMPT.to_string(),
    };
    // What the assistant cannot see for itself: which language to speak, and whether the work
    // area is already known. Both come from the database, so the prompt asks only for what is
    // genuinely missing — the difference between a short conversation and an interrogation.
    system.push_str("\n\n--- SESSION STATE ---\n");
    // Always concrete. The client's language tab is the control, so there is no "not chosen"
    // state to ask about — an unset value means English.
    system.push_str(match chat.language.as_deref() {
        Some("so") => "LANGUAGE: Somali. Write every reply in Somali.",
        _ => "LANGUAGE: English. Write every reply in English.",
    });
    system.push_str(&format!(
        "\nWORK AREA: {}\n",
        if memory.has_categories {
            "known — do not ask about their background again"
        } else {
            "not known yet — ask the two questions in STEP 3"
        }
    ));
    if !memory.context.is_empty() {
        system.push_str("\n\n");
        system.push_str(&memory.context);
    }

    // The opening intake, only while the work the user wants is unknown. The signal is
    // the ACCOUNT's match profile, because that is where the extractor writes categories
    // (the chat's own profile is CV-shaped and never carries them). Once they are known
    // the block disappears — no interrogation, and no tokens spent asking again.
    // Only the legacy job_search prompt needs the ladder appended: it has no ordering of its
    // own. The sequenced assistant carries the same areas inside its STEP 2/3, and appending
    // this as well made it ask "what kind of work?" BEFORE it had asked whether the user
    // wants jobs or a CV at all — two prompts, one of them winning at the wrong moment.
    if chat.purpose == "job_search" && !memory.has_categories {
        system.push_str("\n\n");
        system.push_str(crate::prompts::OPENING_INTAKE_PROMPT);
    }

    let messages: Vec<ChatMessage> = std::iter::once(ChatMessage {
        role: "system".to_string(),
        content: system,
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

    // The two fields that drive the sequence rather than the profile. Parsed here so the
    // language sticks even on a turn that carried no other information ("Somali please").
    let chosen_language = extracted
        .get("language")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| *value == "en" || *value == "so")
        .map(String::from);
    let wants_jobs = extracted
        .get("wantsJobs")
        .and_then(Value::as_bool)
        .unwrap_or(false);

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

    // The language lives in two places on purpose: the chat, so a switch sticks for this
    // conversation, and the account, so the next chat does not ask again. Written on the turn
    // that carried the choice ("Somali please") and nowhere else.
    if let Some(language) = chosen_language.as_deref() {
        state
            .chats()
            .update_one(
                doc! { "_id": &chat.id },
                doc! { "$set": { "language": language } },
            )
            .await?;
        state
            .users()
            .update_one(
                doc! { "_id": user_id },
                doc! { "$set": { "preferred_language": language } },
            )
            .await?;
    }

    // Keep the user's memory record current. Done here, at the end of a turn, because
    // this is the moment the service learns the most — and it means a fact survives even
    // if the user never saves a CV from this conversation or deletes the chat later.
    if let Err(reason) =
        memory::refresh(state, user_id, &format!("chat:{}", chat.id), Some(&merged)).await
    {
        // The turn already succeeded and was billed; failing it now would charge the
        // user for a reply they have and no memory write. A memory that lags one turn is
        // better than a 500 after the answer.
        tracing::warn!(error = ?reason, "memory refresh failed after a chat turn");
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
    // The in-chat job list. The assistant only PROMISES ("I'll look for X"); only the server
    // knows what is actually live, so the list is assembled here and travels as its own event.
    // Sent only when the turn asked for jobs, or every turn would repaint the same list.
    if wants_jobs {
        let account = state.users().find_one(doc! { "_id": user_id }).await?;
        let profile = account
            .as_ref()
            .and_then(|doc| doc.match_profile.clone())
            .filter(|p| !p.categories.is_empty());
        let exclusions = account
            .as_ref()
            .and_then(|doc| doc.alerts.clone())
            .unwrap_or_default();

        // Read what the result line needs BEFORE the profile is moved into the match below.
        let area_slugs: Vec<String> = profile
            .as_ref()
            .map(|p| p.categories.clone())
            .unwrap_or_default();
        let area_label = area_slugs
            .first()
            .and_then(|slug| crate::categories::Category::from_slug(slug))
            .map(|category| category.label().to_string())
            .unwrap_or_else(|| "your area".to_string());

        let (list, reason, fallback) = match profile {
            Some(profile) => {
                let primary =
                    super::jobs::ranked_candidates(state, &profile, &exclusions, &[], 5).await?;
                if !primary.is_empty() {
                    (primary, "ok".to_string(), false)
                } else {
                    // Nothing in their own work area. Widening to the rest of that GROUP is
                    // the difference between an empty screen and something they can actually
                    // apply for: a WASH officer's own category can be empty on a day when
                    // Humanitarian & Development has fifteen live roles.
                    let mut widened = Vec::new();
                    let siblings: Vec<String> = profile
                        .categories
                        .iter()
                        .flat_map(|slug| crate::categories::sibling_slugs(slug))
                        .collect();
                    if !siblings.is_empty() {
                        let mut wider = profile.clone();
                        wider.categories = siblings;
                        widened =
                            super::jobs::ranked_candidates(state, &wider, &exclusions, &[], 5)
                                .await?;
                    }

                    if !widened.is_empty() {
                        (widened, "no_live_jobs".to_string(), true)
                    } else {
                        // Still nothing anywhere near it. Rather than an empty screen, show
                        // the newest live postings and say plainly that they are not a match.
                        let found: Vec<crate::models::JobDoc> = state
                            .jobs()
                            .find(super::jobs::live_job_filter())
                            .sort(doc! { "created_at": -1, "_id": -1 })
                            .limit(5)
                            .await?
                            .try_collect()
                            .await?;
                        let newest: Vec<(f64, crate::models::JobDoc)> =
                            found.into_iter().map(|job| (0.0_f64, job)).collect();
                        if newest.is_empty() {
                            (newest, "no_jobs_at_all".to_string(), true)
                        } else {
                            (newest, "no_live_jobs".to_string(), true)
                        }
                    }
                }
            }
            None => (Vec::new(), "no_match_profile".to_string(), false),
        };

        // The assistant cannot see the result — it spoke before the search ran — so the result
        // line is written HERE, deterministically, and appended to the same message. It is the
        // difference between "the jobs are below" (a promise that can be empty) and the truth
        // about what was found. Two languages, because the conversation has one.
        let somali = chat.language.as_deref() == Some("so");
        let total = list.len();
        let area = area_label.clone();
        let line = if total > 0 && !fallback {
            if somali {
                format!("Waxaan kuu helay {total} shaqo — waa kuwan.")
            } else {
                format!("I found {total} — here they are.")
            }
        } else if total > 0 {
            if somali {
                format!(
                    "Ma jiro wax {area} ah oo furan hadda, laakiin waa kuwan {total} oo ugu dhow."
                )
            } else {
                format!("Nothing live in {area} today, so here are {total} close ones.")
            }
        } else if reason == "no_jobs_at_all" {
            if somali {
                "Ma jiro shaqo furan hadda. Dib u soo eeg dhawaan.".to_string()
            } else {
                "There are no live jobs right now. Do check back soon.".to_string()
            }
        } else {
            if somali {
                "Weli ma aanan ogeyn waxa aad rabto, markaa aan wax yar weydiiyo.".to_string()
            } else {
                "I don't know your area yet — let me ask you one thing first.".to_string()
            }
        };
        send_sse(tx, "delta", json!({ "content": format!(" {line}") })).await;

        let payload: Vec<Value> = list
            .iter()
            .map(|(score, job)| json!({ "job": job, "matchScore": score }))
            .collect();
        send_sse(
            tx,
            "jobs",
            json!({
                "jobs": payload,
                "total": total,
                "categories": area_slugs,
                "reason": reason,
                "fallback": fallback
            }),
        )
        .await;
    }

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
    // Saving a CV is the user accepting a version of themselves: it is a memory event.
    if let Err(reason) = memory::refresh(
        state,
        user_id,
        &format!("resume:{}", resume.id),
        Some(&resume.profile),
    )
    .await
    {
        tracing::warn!(error = ?reason, "memory refresh failed after saving a CV");
    }
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
