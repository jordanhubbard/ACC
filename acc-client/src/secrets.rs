//! Secret-store client.
//!
//! Wraps the hub-node `/api/secrets/*key` endpoints. Authentication uses the
//! same Bearer token the rest of the client uses (loaded from
//! `ACC_AGENT_TOKEN` / `~/.acc/.env`).
//!
//! The hub stores secrets either in plaintext or via the encrypted Vault
//! (when `VAULT_PASSWORD` is set on the server). Both paths return the
//! same JSON envelope: `{"ok": true, "key": "...", "value": "..."}`.
//!
//! Keys may contain slashes — the path is forwarded verbatim. Recommended
//! convention: `<scope>/<sub-scope>/<name>`, e.g.
//! `slack/omgjkh/rocky/bot-token`.
//!
//! Missing keys map to `Ok(None)` (HTTP 404). The locked-vault state
//! (HTTP 503) maps to `Err(Error::Api { status: 503, .. })` — callers
//! that want to fall back to env vars should match on it explicitly.

use crate::{Client, Error, Result};
use serde_json::Value;

pub struct SecretsApi<'a> {
    pub(crate) client: &'a Client,
}

impl<'a> SecretsApi<'a> {
    /// Fetch a secret by key. Returns `Ok(None)` when the key is unknown
    /// (HTTP 404). All other non-2xx responses surface as `Err`.
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let path = format!("/api/secrets/{}", key.trim_start_matches('/'));
        match self.client.request_json("GET", &path, None).await {
            Ok(v) => Ok(extract_value_string(&v)),
            Err(Error::Api { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Fetch a secret and require it to be present. Convenience for callers
    /// that treat absence as a hard error.
    pub async fn require(&self, key: &str) -> Result<String> {
        match self.get(key).await? {
            Some(v) => Ok(v),
            None => Err(Error::Api {
                status: 404,
                body: acc_model::ApiError {
                    error: "secret_not_found".into(),
                    message: Some(format!("secret '{key}' not found")),
                    extra: Default::default(),
                },
            }),
        }
    }
}

fn extract_value_string(envelope: &Value) -> Option<String> {
    let v = envelope.get("value")?;
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        // The hub may store complex JSON values; flatten anything non-string.
        other => Some(other.to_string()),
    }
}
