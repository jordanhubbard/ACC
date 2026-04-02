/// /routes/providers.rs — List configured infrastructure providers.

use axum::{
    extract::State,
    response::Json,
    routing::get,
    Router,
};
use serde_json::{json, Value};
use std::sync::Arc;
use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/providers", get(list_providers))
}

async fn list_providers(State(state): State<Arc<AppState>>) -> Json<Value> {
    let tokenhub_url = std::env::var("TOKENHUB_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8090".to_string());
    let minio_endpoint = std::env::var("MINIO_ENDPOINT").unwrap_or_default();
    let sc_url = std::env::var("SC_URL")
        .unwrap_or_else(|_| "http://localhost:8793".to_string());
    let crush_url = std::env::var("CRUSH_SERVER_URL")
        .unwrap_or_else(|_| "http://localhost:8795".to_string());

    // Supervisor running?
    let supervisor_running = state.supervisor.is_some();

    let providers = vec![
        json!({
            "id":     "tokenhub",
            "kind":   "llm",
            "label":  "TokenHub (LLM Aggregator)",
            "url":    tokenhub_url,
            "status": "configured",
            "enabled": true,
        }),
        json!({
            "id":     "minio",
            "kind":   "storage",
            "label":  "MinIO / S3 Storage",
            "url":    minio_endpoint,
            "status": if minio_endpoint.is_empty() { "unconfigured" } else { "configured" },
            "enabled": !minio_endpoint.is_empty(),
        }),
        json!({
            "id":     "squirrelchat",
            "kind":   "messaging",
            "label":  "SquirrelChat",
            "url":    sc_url,
            "status": "configured",
            "enabled": true,
        }),
        json!({
            "id":     "crush-server",
            "kind":   "coding",
            "label":  "Crush Server (coding agent bridge)",
            "url":    crush_url,
            "status": "configured",
            "enabled": true,
        }),
        json!({
            "id":     "supervisor",
            "kind":   "system",
            "label":  "Internal Supervisor",
            "url":    "",
            "status": if supervisor_running { "running" } else { "disabled" },
            "enabled": supervisor_running,
        }),
    ];

    Json(json!({ "providers": providers }))
}
