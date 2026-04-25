use crate::brain::BrainQueue;
use crate::supervisor::SupervisorHandle;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct QueueData {
    #[serde(default)]
    pub items: Vec<serde_json::Value>,
    #[serde(default)]
    pub completed: Vec<serde_json::Value>,
}

pub struct AppState {
    /// Static agent tokens from config (plaintext, never change at runtime).
    pub auth_tokens: HashSet<String>,
    /// In-memory cache of user token SHA-256 hashes (loaded from auth.db, updated on add/delete).
    pub user_token_hashes: std::sync::RwLock<HashSet<String>>,
    /// In-memory map of token_hash → role ("owner" | "collaborator").
    /// Kept in sync with auth.db by the user management endpoints.
    pub user_token_roles: std::sync::RwLock<HashMap<String, String>>,
    /// Auth SQLite database (always-on).
    pub auth_db: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
    /// Fleet task pool SQLite database (always-on).
    pub fleet_db: Arc<tokio::sync::Mutex<Connection>>,
    pub queue_path: String,
    pub agents_path: String,
    pub secrets_path: String,
    pub bus_log_path: String,
    pub projects_path: String,
    pub queue: RwLock<QueueData>,
    pub agents: RwLock<serde_json::Value>,
    pub secrets: RwLock<serde_json::Map<String, serde_json::Value>>,
    pub projects: RwLock<Vec<serde_json::Value>>,
    pub brain: Arc<BrainQueue>,
    pub bus_tx: broadcast::Sender<String>,
    pub bus_seq: AtomicU64,
    pub start_time: std::time::SystemTime,
    pub fs_root: String,
    pub supervisor: Option<Arc<SupervisorHandle>>,
    /// Maximum number of bytes accepted for a single blob upload.
    pub max_blob_bytes: u64,
}

impl AppState {
    fn bearer_token<'a>(&self, headers: &'a axum::http::HeaderMap) -> &'a str {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim_start_matches("Bearer ")
            .trim()
    }

    /// Returns true if the request carries a valid agent token (from config).
    /// Agent tokens are treated as owner-level — they can perform any operation.
    pub fn is_admin_authed(&self, headers: &axum::http::HeaderMap) -> bool {
        if self.auth_tokens.is_empty() {
            return true;
        }
        let token = self.bearer_token(headers);
        use subtle::ConstantTimeEq;
        for valid in &self.auth_tokens {
            let a: &[u8] = token.as_bytes();
            let b: &[u8] = valid.as_bytes();
            if a.len() == b.len() && bool::from(a.ct_eq(b)) {
                return true;
            }
        }
        false
    }

    /// Returns true if the request is authenticated by either an agent token or a user token
    /// (any role).
    pub fn is_authed(&self, headers: &axum::http::HeaderMap) -> bool {
        let user_hashes = self.user_token_hashes.read().unwrap();
        if self.auth_tokens.is_empty() && user_hashes.is_empty() {
            return true; // dev mode — no tokens configured at all
        }

        // Agent tokens implicitly authenticate (owner-level).
        if self.is_admin_authed(headers) {
            return true;
        }

        // Check user tokens (SHA-256 hash of the bearer token)
        let token = self.bearer_token(headers);
        if !token.is_empty() {
            let hash = Self::hash_bearer(token);
            use subtle::ConstantTimeEq;
            for valid_hash in user_hashes.iter() {
                let a: &[u8] = hash.as_bytes();
                let b: &[u8] = valid_hash.as_bytes();
                if a.len() == b.len() && bool::from(a.ct_eq(b)) {
                    return true;
                }
            }
        }

        false
    }

    /// Returns the role string (`"owner"` or `"collaborator"`) associated with
    /// the request's bearer token, or `None` when:
    ///
    /// * the request carries no bearer token, or
    /// * the token is not found in either the agent-token set or the user-role
    ///   map (i.e. the caller is unauthenticated).
    ///
    /// Agent tokens from config are always mapped to `"owner"`.
    /// In dev mode (no tokens configured at all) every caller is implicitly
    /// considered `"owner"`.
    pub fn token_role<'h>(&self, headers: &'h axum::http::HeaderMap) -> Option<&'static str> {
        // Dev mode — no tokens configured; treat everyone as owner.
        let user_hashes = self.user_token_hashes.read().unwrap();
        if self.auth_tokens.is_empty() && user_hashes.is_empty() {
            return Some("owner");
        }
        drop(user_hashes);

        // Agent tokens (from config) are always owner-level.
        if self.is_admin_authed(headers) {
            return Some("owner");
        }

        // Look up user-token role from the in-memory map.
        let token = self.bearer_token(headers);
        if token.is_empty() {
            return None;
        }
        let hash = Self::hash_bearer(token);
        let roles = self.user_token_roles.read().unwrap();
        use subtle::ConstantTimeEq;
        for (stored_hash, role) in roles.iter() {
            let a: &[u8] = hash.as_bytes();
            let b: &[u8] = stored_hash.as_bytes();
            if a.len() == b.len() && bool::from(a.ct_eq(b)) {
                // Map the stored (heap-allocated) role string to a 'static str
                // so callers don't need to hold the lock guard.
                return Some(if role == "owner" { "owner" } else { "collaborator" });
            }
        }
        None
    }

    /// Returns true if the request is authenticated AND the caller holds the
    /// `owner` role.
    ///
    /// Gate destructive / privileged operations on this:
    ///   - agent registration / deletion
    ///   - exec dispatch
    ///   - secrets write / delete
    ///
    /// Agent tokens (from config) are always treated as owner-level.
    /// In dev mode (no tokens at all) every caller is owner.
    pub fn is_owner_authed(&self, headers: &axum::http::HeaderMap) -> bool {
        // Dev mode or agent token → owner.
        if self.is_admin_authed(headers) {
            return true;
        }

        // User token: look up role.
        let token = self.bearer_token(headers);
        if token.is_empty() {
            return false;
        }
        let hash = Self::hash_bearer(token);
        let roles = self.user_token_roles.read().unwrap();
        use subtle::ConstantTimeEq;
        for (stored_hash, role) in roles.iter() {
            let a: &[u8] = hash.as_bytes();
            let b: &[u8] = stored_hash.as_bytes();
            if a.len() == b.len() && bool::from(a.ct_eq(b)) {
                return role == "owner";
            }
        }
        false
    }

    /// Returns true if the request is authenticated AND the caller holds at
    /// least the `collaborator` role (i.e. any authenticated user token, or
    /// an agent/owner token).
    ///
    /// Use this to gate endpoints that require a real user identity but do not
    /// need owner-level privileges — for example, commenting on a task or
    /// submitting a review.
    ///
    /// Agent tokens (from config) always satisfy this check.
    /// In dev mode (no tokens configured at all) every caller satisfies it.
    pub fn is_collaborator_authed(&self, headers: &axum::http::HeaderMap) -> bool {
        // is_authed already accepts any valid token (agent or user, any role).
        self.is_authed(headers)
    }

    /// SHA-256 of a raw bearer token string, returned as a lowercase hex string.
    pub fn hash_bearer(token: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }
}

pub async fn load_all(state: &Arc<AppState>) {
    load_queue(state).await;
    load_agents(state).await;
    load_secrets(state).await;
    load_projects(state).await;
}

pub async fn load_projects(state: &Arc<AppState>) {
    match tokio::fs::read_to_string(&state.projects_path).await {
        Ok(content) => {
            if let Ok(data) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                *state.projects.write().await = data;
                tracing::info!("Loaded projects from {}", state.projects_path);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("Projects file not found, starting empty");
        }
        Err(e) => tracing::warn!("Failed to load projects: {}", e),
    }
}

pub async fn load_queue(state: &Arc<AppState>) {
    match tokio::fs::read_to_string(&state.queue_path).await {
        Ok(content) => {
            if let Ok(data) = serde_json::from_str::<QueueData>(&content) {
                *state.queue.write().await = data;
                tracing::info!("Loaded queue from {}", state.queue_path);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("Queue file not found, starting empty");
        }
        Err(e) => {
            tracing::warn!("Failed to load queue: {}", e);
        }
    }
}

pub async fn flush_queue(state: &Arc<AppState>) {
    let data = state.queue.read().await;
    let content = match serde_json::to_string_pretty(&*data) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to serialize queue: {}", e);
            return;
        }
    };
    drop(data);
    if let Err(e) = write_atomic(&state.queue_path, &content).await {
        tracing::warn!("Failed to flush queue: {}", e);
    }
}

pub async fn load_agents(state: &Arc<AppState>) {
    match tokio::fs::read_to_string(&state.agents_path).await {
        Ok(content) => {
            if let Ok(data) = serde_json::from_str::<serde_json::Value>(&content) {
                let obj = if data.is_array() {
                    // legacy array format -> convert to map
                    let mut map = serde_json::Map::new();
                    for agent in data.as_array().unwrap() {
                        if let Some(name) = agent.get("name").and_then(|n| n.as_str()) {
                            map.insert(name.to_string(), agent.clone());
                        }
                    }
                    serde_json::Value::Object(map)
                } else {
                    data
                };
                *state.agents.write().await = obj;
                tracing::info!("Loaded agents from {}", state.agents_path);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("Agents file not found, starting empty");
        }
        Err(e) => tracing::warn!("Failed to load agents: {}", e),
    }
}

pub async fn flush_agents(state: &Arc<AppState>) {
    let data = state.agents.read().await;
    let content = match serde_json::to_string_pretty(&*data) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to serialize agents: {}", e);
            return;
        }
    };
    drop(data);
    if let Some(parent) = std::path::Path::new(&state.agents_path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Err(e) = write_atomic(&state.agents_path, &content).await {
        tracing::warn!("Failed to flush agents: {}", e);
    }
}

pub async fn load_secrets(state: &Arc<AppState>) {
    match tokio::fs::read_to_string(&state.secrets_path).await {
        Ok(content) => {
            if let Ok(data) =
                serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&content)
            {
                *state.secrets.write().await = data;
                tracing::info!("Loaded secrets from {}", state.secrets_path);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("Failed to load secrets: {}", e),
    }
}

pub async fn flush_secrets(state: &Arc<AppState>) {
    let data = state.secrets.read().await;
    let content = match serde_json::to_string_pretty(&*data) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to serialize secrets: {}", e);
            return;
        }
    };
    drop(data);
    if let Some(parent) = std::path::Path::new(&state.secrets_path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Err(e) = write_atomic(&state.secrets_path, &content).await {
        tracing::warn!("Failed to flush secrets: {}", e);
    }
    // chmod 600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ =
            std::fs::set_permissions(&state.secrets_path, std::fs::Permissions::from_mode(0o600));
    }
}

async fn write_atomic(path: &str, content: &str) -> std::io::Result<()> {
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = format!("{}.tmp", path);
    tokio::fs::write(&tmp, content).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}
