use futures_util::TryStreamExt;
use mongodb::bson::Document;
use mongodb::options::FindOptions;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::convert::Infallible;

use axum::response::sse::{Event, KeepAlive, Sse};
use tokio_stream::wrappers::ReceiverStream;

use crate::error::AppError;

pub type SseTx = tokio::sync::mpsc::Sender<Result<Event, Infallible>>;
pub type SseStream = Sse<ReceiverStream<Result<Event, Infallible>>>;

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

pub fn sse_event(name: &str, data: Value) -> Result<Event, Infallible> {
    Ok(Event::default().event(name).data(data.to_string()))
}

pub async fn send_sse(tx: &SseTx, name: &str, data: Value) {
    let _ = tx.send(sse_event(name, data)).await;
}

#[allow(dead_code)]
pub fn keep_alive() -> KeepAlive {
    KeepAlive::default()
}
