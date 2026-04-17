//! NVIDIA header-stripping HTTP proxy.
//!
//! Replaces nvidia-proxy.py. Listens on 127.0.0.1:9099 and forwards all
//! requests to NVIDIA_API_BASE, stripping the `anthropic-beta` header that
//! NVIDIA's LiteLLM endpoint rejects.

use std::net::SocketAddr;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Request, State},
    response::Response,
    routing::any,
    Router,
};
use bytes::Bytes;
use http_body_util::BodyExt;
use reqwest::Client;

const STRIP_HEADERS: &[&str] = &["anthropic-beta", "host", "content-length", "transfer-encoding"];

#[derive(Clone)]
struct ProxyState {
    client: Client,
    upstream: String,
}

pub async fn run(args: &[String]) {
    crate::config::load_env_file(
        &crate::config::acc_dir().join(".env"),
    );

    let mut port: u16 = 9099;
    let mut upstream = std::env::var("NVIDIA_API_BASE")
        .unwrap_or_else(|_| "https://inference-api.nvidia.com".into());

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                if let Some(p) = args.get(i) {
                    port = p.parse().unwrap_or(9099);
                }
            }
            "--target" => {
                i += 1;
                if let Some(t) = args.get(i) {
                    upstream = t.clone();
                }
            }
            _ => {}
        }
        i += 1;
    }

    let upstream = upstream.trim_end_matches('/').to_string();

    let client = Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .expect("failed to build proxy client");

    let state = ProxyState { client, upstream: upstream.clone() };

    let app = Router::new()
        .route("/{*path}", any(proxy_handler))
        .route("/", any(proxy_handler))
        .with_state(state);

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    eprintln!("[proxy] listening on {addr} → {upstream}");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind proxy port");

    axum::serve(listener, app)
        .await
        .expect("proxy server error");
}

async fn proxy_handler(State(state): State<ProxyState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();

    // Collect body (buffered — required to rebuild for reqwest)
    let body_bytes: Bytes = match body.collect().await {
        Ok(c) => c.to_bytes(),
        Err(e) => {
            return error_response(400, &format!("body read error: {e}"));
        }
    };

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let url = format!("{}{}", state.upstream, path_and_query);

    // Map axum method → reqwest method (both are http::Method from same crate version)
    let method: reqwest::Method = match parts.method.as_str().parse() {
        Ok(m) => m,
        Err(_) => reqwest::Method::GET,
    };

    // Build forwarded headers, stripping disallowed ones
    let mut fwd_headers = reqwest::header::HeaderMap::new();
    for (name, value) in parts.headers.iter() {
        if STRIP_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(value.as_bytes()),
        ) {
            fwd_headers.insert(n, v);
        }
    }

    let upstream_resp = match state
        .client
        .request(method, &url)
        .headers(fwd_headers)
        .body(body_bytes)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return error_response(502, &format!("upstream error: {e}")),
    };

    let status = upstream_resp.status().as_u16();
    let resp_headers = upstream_resp.headers().clone();
    let stream = upstream_resp.bytes_stream();

    let mut resp = Response::builder().status(status);
    for (name, value) in resp_headers.iter() {
        const HOP: &[&str] = &["connection", "keep-alive", "transfer-encoding", "te",
                                "trailer", "upgrade", "proxy-authorization", "proxy-authenticate"];
        if HOP.contains(&name.as_str()) { continue; }
        resp = resp.header(name.as_str(), value.as_bytes());
    }

    resp.body(Body::from_stream(stream))
        .unwrap_or_else(|_| error_response(500, "response build error"))
}

fn error_response(status: u16, msg: &str) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain")
        .body(Body::from(msg.to_string()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_headers_list() {
        assert!(STRIP_HEADERS.contains(&"anthropic-beta"));
        assert!(STRIP_HEADERS.contains(&"host"));
        assert!(STRIP_HEADERS.contains(&"content-length"));
    }

    #[test]
    fn test_error_response_status() {
        let resp = error_response(502, "bad gateway");
        assert_eq!(resp.status(), 502);
    }
}
