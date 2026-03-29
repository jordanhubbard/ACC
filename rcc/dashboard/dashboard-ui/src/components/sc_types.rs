// sc_types.rs — SquirrelChat shared type definitions
// Single source of truth. Both squirrelchat.rs and future component modules import from here.
// Wire format matches squirrelchat/API.md exactly.
//
// REACTIONS DESIGN DECISION (2026-03-29, Rocky):
// Wire format is HashMap<String, Vec<String>>: {"🔥": ["rocky", "natasha"]}
// This is richer than Vec<ScReaction> — supports hover (who reacted), toggle logic,
// dedupe by user. The UI layer computes count/by_me from the map.
// ScReaction is a VIEW type (computed from the map, not deserialized).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Reaction ────────────────────────────────────────────────────────────────

/// VIEW type — computed from ScMessage.reactions map for display.
/// NOT deserialized from wire. Use ScMessage::reaction_counts() / user_reacted().
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScReaction {
    pub emoji: String,
    pub count: usize,
    pub by_me: bool,
}

// ─── Message ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScMessage {
    pub id: Option<String>,
    /// unix ms
    pub ts: Option<u64>,
    /// user id
    #[serde(alias = "from_agent")]
    pub from: Option<String>,
    /// display name
    pub from_name: Option<String>,
    pub text: Option<String>,
    pub channel: Option<String>,
    pub mentions: Option<Vec<String>>,
    /// parent message id if this is a thread reply (null = top-level)
    #[serde(alias = "parent_id")]
    pub thread_id: Option<String>,
    /// number of replies on a top-level message
    #[serde(alias = "reply_count")]
    pub thread_count: Option<u32>,
    /// Wire format: emoji → [user_id, ...].  {"🔥": ["rocky", "natasha"]}
    #[serde(default)]
    pub reactions: HashMap<String, Vec<String>>,
    /// attached files
    #[serde(default)]
    pub files: Vec<ScChannelFile>,
    /// unix ms if edited
    pub edited_at: Option<u64>,
    pub created_at: Option<u64>,
    /// slash command result (server.mjs compat)
    pub slash_result: Option<String>,
}

impl ScMessage {
    /// Returns true if `user_id` has reacted with `emoji`.
    pub fn user_reacted(&self, user_id: &str, emoji: &str) -> bool {
        self.reactions
            .get(emoji)
            .map(|users| users.iter().any(|u| u == user_id))
            .unwrap_or(false)
    }

    /// Returns computed reaction view sorted by count descending, emoji ascending.
    /// Pass `me` = current user id to populate `by_me`.
    pub fn reaction_counts(&self, me: &str) -> Vec<ScReaction> {
        let mut out: Vec<ScReaction> = self
            .reactions
            .iter()
            .map(|(emoji, users)| ScReaction {
                emoji: emoji.clone(),
                count: users.len(),
                by_me: users.iter().any(|u| u == me),
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count).then(a.emoji.cmp(&b.emoji)));
        out
    }
}

// ─── Channel ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScChannel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// "public" | "private" | "dm" — wire field is "type"
    #[serde(rename = "type", alias = "kind")]
    pub channel_type: Option<String>,
    /// DM participants (user ids)
    pub participants: Option<Vec<String>>,
    /// Full member list (populated by GET /api/channels/:id/members)
    #[serde(default)]
    pub members: Vec<ScUser>,
    pub created_at: Option<u64>,
    pub last_message_at: Option<u64>,
    /// Client-side only (not from wire — compute from local unread cursor)
    #[serde(skip)]
    pub unread_count: u32,
}

// ─── User ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScUser {
    pub id: String,
    pub name: String,
    /// "admin" | "user" | "agent"
    pub role: Option<String>,
    pub avatar_url: Option<String>,
    /// "online" | "idle" | "offline"
    pub status: Option<String>,
    pub last_seen: Option<u64>,
}

impl ScUser {
    pub fn is_online(&self) -> bool {
        matches!(self.status.as_deref(), Some("online") | Some("idle"))
    }

    pub fn presence_icon(&self) -> &'static str {
        match self.status.as_deref() {
            Some("online") => "🟢",
            Some("idle") => "🟡",
            _ => "🔴",
        }
    }
}

/// Legacy alias — kept until all callers migrate to ScUser
pub type ScAgent = ScUser;

// ─── Identity (current user) ──────────────────────────────────────────────────

/// Returned by GET /api/me, also stored in localStorage as "sc_identity"
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScIdentity {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub avatar_url: Option<String>,
    /// If true, show "set your name" modal
    #[serde(default)]
    pub needs_name: bool,
    /// Local-only: auth token (not from wire)
    #[serde(skip)]
    pub token: Option<String>,
}

// ─── WebSocket frames ────────────────────────────────────────────────────────

/// Client → Server frames
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScWsClientFrame {
    Auth { token: String },
    Typing { channel: String },
    Subscribe { channels: Vec<String> },
    Unsubscribe { channels: Vec<String> },
    Ping,
    Pong,
}

/// Server → Client frames
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScWsFrame {
    /// Successful auth response
    AuthOk { user: ScUser },
    /// Auth failure
    AuthError { message: String },
    /// New message or thread reply
    Message { data: ScMessage },
    /// Message was edited
    MessageEdit { data: ScMessageEdit },
    /// Message was deleted
    MessageDelete { data: ScMessageDeleteEvent },
    /// Reaction toggle — full updated reactions map for a message
    Reaction { data: ScReactionEvent },
    /// Someone is typing
    Typing { data: ScTypingEvent },
    /// Presence update
    Presence { data: ScPresenceEvent },
    /// New channel created
    ChannelCreate { data: ScChannel },
    /// Channel updated
    ChannelUpdate { data: ScChannel },
    /// Server keepalive
    Ping,
    /// Response to client ping
    Pong { ts: u64 },
    /// Generic connected confirmation
    Connected { session_id: Option<String>, user: Option<ScUser> },
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScMessageEdit {
    pub id: String,
    pub text: String,
    pub edited_at: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScMessageDeleteEvent {
    pub id: String,
}

/// Reaction event — carries the full updated reactions map for the message.
/// Use ScMessage::reaction_counts(me) to get a displayable list.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScReactionEvent {
    pub message_id: String,
    /// Updated reactions map for the whole message
    pub reactions: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScTypingEvent {
    pub user: String,
    pub channel: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScPresenceEvent {
    pub user: String,
    pub status: String, // "online" | "idle" | "offline"
}

// ─── Project ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScProject {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub assignee: Option<String>,
    pub tags: Option<Vec<String>>,
}

// ─── File ────────────────────────────────────────────────────────────────────

/// Project file (legacy listing format from server.mjs)
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScFile {
    pub name: String,
    pub size: Option<u64>,
    pub created_at: Option<String>,
}

/// Channel file attachment
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScChannelFile {
    pub id: String,
    pub filename: String,
    pub size: Option<u64>,
    pub mime_type: Option<String>,
    pub url: Option<String>,
    pub uploader: Option<String>,
    pub created_at: Option<u64>,
}

// ─── Search result ───────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize, Default, PartialEq)]
pub struct ScSearchResult {
    #[serde(flatten)]
    pub message: ScMessage,
    pub score: Option<f32>,
}

// ─── Fallbacks ────────────────────────────────────────────────────────────────

pub const DEFAULT_CHANNELS: &[(&str, &str)] = &[
    ("general", "General"),
    ("agents", "Agents"),
    ("ops", "Ops"),
    ("random", "Random"),
];

pub const FALLBACK_AGENT_NAMES: &[&str] =
    &["natasha", "rocky", "bullwinkle", "sparky", "boris"];

/// Emoji palette for the reaction picker
pub const REACTION_EMOJIS: &[&str] = &["👍", "❤️", "😂", "🔥", "👀", "🎉", "🤔", "✅"];
