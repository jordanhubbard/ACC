/// User auth routes.
///
/// Admin endpoints (guarded by owner role):
///   GET    /api/auth/users              — list all users
///   POST   /api/auth/users              — create user, returns token once
///   DELETE /api/auth/users/:username    — revoke user
///   PATCH  /api/auth/users/:username    — update role (owner only)
///
/// Public endpoints:
///   POST   /api/auth/login              — validate username + token, returns role
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post},
    Json, Router,
};
use rand::Rng;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/auth/users", get(list_users).post(create_user))
        .route("/api/auth/users/:username", delete(delete_user).patch(update_user_role))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

pub fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

fn generate_token() -> String {
    let bytes: [u8; 32] = rand::thread_rng().gen();
    format!("ccc-{}", hex::encode(bytes))
}

fn is_valid_role(role: &str) -> bool {
    matches!(role, "owner" | "collaborator")
}

// ── Login ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    token: String,
}

#[derive(Serialize)]
struct LoginResponse {
    ok: bool,
    username: String,
    role: String,
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let token_hash = hash_token(&body.token);
    let db = state.auth_db.lock().await;
    let found = db
        .query_row(
            "SELECT username, role FROM users WHERE username = ?1 AND token_hash = ?2",
            params![body.username, token_hash],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .ok();

    match found {
        Some((username, role)) => {
            let now = chrono::Utc::now().to_rfc3339();
            let _ = db.execute(
                "UPDATE users SET last_seen = ?1 WHERE username = ?2",
                params![now, username],
            );
            Ok(Json(LoginResponse { ok: true, username, role }))
        }
        None => Err(StatusCode::UNAUTHORIZED),
    }
}

// ── List users ────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct UserEntry {
    id: String,
    username: String,
    role: String,
    created_at: String,
    last_seen: Option<String>,
}

async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserEntry>>, StatusCode> {
    if !state.is_owner_authed(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let db = state.auth_db.lock().await;
    let mut stmt = db
        .prepare(
            "SELECT id, username, role, created_at, last_seen FROM users ORDER BY created_at",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let users: Vec<UserEntry> = stmt
        .query_map([], |row| {
            Ok(UserEntry {
                id: row.get(0)?,
                username: row.get(1)?,
                role: row.get(2)?,
                created_at: row.get(3)?,
                last_seen: row.get(4)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(Json(users))
}

// ── Create user ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateUserRequest {
    username: String,
    /// Defaults to "collaborator" if omitted.
    #[serde(default = "default_role")]
    role: String,
}

fn default_role() -> String {
    "collaborator".to_string()
}

#[derive(Serialize)]
struct CreateUserResponse {
    username: String,
    role: String,
    /// Plaintext token — shown exactly once. Store it somewhere safe.
    token: String,
}

async fn create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<CreateUserResponse>), (StatusCode, Json<serde_json::Value>)> {
    if !state.is_owner_authed(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Owner role required"})),
        ));
    }

    if !is_valid_role(&body.role) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "role must be 'owner' or 'collaborator'"})),
        ));
    }

    let token = generate_token();
    let token_hash = hash_token(&token);
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    {
        let db = state.auth_db.lock().await;
        db.execute(
            "INSERT INTO users (id, username, token_hash, role, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, body.username, token_hash, body.role, now],
        )
        .map_err(|e| {
            let msg = e.to_string();
            let status = if msg.contains("UNIQUE") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({"error": msg})))
        })?;
    }

    // Update in-memory caches
    state
        .user_token_hashes
        .write()
        .unwrap()
        .insert(token_hash.clone());
    state
        .user_token_roles
        .write()
        .unwrap()
        .insert(token_hash, body.role.clone());

    tracing::info!("Created user: {} (role: {})", body.username, body.role);
    Ok((
        StatusCode::CREATED,
        Json(CreateUserResponse {
            username: body.username,
            role: body.role,
            token,
        }),
    ))
}

// ── Update user role ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateRoleRequest {
    role: String,
}

async fn update_user_role(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(username): Path<String>,
    Json(body): Json<UpdateRoleRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !state.is_owner_authed(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Owner role required"})),
        ));
    }

    if !is_valid_role(&body.role) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "role must be 'owner' or 'collaborator'"})),
        ));
    }

    // Fetch the token_hash so we can update the in-memory role cache.
    let token_hash: Option<String> = {
        let db = state.auth_db.lock().await;
        db.query_row(
            "SELECT token_hash FROM users WHERE username = ?1",
            params![username],
            |row| row.get(0),
        )
        .ok()
    };

    {
        let db = state.auth_db.lock().await;
        let affected = db
            .execute(
                "UPDATE users SET role = ?1 WHERE username = ?2",
                params![body.role, username],
            )
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?;
        if affected == 0 {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "User not found"})),
            ));
        }
    }

    // Keep in-memory role cache consistent.
    if let Some(hash) = token_hash {
        state
            .user_token_roles
            .write()
            .unwrap()
            .insert(hash, body.role.clone());
    }

    tracing::info!("Updated role for {}: {}", username, body.role);
    Ok(Json(serde_json::json!({"ok": true, "username": username, "role": body.role})))
}

// ── Delete user ───────────────────────────────────────────────────────────────

async fn delete_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(username): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if !state.is_owner_authed(&headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Owner role required"})),
        ));
    }

    let token_hash: Option<String> = {
        let db = state.auth_db.lock().await;
        db.query_row(
            "SELECT token_hash FROM users WHERE username = ?1",
            params![username],
            |row| row.get(0),
        )
        .ok()
    };

    {
        let db = state.auth_db.lock().await;
        let affected = db
            .execute("DELETE FROM users WHERE username = ?1", params![username])
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?;
        if affected == 0 {
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "User not found"})),
            ));
        }
    }

    if let Some(hash) = token_hash {
        state.user_token_hashes.write().unwrap().remove(&hash);
        state.user_token_roles.write().unwrap().remove(&hash);
    }

    tracing::info!("Deleted user: {}", username);
    Ok(StatusCode::NO_CONTENT)
}
