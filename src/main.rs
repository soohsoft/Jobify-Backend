mod assistant;
mod auth;
// The taxonomy is the wire + database contract for `category`; the group/label accessors
// exist for clients that render the list.
mod categories;
mod config;
mod credits;
mod db;
mod error;
mod links;
mod llm;
mod mail;
mod middleware;
mod models;
/// The education/experience → matcher translation. Called by the Discord profile route and
/// unit-tested on its own, because these lists decide who is sent what.
mod onboarding;
mod otp;
/// The one completion-time model call that decides categories and subcategories. Kept out of
/// `onboarding` on purpose: that module is the deterministic fallback and must stay callable
/// (and testable) with no provider at all.
mod profile_llm;
mod routes;
mod state;
mod util;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jobify_backend=info,tower_http=info".into()),
        )
        .init();

    let config = crate::config::Config::from_env();
    let db = crate::db::connect(&config).await?;
    let state = crate::state::AppState::new(db, config.clone());
    let mail_live = state.mailer.is_live();
    // An unconfigured provider is a valid deployment state: every assistant call answers
    // 502 and charges nothing. Say so at startup instead of discovering it per request.
    tracing::info!(
        llm_configured = config.llm_configured(),
        llm_model = %config.llm_model,
        "assistant provider"
    );

    let routes = crate::routes::app(state);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, mail_live, "jobify-backend listening");
    // Connect info is enabled so `POST /auth/magic` can key its attempt limit on the real
    // peer address rather than on a header the caller controls.
    axum::serve(
        listener,
        routes.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
