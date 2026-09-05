use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

impl TokenUsage {
    #[allow(dead_code)]
    pub fn sum(parts: &[Option<TokenUsage>]) -> Self {
        let mut out = TokenUsage::default();
        for part in parts {
            let Some(part) = part else { continue };
            out.prompt_tokens += part.prompt_tokens;
            out.completion_tokens += part.completion_tokens;
            out.total_tokens += part.total_tokens;
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct ChatResult {
    pub content: String,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Copy)]
pub struct ChatOptions {
    pub temperature: f64,
    pub json_mode: bool,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            temperature: 0.3,
            json_mode: false,
        }
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
    delta: Option<ApiMessage>,
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    choices: Vec<ApiChoice>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

pub struct LlmClient {
    client: reqwest::Client,
    config: Config,
}

impl LlmClient {
    pub fn new(config: Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    pub fn configured(&self) -> bool {
        self.config.deepseek_configured()
    }

    fn endpoint(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.deepseek_base_url.trim_end_matches('/')
        )
    }

    fn base_body(&self, messages: &[ChatMessage], options: ChatOptions) -> Value {
        let mut body = json!({
            "model": self.config.deepseek_model,
            "messages": messages,
            "temperature": options.temperature,
            "stream": false,
        });

        if options.json_mode {
            body["response_format"] = json!({ "type": "json_object" });
        }

        body
    }

    pub async fn chat(
        &self,
        messages: &[ChatMessage],
        options: ChatOptions,
    ) -> Result<ChatResult, AppError> {
        if !self.configured() {
            return Err(AppError::BadGateway(
                "DeepSeek API key is not configured".to_string(),
            ));
        }

        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.config.deepseek_api_key)
            .json(&self.base_body(messages, options))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::BadGateway(format!(
                "DeepSeek request failed ({status}): {}",
                text.chars().take(300).collect::<String>()
            )));
        }

        let data: ApiResponse = response.json().await?;
        let content = data
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.message)
            .map(|message| message.content)
            .unwrap_or_default();

        Ok(ChatResult {
            content,
            usage: data.usage.map(usage_from_api),
        })
    }

    /// Streams a completion and invokes `on_delta` for each content fragment.
    pub async fn stream<F>(
        &self,
        messages: &[ChatMessage],
        options: ChatOptions,
        mut on_delta: F,
    ) -> Result<ChatResult, AppError>
    where
        F: FnMut(&str) + Send,
    {
        if !self.configured() {
            return Err(AppError::BadGateway(
                "DeepSeek API key is not configured".to_string(),
            ));
        }

        let mut body = self.base_body(messages, options);
        body["stream"] = json!(true);
        body["stream_options"] = json!({ "include_usage": true });

        let response = self
            .client
            .post(self.endpoint())
            .bearer_auth(&self.config.deepseek_api_key)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::BadGateway(format!(
                "DeepSeek stream failed ({status}): {}",
                text.chars().take(300).collect::<String>()
            )));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut content = String::new();
        let mut usage: Option<TokenUsage> = None;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer.drain(..=pos);

                if let Some(payload) = line.strip_prefix("data:") {
                    let payload = payload.trim();
                    if payload.is_empty() || payload == "[DONE]" {
                        continue;
                    }
                    if let Ok(event) = serde_json::from_str::<ApiResponse>(payload) {
                        if let Some(delta) = event
                            .choices
                            .first()
                            .and_then(|choice| choice.delta.as_ref())
                            .and_then(|message| {
                                if message.content.is_empty() {
                                    None
                                } else {
                                    Some(message.content.as_str())
                                }
                            })
                        {
                            content.push_str(delta);
                            on_delta(delta);
                        }
                        if event.usage.is_some() {
                            usage = event.usage.map(usage_from_api);
                        }
                    }
                }
            }
        }

        Ok(ChatResult { content, usage })
    }
}

fn usage_from_api(usage: ApiUsage) -> TokenUsage {
    TokenUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
    }
}

/// Rough token estimation used only when DeepSeek does not return usage data.
pub fn estimate_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4).max(1)
}

pub fn estimate_prompt_tokens(messages: &[ChatMessage]) -> u64 {
    messages
        .iter()
        .map(|message| estimate_tokens(&message.content) + 4)
        .sum()
}
