//! Thin Slack Web API client used by the gateway tool surface.
//!
//! Wraps the small subset of `users.*`, `conversations.*`, and `team.*`
//! endpoints the LLM tools need for user lookup and channel introspection.
//! Authentication uses the bot token (`xoxb-`); user-token-only endpoints
//! are out of scope here.
//!
//! All methods return the parsed JSON envelope as `serde_json::Value`.
//! Slack errors (`{"ok": false, "error": "..."}`) are translated into
//! `Err(String)` so the calling tool can surface them to the model.

use reqwest::Client as Http;
use serde_json::Value;

const SLACK_API: &str = "https://slack.com/api";

#[derive(Clone)]
pub struct SlackApiClient {
    bot_token: String,
    http: Http,
}

impl SlackApiClient {
    pub fn new(bot_token: String) -> Self {
        let http = Http::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .expect("http client");
        Self { bot_token, http }
    }

    async fn get(&self, method: &str, query: &[(&str, String)]) -> Result<Value, String> {
        let url = format!("{SLACK_API}/{method}");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.bot_token)
            .query(query)
            .send()
            .await
            .map_err(|e| format!("slack {method}: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("slack {method}: parse error: {e}"))?;
        if !status.is_success() {
            return Err(format!("slack {method}: HTTP {status}"));
        }
        if !body["ok"].as_bool().unwrap_or(false) {
            let err = body["error"].as_str().unwrap_or("unknown_error");
            return Err(format!("slack {method}: {err}"));
        }
        Ok(body)
    }

    /// `users.info` — full profile for one user ID.
    pub async fn users_info(&self, user_id: &str) -> Result<Value, String> {
        self.get("users.info", &[("user", user_id.to_string())])
            .await
    }

    /// `users.list` — paginated user directory. `cursor` is empty on the
    /// first call; the response carries `response_metadata.next_cursor`.
    pub async fn users_list(
        &self,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Value, String> {
        let mut q: Vec<(&str, String)> = Vec::new();
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor.filter(|c| !c.is_empty()) {
            q.push(("cursor", c.to_string()));
        }
        self.get("users.list", &q).await
    }

    /// `conversations.members` — paginated member list for a channel.
    pub async fn conversations_members(
        &self,
        channel: &str,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<Value, String> {
        let mut q: Vec<(&str, String)> = vec![("channel", channel.to_string())];
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor.filter(|c| !c.is_empty()) {
            q.push(("cursor", c.to_string()));
        }
        self.get("conversations.members", &q).await
    }

    /// `conversations.history` — recent messages in a channel. `limit` is
    /// capped at 100 by the Slack API.
    pub async fn conversations_history(
        &self,
        channel: &str,
        limit: Option<u32>,
        cursor: Option<&str>,
        oldest: Option<&str>,
    ) -> Result<Value, String> {
        let mut q: Vec<(&str, String)> = vec![("channel", channel.to_string())];
        if let Some(l) = limit {
            q.push(("limit", l.to_string()));
        }
        if let Some(c) = cursor.filter(|c| !c.is_empty()) {
            q.push(("cursor", c.to_string()));
        }
        if let Some(o) = oldest.filter(|o| !o.is_empty()) {
            q.push(("oldest", o.to_string()));
        }
        self.get("conversations.history", &q).await
    }

    /// `team.info` — workspace metadata.
    pub async fn team_info(&self) -> Result<Value, String> {
        self.get("team.info", &[]).await
    }
}
