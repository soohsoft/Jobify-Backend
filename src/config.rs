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
    /// The scraper sends this as `x-api-key`. There is deliberately no second name for it.
    pub internal_api_key: String,
    /// Where the web portal lives, with no trailing slash. `POST /internal/discord/link-token`
    /// builds `<APP_BASE_URL>/enter?t=<raw>` from it; the bot does not need to know the host.
    pub app_base_url: String,
    /// OpenAI-compatible chat-completions base, e.g. `https://api.deepseek.com`. The
    /// `LLM_*` names win; the historical `DEEPSEEK_*` ones are still read so an existing
    /// `.env` keeps working.
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    /// Free smart queries per 30-day period, before the paid balance is touched.
    pub monthly_smart_queries: i64,
    /// The fit percentage a listing must reach to be SHOWN or NOTIFIED. One number for the
    /// portal list, the bot's list and the alert queue, so a row cannot be good enough for
    /// one surface and not another. 0–100; anything outside that range is clamped.
    pub match_threshold: i64,
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
            // Trailing slashes are stripped here so the minted URL is never `//enter`.
            app_base_url: env_var("APP_BASE_URL", "http://localhost:5173")
                .trim_end_matches('/')
                .to_string(),
            llm_base_url: env_var_first(&["LLM_BASE_URL", "DEEPSEEK_BASE_URL"], "")
                .trim_end_matches('/')
                .to_string(),
            llm_api_key: env_var_first(&["LLM_API_KEY", "DEEPSEEK_API_KEY"], ""),
            llm_model: env_var_first(&["LLM_MODEL", "DEEPSEEK_MODEL"], "deepseek-chat"),
            monthly_smart_queries: env_var("SUBSCRIPTION_MONTHLY_SMART_QUERIES", "30")
                .parse()
                .unwrap_or(30)
                .clamp(0, 1000),
            // 70 is the product decision: a category match alone (30) is not an offer, and a
            // row with both its title (30) and description (10) keyword caps saturated is —
            // that is the top of the attainable scale. A garbage or out-of-range value falls
            // back to the default rather than 0 (which would show everything) or 101 (nothing).
            match_threshold: {
                let default = crate::routes::jobs::MATCH_THRESHOLD_DEFAULT;
                env_var("MATCH_THRESHOLD", &default.to_string())
                    .trim()
                    .parse::<i64>()
                    .map(|value| value.clamp(0, 100))
                    .unwrap_or(default)
            },
        }
    }

    /// An empty key or base URL means every assistant call answers 502 rather than
    /// silently trying to reach an unnamed host.
    pub fn llm_configured(&self) -> bool {
        !self.llm_api_key.trim().is_empty() && !self.llm_base_url.trim().is_empty()
    }
}

fn env_var(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

/// First variable that is set AND non-empty. An empty `LLM_API_KEY` line (the shape
/// `.env.example` ships) must fall through to the historical name rather than count as
/// a configured key.
fn env_var_first(keys: &[&str], default: &str) -> String {
    for key in keys {
        if let Ok(value) = env::var(key)
            && !value.trim().is_empty()
        {
            return value;
        }
    }
    default.to_string()
}
