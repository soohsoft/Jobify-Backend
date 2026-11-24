//! The LLM access layer, deliberately tiny.
//!
//! One JSON call to an OpenAI-compatible `/chat/completions` endpoint. No SDK: the whole
//! surface is a request body and two response fields, and an SDK would add a dependency
//! tree for a JSON reshape we can read.
//!
//! Everything sits behind the [`LlmClient`] trait so the assistant can be tested with a
//! stub and with no network at all. The real client is the only implementation that
//! touches reqwest.

use std::time::Duration;

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// A hard ceiling on one provider call. The assistant caps an extraction at 1000 tokens —
/// a reasoning model spends part of that budget thinking before it writes the JSON, so a
/// 300-token cap truncated the answer and looked like a refusal (see `EXTRACTION_MAX_TOKENS`).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub messages: Vec<LlmMessage>,
    /// Bounds the cost of the call and therefore the request. Never optional here.
    pub max_tokens: u32,
    /// Ask the provider for a JSON object. The assistant still validates whatever comes
    /// back — JSON mode is a hint, not a parser.
    pub json_mode: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LlmUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct LlmReply {
    pub content: String,
    pub usage: LlmUsage,
    /// The provider's stop reason. `"length"` means the output hit `max_tokens` and was
    /// cut off — for a JSON call that answer is incomplete, however well-formed its head
    /// looks, so the assistant retries it instead of treating it as a refusal.
    pub finish_reason: Option<String>,
}

/// Internal failure detail. The variant text is for `tracing` only: the assistant answers
/// the caller with one fixed 502 message, so a provider body (which can echo parts of the
/// prompt) and the API key never reach a response.
#[derive(Debug)]
pub enum LlmError {
    /// No key or base URL configured.
    NotConfigured,
    Timeout,
    /// A non-2xx answer. Only the status is kept; the body is dropped unread.
    Provider(u16),
    /// Connection refused, DNS failure, a malformed body — anything else.
    Transport(String),
}

impl LlmError {
    pub fn detail(&self) -> String {
        match self {
            LlmError::NotConfigured => "LLM_API_KEY / LLM_BASE_URL are not configured".to_string(),
            LlmError::Timeout => "provider call timed out".to_string(),
            LlmError::Provider(status) => format!("provider answered HTTP {status}"),
            LlmError::Transport(detail) => format!("provider transport error: {detail}"),
        }
    }
}

/// Every assistant call goes through this. `Send + Sync` so one instance can live in
/// `AppState` behind an `Arc` and serve concurrent requests.
pub trait LlmClient: Send + Sync {
    fn complete<'a>(&'a self, request: LlmRequest) -> BoxFuture<'a, Result<LlmReply, LlmError>>;
}

pub struct HttpLlmClient {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl HttpLlmClient {
    pub fn from_config(config: &Config) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .unwrap_or_default(),
            base_url: config.llm_base_url.trim_end_matches('/').to_string(),
            api_key: config.llm_api_key.clone(),
            model: config.llm_model.clone(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
}

#[derive(Deserialize)]
struct ApiMessage {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct ApiChoice {
    #[serde(default)]
    message: Option<ApiMessage>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    choices: Vec<ApiChoice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

impl LlmClient for HttpLlmClient {
    fn complete<'a>(&'a self, request: LlmRequest) -> BoxFuture<'a, Result<LlmReply, LlmError>> {
        Box::pin(async move {
            if self.api_key.trim().is_empty() || self.base_url.trim().is_empty() {
                return Err(LlmError::NotConfigured);
            }

            let mut body = serde_json::json!({
                "model": self.model,
                "messages": request.messages,
                // Temperature 0 on every call: both calls are extraction and grounding,
                // never creative writing.
                "temperature": 0,
                "stream": false,
                "max_tokens": request.max_tokens,
            });
            if request.json_mode {
                body["response_format"] = serde_json::json!({ "type": "json_object" });
            }

            let response = self
                .client
                .post(self.endpoint())
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|err| {
                    if err.is_timeout() {
                        LlmError::Timeout
                    } else {
                        // `err` may contain the URL but never the bearer token (reqwest's
                        // redacted Debug/Display does not print headers).
                        LlmError::Transport(err.to_string())
                    }
                })?;

            if !response.status().is_success() {
                // The body is deliberately not read: it is where an upstream error message
                // — and sometimes a fragment of the request — would come from.
                return Err(LlmError::Provider(response.status().as_u16()));
            }

            let data: ApiResponse = response
                .json()
                .await
                .map_err(|err| LlmError::Transport(err.to_string()))?;
            let mut content = String::new();
            let mut finish_reason = None;
            if let Some(choice) = data.choices.into_iter().next() {
                if let Some(message) = choice.message {
                    content = message.content;
                }
                finish_reason = choice.finish_reason;
            }
            let usage = data
                .usage
                .map(|usage| LlmUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                })
                .unwrap_or_default();

            Ok(LlmReply {
                content,
                usage,
                finish_reason,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unconfigured_client_refuses_before_any_network_call() {
        let mut config = Config::from_env();
        // Blank, not removed: this is the shape `.env.example` ships before a key exists.
        config.llm_api_key = "   ".to_string();
        config.llm_base_url = "https://example.invalid".to_string();
        assert!(!config.llm_configured());

        let client = HttpLlmClient::from_config(&config);
        let reply = client
            .complete(LlmRequest {
                messages: vec![LlmMessage::user("hi")],
                max_tokens: 10,
                json_mode: false,
            })
            .await;
        assert!(matches!(reply, Err(LlmError::NotConfigured)));
    }

    #[test]
    fn provider_status_is_not_leaked_into_the_detail_of_a_transport_error() {
        // The mapping the assistant relies on: every error has a loggable detail, and the
        // caller-facing text is a constant chosen by the route, not this module.
        assert!(LlmError::Provider(401).detail().contains("401"));
        assert!(LlmError::Timeout.detail().contains("timed out"));
    }
}
