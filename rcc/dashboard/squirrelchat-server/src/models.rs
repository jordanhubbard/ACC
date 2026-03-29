use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Message ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub ts: i64,
    pub from_agent: String,
    pub text: String,
    pub channel: String,
    pub mentions: Vec<String>,
    pub thread_id: Option<i64>,
    pub reply_count: i64,
    /// HashMap<emoji, Vec<agent_id>>
    pub reactions: HashMap<String, Vec<String>>,
    pub slash_result: Option<String>,
}

// ── Channel ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub channel_type: String,
    pub created_by: Option<String>,
    pub created_at: i64,
    pub description: Option<String>,
}

// ── User / Agent ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub user_type: String,
    pub online: bool,
    pub status: String,
    pub last_seen: Option<i64>,
}

// ── Project ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub assignee: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

// ── Project File ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub id: i64,
    pub filename: String,
    pub size: Option<i64>,
    pub encoding: String,
    pub created_at: i64,
}

// ── WS frames (server → client) ───────────────────────────────────────────────

/// Reaction event carrying the full updated reactions map.
/// Matches ScReactionEvent in sc_types.rs: { message_id, reactions: HashMap<emoji, Vec<user_id>> }
#[derive(Debug, Clone, Serialize)]
pub struct ReactionEventData {
    pub message_id: String,
    pub reactions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    Message { data: Message },
    MessageEdit { data: MessageEditData },
    MessageDelete { data: MessageDeleteData },
    Reaction { data: ReactionEventData },
    Presence { data: PresenceData },
    ChannelCreate { data: Channel },
    Connected { session_id: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageEditData {
    pub id: String,
    pub text: String,
    pub edited_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageDeleteData {
    pub id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PresenceData {
    pub user: String,
    pub status: String,
}

// ── WS frames (client → server) ───────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Ping,
    Heartbeat { agent: String, status: String },
}
