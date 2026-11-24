//! The purpose-bound assistant.
//!
//! This is NOT a chatbot. It answers exactly three kinds of question — job/tender search,
//! help with how Jobify works, and the member's own account/credits status — and refuses
//! everything else with one fixed sentence. Every fact it states comes from the database or
//! from a constant; the model is used to understand the message and, for a search, to
//! phrase rows that Rust already selected.
//!
//! Two cheap calls, temperature 0:
//!   1. extraction — message -> `{intent, location, categories, keywords, kind}`, validated
//!      here (unknown category slugs are dropped, not repaired).
//!   2. phrasing — only for `search`, only after the database returned rows. It receives
//!      the rows and nothing else, and is told never to invent a job, employer, deadline
//!      or URL. An empty result set never reaches the model at all.
//!
//! One query is charged per supported answer. `unsupported` answers charge nothing, and a
//! provider failure refunds the reservation — a user is never billed for a request that
//! produced no grounded answer.

use std::time::Duration;

use futures_util::TryStreamExt;
use mongodb::bson::{Document, doc};
use mongodb::options::FindOptions;
use serde::Deserialize;
use serde_json::json;

use crate::categories::Category;
use crate::credits;
use crate::error::AppError;
use crate::llm::{LlmMessage, LlmRequest};
use crate::models::{KIND_JOB, KIND_TENDER};
use crate::routes::jobs::live_job_filter;
use crate::routes::subscription::subscription_for;
use crate::state::AppState;
use crate::util::{escape_regex, nairobi_today, now_iso, uuid_id};

/// 20 smart queries per user per 10 minutes. In memory on purpose (see `AppState`).
pub const RATE_MAX: usize = 20;
pub const RATE_WINDOW: Duration = Duration::from_secs(600);

/// A search answer is at most three lines, so at most three rows are ever considered.
pub const MAX_SEARCH_ROWS: usize = 3;

const MAX_MESSAGE_CHARS: usize = 500;
const MAX_KEYWORDS: usize = 6;
const MAX_TERM_CHARS: usize = 40;
const MAX_CATEGORIES: usize = 5;
const EXTRACTION_MAX_TOKENS: u32 = 300;
const PHRASING_MAX_TOKENS: u32 = 200;

/// The only sentence a message outside the three supported purposes ever gets. Fixed, so
/// the bot can rely on it and no model can be argued into answering.
pub const UNSUPPORTED_REPLY: &str = "I can help with finding jobs and tenders, alerts, your account, or credits. What would you like?";

/// The answer for a search that matched nothing. Deliberately not model-written.
pub const NOTHING_MATCHES: &str = "Nothing live matches that right now.";

/// What a provider failure says. No status code, no upstream body, no key.
pub const LLM_FAILURE_MESSAGE: &str =
    "The assistant is unavailable right now. You have not been charged — please try again.";

/// Help is a constant: the rules of the product do not change per user, and a model asked
/// to explain billing is a model that can invent a price.
pub const HELP_TEXT: &str = "\
Here is how Jobify works.
• Register: message the bot once. It creates your account from your Discord id — no email needed.
• What it does: it finds live jobs and tenders and sends you the ones that match your profile.
• Alerts: set your sectors, skills and location with the bot's profile command, and alerts start automatically. You can pause them any time.
• Search: ask me things like \"warehouse jobs in Hargeisa\" or \"tenders for construction\".
• Credits: each smart question costs 1 query. You get 30 free queries every 30 days; after that you buy credits. Use the Buy Credits action in the bot to top up.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    Search,
    Help,
    Status,
    Unsupported,
}

impl Intent {
    pub fn as_str(self) -> &'static str {
        match self {
            Intent::Search => "search",
            Intent::Help => "help",
            Intent::Status => "status",
            Intent::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    Job,
    Tender,
    Both,
}

/// The validated routing decision. This — not the raw model output — drives the query.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed {
    pub intent: Intent,
    pub location: Option<String>,
    pub categories: Vec<String>,
    pub keywords: Vec<String>,
    pub kind: SearchKind,
}

impl Routed {
    fn unsupported() -> Self {
        Self {
            intent: Intent::Unsupported,
            location: None,
            categories: Vec::new(),
            keywords: Vec::new(),
            kind: SearchKind::Both,
        }
    }
}

/// The shape the extraction call is asked for. Every field defaults so a truncated or
/// partial answer degrades to `unsupported` instead of failing the request.
#[derive(Deserialize, Default)]
struct RawExtraction {
    #[serde(default)]
    intent: String,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    kind: Option<String>,
}

/// Take the outermost `{...}` out of whatever the model surrounded it with (fences,
/// prose). No JSON object means no routing decision, which means `unsupported`.
fn json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(&raw[start..=end])
}

fn clean_term(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_TERM_CHARS {
        return None;
    }
    Some(value.to_string())
}

/// Parse AND validate the extraction answer.
///
/// A category slug the taxonomy does not know is dropped rather than repaired: the query
/// then simply does not filter on it, which returns more rows instead of silently
/// returning none for a slug nobody stores.
pub fn parse_extraction(raw: &str) -> Routed {
    let Some(object) = json_object(raw) else {
        return Routed::unsupported();
    };
    let Ok(extraction) = serde_json::from_str::<RawExtraction>(object) else {
        return Routed::unsupported();
    };

    let intent = match extraction.intent.trim().to_ascii_lowercase().as_str() {
        "search" => Intent::Search,
        "help" => Intent::Help,
        "status" => Intent::Status,
        _ => Intent::Unsupported,
    };
    if intent == Intent::Unsupported {
        return Routed::unsupported();
    }

    let mut categories: Vec<String> = Vec::new();
    for value in &extraction.categories {
        if let Some(category) = Category::from_slug(value.trim()) {
            let slug = category.slug().to_string();
            if !categories.contains(&slug) {
                categories.push(slug);
            }
        }
        if categories.len() >= MAX_CATEGORIES {
            break;
        }
    }

    let mut keywords: Vec<String> = Vec::new();
    for value in &extraction.keywords {
        let Some(term) = clean_term(value) else {
            continue;
        };
        if !keywords.iter().any(|kept| kept.eq_ignore_ascii_case(&term)) {
            keywords.push(term);
        }
        if keywords.len() >= MAX_KEYWORDS {
            break;
        }
    }

    let location = extraction
        .location
        .as_deref()
        .and_then(clean_term)
        .map(|value| value.chars().take(80).collect::<String>());

    let kind = match extraction
        .kind
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some(KIND_TENDER) => SearchKind::Tender,
        Some(KIND_JOB) => SearchKind::Job,
        _ => SearchKind::Both,
    };

    Routed {
        intent,
        location,
        categories,
        keywords,
        kind,
    }
}

fn canonical_slug_list() -> String {
    Category::ALL
        .iter()
        .map(|category| category.slug())
        .collect::<Vec<_>>()
        .join(", ")
}

fn extraction_messages(message: &str) -> Vec<LlmMessage> {
    let system = format!(
        "You route ONE message for the Jobify assistant. Jobify helps people find jobs and \
tenders in Somalia and the region, understand their account, and buy credits. Reply with \
a single JSON object and nothing else:\n\
{{\"intent\":\"search|help|status|unsupported\",\"location\":null,\"categories\":[],\"keywords\":[],\"kind\":\"job|tender|both\"}}\n\
Rules:\n\
- search: the member is looking for jobs or tenders. Fill location when a place is named. \
Fill categories only with slugs from the list below. Put the role or skill words in \
keywords (at most 4, keep them short). kind is \"tender\" only when they asked for tenders, \
\"job\" when they explicitly excluded tenders, otherwise \"both\".\n\
- help: how to register, what the bot does, how alerts work, how to change preferences, \
what credits are, how to buy them.\n\
- status: their own subscription, their own credits, or what was sent to them today.\n\
- unsupported: anything else — general knowledge, opinions, coding, news, chit-chat, \
writing, translation, or anything not about Jobify's jobs, tenders, alerts, account or credits.\n\
Canonical category slugs: {}",
        canonical_slug_list()
    );
    vec![LlmMessage::system(system), LlmMessage::user(message)]
}

/// One row of a grounded search answer. Built from `JobDoc`, so every field exists in the
/// database — this struct is the ONLY thing the phrasing call is allowed to talk about.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRow {
    pub job_id: String,
    pub title: String,
    pub organization: String,
    pub location: Option<String>,
    pub kind: String,
    pub deadline: Option<String>,
}

impl SearchRow {
    pub fn to_json(&self) -> serde_json::Value {
        json!({
            "job_id": self.job_id,
            "title": self.title,
            "organization": self.organization,
            "location": self.location,
            "kind": self.kind,
            "deadline": self.deadline,
        })
    }
}

/// The deterministic rendering used both as the fallback and in the prompt's row block.
pub fn render_rows(rows: &[SearchRow]) -> String {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let location = row.location.as_deref().unwrap_or("location not listed");
            let deadline = row.deadline.as_deref().unwrap_or("no deadline listed");
            format!(
                "{}. {} — {} — {} — {} — deadline: {}",
                index + 1,
                row.title,
                row.organization,
                location,
                row.kind,
                deadline
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The phrasing call. The system prompt carries the grounding rules; the user message
/// carries ONLY the rows Rust selected. There is deliberately no conversation history, no
/// user profile and no database access behind this call.
pub fn phrasing_messages(rows: &[SearchRow]) -> Vec<LlmMessage> {
    let system = "You write short search-result summaries for Jobify. Use ONLY the rows in \
the user message. Never invent a job, employer, location, deadline, salary or URL that is \
not in them, and never add advice or commentary. Write at most 3 short lines, one line per \
row, plain text. If the rows list is empty, reply exactly: Nothing live matches that right now.";
    let body = if rows.is_empty() {
        "Rows:\n(no rows)".to_string()
    } else {
        format!("Rows:\n{}", render_rows(rows))
    };
    vec![LlmMessage::system(system), LlmMessage::user(body)]
}

/// Keep the model's phrasing only when it stays inside the grounding rules; otherwise use
/// the deterministic rendering of the same rows. A URL is rejected outright because the
/// rows never carry one, so any URL is invented.
pub fn grounded_reply(raw: &str, rows: &[SearchRow]) -> String {
    let text = raw.trim();
    let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
    if text.is_empty() || lines == 0 || lines > MAX_SEARCH_ROWS || text.contains("http") {
        return render_rows(rows);
    }
    text.to_string()
}

#[derive(Debug, Clone)]
pub struct Answer {
    pub intent: Intent,
    pub reply: String,
    pub rows: Vec<SearchRow>,
    pub credits: credits::Snapshot,
    pub charged_queries: i64,
}

/// Live rows for the routed filters. The visibility rule is `live_job_filter()` — the same
/// function the feed and the matcher use, so the assistant can never offer a job the feed
/// itself would hide.
pub async fn search_jobs(state: &AppState, routed: &Routed) -> Result<Vec<SearchRow>, AppError> {
    let mut clauses: Vec<Document> = vec![live_job_filter()];

    if let Some(location) = &routed.location {
        clauses.push(doc! {
            "location": { "$regex": escape_regex(location), "$options": "i" }
        });
    }
    if !routed.categories.is_empty() {
        clauses.push(doc! { "category": { "$in": &routed.categories } });
    }
    if !routed.keywords.is_empty() {
        let mut alternatives: Vec<Document> = Vec::new();
        for keyword in &routed.keywords {
            let pattern = doc! { "$regex": escape_regex(keyword), "$options": "i" };
            alternatives.push(doc! { "title": pattern.clone() });
            alternatives.push(doc! { "organization": pattern.clone() });
            alternatives.push(doc! { "description": pattern });
        }
        clauses.push(doc! { "$or": alternatives });
    }
    // Same rule as `GET /jobs`: "job" must also match the legacy rows that have no `kind`
    // field, so it is a separate clause rather than another key on a shared document.
    match routed.kind {
        SearchKind::Both => {}
        SearchKind::Tender => clauses.push(doc! { "kind": KIND_TENDER }),
        SearchKind::Job => clauses.push(doc! { "$or": [{ "kind": KIND_JOB }, { "kind": null }] }),
    }

    let filter = match clauses.len() {
        1 => clauses.remove(0),
        _ => doc! { "$and": clauses },
    };
    let options = FindOptions::builder()
        .sort(doc! { "created_at": -1, "_id": -1 })
        .limit(MAX_SEARCH_ROWS as i64)
        .build();

    let mut cursor = state.jobs().find(filter).with_options(options).await?;
    let mut rows: Vec<SearchRow> = Vec::new();
    while let Some(job) = cursor.try_next().await? {
        rows.push(SearchRow {
            job_id: job.id,
            title: job.title,
            organization: job.organization,
            location: job.location,
            kind: job.kind,
            deadline: job.deadline,
        });
    }
    Ok(rows)
}

fn date_only(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|at| at.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| value.to_string())
}

/// The account answer, built from the member's own rows only.
async fn status_reply(
    state: &AppState,
    user_id: &str,
    credits: &credits::Snapshot,
) -> Result<String, AppError> {
    let subscription = subscription_for(state, user_id).await?;
    let mut lines = vec![format!(
        "Credits: {} of {} free queries left this period (ends {}), plus {} purchased credits.",
        credits.remaining_this_period,
        credits.monthly_allowance,
        date_only(&credits.period_end),
        credits.balance
    )];
    lines.push(match subscription.status.as_str() {
        "active" => format!(
            "Subscription: active until {}.",
            subscription
                .period_end
                .as_deref()
                .map(date_only)
                .unwrap_or_else(|| "the end of the period".to_string())
        ),
        _ => "Subscription: none — use Buy Credits to start one.".to_string(),
    });

    let today = nairobi_today();
    let alerts_today = state
        .notifications()
        .count_documents(doc! { "user_id": user_id, "created_at": { "$gte": &today } })
        .await?;
    if alerts_today > 0 {
        let options = FindOptions::builder().limit(3).build();
        let mut cursor = state
            .notifications()
            .find(doc! { "user_id": user_id, "created_at": { "$gte": &today } })
            .with_options(options)
            .await?;
        let mut titles: Vec<String> = Vec::new();
        while let Some(notification) = cursor.try_next().await? {
            titles.push(notification.title);
        }
        lines.push(format!(
            "I sent you {} alert(s) today: {}.",
            alerts_today,
            titles.join("; ")
        ));
    } else {
        lines.push("I have not sent you any alerts today.".to_string());
    }

    Ok(lines.join("\n"))
}

async fn record_usage(
    state: &AppState,
    user_id: &str,
    intent: Intent,
    prompt_tokens: u64,
    completion_tokens: u64,
    charged: i64,
    outcome: &str,
) {
    let row = crate::models::AssistantUsageDoc {
        id: uuid_id(),
        user_id: user_id.to_string(),
        at: now_iso(),
        intent: intent.as_str().to_string(),
        prompt_tokens,
        completion_tokens,
        charged_queries: charged,
        outcome: outcome.to_string(),
    };
    // Usage is bookkeeping: a failed insert must not fail an answer the member already has.
    if let Err(err) = state.assistant_usage().insert_one(&row).await {
        tracing::warn!(user_id, error = %err, "assistant usage row not recorded");
    }
}

/// The one function both `POST /assistant/query` and `POST /internal/assistant/query` call.
///
/// Order matters: rate limit, then reserve, then work. The reservation means a 402 lands
/// before any provider call, and every failure after it refunds.
pub async fn answer(state: &AppState, user_id: &str, message: &str) -> Result<Answer, AppError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(AppError::BadRequest(
            "message must not be empty".to_string(),
        ));
    }
    if message.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::BadRequest(format!(
            "message must be {MAX_MESSAGE_CHARS} characters or fewer"
        )));
    }
    if !state.assistant_limiter.check(user_id) {
        return Err(AppError::TooManyRequests(
            "You are asking too quickly. Please wait a few minutes and try again.".to_string(),
        ));
    }

    let charge = credits::charge(state, user_id).await?;

    let reply = match state
        .llm
        .complete(LlmRequest {
            messages: extraction_messages(message),
            max_tokens: EXTRACTION_MAX_TOKENS,
            json_mode: true,
        })
        .await
    {
        Ok(reply) => reply,
        Err(err) => {
            tracing::warn!(user_id, error = %err.detail(), "assistant extraction failed");
            credits::refund(state, user_id, charge).await;
            return Err(AppError::LlmUnavailable(LLM_FAILURE_MESSAGE.to_string()));
        }
    };

    let routed = parse_extraction(&reply.content);
    let mut prompt_tokens = reply.usage.prompt_tokens;
    let mut completion_tokens = reply.usage.completion_tokens;

    // Outside the three purposes: fixed sentence, nothing charged. The reservation is put
    // back exactly where it came from.
    if routed.intent == Intent::Unsupported {
        credits::refund(state, user_id, charge).await;
        let snapshot = credits::snapshot(state, user_id).await?;
        tracing::info!(
            user_id,
            "assistant: unsupported message refused, nothing charged"
        );
        return Ok(Answer {
            intent: Intent::Unsupported,
            reply: UNSUPPORTED_REPLY.to_string(),
            rows: Vec::new(),
            credits: snapshot,
            charged_queries: 0,
        });
    }

    let (reply_text, rows, outcome) = match routed.intent {
        Intent::Help => (HELP_TEXT.to_string(), Vec::new(), "help"),
        Intent::Status => {
            let snapshot = credits::snapshot(state, user_id).await?;
            let text = status_reply(state, user_id, &snapshot).await?;
            (text, Vec::new(), "status")
        }
        Intent::Search => {
            let rows = search_jobs(state, &routed).await?;
            if rows.is_empty() {
                // No provider call at all: an empty result set cannot be phrased into
                // something, and the constant cannot invent a row.
                (NOTHING_MATCHES.to_string(), rows, "search_empty")
            } else {
                match state
                    .llm
                    .complete(LlmRequest {
                        messages: phrasing_messages(&rows),
                        max_tokens: PHRASING_MAX_TOKENS,
                        json_mode: false,
                    })
                    .await
                {
                    Ok(phrased) => {
                        prompt_tokens += phrased.usage.prompt_tokens;
                        completion_tokens += phrased.usage.completion_tokens;
                        let text = grounded_reply(&phrased.content, &rows);
                        (text, rows, "search_results")
                    }
                    Err(err) => {
                        tracing::warn!(user_id, error = %err.detail(), "assistant phrasing failed");
                        credits::refund(state, user_id, charge).await;
                        return Err(AppError::LlmUnavailable(LLM_FAILURE_MESSAGE.to_string()));
                    }
                }
            }
        }
        Intent::Unsupported => unreachable!("handled above"),
    };

    record_usage(
        state,
        user_id,
        routed.intent,
        prompt_tokens,
        completion_tokens,
        1,
        outcome,
    )
    .await;
    let snapshot = credits::snapshot(state, user_id).await?;

    Ok(Answer {
        intent: routed.intent,
        reply: reply_text,
        rows,
        credits: snapshot,
        charged_queries: 1,
    })
}

/// Resolve a Discord snowflake to the Jobify user id. The bot's only identity input.
pub async fn user_for_discord(state: &AppState, discord_id: &str) -> Result<String, AppError> {
    let id = discord_id.trim();
    if id.is_empty() {
        return Err(AppError::BadRequest("discord_id is required".to_string()));
    }
    state
        .discord_links()
        .find_one(doc! { "discord_id": id })
        .await?
        .map(|link| link.user_id)
        .ok_or_else(|| {
            AppError::NotFound(
                "This Discord account is not registered with Jobify yet.".to_string(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credits::Charge;
    use crate::llm::{LlmClient, LlmError, LlmReply, LlmUsage};
    use futures_util::future::{BoxFuture, ready};
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// A deterministic stand-in for the provider. Every test runs with this, so the suite
    /// has no network dependency and no key.
    struct StubLlm {
        replies: Mutex<VecDeque<Result<LlmReply, LlmError>>>,
        seen: Mutex<Vec<LlmRequest>>,
    }

    impl StubLlm {
        fn new(replies: Vec<Result<LlmReply, LlmError>>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into_iter().collect()),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn seen(&self) -> Vec<LlmRequest> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl LlmClient for StubLlm {
        fn complete<'a>(
            &'a self,
            request: LlmRequest,
        ) -> BoxFuture<'a, Result<LlmReply, LlmError>> {
            self.seen.lock().unwrap().push(request);
            let next = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err(LlmError::Transport("stub exhausted".to_string())));
            Box::pin(ready(next))
        }
    }

    fn text(content: &str) -> Result<LlmReply, LlmError> {
        Ok(LlmReply {
            content: content.to_string(),
            usage: LlmUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            },
        })
    }

    fn extraction(intent: &str) -> Result<LlmReply, LlmError> {
        text(&format!("{{\"intent\":\"{intent}\"}}"))
    }

    /// A throwaway database per test. Returns `None` when no mongod is reachable, matching
    /// the existing visibility test's behaviour: the suite still passes offline.
    async fn test_state(
        db_name: &str,
        llm: Arc<dyn LlmClient>,
        allowance: i64,
    ) -> Option<AppState> {
        let uri = std::env::var("MONGODB_URI")
            .unwrap_or_else(|_| "mongodb://127.0.0.1:27017/jobify".to_string());
        let Ok(client) = mongodb::Client::with_uri_str(&uri).await else {
            eprintln!("skipping: no MongoDB reachable at {uri}");
            return None;
        };
        let db = client.database(db_name);
        let _ = db.drop().await;
        let mut config = crate::config::Config::from_env();
        config.monthly_smart_queries = allowance;
        Some(AppState::with_llm(db, config, llm))
    }

    async fn seed_job(state: &AppState) {
        use mongodb::bson::doc;
        state
            .db
            .collection::<mongodb::bson::Document>("jobs")
            .insert_one(doc! {
                "_id": "job-1",
                "title": "Warehouse Assistant",
                "organization": "Horn Relief",
                "location": "Hargeisa",
                "description": "Warehouse duties",
                "category": "logistics_and_supply_chain",
                "kind": "job",
                // Far future, so the live rule keeps it: the 55 production rows are all
                // outside the window and must not be used to test a positive match.
                "deadline": "2099-01-01",
                "created_at": now_iso(),
            })
            .await
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // Routing (pure)
    // -----------------------------------------------------------------------

    #[test]
    fn every_intent_is_routed_from_the_extraction_answer() {
        assert_eq!(
            parse_extraction(r#"{"intent":"search"}"#).intent,
            Intent::Search
        );
        assert_eq!(
            parse_extraction(r#"{"intent":"help"}"#).intent,
            Intent::Help
        );
        assert_eq!(
            parse_extraction(r#"{"intent":"status"}"#).intent,
            Intent::Status
        );
        assert_eq!(
            parse_extraction(r#"{"intent":"unsupported"}"#).intent,
            Intent::Unsupported
        );
        // Anything the model made up is also unsupported — never a guess at a purpose.
        assert_eq!(
            parse_extraction(r#"{"intent":"write_my_cv"}"#).intent,
            Intent::Unsupported
        );
        assert_eq!(parse_extraction("no json here").intent, Intent::Unsupported);
        assert_eq!(parse_extraction("").intent, Intent::Unsupported);
    }

    #[test]
    fn fenced_json_and_partial_payloads_are_tolerated() {
        let routed = parse_extraction(
            "```json\n{\"intent\":\"search\",\"location\":\"Mogadishu\",\"keywords\":[\"driver\"]}\n```",
        );
        assert_eq!(routed.intent, Intent::Search);
        assert_eq!(routed.location.as_deref(), Some("Mogadishu"));
        assert_eq!(routed.keywords, vec!["driver".to_string()]);
    }

    #[test]
    fn unknown_category_slugs_are_dropped_and_known_ones_canonicalised() {
        let routed = parse_extraction(
            r#"{"intent":"search","categories":["ict","made_up","  wash  ","ICT"]}"#,
        );
        assert_eq!(
            routed.categories,
            vec!["ict".to_string(), "wash".to_string()]
        );
    }

    #[test]
    fn search_kind_defaults_to_both() {
        assert_eq!(
            parse_extraction(r#"{"intent":"search","kind":"tender"}"#).kind,
            SearchKind::Tender
        );
        assert_eq!(
            parse_extraction(r#"{"intent":"search","kind":"job"}"#).kind,
            SearchKind::Job
        );
        assert_eq!(
            parse_extraction(r#"{"intent":"search","kind":"nonsense"}"#).kind,
            SearchKind::Both
        );
    }

    // -----------------------------------------------------------------------
    // Grounding (pure)
    // -----------------------------------------------------------------------

    fn row() -> SearchRow {
        SearchRow {
            job_id: "job-1".to_string(),
            title: "Warehouse Assistant".to_string(),
            organization: "Horn Relief".to_string(),
            location: Some("Hargeisa".to_string()),
            kind: "job".to_string(),
            deadline: Some("2099-01-01".to_string()),
        }
    }

    #[test]
    fn the_phrasing_prompt_carries_the_rows_and_the_no_invention_rule() {
        let rows = vec![row()];
        let messages = phrasing_messages(&rows);
        let system = &messages[0].content;
        let user = &messages[1].content;

        assert!(system.contains("Never invent a job, employer"));
        assert!(system.contains("Nothing live matches that right now."));
        // The only entities in the request are the rows Rust selected.
        assert!(user.contains("Warehouse Assistant"));
        assert!(user.contains("Horn Relief"));
        assert!(user.contains("Hargeisa"));
        assert_eq!(user.lines().count(), 2, "`Rows:` + exactly one row line");
        // The caller's message is not forwarded: the phrasing call cannot be talked into
        // answering something else.
        assert!(!user.to_lowercase().contains("jobs in mogadishu"));
    }

    #[test]
    fn a_phrasing_that_invents_a_url_or_runs_long_falls_back_to_the_rows() {
        let rows = vec![row()];
        let invented = grounded_reply("Apply now at https://fake.example/job", &rows);
        assert_eq!(invented, render_rows(&rows));
        assert!(!invented.contains("http"));

        let long = grounded_reply("one\ntwo\nthree\nfour", &rows);
        assert_eq!(long, render_rows(&rows));

        let ok = grounded_reply("Warehouse Assistant at Horn Relief in Hargeisa.", &rows);
        assert_eq!(ok, "Warehouse Assistant at Horn Relief in Hargeisa.");
    }

    // -----------------------------------------------------------------------
    // Ledger (database, no network)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn charging_uses_the_allowance_then_the_balance_then_402() {
        let Some(state) = test_state("jobify_test_credits_order", StubLlm::new(vec![]), 2).await
        else {
            return;
        };

        assert_eq!(
            credits::charge(&state, "user-1").await.unwrap(),
            Charge::Allowance
        );
        assert_eq!(
            credits::charge(&state, "user-1").await.unwrap(),
            Charge::Allowance
        );
        let refused = credits::charge(&state, "user-1").await.unwrap_err();
        assert_eq!(refused.code(), "out_of_credits");
        assert_eq!(refused.message(), credits::OUT_OF_CREDITS_MESSAGE);
        assert_eq!(
            refused.status_code(),
            axum::http::StatusCode::PAYMENT_REQUIRED
        );

        let snapshot = credits::top_up(&state, "user-1", 1).await.unwrap();
        assert_eq!(snapshot.balance, 1);
        assert_eq!(
            credits::charge(&state, "user-1").await.unwrap(),
            Charge::Balance
        );
        assert!(credits::charge(&state, "user-1").await.is_err());

        let snapshot = credits::snapshot(&state, "user-1").await.unwrap();
        assert_eq!(snapshot.monthly_allowance, 2);
        assert_eq!(snapshot.used_this_period, 2);
        assert_eq!(snapshot.balance, 0);
        assert_eq!(snapshot.remaining_this_period, 0);
        assert!(!snapshot.period_end.is_empty());
    }

    #[tokio::test]
    async fn a_refund_restores_the_bucket_the_charge_came_from() {
        let Some(state) = test_state("jobify_test_credits_refund", StubLlm::new(vec![]), 1).await
        else {
            return;
        };
        let charge = credits::charge(&state, "user-1").await.unwrap();
        assert_eq!(charge, Charge::Allowance);
        credits::refund(&state, "user-1", charge).await;
        let snapshot = credits::snapshot(&state, "user-1").await.unwrap();
        assert_eq!(snapshot.used_this_period, 0);

        // Spend the allowance, then one bought query, and refund that one too.
        credits::charge(&state, "user-1").await.unwrap();
        credits::top_up(&state, "user-1", 1).await.unwrap();
        let charge = credits::charge(&state, "user-1").await.unwrap();
        assert_eq!(charge, Charge::Balance);
        credits::refund(&state, "user-1", charge).await;
        let snapshot = credits::snapshot(&state, "user-1").await.unwrap();
        assert_eq!(snapshot.balance, 1);
        assert_eq!(snapshot.used_this_period, 1);
    }

    // -----------------------------------------------------------------------
    // The whole engine (database + stub LLM)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn unsupported_returns_the_canned_text_and_charges_nothing() {
        let llm = StubLlm::new(vec![extraction("unsupported")]);
        let Some(state) = test_state("jobify_test_assistant_unsupported", llm.clone(), 3).await
        else {
            return;
        };

        let answer = answer(&state, "user-1", "write me a poem about the sea")
            .await
            .unwrap();
        assert_eq!(answer.intent, Intent::Unsupported);
        assert_eq!(answer.reply, UNSUPPORTED_REPLY);
        assert_eq!(answer.charged_queries, 0);
        assert!(answer.rows.is_empty());
        // Exactly one provider call: the extraction. There is no phrasing for a refusal.
        assert_eq!(llm.seen().len(), 1);

        let snapshot = credits::snapshot(&state, "user-1").await.unwrap();
        assert_eq!(snapshot.used_this_period, 0);
        assert_eq!(snapshot.balance, 0);
        assert_eq!(snapshot.remaining_this_period, 3);
    }

    #[tokio::test]
    async fn help_is_the_constant_and_costs_one_query() {
        let llm = StubLlm::new(vec![extraction("help")]);
        let Some(state) = test_state("jobify_test_assistant_help", llm.clone(), 3).await else {
            return;
        };
        let answer = answer(&state, "user-1", "how do I register?")
            .await
            .unwrap();
        assert_eq!(answer.intent, Intent::Help);
        assert_eq!(answer.reply, HELP_TEXT);
        assert_eq!(answer.charged_queries, 1);
        assert_eq!(
            llm.seen().len(),
            1,
            "help is answered without a phrasing call"
        );
        assert_eq!(answer.credits.used_this_period, 1);
    }

    #[tokio::test]
    async fn status_reports_the_users_own_credits_and_charges_one() {
        let llm = StubLlm::new(vec![extraction("status")]);
        let Some(state) = test_state("jobify_test_assistant_status", llm.clone(), 3).await else {
            return;
        };
        let answer = answer(&state, "user-1", "what are my credits?")
            .await
            .unwrap();
        assert_eq!(answer.intent, Intent::Status);
        assert!(answer.reply.contains("Credits:"));
        assert!(answer.reply.contains("Subscription:"));
        assert!(answer.reply.contains("alerts today"));
        assert_eq!(answer.charged_queries, 1);
    }

    #[tokio::test]
    async fn an_empty_result_set_answers_the_constant_and_never_calls_the_model() {
        let llm = StubLlm::new(vec![extraction("search")]);
        let Some(state) = test_state("jobify_test_assistant_empty", llm.clone(), 3).await else {
            return;
        };
        // No jobs seeded: the throwaway database is empty.
        let answer = answer(&state, "user-1", "jobs in Mogadishu").await.unwrap();
        assert_eq!(answer.intent, Intent::Search);
        assert_eq!(answer.reply, NOTHING_MATCHES);
        assert!(answer.rows.is_empty());
        assert_eq!(llm.seen().len(), 1, "extraction only; no phrasing call");
        assert_eq!(answer.charged_queries, 1);
    }

    #[tokio::test]
    async fn search_answers_from_the_database_and_hands_only_those_rows_to_the_model() {
        let llm = StubLlm::new(vec![
            text(r#"{"intent":"search","keywords":["warehouse"],"location":"Hargeisa"}"#),
            text("1. Warehouse Assistant at Horn Relief in Hargeisa."),
        ]);
        let Some(state) = test_state("jobify_test_assistant_search", llm.clone(), 3).await else {
            return;
        };
        seed_job(&state).await;

        let answer = answer(&state, "user-1", "warehouse jobs in Hargeisa")
            .await
            .unwrap();
        assert_eq!(answer.intent, Intent::Search);
        assert_eq!(answer.rows.len(), 1);
        assert_eq!(answer.rows[0].title, "Warehouse Assistant");
        assert!(answer.reply.contains("Warehouse Assistant"));
        assert_eq!(answer.charged_queries, 1);
        assert_eq!(answer.credits.used_this_period, 1);

        let seen = llm.seen();
        assert_eq!(seen.len(), 2, "extraction then phrasing");
        let phrasing = &seen[1];
        assert!(phrasing.messages[0].content.contains("Never invent a job"));
        let body = &phrasing.messages[1].content;
        assert!(body.contains("Warehouse Assistant"));
        assert!(body.contains("Horn Relief"));
        assert!(!body.contains("http"));
    }

    #[tokio::test]
    async fn a_provider_failure_is_502_with_zero_queries_charged() {
        let llm = StubLlm::new(vec![Err(LlmError::Provider(401))]);
        let Some(state) = test_state("jobify_test_assistant_provider", llm, 3).await else {
            return;
        };
        let err = answer(&state, "user-1", "jobs in Mogadishu")
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), axum::http::StatusCode::BAD_GATEWAY);
        assert_eq!(err.code(), "llm_unavailable");
        assert_eq!(err.message(), LLM_FAILURE_MESSAGE);
        // No upstream status, body or key reaches the caller.
        assert!(!err.message().contains("401"));

        let snapshot = credits::snapshot(&state, "user-1").await.unwrap();
        assert_eq!(snapshot.used_this_period, 0);
        assert_eq!(snapshot.balance, 0);
        assert_eq!(snapshot.remaining_this_period, 3);

        let usage = state
            .assistant_usage()
            .count_documents(doc! { "user_id": "user-1" })
            .await
            .unwrap();
        assert_eq!(usage, 0, "a failed answer records no usage");
    }

    #[tokio::test]
    async fn the_twenty_first_query_in_ten_minutes_is_429() {
        let replies: Vec<Result<LlmReply, LlmError>> = (0..RATE_MAX + 5)
            .map(|_| extraction("unsupported"))
            .collect();
        let llm = StubLlm::new(replies);
        let Some(state) = test_state("jobify_test_assistant_rate", llm, 30).await else {
            return;
        };
        for _ in 0..RATE_MAX {
            answer(&state, "user-1", "hello there").await.unwrap();
        }
        let err = answer(&state, "user-1", "hello there").await.unwrap_err();
        assert_eq!(err.status_code(), axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.code(), "rate_limited");
        // Nothing was charged for the refused call either.
        let snapshot = credits::snapshot(&state, "user-1").await.unwrap();
        assert_eq!(snapshot.used_this_period, 0);
    }

    #[tokio::test]
    async fn a_member_with_no_credits_is_refused_before_any_provider_call() {
        let llm = StubLlm::new(vec![]);
        let Some(state) = test_state("jobify_test_assistant_402", llm.clone(), 0).await else {
            return;
        };
        let err = answer(&state, "user-1", "jobs in Mogadishu")
            .await
            .unwrap_err();
        assert_eq!(err.code(), "out_of_credits");
        assert!(
            llm.seen().is_empty(),
            "402 lands before the extraction call"
        );
    }
}
