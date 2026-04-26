//! Per-item mutation endpoints on `/api/item/{id}/*`.
//!
//! These are the queue workers' write path: claim, complete, fail, comment,
//! and keepalive against individual queue items. The heartbeat endpoint
//! (`/api/heartbeat/{agent}`) lives here too — it's agent-liveness coupled
//! to the same queue-worker loop.

use crate::{Client, Error, Result};
use acc_model::{
    ClaimItemRequest, CommentItemRequest, CompleteItemRequest, FailItemRequest, HeartbeatRequest,
    KeepaliveRequest,
};

#[derive(Debug, Clone, Copy)]
pub struct ItemsApi<'a> {
    pub(crate) client: &'a Client,
}

impl<'a> ItemsApi<'a> {
    /// POST /api/item/{id}/claim
    ///
    /// Returns [`Error::Conflict`] if another agent already claimed the
    /// item (HTTP 409).
    pub async fn claim(self, id: &str, agent: &str, note: Option<&str>) -> Result<()> {
        let body = ClaimItemRequest {
            agent: agent.to_string(),
            note: note.map(str::to_string),
        };
        self.post(&format!("/api/item/{id}/claim"), &body).await
    }

    /// POST /api/item/{id}/complete
    pub async fn complete(
        self,
        id: &str,
        agent: &str,
        result: Option<&str>,
        resolution: Option<&str>,
    ) -> Result<()> {
        let body = CompleteItemRequest {
            agent: agent.to_string(),
            result: result.map(str::to_string),
            resolution: resolution.map(str::to_string),
        };
        self.post(&format!("/api/item/{id}/complete"), &body).await
    }

    /// POST /api/item/{id}/fail
    pub async fn fail(self, id: &str, agent: &str, reason: &str) -> Result<()> {
        let body = FailItemRequest {
            agent: agent.to_string(),
            reason: reason.to_string(),
        };
        self.post(&format!("/api/item/{id}/fail"), &body).await
    }

    /// POST /api/item/{id}/comment
    pub async fn comment(self, id: &str, agent: &str, comment: &str) -> Result<()> {
        let body = CommentItemRequest {
            agent: agent.to_string(),
            comment: comment.to_string(),
        };
        self.post(&format!("/api/item/{id}/comment"), &body).await
    }

    /// POST /api/item/{id}/keepalive
    pub async fn keepalive(self, id: &str, agent: &str, note: Option<&str>) -> Result<()> {
        let body = KeepaliveRequest {
            agent: agent.to_string(),
            note: note.map(str::to_string),
        };
        self.post(&format!("/api/item/{id}/keepalive"), &body).await
    }

    /// POST /api/heartbeat/{agent} — agent liveness beacon.
    pub async fn heartbeat(self, agent: &str, req: &HeartbeatRequest) -> Result<()> {
        self.post(&format!("/api/heartbeat/{agent}"), req).await
    }

    async fn post<B: serde::Serialize>(self, path: &str, body: &B) -> Result<()> {
        let resp = self
            .client
            .http()
            .post(self.client.url(path))
            .json(body)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if (200..300).contains(&status) {
            return Ok(());
        }
        let bytes = resp.bytes().await?;
        Err(Error::from_response(status, &bytes))
    }
}
