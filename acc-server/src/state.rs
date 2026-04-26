use crate::brain::BrainQueue;
use crate::supervisor::SupervisorHandle;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
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
    /// Cached soul packages keyed by agent name.
    /// Populated when an agent responds to a soul.export bus event.
    pub soul_store: RwLock<HashMap<String, serde_json::Value>>,
    /// In-memory blob metadata store. Keyed by blob_id.
    pub blob_store: RwLock<HashMap<String, crate::bus_types::BlobMeta>>,
    /// Filesystem path where blob data is stored (one file per blob_id).
    pub blobs_path: String,
    /// Path to the dead-letter queue JSONL file.
    pub dlq_path: String,
    /// Per-token role map: SHA-256(token) → role string.
    /// Populated from the auth DB on startup and kept in sync by create_user /
    /// delete_user.  Consulted by `is_role_authed` to gate role-restricted
    /// endpoints at runtime.
    pub user_token_roles: std::sync::RwLock<std::collections::HashMap<String, String>>,
    /// Watchdog state: tracks abandoned-work detection and alerts.
    pub watchdog: crate::routes::watchdog::WatchdogState,
}

impl AppState {
    /// Extract raw bearer token from Authorization header.
    pub fn bearer_token_str<'a>(&self, headers: &'a axum::http::HeaderMap) -> &'a str {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim_start_matches("Bearer ")
            .trim()
    }

    /// Find agent name by matching token against agents registry.
    pub async fn agent_from_token(&self, token: &str) -> Option<String> {
        let agents = self.agents.read().await;
        if let Some(obj) = agents.as_object() {
            for (name, agent) in obj {
                if agent.get("token").and_then(|t| t.as_str()) == Some(token) {
                    return Some(name.clone());
                }
            }
        }
        None
    }

    fn bearer_token<'a>(&self, headers: &'a axum::http::HeaderMap) -> &'a str {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim_start_matches("Bearer ")
            .trim()
    }

    /// Returns true if the request carries a valid agent token (from config).
    /// Used to gate admin-only endpoints.
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

    /// Returns true if the request is authenticated by either an agent token or a user token.
    pub fn is_authed(&self, headers: &axum::http::HeaderMap) -> bool {
        let user_hashes = self.user_token_hashes.read().unwrap();
        if self.auth_tokens.is_empty() && user_hashes.is_empty() {
            return true; // dev mode — no tokens configured at all
        }

        // Check agent tokens (plaintext)
        if self.is_admin_authed(headers) {
            return true;
        }

        // Check user tokens (SHA-256 hash of the bearer token)
        let token = self.bearer_token(headers);
        if !token.is_empty() {
            use sha2::{Sha256, Digest};
            let mut hasher = Sha256::new();
            hasher.update(token.as_bytes());
            let hash = hex::encode(hasher.finalize());
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

    /// Compute the SHA-256 hex digest of a bearer token string.
    fn hash_bearer(token: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Returns true if the request is authenticated **and** the token's assigned
    /// role matches `required_role`.
    ///
    /// Admin (agent) tokens satisfy any role requirement — they are considered
    /// super-users.  User tokens are checked against `user_token_roles` (keyed
    /// by SHA-256 hash).  An empty `required_role` string is accepted by any
    /// authenticated token (equivalent to `is_authed`).
    ///
    /// Returns false when:
    ///   * The request is not authenticated at all, or
    ///   * The token is a valid user token but its role does not match
    ///     `required_role`.
    pub fn is_role_authed(&self, headers: &axum::http::HeaderMap, required_role: &str) -> bool {
        // Admin (config) tokens are super-users — they pass every role gate.
        if self.is_admin_authed(headers) {
            return true;
        }

        let token = self.bearer_token(headers);
        if token.is_empty() {
            return false;
        }

        let token_hash = Self::hash_bearer(token);

        // Verify the token is a known user token first (defence-in-depth).
        {
            use subtle::ConstantTimeEq;
            let user_hashes = self.user_token_hashes.read().unwrap();
            let known = user_hashes.iter().any(|h| {
                let a: &[u8] = token_hash.as_bytes();
                let b: &[u8] = h.as_bytes();
                a.len() == b.len() && bool::from(a.ct_eq(b))
            });
            if !known {
                return false;
            }
        }

        // An empty required_role means "any authenticated user".
        if required_role.is_empty() {
            return true;
        }

        // Check that this token's role matches the required role.
        let roles = self.user_token_roles.read().unwrap();
        roles
            .get(&token_hash)
            .map(|r| r.as_str() == required_role)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::auth::hash_token;
    use axum::http::{HeaderMap, HeaderValue};
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn auth_header(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", token)).unwrap(),
        );
        headers
    }

    /// Build a minimal AppState suitable for auth method unit tests.
    /// `admin_tokens`  — set of plaintext agent/admin tokens.
    /// `user_tokens`   — vec of (plaintext_token, optional_role).
    fn make_state(
        admin_tokens: Vec<&str>,
        user_tokens: Vec<(&str, Option<&str>)>,
    ) -> AppState {
        let auth_tokens: HashSet<String> = admin_tokens.iter().map(|t| t.to_string()).collect();

        let mut hashes: HashSet<String> = HashSet::new();
        let mut roles: HashMap<String, String> = HashMap::new();
        for (tok, role) in &user_tokens {
            let h = hash_token(tok);
            hashes.insert(h.clone());
            if let Some(r) = role {
                roles.insert(h, r.to_string());
            }
        }

        // We need a real (in-memory) auth DB and fleet DB to satisfy AppState's
        // fields, but they are not exercised by the auth method tests below.
        let auth_conn = crate::db::open_auth(":memory:").unwrap();
        let auth_db = Arc::new(tokio::sync::Mutex::new(auth_conn));
        let fleet_conn = crate::db::open_fleet(":memory:").unwrap();
        let fleet_db = Arc::new(tokio::sync::Mutex::new(fleet_conn));

        AppState {
            auth_tokens,
            user_token_hashes: std::sync::RwLock::new(hashes),
            user_token_roles: std::sync::RwLock::new(roles),
            auth_db,
            fleet_db,
            queue_path: String::new(),
            agents_path: String::new(),
            secrets_path: String::new(),
            bus_log_path: String::new(),
            projects_path: String::new(),
            queue: tokio::sync::RwLock::new(QueueData::default()),
            agents: tokio::sync::RwLock::new(serde_json::Value::Object(
                serde_json::Map::new(),
            )),
            secrets: tokio::sync::RwLock::new(serde_json::Map::new()),
            projects: tokio::sync::RwLock::new(Vec::new()),
            brain: Arc::new(crate::brain::BrainQueue::new()),
            bus_tx: tokio::sync::broadcast::channel(4).0,
            bus_seq: std::sync::atomic::AtomicU64::new(0),
            start_time: std::time::SystemTime::now(),
            fs_root: String::new(),
            supervisor: None,
            soul_store: tokio::sync::RwLock::new(HashMap::new()),
            blob_store: tokio::sync::RwLock::new(HashMap::new()),
            blobs_path: String::new(),
            dlq_path: String::new(),
            watchdog: crate::routes::watchdog::WatchdogState::new(),
        }
    }

    // ── is_role_authed — admin token tests ───────────────────────────────────

    /// Admin tokens satisfy any role gate (super-user rule).
    #[test]
    fn admin_token_satisfies_any_role() {
        let state = make_state(vec!["admin-tok"], vec![]);
        let headers = auth_header("admin-tok");
        assert!(state.is_role_authed(&headers, "admin"));
        assert!(state.is_role_authed(&headers, "user"));
        assert!(state.is_role_authed(&headers, ""));
        assert!(state.is_role_authed(&headers, "anything"));
    }

    /// Admin token also satisfies an empty required_role.
    #[test]
    fn admin_token_satisfies_empty_role() {
        let state = make_state(vec!["admin-tok"], vec![]);
        let headers = auth_header("admin-tok");
        assert!(state.is_role_authed(&headers, ""));
    }

    // ── is_role_authed — user token role match ───────────────────────────────

    /// User token with role "admin" is accepted when "admin" is required.
    #[test]
    fn user_token_correct_role_accepted() {
        let state = make_state(vec![], vec![("user-tok-1", Some("admin"))]);
        let headers = auth_header("user-tok-1");
        assert!(state.is_role_authed(&headers, "admin"));
    }

    /// User token with role "user" is accepted when "user" is required.
    #[test]
    fn user_token_user_role_accepted() {
        let state = make_state(vec![], vec![("user-tok-2", Some("user"))]);
        let headers = auth_header("user-tok-2");
        assert!(state.is_role_authed(&headers, "user"));
    }

    // ── is_role_authed — user token role mismatch ────────────────────────────

    /// User token with role "user" is rejected when "admin" is required.
    #[test]
    fn user_token_wrong_role_rejected() {
        let state = make_state(vec![], vec![("user-tok-3", Some("user"))]);
        let headers = auth_header("user-tok-3");
        assert!(!state.is_role_authed(&headers, "admin"));
    }

    /// User token with no role assigned is rejected for any non-empty required role.
    #[test]
    fn user_token_no_role_rejected_for_specific_role() {
        let state = make_state(vec![], vec![("user-tok-4", None)]);
        let headers = auth_header("user-tok-4");
        assert!(!state.is_role_authed(&headers, "admin"));
        assert!(!state.is_role_authed(&headers, "user"));
    }

    /// User token with no role is accepted when required_role is empty
    /// (empty means "any authenticated user").
    #[test]
    fn user_token_no_role_accepted_for_empty_required_role() {
        let state = make_state(vec![], vec![("user-tok-5", None)]);
        let headers = auth_header("user-tok-5");
        assert!(state.is_role_authed(&headers, ""));
    }

    // ── is_role_authed — unknown / missing token ─────────────────────────────

    /// An unknown token is rejected even if a role is provided.
    #[test]
    fn unknown_token_rejected() {
        let state = make_state(vec!["admin-tok"], vec![("known-user", Some("admin"))]);
        let headers = auth_header("completely-unknown-token");
        assert!(!state.is_role_authed(&headers, "admin"));
        assert!(!state.is_role_authed(&headers, ""));
    }

    /// A missing Authorization header is rejected.
    #[test]
    fn missing_auth_header_rejected() {
        let state = make_state(vec!["admin-tok"], vec![("known-user", Some("user"))]);
        let headers = HeaderMap::new(); // no Authorization header
        assert!(!state.is_role_authed(&headers, "user"));
        assert!(!state.is_role_authed(&headers, ""));
    }

    // ── is_role_authed — runtime reload of user_token_roles ─────────────────

    /// After a new role mapping is inserted into user_token_roles at runtime
    /// (simulating a server-restart reload or a live create_user call), the
    /// updated role is immediately visible to is_role_authed.
    #[test]
    fn runtime_role_update_is_visible() {
        // Start with a user token that has no role.
        let state = make_state(vec![], vec![("reload-tok", None)]);
        let headers = auth_header("reload-tok");

        // Before the reload, the token has no role → rejected for "admin".
        assert!(!state.is_role_authed(&headers, "admin"));

        // Simulate a runtime reload: insert the hash→role mapping.
        let token_hash = hash_token("reload-tok");
        state
            .user_token_roles
            .write()
            .unwrap()
            .insert(token_hash, "admin".to_string());

        // After the in-memory update, the same token now passes the "admin" gate.
        assert!(state.is_role_authed(&headers, "admin"));
    }

    /// Removing a role mapping from user_token_roles at runtime (e.g. delete_user)
    /// immediately revokes role access for that token.
    #[test]
    fn runtime_role_removal_revokes_access() {
        let state = make_state(vec![], vec![("revoke-tok", Some("admin"))]);
        let headers = auth_header("revoke-tok");

        // Token starts with "admin" role.
        assert!(state.is_role_authed(&headers, "admin"));

        // Simulate removal (e.g. delete_user or role downgrade).
        let token_hash = hash_token("revoke-tok");
        state.user_token_roles.write().unwrap().remove(&token_hash);

        // Role access is revoked immediately.
        assert!(!state.is_role_authed(&headers, "admin"));
    }

    // ── is_authed — existing behaviour is unchanged ──────────────────────────

    /// is_authed still works for both admin and user tokens.
    #[test]
    fn is_authed_still_works() {
        let state = make_state(vec!["admin-tok"], vec![("user-tok", Some("user"))]);

        assert!(state.is_authed(&auth_header("admin-tok")));
        assert!(state.is_authed(&auth_header("user-tok")));
        assert!(!state.is_authed(&auth_header("bad-tok")));
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
