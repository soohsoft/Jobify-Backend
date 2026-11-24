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
        }
    }
}

fn env_var(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}
