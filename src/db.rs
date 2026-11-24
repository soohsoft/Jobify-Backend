use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Client, Database, IndexModel};

use crate::config::Config;

pub async fn connect(config: &Config) -> mongodb::error::Result<Database> {
    let client = Client::with_uri_str(&config.mongodb_uri).await?;
    let db = client.database(&config.database_name);
    ensure_indexes(&db).await?;
    Ok(db)
}

/// Indexes only. No seeding: `sources` already holds the four configured boards and the
/// live collection must not be touched at startup.
async fn ensure_indexes(db: &Database) -> mongodb::error::Result<()> {
    // Prevent duplicate registrations racing the find-then-insert in auth.
    db.collection::<mongodb::bson::Document>("users")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "email": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // One saved row per (user, job). The unique index is what makes saving idempotent by
    // construction: a second save upserts onto the same row instead of stacking a copy.
    db.collection::<mongodb::bson::Document>("saved_jobs")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "user_id": 1, "job_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // The saved list is always read newest-first for one user.
    db.collection::<mongodb::bson::Document>("saved_jobs")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "user_id": 1, "created_at": -1 })
                .build(),
        )
        .await?;

    // Matches the ingest upsert filter, so job upserts stay idempotent per source.
    db.collection::<mongodb::bson::Document>("jobs")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "external_id": 1, "source": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // The feed and the matcher both order by recency and filter by category.
    db.collection::<mongodb::bson::Document>("jobs")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "category": 1, "created_at": -1 })
                .build(),
        )
        .await?;

    // One notification per (user, job). Partial, so notifications carrying no job_id are
    // not all treated as the same null key.
    db.collection::<mongodb::bson::Document>("notifications")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "user_id": 1, "job_id": 1 })
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .partial_filter_expression(doc! { "job_id": { "$exists": true } })
                        .build(),
                )
                .build(),
        )
        .await?;

    Ok(())
}
