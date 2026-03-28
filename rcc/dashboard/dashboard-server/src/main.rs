use axum::{
    Router,
    routing::get,
    extract::State,
    response::Response,
    http::StatusCode,
    body::Body,
};
use std::sync::Arc;
use tower_http::services::ServeDir;
use tracing_subscriber::prelude::*;

#[derive(Clone)]
struct AppState {
    rcc_url: String,
    agent_token: String,
    client: reqwest::Client,
    stream_client: reqwest::Client,
}

fn json_error(status: StatusCode, msg: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from(format!(r#"{{"error":"{}"}}"#, msg)))
        .unwrap()
}

async fn proxy_to_rcc(
    state: &AppState,
    method: reqwest::Method,
    path: &str,
    body: Option<bytes::Bytes>,
) -> Response<Body> {
    let url = format!("{}{}", state.rcc_url, path);

    let mut builder = state
        .client
        .request(method, &url)
        .header("Authorization", format!("Bearer {}", state.agent_token));

    if let Some(b) = body {
        builder = builder
            .header("Content-Type", "application/json")
            .body(b);
    }

    match builder.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            match resp.bytes().await {
                Ok(bytes) => Response::builder()
                    .status(status)
                    .header("Content-Type", ct)
                    .header("Access-Control-Allow-Origin", "*")
                    .body(Body::from(bytes))
                    .unwrap(),
                Err(_) => json_error(StatusCode::BAD_GATEWAY, "upstream read error"),
            }
        }
        Err(e) => {
            tracing::warn!("Proxy {} error: {}", path, e);
            json_error(StatusCode::BAD_GATEWAY, &e.to_string())
        }
    }
}

async fn api_get(
    State(state): State<Arc<AppState>>,
    uri: axum::http::Uri,
) -> Response<Body> {
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| uri.path());
    proxy_to_rcc(&state, reqwest::Method::GET, path, None).await
}

async fn api_post(
    State(state): State<Arc<AppState>>,
    uri: axum::http::Uri,
    body: bytes::Bytes,
) -> Response<Body> {
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| uri.path());
    proxy_to_rcc(&state, reqwest::Method::POST, path, Some(body)).await
}

async fn bus_get(
    State(state): State<Arc<AppState>>,
    uri: axum::http::Uri,
) -> Response<Body> {
    let path = uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or_else(|| uri.path());
    proxy_to_rcc(&state, reqwest::Method::GET, path, None).await
}

async fn bus_stream(State(state): State<Arc<AppState>>) -> Response<Body> {
    let url = format!("{}/bus/stream", state.rcc_url);
    match state
        .stream_client
        .get(&url)
        .header("Authorization", format!("Bearer {}", state.agent_token))
        .header("Accept", "text/event-stream")
        .send()
        .await
    {
        Ok(resp) => {
            let stream = resp.bytes_stream();
            Response::builder()
                .status(200)
                .header("Content-Type", "text/event-stream")
                .header("Cache-Control", "no-cache")
                .header("Connection", "keep-alive")
                .header("Access-Control-Allow-Origin", "*")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        Err(e) => {
            tracing::warn!("SSE upstream error: {}", e);
            // Return a valid SSE stream with a keepalive comment so the client
            // doesn't crash — it will reconnect on its own.
            Response::builder()
                .status(200)
                .header("Content-Type", "text/event-stream")
                .header("Cache-Control", "no-cache")
                .body(Body::from(": upstream unavailable\n\n"))
                .unwrap()
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dashboard_server=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let port = std::env::var("RCC_DASHBOARD_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(8790);

    let rcc_url = std::env::var("RCC_URL")
        .unwrap_or_else(|_| "http://localhost:8789".to_string());

    let agent_token = std::env::var("RCC_AGENT_TOKEN").unwrap_or_default();

    // Operator handle for informational logging
    let operator = std::env::var("OPERATOR_HANDLE").unwrap_or_else(|_| "jkh".to_string());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("HTTP client build failed");

    // No timeout for SSE streams — they are long-lived by design.
    let stream_client = reqwest::Client::builder()
        .build()
        .expect("Stream client build failed");

    let state = Arc::new(AppState {
        rcc_url: rcc_url.clone(),
        agent_token,
        client,
        stream_client,
    });

    let dist = std::env::var("DASHBOARD_DIST").unwrap_or_else(|_| "dist".to_string());

    // Route priority (axum picks most-specific first):
    //   /bus/stream  →  SSE passthrough (no timeout)
    //   /api/*       →  JSON proxy (GET + POST)
    //   /bus/*       →  bus proxy (GET)
    //   fallback     →  serve dist/ static files (SPA)
    let app = Router::new()
        .route("/bus/stream", get(bus_stream))
        .route("/api/*path", get(api_get).post(api_post))
        .route("/bus/*path", get(bus_get))
        .with_state(state)
        .fallback_service(ServeDir::new(&dist).append_index_html_on_directories(true));

    tracing::info!(
        port,
        rcc_url,
        operator,
        "RCC Dashboard v2 starting"
    );
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}"))
        .await
        .expect("Failed to bind");
    axum::serve(listener, app).await.expect("Server error");
}
