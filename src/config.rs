use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub cors_origin: String,
    pub mongodb_uri: String,
    pub database_name: String,
    pub jwt_secret: String,
    pub jwt_issuer: String,
    pub internal_api_key: String,
    pub waafi_api_key: String,
    pub waafi_account_id: String,
    pub waafi_api_url: String,
    pub waafi_verify_url: String,
    pub waafi_callback_url: String,
    pub deepseek_api_key: String,
    pub deepseek_base_url: String,
    pub deepseek_model: String,
    /// Output cap for a conversational reply. Output is the expensive side of the
    /// bill (~4x input on DeepSeek Flash), so this is the single most important
    /// number protecting the margin: without it one runaway response can cost
    /// more than a whole CV interview.
    pub llm_max_tokens_chat: u64,
    /// Output cap for the JSON extraction call that runs on every chat turn.
    pub llm_max_tokens_extract: u64,
    /// Ceiling on tokens one chat session may consume, regardless of balance.
    /// A CV is ~60 interview turns, so an unbounded session is an unbounded cost
    /// even when every individual request is capped.
    pub chat_session_token_budget: u64,
    /// Tokens granted to a newly registered account. With none, the first chat is refused
    /// with a 402 and the product is dead on arrival, so every new user starts with a
    /// working balance. Applied only at creation.
    pub signup_grant_tokens: u64,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();

        Self {
            host: env_var("HOST", "0.0.0.0"),
            port: env_var("PORT", "3000").parse().unwrap_or(3000),
            cors_origin: env_var("CORS_ORIGIN", "*"),
            mongodb_uri: env_var("MONGODB_URI", "mongodb://127.0.0.1:27017/jobify"),
            database_name: env_var("MONGODB_DB_NAME", "jobify"),
            jwt_secret: env_var("JWT_SECRET", "jobify-dev-secret-change-me"),
            jwt_issuer: env_var("JWT_ISSUER", "jobify-backend"),
            internal_api_key: env_var(
                "SCRAPER_INTERNAL_API_KEY",
                "jobify-scraper-dev-key-change-me",
            ),
            waafi_api_key: env_var("WAAFI_API_KEY", ""),
            waafi_account_id: env_var("WAAFI_ACCOUNT_ID", ""),
            waafi_api_url: env_var("WAAFI_API_URL", "https://api.waafipay.net"),
            waafi_verify_url: env_var(
                "WAAFI_VERIFY_URL",
                "https://api.waafipay.net/v1/payments/verify",
            ),
            waafi_callback_url: env_var("WAAFI_CALLBACK_URL", ""),
            deepseek_api_key: env_var("DEEPSEEK_API_KEY", ""),
            deepseek_base_url: env_var("DEEPSEEK_BASE_URL", "https://api.deepseek.com"),
            deepseek_model: env_var("DEEPSEEK_MODEL", "deepseek-chat"),
            llm_max_tokens_chat: env_u64("LLM_MAX_TOKENS_CHAT", 1_500),
            llm_max_tokens_extract: env_u64("LLM_MAX_TOKENS_EXTRACT", 2_000),
            chat_session_token_budget: env_u64("CHAT_SESSION_TOKEN_BUDGET", 150_000),
            signup_grant_tokens: env_u64("SIGNUP_GRANT_TOKENS", 300_000),
        }
    }

    pub fn waafi_configured(&self) -> bool {
        !self.waafi_api_key.is_empty() && !self.waafi_account_id.is_empty()
    }

    pub fn deepseek_configured(&self) -> bool {
        !self.deepseek_api_key.is_empty()
    }
}

fn env_var(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(default)
}
