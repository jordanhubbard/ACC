/// /api/agentos — AgentOS routes (stub implementation)
/// Full implementation is SOA-010 (complex, later phase).
/// This stub provides the endpoints the dashboard WASM uses today.

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Json},
    routing::{get, post, put},
    Router,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/agentos/tasks", get(get_agentos_tasks).post(post_agentos_task))
        .route("/api/agentos/tasks/:id", put(put_agentos_task))
        .route("/api/agentos/status", get(get_agentos_status))
        .route("/api/agentos/deploy", post(post_agentos_deploy))
        .route("/api/agentos/timeline", get(get_timeline))
        .route("/api/agentos/events", get(get_events))
        .route("/api/agentos/cap-events", get(get_cap_events).post(post_cap_event))
        .route("/api/agentos/cap-events/push", post(push_cap_event))
        .route("/api/agentos/slots", get(get_slots))
        .route("/api/agentos/shell", get(shell_stub))
        .route("/api/agentos/debug/sessions", get(debug_sessions))
        .route("/api/upvote/:id", post(upvote))
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn is_agentos_item(item: &serde_json::Value) -> bool {
    let id_match = item.get("id").and_then(|v| v.as_str())
        .map(|id| id.starts_with("agentos-"))
        .unwrap_or(false);
    let tag_match = item.get("tags").and_then(|t| t.as_array())
        .map(|arr| arr.iter().any(|t| t.as_str() == Some("agentos")))
        .unwrap_or(false);
    id_match || tag_match
}

// ── GET /api/agentos/tasks ────────────────────────────────────────────────

async fn get_agentos_tasks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let q = state.queue.read().await;
    let items: Vec<&serde_json::Value> = q.items.iter().filter(|i| is_agentos_item(i)).collect();
    let completed: Vec<&serde_json::Value> = q.completed.iter().filter(|i| is_agentos_item(i)).collect();
    Json(json!({"ok": true, "items": items, "completed": completed})).into_response()
}

// ── POST /api/agentos/tasks ───────────────────────────────────────────────

async fn post_agentos_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let title = match body.get("title").and_then(|t| t.as_str()) {
        Some(t) => t.to_string(),
        None => return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": "title required"}))).into_response(),
    };

    let now = chrono::Utc::now().to_rfc3339();
    let id = format!("agentos-{}", uuid::Uuid::new_v4());

    let mut tags: Vec<Value> = body.get("tags").and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    if !tags.iter().any(|t| t.as_str() == Some("agentos")) {
        tags.push(json!("agentos"));
    }

    let item = json!({
        "id": id,
        "itemVersion": 1,
        "created": now,
        "source": "agentos-api",
        "assignee": body.get("assignee").and_then(|s| s.as_str()).unwrap_or("all"),
        "priority": body.get("priority").and_then(|s| s.as_str()).unwrap_or("normal"),
        "status": "pending",
        "title": title,
        "description": body.get("description").and_then(|s| s.as_str()).unwrap_or(""),
        "notes": body.get("notes").and_then(|s| s.as_str()).unwrap_or(""),
        "preferred_executor": body.get("preferred_executor").and_then(|s| s.as_str()).unwrap_or("inference_key"),
        "journal": [],
        "choices": body.get("choices").cloned().unwrap_or(json!([])),
        "choiceRecorded": null,
        "votes": [],
        "attempts": 0,
        "maxAttempts": body.get("maxAttempts").and_then(|n| n.as_u64()).unwrap_or(3),
        "claimedBy": null,
        "claimedAt": null,
        "completedAt": null,
        "result": null,
        "tags": tags,
    });

    let mut q = state.queue.write().await;
    q.items.push(item.clone());
    drop(q);
    crate::state::flush_queue(&state).await;

    (axum::http::StatusCode::CREATED, Json(json!({"ok": true, "item": item}))).into_response()
}

// ── PUT /api/agentos/tasks/:id ────────────────────────────────────────────

async fn put_agentos_task(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    if !id.starts_with("agentos-") {
        return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": "Not an agentos task"}))).into_response();
    }

    let mut q = state.queue.write().await;
    let pos = q.items.iter().position(|i| i.get("id").and_then(|v| v.as_str()) == Some(&id));
    match pos {
        None => (axum::http::StatusCode::NOT_FOUND, Json(json!({"error": "Task not found"}))).into_response(),
        Some(pos) => {
            let item = &mut q.items[pos];
            let obj = item.as_object_mut().unwrap();
            let allowed = ["status", "notes", "result", "assignee", "priority", "title", "description"];
            if let Some(body_obj) = body.as_object() {
                for key in &allowed {
                    if let Some(val) = body_obj.get(*key) {
                        obj.insert(key.to_string(), val.clone());
                    }
                }
            }
            let version = obj.get("itemVersion").and_then(|n| n.as_u64()).unwrap_or(0) + 1;
            obj.insert("itemVersion".into(), json!(version));
            let updated = item.clone();
            drop(q);
            crate::state::flush_queue(&state).await;
            (axum::http::StatusCode::OK, Json(json!({"ok": true, "item": updated}))).into_response()
        }
    }
}

// ── GET /api/agentos/status ───────────────────────────────────────────────

async fn get_agentos_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let agents_data = state.agents.read().await;
    let agents: Vec<Value> = if let Some(map) = agents_data.as_object() {
        map.values().cloned().collect()
    } else {
        vec![]
    };
    drop(agents_data);

    let q = state.queue.read().await;
    let tasks_pending = q.items.iter().filter(|i| is_agentos_item(i)).count();
    let tasks_completed = q.completed.iter().filter(|i| is_agentos_item(i)).count();

    Json(json!({
        "ok": true,
        "agents": agents,
        "tasks_pending": tasks_pending,
        "tasks_completed": tasks_completed,
    })).into_response()
}

// ── POST /api/agentos/deploy ──────────────────────────────────────────────

async fn post_agentos_deploy(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let agent = match body.get("agent").and_then(|v| v.as_str()) {
        Some(a) => a.to_string(),
        None => return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error": "agent required"}))).into_response(),
    };
    let module_url = body.get("module_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let capability = body.get("capability").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let now = chrono::Utc::now().to_rfc3339();
    let id = format!("agentos-deploy-{}", uuid::Uuid::new_v4());

    let item = json!({
        "id": id,
        "itemVersion": 1,
        "created": now,
        "source": "agentos-deploy",
        "assignee": agent,
        "priority": "high",
        "status": "pending",
        "title": format!("Deploy {} ({})", agent, capability),
        "description": format!("Deploy agent: {}, module: {}, capability: {}", agent, module_url, capability),
        "notes": "",
        "preferred_executor": "claude_cli",
        "journal": [],
        "choices": [],
        "choiceRecorded": null,
        "votes": [],
        "attempts": 0,
        "maxAttempts": 3,
        "claimedBy": null,
        "claimedAt": null,
        "completedAt": null,
        "result": null,
        "tags": ["agentos", "deploy"],
        "deploy": {
            "agent": agent,
            "module_url": module_url,
            "capability": capability,
        }
    });

    let mut q = state.queue.write().await;
    q.items.push(item.clone());
    drop(q);
    crate::state::flush_queue(&state).await;

    (axum::http::StatusCode::CREATED, Json(json!({"ok": true, "id": id, "item": item}))).into_response()
}

// ── GET /api/agentos/timeline ─────────────────────────────────────────────

async fn get_timeline(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let limit: usize = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100);

    // Read from bus log as event source
    let bus_path = std::env::var("BUS_LOG_PATH")
        .unwrap_or_else(|_| "./data/bus.jsonl".to_string());

    let events: Vec<Value> = match tokio::fs::read_to_string(&bus_path).await {
        Ok(content) => content.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
        Err(_) => vec![],
    };

    Json(json!({"ok": true, "events": events, "count": events.len()}))
}

// ── GET /api/agentos/events ───────────────────────────────────────────────

async fn get_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let limit: usize = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50);

    // Cap events stored in data/cap-events.jsonl
    let path = std::env::var("CAP_EVENTS_PATH")
        .unwrap_or_else(|_| "./data/cap-events.jsonl".to_string());

    let events: Vec<Value> = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
        Err(_) => vec![],
    };

    Json(json!({"ok": true, "events": events}))
}

// ── GET/POST /api/agentos/cap-events ─────────────────────────────────────

async fn get_cap_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let limit: usize = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100);
    let path = std::env::var("CAP_EVENTS_PATH")
        .unwrap_or_else(|_| "./data/cap-events.jsonl".to_string());

    let events: Vec<Value> = match tokio::fs::read_to_string(&path).await {
        Ok(content) => content.lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect(),
        Err(_) => vec![],
    };

    Json(json!({"ok": true, "events": events, "count": events.len()}))
}

async fn post_cap_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    push_event_to_log(body).await;
    Json(json!({"ok": true}))
}

async fn push_cap_event(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    push_event_to_log(body).await;
    Json(json!({"ok": true}))
}

async fn push_event_to_log(mut body: Value) {
    if body.get("ts").is_none() {
        body.as_object_mut().unwrap().insert("ts".to_string(), json!(chrono::Utc::now().to_rfc3339()));
    }
    let path = std::env::var("CAP_EVENTS_PATH")
        .unwrap_or_else(|_| "./data/cap-events.jsonl".to_string());
    if let Some(parent) = std::path::Path::new(&path).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    if let Ok(mut line) = serde_json::to_string(&body) {
        line.push('\n');
        use tokio::io::AsyncWriteExt;
        if let Ok(mut f) = tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await {
            let _ = f.write_all(line.as_bytes()).await;
        }
    }
}

// ── GET /api/agentos/slots ────────────────────────────────────────────────

async fn get_slots() -> impl IntoResponse {
    Json(json!({"ok": true, "slots": [], "note": "slot management is SOA-010"}))
}

// ── GET /api/agentos/shell ────────────────────────────────────────────────

async fn shell_stub() -> impl IntoResponse {
    (axum::http::StatusCode::NOT_IMPLEMENTED, Json(json!({"error": "shell endpoint not yet implemented in Rust — SOA-010"})))
}

// ── GET /api/agentos/debug/sessions ──────────────────────────────────────

async fn debug_sessions() -> impl IntoResponse {
    Json(json!({"ok": true, "sessions": [], "note": "debug sessions are SOA-010"}))
}

// ── POST /api/upvote/:id ──────────────────────────────────────────────────

async fn upvote(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let voter = body.get("agent").or_else(|| body.get("voter"))
        .and_then(|v| v.as_str())
        .unwrap_or("anonymous")
        .to_string();

    let mut queue = state.queue.write().await;
    let item = queue.items.iter_mut().find(|i| i.get("id").and_then(|v| v.as_str()) == Some(&id));
    match item {
        None => {
            drop(queue);
            (axum::http::StatusCode::NOT_FOUND, Json(json!({"error":"Item not found"}))).into_response()
        }
        Some(item) => {
            let votes = item.as_object_mut().unwrap()
                .entry("votes")
                .or_insert(json!([]))
                .as_array_mut()
                .unwrap();
            if !votes.iter().any(|v| v.as_str() == Some(&voter)) {
                votes.push(json!(voter));
            }
            let updated = item.clone();
            drop(queue);
            crate::state::flush_queue(&state).await;
            (axum::http::StatusCode::OK, Json(json!({"ok": true, "item": updated}))).into_response()
        }
    }
}
