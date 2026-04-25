use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse, Json,
    },
    routing::{get, post},
    Router,
};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::{json, Value};
use std::convert::Infallible;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio_stream::wrappers::BroadcastStream;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // /api/bus/* — used by dashboard UI and API clients
        .route("/api/bus/stream", get(bus_stream))
        .route("/api/bus/send", post(bus_send))
        .route("/api/bus/messages", get(bus_messages))
        .route("/api/bus/presence", get(bus_presence))
        // /bus/* — used by ClawChat (nginx proxies /bus/ → 8789/bus/)
        .route("/bus/stream", get(bus_stream))
        .route("/bus/send", post(bus_send))
        .route("/bus/messages", get(bus_messages))
        .route("/bus/presence", get(bus_presence))
}

// ── Query params for /bus/messages ────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct BusQuery {
    /// Max number of messages to return (default 500, max 2000).
    limit: Option<usize>,
    /// Filter by subject (channel). Matches exact string.
    subject: Option<String>,
    /// Filter by message type ("text", "reaction", etc.).
    #[serde(rename = "type")]
    msg_type: Option<String>,
    /// Filter replies: return only messages with this thread_id.
    thread_id: Option<String>,
    /// DM filter: combined with `from`, returns messages between two users.
    to: Option<String>,
    /// DM filter peer (used with `to`).
    from: Option<String>,
    /// Return only messages with ts > since (ISO-8601).
    since: Option<String>,
    /// Filter chunked blob uploads: return only messages with this upload_id.
    upload_id: Option<String>,
}

// ── SSE stream ────────────────────────────────────────────────────────────────

async fn bus_stream(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let replay = load_bus_messages(&state.bus_log_path, 50, &BusQuery::default()).await;

    let rx = state.bus_tx.subscribe();
    let live = BroadcastStream::new(rx).filter_map(|msg| async move {
        match msg {
            Ok(data) => Some(Ok(Event::default().data(data))),
            Err(_) => None,
        }
    });

    let connected = stream::once(async { Ok(Event::default().data(r#"{"type":"connected"}"#)) });
    let replayed = stream::iter(replay.into_iter().map(|msg| Ok(Event::default().data(msg))));

    let combined = connected.chain(replayed).chain(live);
    Sse::new(combined).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(30))
            .text("ping"),
    )
}

// ── POST /bus/send ────────────────────────────────────────────────────────────

async fn bus_send(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Unauthorized"})),
        )
            .into_response();
    }

    let seq = state.bus_seq.fetch_add(1, Ordering::SeqCst);
    let now = chrono::Utc::now().to_rfc3339();

    let mut msg = body;
    if let Some(obj) = msg.as_object_mut() {
        // Assign stable id if not provided by the sender.
        obj.entry("id").or_insert_with(|| json!(format!("msg-{}", seq)));
        obj.insert("seq".into(), json!(seq));
        obj.insert("ts".into(), json!(now));

        // ── blob_meta injection ──────────────────────────────────────────────
        // For every message of type "blob" we synthesise a server-side
        // `blob_meta` object so that consumers (UI, tests) never have to
        // re-derive it from raw fields.
        if obj.get("type").and_then(|v| v.as_str()) == Some("blob") {
            // Clamp any caller-supplied ttl_secs to BLOB_MAX_TTL_SECS before
            // the message is stored, so the stored value is always ≤ the cap.
            use crate::blob_store::BLOB_MAX_TTL_SECS;
            if let Some(ttl_val) = obj.get_mut("ttl_secs") {
                if let Some(ttl) = ttl_val.as_u64() {
                    if ttl > BLOB_MAX_TTL_SECS {
                        *ttl_val = json!(BLOB_MAX_TTL_SECS);
                    }
                }
            }
            // Collect all borrowed values as owned Strings up front so that
            // we hold no immutable borrows when we later call obj.insert().
            let mime       = obj.get("mime").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let enc        = obj.get("enc").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let body_owned = obj.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();

            // Chunked-upload coordination fields (cloned before any mut borrow).
            let chunk_index_val = obj.get("chunk_index").cloned();
            let chunk_total_val = obj.get("chunk_total").cloned();
            let upload_id_val   = obj.get("upload_id").cloned();

            // size_bytes is the byte-length of the raw body string (the
            // base64-encoded chunk payload as sent on the wire).
            let size_bytes = body_owned.len() as u64;

            // render_as: image/* → "image", everything else → "download".
            let render_as = if mime.starts_with("image/") { "image" } else { "download" };

            // No immutable borrows outstanding from here on — safe to mutate.
            obj.insert(
                "blob_meta".into(),
                json!({
                    "mime":       mime,
                    "enc":        enc,
                    "render_as":  render_as,
                    "size_bytes": size_bytes,
                }),
            );

            // ── chunked-upload disk layout ───────────────────────────────────
            // When chunk_index / chunk_total are present, write each
            // intermediate chunk to  <fs_root>/<upload_id>/chunks/chunk_XXXXXXXX
            // (zero-padded 8-digit index).
            //
            // On receipt of the *final* chunk (chunk_index == chunk_total - 1):
            //   1. Concatenate all chunk files in order into  …/<upload_id>/data
            //   2. Remove the chunks/ sub-directory.
            //   3. Write meta.json atomically (tmp → rename) alongside data.
            if let (Some(idx_val), Some(total_val), Some(uid_val)) = (
                chunk_index_val,
                chunk_total_val,
                upload_id_val,
            ) {
                if let (Some(chunk_index), Some(chunk_total), Some(upload_id)) = (
                    idx_val.as_u64(),
                    total_val.as_u64(),
                    uid_val.as_str(),
                ) {
                    let body_data  = body_owned.as_bytes().to_vec();
                    let upload_id  = upload_id.to_string();
                    let mime_owned = mime.clone();
                    let enc_owned  = enc.clone();
                    let fs_root    = state.fs_root.clone();

                    // Write the chunk file; errors are logged and swallowed —
                    // the message is always committed to the bus log.
                    let _ = write_chunk(
                        &fs_root,
                        &upload_id,
                        chunk_index,
                        chunk_total,
                        &body_data,
                        &mime_owned,
                        &enc_owned,
                    )
                    .await
                    .map_err(|e| {
                        tracing::warn!(
                            upload_id = %upload_id,
                            chunk     = chunk_index,
                            error     = %e,
                            "bus_send: failed to write chunk file"
                        );
                    });
                }
            }
        }
    }

    let msg_str = serde_json::to_string(&msg).unwrap_or_default();
    let log_line = format!("{}\n", msg_str);
    let _ = append_line(&state.bus_log_path, &log_line).await;
    let _ = state.bus_tx.send(msg_str);

    Json(json!({"ok": true, "message": msg})).into_response()
}

// ── GET /bus/messages ─────────────────────────────────────────────────────────

async fn bus_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<BusQuery>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let limit = q.limit.unwrap_or(500).min(2000);
    let msgs = load_bus_messages(&state.bus_log_path, limit, &q).await;
    let parsed: Vec<Value> = msgs
        .iter()
        .filter_map(|s| serde_json::from_str(s).ok())
        .collect();
    Json(json!(parsed)).into_response()
}

// ── GET /bus/presence ─────────────────────────────────────────────────────────

async fn bus_presence(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"Unauthorized"}))).into_response();
    }
    let agents = state.agents.read().await;
    let now = chrono::Utc::now();

    let mut presence = serde_json::Map::new();
    if let Some(obj) = agents.as_object() {
        for (name, agent) in obj {
            let last_seen_str = agent.get("last_seen")
                .or_else(|| agent.get("lastSeen"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let online = chrono::DateTime::parse_from_rfc3339(last_seen_str)
                .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_seconds() < 600)
                .unwrap_or(false);
            presence.insert(name.clone(), json!({
                "status": if online { "online" } else { "offline" },
                "last_seen": last_seen_str,
            }));
        }
    }
    Json(Value::Object(presence)).into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn load_bus_messages(path: &str, limit: usize, q: &BusQuery) -> Vec<String> {
    let content = match tokio::fs::read_to_string(path).await {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(limit * 4) // over-fetch to account for filtered-out messages
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .filter(|line| {
            let Ok(v) = serde_json::from_str::<Value>(line) else { return false };

            if let Some(subj) = &q.subject {
                if v.get("subject").and_then(|s| s.as_str()) != Some(subj.as_str()) {
                    return false;
                }
            }
            if let Some(t) = &q.msg_type {
                if v.get("type").and_then(|s| s.as_str()) != Some(t.as_str()) {
                    return false;
                }
            }
            if let Some(tid) = &q.thread_id {
                if v.get("thread_id").and_then(|s| s.as_str()) != Some(tid.as_str()) {
                    return false;
                }
            }
            if let Some(to_user) = &q.to {
                let msg_to = v.get("to").and_then(|s| s.as_str()).unwrap_or("");
                let msg_from = v.get("from").and_then(|s| s.as_str()).unwrap_or("");
                let from_user = q.from.as_deref().unwrap_or("");
                if !((msg_to == to_user && msg_from == from_user)
                    || (msg_to == from_user && msg_from == to_user))
                {
                    return false;
                }
            }
            if let Some(since) = &q.since {
                let msg_ts = v.get("ts").and_then(|s| s.as_str()).unwrap_or("");
                if msg_ts <= since.as_str() {
                    return false;
                }
            }
            if let Some(uid) = &q.upload_id {
                if v.get("upload_id").and_then(|s| s.as_str()) != Some(uid.as_str()) {
                    return false;
                }
            }
            true
        })
        .take(limit)
        .map(|s| s.to_string())
        .collect()
}

async fn append_line(path: &str, line: &str) -> std::io::Result<()> {
    use tokio::fs::OpenOptions;
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    Ok(())
}

// ── Chunked-upload disk layout helpers ───────────────────────────────────────
//
// Directory structure under <fs_root>:
//
//   <upload_id>/
//     chunks/
//       chunk_00000000   ← body bytes of chunk 0
//       chunk_00000001   ← body bytes of chunk 1
//       …
//     data               ← assembled payload (written on final chunk)
//     meta.json          ← upload metadata  (written atomically on final chunk)
//
// The `chunks/` sub-directory is removed once assembly is complete so that
// only `data` and `meta.json` remain.

/// Write one chunk to disk and, if it is the final chunk, assemble `data` +
/// `meta.json` and remove `chunks/`.
async fn write_chunk(
    fs_root:     &str,
    upload_id:   &str,
    chunk_index: u64,
    chunk_total:  u64,
    body:        &[u8],
    mime:        &str,
    enc:         &str,
) -> std::io::Result<()> {
    let upload_dir  = Path::new(fs_root).join(upload_id);
    let chunks_dir  = upload_dir.join("chunks");
    tokio::fs::create_dir_all(&chunks_dir).await?;

    // Write this chunk: chunk_XXXXXXXX (8-digit, zero-padded).
    let chunk_name = format!("chunk_{:08}", chunk_index);
    let chunk_path = chunks_dir.join(&chunk_name);
    tokio::fs::write(&chunk_path, body).await?;

    tracing::debug!(
        upload_id,
        chunk_index,
        chunk_total,
        path = %chunk_path.display(),
        "bus_send: wrote chunk file"
    );

    // Final chunk triggers assembly.
    if chunk_index + 1 == chunk_total {
        assemble_upload(&upload_dir, &chunks_dir, chunk_total, mime, enc).await?;
    }

    Ok(())
}

/// Concatenate all chunk files into `data`, write `meta.json` atomically,
/// then remove the `chunks/` directory.
async fn assemble_upload(
    upload_dir:  &Path,
    chunks_dir:  &Path,
    chunk_total: u64,
    mime:        &str,
    enc:         &str,
) -> std::io::Result<()> {
    // Concatenate chunks 0..chunk_total in order into `data`.
    let data_path = upload_dir.join("data");
    {
        use tokio::fs::OpenOptions;
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&data_path)
            .await?;

        for idx in 0..chunk_total {
            let chunk_path = chunks_dir.join(format!("chunk_{:08}", idx));
            let chunk_bytes = tokio::fs::read(&chunk_path).await.map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("chunk {idx} missing during assembly: {e}"),
                )
            })?;
            out.write_all(&chunk_bytes).await?;
        }
        out.flush().await?;
    }

    // Write meta.json atomically: write to a .tmp sibling, then rename.
    let meta_path     = upload_dir.join("meta.json");
    let meta_tmp_path = upload_dir.join("meta.json.tmp");
    let meta = json!({
        "chunk_total": chunk_total,
        "mime":        mime,
        "enc":         enc,
    });
    tokio::fs::write(&meta_tmp_path, serde_json::to_vec_pretty(&meta).unwrap_or_default())
        .await?;
    tokio::fs::rename(&meta_tmp_path, &meta_path).await?;

    tracing::debug!(
        path = %upload_dir.display(),
        "bus_send: assembled upload; wrote data and meta.json"
    );

    // Remove the chunks/ directory now that assembly is complete.
    if let Err(e) = tokio::fs::remove_dir_all(chunks_dir).await {
        tracing::warn!(
            path  = %chunks_dir.display(),
            error = %e,
            "bus_send: failed to remove chunks/ directory after assembly"
        );
        // Non-fatal: data and meta.json are correct; dangling chunks/ is
        // a minor storage leak but does not corrupt the assembled payload.
    }

    Ok(())
}
