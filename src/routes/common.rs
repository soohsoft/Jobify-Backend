use futures_util::TryStreamExt;
use mongodb::bson::Document;
use mongodb::options::FindOptions;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::AppError;

/// One page of a collection plus the unpaginated total, so the response meta carries both.
pub async fn paginate<T>(
    collection: &mongodb::Collection<T>,
    filter: Document,
    page: u64,
    limit: u64,
    sort: Document,
) -> Result<(Vec<T>, u64), AppError>
where
    T: Serialize + DeserializeOwned + Unpin + Send + Sync,
{
    let total = collection.count_documents(filter.clone()).await?;
    let skip = (page.saturating_sub(1)) * limit;

    let options = FindOptions::builder()
        .sort(sort)
        .skip(skip)
        .limit(limit as i64)
        .build();

    let mut cursor = collection.find(filter).with_options(options).await?;
    let mut data = Vec::new();
    while let Some(doc) = cursor.try_next().await? {
        data.push(doc);
    }

    Ok((data, total))
}

pub fn total_pages(total: u64, limit: u64) -> u64 {
    (total as f64 / limit as f64).ceil() as u64
}
