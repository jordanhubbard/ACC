/// ccc-agent listen — ClawBus exec listener (Rust port of agent-listener.mjs)
///
/// Connects to $CCC_URL/api/bus/stream (SSE), watches for ccc.exec messages
/// addressed to this agent, runs them via /bin/sh, and posts results back to
/// $CCC_URL/api/exec/<id>/result.
///
/// Config (from ~/.ccc/.env or env vars):
///   CCC_URL          — CCC server base URL
///   CCC_AGENT_TOKEN  — Bearer token for auth
///   AGENT_NAME       — This agent's name (used to filter targeted messages)
///   CLAWBUS_TOKEN    — Shared secret for HMAC-SHA256 signature verification

use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;

struct Config {
    ccc_url: String,
    agent_token: String,
    agent_name: String,
    clawbus_token: String,
    client: reqwest::Client,
}

// ── Entry point ───────────────────────────────────────────────────────────

pub async fn run(_args: &[String]) {
    load_dotenv();

    let config = Config {
        ccc_url:       require_env("CCC_URL"),
        agent_token:   require_env("CCC_AGENT_TOKEN"),
        agent_name:    require_env("AGENT_NAME"),
        clawbus_token: std::env::var("CLAWBUS_TOKEN")
            .or_else(|_| std::env::var("SQUIRRELBUS_TOKEN"))
            .unwrap_or_default(),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(0))   // no timeout on the stream itself
            .build()
            .expect("failed to build HTTP client"),
    };

    eprintln!("[exec-listen] agent={} url={}", config.agent_name, config.ccc_url);

    // Reconnect loop with exponential backoff (cap 60s)
    let mut backoff = 2u64;
    loop {
        match stream_loop(&config).await {
            Ok(_) => {
                eprintln!("[exec-listen] stream closed — reconnecting in {backoff}s");
            }
            Err(e) => {
                eprintln!("[exec-listen] error: {e} — reconnecting in {backoff}s");
            }
        }
        tokio::time::sleep(Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(60);
    }
}

// ── SSE stream loop ───────────────────────────────────────────────────────

async fn stream_loop(cfg: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("{}/api/bus/stream", cfg.ccc_url);
    eprintln!("[exec-listen] connecting to {url}");

    let resp = cfg.client
        .get(&url)
        .bearer_auth(&cfg.agent_token)
        .header("Accept", "text/event-stream")
        .send()
        .await?;

    if !resp.status().is_success() {
        return Err(format!("stream returned HTTP {}", resp.status()).into());
    }

    let mut stream = resp.bytes_stream();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        // SSE events are separated by blank lines (\n\n)
        while let Some(end) = buf.find("\n\n") {
            let event = buf[..end].to_string();
            buf.drain(..end + 2);
            process_event(cfg, &event).await;
        }
    }

    Ok(())
}

// ── SSE event → exec dispatch ─────────────────────────────────────────────

async fn process_event(cfg: &Config, event: &str) {
    for line in event.lines() {
        let data = match line.strip_prefix("data: ") {
            Some(d) if !d.is_empty() => d,
            _ => continue,
        };
        let msg: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        handle_message(cfg, &msg).await;
    }
}

async fn handle_message(cfg: &Config, msg: &Value) {
    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if msg_type != "ccc.exec" {
        return;
    }

    // Check target: accept messages for this agent or broadcast ("all")
    let to = msg.get("to").and_then(|v| v.as_str()).unwrap_or("");
    if to != "all" && to != cfg.agent_name {
        return;
    }

    // Body is a JSON-encoded string containing the exec envelope
    let body_str = msg.get("body").and_then(|v| v.as_str()).unwrap_or("");
    let envelope: Value = match serde_json::from_str(body_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[exec-listen] failed to parse exec envelope: {e}");
            return;
        }
    };

    // Verify HMAC signature (skip if no CLAWBUS_TOKEN configured)
    if !cfg.clawbus_token.is_empty() && !verify_sig(&envelope, &cfg.clawbus_token) {
        eprintln!("[exec-listen] HMAC verification failed — dropping message");
        return;
    }

    let exec_id = match envelope.get("execId").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => return,
    };
    let code = match envelope.get("code").and_then(|v| v.as_str()) {
        Some(c) if !c.is_empty() => c.to_string(),
        _ => return,
    };
    let timeout_ms = envelope.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(30_000);

    eprintln!("[exec-listen] exec:{exec_id} timeout:{timeout_ms}ms");

    let result = run_shell(&code, timeout_ms).await;
    post_result(cfg, &exec_id, result).await;
}

// ── Shell execution ───────────────────────────────────────────────────────

struct ExecResult {
    stdout: String,
    stderr: String,
    exit_code: i32,
    timed_out: bool,
}

async fn run_shell(code: &str, timeout_ms: u64) -> ExecResult {
    let fut = Command::new("/bin/sh")
        .arg("-c")
        .arg(code)
        .output();

    match timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok(out)) => ExecResult {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
            timed_out: false,
        },
        Ok(Err(e)) => ExecResult {
            stdout: String::new(),
            stderr: format!("exec error: {e}"),
            exit_code: -1,
            timed_out: false,
        },
        Err(_) => ExecResult {
            stdout: String::new(),
            stderr: format!("timed out after {timeout_ms}ms"),
            exit_code: -1,
            timed_out: true,
        },
    }
}

// ── POST result back to server ────────────────────────────────────────────

async fn post_result(cfg: &Config, exec_id: &str, result: ExecResult) {
    let url = format!("{}/api/exec/{exec_id}/result", cfg.ccc_url);
    let body = json!({
        "agent":     cfg.agent_name,
        "execId":    exec_id,
        "stdout":    result.stdout,
        "stderr":    result.stderr,
        "exitCode":  result.exit_code,
        "timedOut":  result.timed_out,
        "ts":        chrono::Utc::now().to_rfc3339(),
    });

    match cfg.client
        .post(&url)
        .bearer_auth(&cfg.agent_token)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() =>
            eprintln!("[exec-listen] exec:{exec_id} result posted (exit={})", result.exit_code),
        Ok(r) =>
            eprintln!("[exec-listen] exec:{exec_id} result POST returned HTTP {}", r.status()),
        Err(e) =>
            eprintln!("[exec-listen] exec:{exec_id} result POST failed: {e}"),
    }
}

// ── HMAC verification ─────────────────────────────────────────────────────

fn verify_sig(envelope: &Value, secret: &str) -> bool {
    let sig = match envelope.get("sig").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => return false,
    };
    // Reconstruct payload without sig field for verification
    let mut payload = envelope.clone();
    payload.as_object_mut().unwrap().remove("sig");

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC takes any key size");
    mac.update(payload.to_string().as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    // Constant-time compare
    use subtle::ConstantTimeEq;
    bool::from(expected.as_bytes().ct_eq(sig.as_bytes()))
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn load_dotenv() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let path = format!("{home}/.ccc/.env");
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() { continue; }
        if let Some((k, v)) = line.split_once('=') {
            // Only set if not already in environment (env vars win over .env)
            if std::env::var(k).is_err() {
                std::env::set_var(k, v);
            }
        }
    }
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        eprintln!("[exec-listen] {key} not set — check ~/.ccc/.env");
        std::process::exit(1);
    })
}
