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
    //
    // PARTIAL, and that is the whole point. A Discord account has no email, so several
    // user documents carry `email: ""` (and the field may be absent). A plain unique index
    // on `email` treats every empty/missing value as the same null key, which would allow
    // exactly ONE Discord user in the entire database. The partial expression covers only
    // real addresses, so many Discord accounts coexist while two people still cannot share
    // a password account's address.
    let users = db.collection::<mongodb::bson::Document>("users");
    // The pre-Discord index was named `email_1` and is strictly weaker than the one below.
    // Dropping by name is idempotent (a missing index is an error we ignore) and is what
    // lets `create_index` change the options instead of failing with a conflict.
    let _ = users.drop_index("email_1").await;
    users
        .create_index(
            IndexModel::builder()
                .keys(doc! { "email": 1 })
                .options(
                    IndexOptions::builder()
                        .unique(true)
                        .name("email_unique_nonempty".to_string())
                        .partial_filter_expression(doc! { "email": { "$gt": "" } })
                        .build(),
                )
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

    // The same index set on BOTH listing collections. Tenders are jobs with a different
    // destination, so the ingest upsert key, the category+recency feed scan and the plain
    // recency sort are identical. Creating them on an empty/new collection is fine: the
    // build succeeds and the index is there before the first write.
    for listing in ["jobs", "tenders"] {
        let collection = db.collection::<mongodb::bson::Document>(listing);

        // Matches the ingest upsert filter, so upserts stay idempotent per source.
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "external_id": 1, "source": 1 })
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
            )
            .await?;

        // The feed and the matcher both order by recency and filter by category.
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "category": 1, "created_at": -1 })
                    .build(),
            )
            .await?;

        // A merged feed (`job` + `tender`) sorts the two collections by this field, and a
        // single-collection read still needs it for the newest-first page.
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "created_at": -1 })
                    .build(),
            )
            .await?;
    }

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

    // One Jobify user per Discord account, enforced by the database rather than by the
    // find-then-write in the route: two bot calls for a new member race each other.
    db.collection::<mongodb::bson::Document>("discord_links")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "discord_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // Lookups by user, for the portal and for account deletion.
    db.collection::<mongodb::bson::Document>("discord_links")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "user_id": 1 })
                .options(IndexOptions::builder().unique(true).build())
                .build(),
        )
        .await?;

    // A link token stops being useful at `expires_at`, so the row may go with it. TTL only
    // acts on real BSON dates, which is why the model stores `expires_at` as a date and not
    // an RFC3339 string. Expiry correctness does not depend on this: the exchange query
    // filters on the date itself.
    db.collection::<mongodb::bson::Document>("link_tokens")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "expires_at": 1 })
                .options(
                    IndexOptions::builder()
                        .name("expires_at_ttl".to_string())
                        .expire_after(std::time::Duration::from_secs(0))
                        .build(),
                )
                .build(),
        )
        .await?;

    // The smart-query ledger is read and decremented by `user_id` on every assistant call.
    // Deliberately NOT unique: the nine legacy rows predate any index here, and a duplicate
    // would then block startup. `credits::load_or_create` upserts on the same key.
    db.collection::<mongodb::bson::Document>("credits")
        .create_index(IndexModel::builder().keys(doc! { "user_id": 1 }).build())
        .await?;

    // One row per answered smart query; always read per user, newest first.
    db.collection::<mongodb::bson::Document>("assistant_usage")
        .create_index(
            IndexModel::builder()
                .keys(doc! { "user_id": 1, "at": -1 })
                .build(),
        )
        .await?;

    Ok(())
}
