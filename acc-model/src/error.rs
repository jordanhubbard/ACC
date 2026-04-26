use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Wire shape of error responses from the server.
///
/// The server always emits `{ "error": "<code>", ... }` with 4xx/5xx
/// statuses. Some endpoints add contextual fields (`duplicate_id`,
/// `pending`, `active`, `max`, `message`) — we preserve these in `extra`
/// so callers can surface them without losing fidelity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
