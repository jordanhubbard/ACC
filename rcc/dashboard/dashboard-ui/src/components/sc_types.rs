// sc_types.rs — SquirrelChat shared type definitions
// Single source of truth. Both squirrelchat.rs and future component modules import from here.
// Wire format matches squirrelchat/API.md exactly.

use serde::{Deserialize, Serialize};

// ─── Reaction ────────────────────────────────────────────────────────────────

/// Aggregated reaction as returned by the API.
/// Backend stores per-user reactions internally and serves this shape.
/// `[{ "emoji": "🔥", "count": 2, "by_me": true }, ...]`
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScReaction {
    pub emoji: String,
    pub count: u32,
    /// Whether the current user has applied this reaction
    #[serde(default)]
    pub by_me: bool,
}

// ─── Message ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScMessage {
    pub id: Option<String>,
    /// unix ms
    pub ts: Option<u64>,
    /// user id (server-inferred from token in Phase 2; still in body for Node compat)
    pub from: Option<String>,
    /// display name
    pub from_name: Option<String>,
    pub text: Option<String>,
    pub channel: Option<String>,
    pub mentions: Option<Vec<String>>,
    /// parent message id if this is a thread reply (null if top-level)
    pub parent_id: Option<String>,
    /// number of replies on a top-level message
    pub thread_count: Option<u32>,
    /// aggregated reactions — `[{emoji, count, by_me}]`
    #[serde(default)]
    pub reactions: Vec<ScReaction>,
    /// attached files
    #[serde(default)]
    pub files: Vec<ScChannelFile>,
    /// unix ms if edited
    pub edited_at: Option<u64>,
    pub created_at: Option<u64>,
    /// legacy slash command result field (server.mjs compat)
    pub slash_result: Option<String>,
}

impl ScMessage {
    /// Returns true if the current user has reacted with this emoji.
    pub fn by_me(&self, emoji: &str) -> bool {
        self.reactions
            .iter()
            .any(|r| r.emoji == emoji && r.by_me)
    }

    /// Returns sorted reactions by count descending (for display).
    pub fn sorted_reactions(&self) -> Vec<&ScReaction> {
        let mut sorted: Vec<&ScReaction> = self.reactions.iter().collect();
        sorted.sort_by(|a, b| b.count.cmp(&a.count).then(a.emoji.cmp(&b.emoji)));
        sorted
    }
}

// ─── Channel ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct ScChannel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// "public" | "private" | "dm"
    pub kind: Option<String>,
    /// DM participants (user ids)
    pub participants: Option<Vec<String>>,
    /// Full member list (populated by GET /api/channels/:id/members)
    #[serde(default)]
    pub members: Vec<ScUser>,
    pub created_at: Option<u64>,
    pub last_message_at: Option<u64>,
    /// Unread message count (from wire on channel list response)
    #[serde(default)]
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
    /// If true, the user has no name set yet — show the "set your name" modal
    #[serde(default)]
    pub needs_name: bool,
    /// Local-only: auth token (not returned by server, stored separately)
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
    MessageEdit {
        data: ScMessageEdit,
    },
    /// Message was deleted
    MessageDelete { data: ScMessageDeleteEvent },
    /// Reaction added or removed
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
    Connected { user: Option<ScUser> },
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

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScReactionEvent {
    pub message_id: String,
    pub emoji: String,
    pub user: String,
    pub action: String, // "added" | "removed"
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

/// Channel file (new format per API.md)
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

/// Returned by GET /api/search — just a Message with a score
#[derive(Clone, Debug, Deserialize, Default, PartialEq)]
pub struct ScSearchResult {
    #[serde(flatten)]
    pub message: ScMessage,
    pub score: Option<f32>,
}

// ─── Fallbacks (used before dynamic data loads) ───────────────────────────────

pub const DEFAULT_CHANNELS: &[(&str, &str)] = &[
    ("general", "General"),
    ("agents", "Agents"),
    ("ops", "Ops"),
    ("random", "Random"),
];

pub const FALLBACK_AGENT_NAMES: &[&str] =
    &["natasha", "rocky", "bullwinkle", "sparky", "boris"];

/// The emoji palette for the reaction picker (small curated set)
pub const REACTION_EMOJIS: &[&str] = &["👍", "❤️", "😂", "🔥", "👀", "🎉", "🤔", "✅"];
