use mongodb::{Client, Database, bson::doc};

use crate::config::Config;
use crate::models::default_job_sources;

pub async fn connect(config: &Config) -> mongodb::error::Result<Database> {
    let client = Client::with_uri_str(&config.mongodb_uri).await?;
    let db = client.database(&config.database_name);
    seed_default_sources(&db).await?;
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
