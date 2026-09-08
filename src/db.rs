use mongodb::{Client, Database, IndexModel, bson::doc, options::IndexOptions};

use crate::config::Config;
use crate::models::default_job_sources;

pub async fn connect(config: &Config) -> mongodb::error::Result<Database> {
    let client = Client::with_uri_str(&config.mongodb_uri).await?;
    let db = client.database(&config.database_name);
    seed_default_sources(&db).await?;
    ensure_indexes(&db).await?;
    Ok(db)
}

async fn seed_default_sources(db: &Database) -> mongodb::error::Result<()> {
    let sources = db.collection::<crate::models::SourceConfig>("sources");
    let count = sources.count_documents(doc! {}).await?;
    if count == 0 {
        sources.insert_many(default_job_sources()).await?;
    }
    Ok(())
}

async fn ensure_indexes(db: &Database) -> mongodb::error::Result<()> {
    // Prevent duplicate registrations racing the find-then-insert in auth.
    db.collection::<crate::models::UserDoc>("users")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "email": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // Matches the ingest upsert filter, so job upserts stay idempotent per source.
    db.collection::<crate::models::JobDoc>("jobs")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "external_id": 1, "source": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    Ok(())
}
