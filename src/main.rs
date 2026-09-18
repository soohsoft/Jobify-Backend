mod auth;
// Ingest validation uses `Category::from_slug`; the group/label accessors are
// for the job-metadata endpoint and job matching still to come.
#[allow(dead_code)]
mod categories;
mod config;
mod db;
mod error;
mod llm;
mod memory;
mod middleware;
mod models;
mod prompts;
mod routes;
mod services;
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

    let routes = crate::routes::app(state);

    let addr = format!("{}:{}", config.host, config.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("jobify-backend listening on {addr}");
    axum::serve(listener, routes).await?;

    Ok(())
}
