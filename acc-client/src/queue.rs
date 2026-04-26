//! Queue list/get operations on `/api/queue` and `/api/item/{id}`.

use crate::{Client, Error, Result};
use acc_model::QueueItem;
use serde::Deserialize;

/// Entry point for queue list/get operations. Obtain via [`Client::queue`].
#[derive(Debug, Clone, Copy)]
pub struct QueueApi<'a> {
    pub(crate) client: &'a Client,
}

impl<'a> QueueApi<'a> {
    /// GET /api/queue — list queue items.
    ///
    /// The server returns either a bare array or `{"items": [...]}`; this
    /// accepts both.
    pub async fn list(self) -> Result<Vec<QueueItem>> {
        let resp = self
            .client
            .http()
            .get(self.client.url("/api/queue"))
            .send()
            .await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?;
        if !(200..300).contains(&status) {
            return Err(Error::from_response(status, &bytes));
        }
        let env: ListEnvelope = serde_json::from_slice(&bytes)?;
        Ok(match env {
            ListEnvelope::Wrapped { items } => items,
            ListEnvelope::Bare(v) => v,
        })
    }

    /// GET /api/item/{id}
    pub async fn get(self, id: &str) -> Result<QueueItem> {
        let resp = self
            .client
            .http()
            .get(self.client.url(&format!("/api/item/{id}")))
            .send()
            .await?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await?;
        if !(200..300).contains(&status) {
            return Err(Error::from_response(status, &bytes));
        }
        let env: SingleEnvelope = serde_json::from_slice(&bytes)?;
        Ok(match env {
            SingleEnvelope::Wrapped { item } => item,
            SingleEnvelope::Bare(v) => v,
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ListEnvelope {
    Wrapped { items: Vec<QueueItem> },
    Bare(Vec<QueueItem>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SingleEnvelope {
    Wrapped { item: QueueItem },
    Bare(QueueItem),
}
