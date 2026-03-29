use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Heartbeat / Agent ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeartbeatData {
    pub agent: Option<String>,
    pub ts: Option<String>,
    pub host: Option<String>,
    pub model: Option<String>,
    pub online: Option<bool>,
    pub decommissioned: Option<bool>,
    pub status: Option<String>,
}

pub type HeartbeatMap = HashMap<String, HeartbeatData>;

// ── Work Queue ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct QueueItem {
    pub id: String,
    pub title: String,
    pub priority: Option<String>,
    pub assignee: Option<String>,
    pub status: Option<String>,
    pub created_at: Option<String>,
    pub body: Option<String>,
    pub tags: Option<Vec<String>>,
    pub claimed_by: Option<String>,
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueueResponse {
    pub items: Vec<QueueItem>,
    pub completed: Option<Vec<QueueItem>>,
}

// ── SquirrelBus ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BusMessage {
    pub id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub text: Option<String>,
    pub ts: Option<String>,
    #[serde(rename = "type")]
    pub msg_type: Option<String>,
}

// ── Git Commits ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitCommit {
    pub sha: Option<String>,
    pub commit: Option<GitCommitDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitCommitDetail {
    pub message: Option<String>,
    pub author: Option<GitCommitAuthor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitCommitAuthor {
    pub name: Option<String>,
    pub date: Option<String>,
}

// ── GitHub Issues ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GhIssue {
    pub id: i64,
    pub repo: String,
    pub title: String,
    pub state: String,
    pub labels: Option<String>,
    pub url: Option<String>,
    pub author: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub wq_id: Option<String>,
    pub milestone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GhIssuesResponse {
    pub ok: Option<bool>,
    pub issues: Vec<GhIssue>,
    pub count: Option<u32>,
    pub last_sync: Option<SyncLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncLog {
    pub repo: Option<String>,
    pub synced_at: Option<String>,
    pub count: Option<u32>,
    pub status: Option<String>,
}

// ── Projects ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Project {
    pub id: String,
    pub display_name: Option<String>,
    pub enabled: Option<bool>,
}

// ── Metrics ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsData {
    pub queue_depth: Option<u32>,
    pub completion_rate: Option<f64>,
    pub active_agents: Option<u32>,
    pub pending: Option<u32>,
    pub in_progress: Option<u32>,
    pub completed: Option<u32>,
}
