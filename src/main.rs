mod auth;
// The taxonomy is the wire + database contract for `category`; the group/label accessors
// exist for clients that render the list.
mod categories;
mod config;
mod db;
mod error;
mod mail;
mod middleware;
mod models;
mod otp;
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

    let routes = crate::routes::app(state);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(addr = %addr, mail_live, "jobify-backend listening");
    axum::serve(listener, routes)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
