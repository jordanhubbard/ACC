use leptos::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A ClawBus message.
/// All fields are optional for backward compat; new fields use #[serde(default)].
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BusMessage {
    pub id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub ts: Option<String>,
    pub seq: Option<i64>,
    #[serde(rename = "type")]
    pub msg_type: Option<String>,
    pub body: Option<String>,
    pub subject: Option<String>,
    pub mime: Option<String>,
    /// Set on replies — the id of the parent message.
    #[serde(default)]
    pub thread_id: Option<String>,
    /// Zulip-style topic within a channel (optional label on messages).
    #[serde(default)]
    pub topic: Option<String>,
    // ── Reaction fields (only present when type == "reaction") ────────────────
    /// Target message id this reaction is for.
    #[serde(default)]
    pub target: Option<String>,
    /// The emoji character.
    #[serde(default)]
    pub emoji: Option<String>,
    /// "add" or "remove".
    #[serde(default)]
    pub action: Option<String>,
}

impl BusMessage {
    pub fn stable_id(&self) -> String {
        self.id.clone()
            .or_else(|| self.seq.map(|s| format!("msg-{s}")))
            .or_else(|| self.ts.clone())
            .unwrap_or_default()
    }

    pub fn display_from(&self) -> String {
        self.from.clone().unwrap_or_else(|| "?".to_string())
    }

    pub fn is_text(&self) -> bool {
        matches!(self.msg_type.as_deref(), Some("text") | None)
    }

    pub fn is_reaction(&self) -> bool {
        self.msg_type.as_deref() == Some("reaction")
    }

    /// Top-level = not a thread reply.
    pub fn is_top_level(&self) -> bool {
        self.thread_id.is_none()
    }
}

/// An agent presence entry.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceEntry {
    pub agent: String,
    pub online: bool,
}

/// Aggregated reaction summary for one emoji on one message.
#[derive(Clone, Debug, PartialEq)]
pub struct ReactionGroup {
    pub emoji: String,
    pub count: usize,
    pub reacted_by_me: bool,
}

/// Shared app context — provided at the root, used everywhere via use_context.
#[derive(Clone, Copy)]
pub struct ChatContext {
    pub token: leptos::ReadSignal<Option<String>>,
    pub set_token: leptos::WriteSignal<Option<String>>,
    pub username: leptos::ReadSignal<String>,

    /// ALL messages from the bus (text, reactions, system).
    pub messages: leptos::ReadSignal<Vec<BusMessage>>,
    pub set_messages: leptos::WriteSignal<Vec<BusMessage>>,

    /// Current view: "#general", "#ops", "dm:boris", etc.
    pub active_channel: leptos::RwSignal<String>,

    /// Thread side-pane: None = closed, Some(msg_id) = open for that message.
    pub open_thread: leptos::RwSignal<Option<String>>,

    /// Cmd+K command palette visibility.
    pub palette_open: leptos::RwSignal<bool>,

    /// Which message's emoji picker is open (None = none).
    pub emoji_target: leptos::RwSignal<Option<String>>,

    pub presence: leptos::ReadSignal<Vec<PresenceEntry>>,
    pub connected: leptos::ReadSignal<bool>,

    /// Per-channel unread watermarks: channel → message count seen.
    pub read_counts: leptos::RwSignal<HashMap<String, usize>>,
}

impl ChatContext {
    /// Top-level text messages for a channel (no thread replies).
    pub fn channel_messages(&self, channel: &str) -> Vec<BusMessage> {
        self.messages
            .get()
            .into_iter()
            .filter(|m| m.is_text() && m.is_top_level() && m.subject.as_deref() == Some(channel))
            .collect()
    }

    /// DM messages between `me` and `peer`.
    pub fn dm_messages(&self, me: &str, peer: &str) -> Vec<BusMessage> {
        self.messages
            .get()
            .into_iter()
            .filter(|m| {
                if !m.is_text() { return false; }
                let from = m.from.as_deref().unwrap_or("");
                let to   = m.to.as_deref().unwrap_or("");
                (from == me && to == peer) || (from == peer && to == me)
            })
            .collect()
    }

    /// Thread replies for a parent message id.
    pub fn thread_replies(&self, parent_id: &str) -> Vec<BusMessage> {
        self.messages
            .get()
            .into_iter()
            .filter(|m| m.is_text() && m.thread_id.as_deref() == Some(parent_id))
            .collect()
    }

    /// Compute reaction groups for a message (net adds - removes, grouped by emoji).
    pub fn reactions_for(&self, msg_id: &str, me: &str) -> Vec<ReactionGroup> {
        let msgs = self.messages.get();
        let mut counts: HashMap<String, (usize, bool)> = HashMap::new();

        for m in msgs.iter().filter(|m| m.is_reaction() && m.target.as_deref() == Some(msg_id)) {
            let emoji = m.emoji.clone().unwrap_or_default();
            if emoji.is_empty() { continue; }
            let from   = m.from.as_deref().unwrap_or("");
            let action = m.action.as_deref().unwrap_or("add");
            let entry  = counts.entry(emoji).or_default();
            if action == "add" {
                entry.0 += 1;
                if from == me { entry.1 = true; }
            } else if action == "remove" && entry.0 > 0 {
                entry.0 -= 1;
                if from == me { entry.1 = false; }
            }
        }

        let mut groups: Vec<ReactionGroup> = counts
            .into_iter()
            .filter(|(_, (count, _))| *count > 0)
            .map(|(emoji, (count, mine))| ReactionGroup { emoji, count, reacted_by_me: mine })
            .collect();
        groups.sort_by_key(|r| r.emoji.clone());
        groups
    }

    /// Count thread replies for a message.
    pub fn reply_count(&self, msg_id: &str) -> usize {
        self.messages
            .get()
            .iter()
            .filter(|m| m.is_text() && m.thread_id.as_deref() == Some(msg_id))
            .count()
    }

    /// All known channels discovered from message subjects (plus defaults).
    pub fn discovered_channels(&self) -> Vec<String> {
        let msgs = self.messages.get();
        let defaults: Vec<&str> = DEFAULT_CHANNELS.iter().map(|(id, _)| *id).collect();
        let mut seen = std::collections::HashSet::new();
        let mut extras = Vec::new();
        for m in &msgs {
            if let Some(subj) = &m.subject {
                if subj.starts_with('#') && !defaults.contains(&subj.as_str()) && seen.insert(subj.clone()) {
                    extras.push(subj.clone());
                }
            }
        }
        extras.sort();
        extras
    }

    /// Known DM peers discovered from messages.
    pub fn dm_peers(&self, me: &str) -> Vec<String> {
        let msgs = self.messages.get();
        let mut seen = std::collections::HashSet::new();
        for m in &msgs {
            if !m.is_text() { continue; }
            let from = m.from.as_deref().unwrap_or("");
            let to   = m.to.as_deref().unwrap_or("");
            // DM = has a specific 'to' that is not "all" and is not blank
            if to != "all" && !to.is_empty() {
                if from == me && !to.is_empty() { seen.insert(to.to_string()); }
                if to == me && !from.is_empty() { seen.insert(from.to_string()); }
            }
        }
        seen.remove(me);
        let mut v: Vec<String> = seen.into_iter().collect();
        v.sort();
        v
    }

    /// Is the current view a DM?
    pub fn is_dm_view(&self) -> bool {
        self.active_channel.get().starts_with("dm:")
    }

    /// Get the DM peer name from active_channel ("dm:boris" → "boris").
    pub fn dm_peer(&self) -> String {
        self.active_channel.get().trim_start_matches("dm:").to_string()
    }
}

/// Default channels always in the sidebar.
pub const DEFAULT_CHANNELS: &[(&str, &str)] = &[
    ("#general", "general"),
    ("#ops",     "ops"),
    ("#ai",      "ai"),
    ("#data",    "data"),
];

/// Common quick-reaction emojis.
pub const QUICK_REACTIONS: &[&str] = &[
    "👍", "👎", "❤️",  "😂", "😮",  "😢",
    "🎉", "🚀", "👀",  "🔥", "✅",  "🐛",
    "💯", "🦞", "🤔",  "💪", "🙏",  "⚡",
    "🎯", "📌", "🔑",  "💡", "🌟",  "👏",
];
