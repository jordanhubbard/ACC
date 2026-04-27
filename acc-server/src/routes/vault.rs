use crate::AppState;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, sync::Arc};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/vault/status", get(vault_status))
        .route("/api/vault/unlock", post(vault_unlock))
        .route("/api/vault/lock",   post(vault_lock))
        .route("/api/vault/rotate", post(vault_rotate))
        .route("/api/vault/import", post(vault_import))
        .route("/api/vault/export", get(vault_export))
}

async fn vault_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let enabled = state.vault.is_enabled().await;
    let locked  = state.vault.is_locked().await;
    let count   = state.vault.count().await;
    Json(json!({
        "ok":      true,
        "enabled": enabled,
        "locked":  locked,
        "count":   count,
    })).into_response()
}

#[derive(Deserialize)]
struct UnlockBody {
    password: String,
}

async fn vault_unlock(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UnlockBody>,
) -> impl IntoResponse {
    if !state.is_admin_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match state.vault.unlock(body.password.as_bytes()).await {
        Ok(_) => Json(json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn vault_lock(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.is_admin_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    state.vault.lock().await;
    Json(json!({"ok": true})).into_response()
}

#[derive(Deserialize)]
struct RotateBody {
    old_password: String,
    new_password: String,
}

async fn vault_rotate(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RotateBody>,
) -> impl IntoResponse {
    if !state.is_admin_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match state.vault.rotate_password(body.old_password.as_bytes(), body.new_password.as_bytes()).await {
        Ok(_) => {
            flush_vault_to_db(&state).await;
            Json(json!({"ok": true})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

#[derive(Deserialize)]
struct ImportBody {
    secrets: HashMap<String, String>,
}

async fn vault_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ImportBody>,
) -> impl IntoResponse {
    if !state.is_admin_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    match state.vault.import(body.secrets).await {
        Ok(_) => {
            flush_vault_to_db(&state).await;
            let count = state.vault.count().await;
            Json(json!({"ok": true, "count": count})).into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({"error": e.to_string()}))).into_response(),
    }
}

async fn vault_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.is_admin_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }
    let (salt, blobs) = state.vault.export().await;
    let salt_b64 = salt.map(|s| B64.encode(s));
    Json(json!({
        "ok":      true,
        "salt":    salt_b64,
        "secrets": blobs,
    })).into_response()
}

/// Persist current vault state (salt + encrypted blobs) to the fleet DB.
pub async fn flush_vault_to_db(state: &Arc<AppState>) {
    let (salt, blobs) = state.vault.export().await;
    let conn = state.fleet_db.lock().await;
    if let Some(s) = salt {
        crate::db::db_save_vault_salt(&conn, &s);
    }
    crate::db::db_flush_vault_blobs(&conn, &blobs);
}
