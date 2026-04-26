//! Lightweight mock ACC hub for agent integration tests.
//!
//! Binds to 127.0.0.1:0 (OS-assigned random port) so parallel tests never conflict.
//! Use HubMock::new() for defaults or HubMock::with_state(HubState{...}).await for custom responses.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

// ── Project helpers ───────────────────────────────────────────────────────────

/// A minimal project record used by the mock hub.
///
/// All fields mirror the server's JSON shape so that `acc-client`'s
/// `ProjectsApi::get` can deserialise them without issue.
#[derive(Debug, Clone)]
pub struct MockProject {
    pub id: String,
    pub name: String,
    pub slug: Option<String>,
    pub agentfs_path: Option<String>,
}

impl MockProject {
    pub fn to_json(&self) -> Value {
        let mut obj = json!({
            "id": self.id,
            "name": self.name,
        });
        if let Some(ref s) = self.slug {
            obj["slug"] = Value::String(s.clone());
        }
        if let Some(ref p) = self.agentfs_path {
            obj["agentfs_path"] = Value::String(p.clone());
        }
        obj
    }
}

// ── State ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct HubState {
    /// Items returned by GET /api/queue
    pub queue_items: Vec<Value>,
    /// Tasks returned by GET /api/tasks (filtered by ?status=)
    pub tasks: Vec<Value>,
    /// HTTP status code for POST /api/item/:id/claim  (default 200)
    pub item_claim_status: u16,
    /// HTTP status code for PUT  /api/tasks/:id/claim (default 200)
    pub task_claim_status: u16,
    /// HTTP status code for POST /api/requests/:id/claim (default 200)
    pub request_claim_status: u16,
    /// JSON payloads to stream from GET /bus/stream as SSE data events.
    pub sse_events: Vec<String>,
    /// Agent names returned by GET /api/agents/names
    pub agent_names: Vec<String>,
    /// Accumulates bodies from POST /api/tasks — inspectable by tests
    pub created_tasks: Arc<Mutex<Vec<Value>>>,
    /// HTTP status code for POST /api/tasks (default 201)
    pub task_create_status: u16,
    /// HTTP status code for PUT /api/tasks/:id/review-result (default 200)
    pub review_result_status: u16,
    /// Projects returned by GET /api/projects/:id, keyed by project id.
    pub projects: HashMap<String, MockProject>,
}

impl Default for HubState {
    fn default() -> Self {
        Self {
            queue_items: vec![],
            tasks: vec![],
            item_claim_status: 200,
            task_claim_status: 200,
            request_claim_status: 200,
            sse_events: vec![],
            agent_names: vec![],
            created_tasks: Arc::new(Mutex::new(vec![])),
            task_create_status: 201,
            review_result_status: 200,
            projects: HashMap::new(),
        }
    }
}

// ── Mock server ───────────────────────────────────────────────────────────────

pub struct HubMock {
    pub url: String,
    pub state: Arc<RwLock<HubState>>,
    _handle: tokio::task::JoinHandle<()>,
}

impl HubMock {
    pub async fn new() -> Self {
        Self::with_state(HubState::default()).await
    }

    pub async fn with_queue(items: Vec<Value>) -> Self {
        Self::with_state(HubState { queue_items: items, ..Default::default() }).await
    }

    pub async fn with_tasks(tasks: Vec<Value>) -> Self {
        Self::with_state(HubState { tasks, ..Default::default() }).await
    }

    pub async fn with_sse(events: Vec<String>) -> Self {
        Self::with_state(HubState { sse_events: events, ..Default::default() }).await
    }

    pub async fn with_state(initial: HubState) -> Self {
        let state = Arc::new(RwLock::new(initial));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind random port for hub mock");
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        let app = build_router(state.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        HubMock { url, state, _handle: handle }
    }
}

impl Drop for HubMock {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

type S = Arc<RwLock<HubState>>;

fn build_router(state: S) -> Router {
    Router::new()
        // Heartbeat — queue worker uses /api/heartbeat/:name
        .route("/api/heartbeat/:name",          post(ok))
        .route("/api/agents/:name/heartbeat",   post(ok))
        // Queue worker item routes
        .route("/api/queue",                    get(queue_items))
        .route("/api/item/:id/claim",           post(item_claim))
        .route("/api/item/:id/complete",        post(ok))
        .route("/api/item/:id/fail",            post(ok))
        .route("/api/item/:id/keepalive",       post(ok))
        .route("/api/item/:id/comment",         post(ok))
        // Fleet task routes
        .route("/api/tasks",                    get(task_list).post(task_create))
        .route("/api/tasks/:id/claim",          put(task_claim))
        .route("/api/tasks/:id/complete",       put(ok))
        .route("/api/tasks/:id/unclaim",        put(ok))
        .route("/api/tasks/:id/review-result",  put(task_review_result))
        // User request routes (first-responder)
        .route("/api/requests/:id/claim",       post(request_claim))
        // Exec result (bus worker)
        .route("/api/exec/:id/result",          post(ok))
        // SSE stream (bus listener)
        .route("/bus/stream",                   get(sse_stream))
        .route("/api/bus/stream",               get(sse_stream))
        // Peer discovery
        .route("/api/agents/names",             get(agent_names))
        // Project lookup — used by resolve_workspace
        .route("/api/projects/:id",             get(project_get))
        .with_state(state)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

async fn ok(_: State<S>) -> Json<Value> {
    Json(json!({"ok": true}))
}

async fn queue_items(State(st): State<S>) -> Json<Value> {
    let s = st.read().await;
    Json(json!({"items": s.queue_items}))
}

async fn item_claim(State(st): State<S>, Path(id): Path<String>) -> impl IntoResponse {
    let code = st.read().await.item_claim_status;
    let sc = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    (sc, Json(json!({"ok": code == 200, "item": {"id": id}}))).into_response()
}

async fn task_list(
    State(st): State<S>,
    Query(params): Query<HashMap<String, String>>,
) -> Json<Value> {
    let s = st.read().await;
    let status_filter = params.get("status").cloned().unwrap_or_default();
    let type_filter = params.get("task_type").cloned().unwrap_or_default();
    let matched: Vec<&Value> = s.tasks.iter().filter(|t| {
        let status_ok = status_filter.is_empty() || t["status"].as_str() == Some(&status_filter);
        let type_ok = type_filter.is_empty()
            || t["task_type"].as_str().unwrap_or("work") == type_filter;
        status_ok && type_ok
    }).collect();
    let count = matched.len() as u64;
    Json(json!({"tasks": matched, "count": count}))
}

async fn task_create(State(st): State<S>, Json(body): Json<Value>) -> impl IntoResponse {
    let (code, created_tasks) = {
        let s = st.read().await;
        (s.task_create_status, s.created_tasks.clone())
    };
    let sc = StatusCode::from_u16(code).unwrap_or(StatusCode::CREATED);
    let id = format!("mock-task-{}", chrono::Utc::now().timestamp_millis());
    let mut task = body.clone();
    task["id"] = serde_json::Value::String(id.clone());
    created_tasks.lock().await.push(task.clone());
    (sc, Json(json!({"ok": true, "task": task}))).into_response()
}

async fn task_review_result(State(st): State<S>, Path(_id): Path<String>) -> impl IntoResponse {
    let code = st.read().await.review_result_status;
    let sc = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    (sc, Json(json!({"ok": code == 200}))).into_response()
}

async fn agent_names(State(st): State<S>) -> Json<Value> {
    let names: Vec<Value> = st.read().await.agent_names.iter()
        .map(|n| Value::String(n.clone()))
        .collect();
    Json(json!({"ok": true, "names": names}))
}

async fn sse_stream(State(st): State<S>) -> impl IntoResponse {
    let events = st.read().await.sse_events.clone();
    let body: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
    ([("content-type", "text/event-stream")], body)
}

async fn task_claim(State(st): State<S>, Path(id): Path<String>) -> impl IntoResponse {
    let code = st.read().await.task_claim_status;
    let sc = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    (sc, Json(json!({"ok": code == 200, "task": {"id": id, "title": "mock task", "status": "claimed"}}))).into_response()
}

async fn request_claim(State(st): State<S>, Path(id): Path<String>) -> impl IntoResponse {
    let code = st.read().await.request_claim_status;
    let sc = StatusCode::from_u16(code).unwrap_or(StatusCode::OK);
    let ok = code == 200;
    let body = if ok {
        json!({"ok": true, "request": {"id": id, "status": "claimed"}})
    } else {
        json!({"error": "already_claimed"})
    };
    (sc, Json(body)).into_response()
}

async fn project_get(State(st): State<S>, Path(id): Path<String>) -> impl IntoResponse {
    let s = st.read().await;
    match s.projects.get(&id) {
        Some(p) => (
            StatusCode::OK,
            Json(json!({ "project": p.to_json() })),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "not_found"})),
        )
            .into_response(),
    }
}
