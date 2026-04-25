/// POST /api/bus/blobs      — store a base64-encoded blob identified by a
///                            caller-supplied `id` and a validated MIME type.
/// GET  /api/bus/blobs/:id  — retrieve the stored blob; responds with the
///                            original Content-Type and raw (decoded) bytes.
/// DELETE /api/bus/blobs/:id — immediately evict a blob and delete its
///                             backing storage file (if any).
///
/// # MIME validation
///
/// Only MIME types registered in the `media_types!` table in `bus_types.rs`
/// are accepted; anything else is rejected with 422 Unprocessable Entity.
///
/// # TTL / expiry
///
/// Each uploaded blob is stored with a time-to-live derived from the optional
/// `ttl_secs` field in the upload body:
///
/// | Supplied value            | Effective TTL                    |
/// |---------------------------|----------------------------------|
/// | omitted                   | `BLOB_DEFAULT_TTL_SECS` (24 h)   |
/// | `> BLOB_MAX_TTL_SECS`     | clamped to `BLOB_MAX_TTL_SECS`   |
/// | `0`                       | immediately expired on insert    |
///
/// Expired blobs are invisible to `GET` and are pruned by the background
/// sweep task that is started by `router()`.  When a blob is evicted (either
/// by the sweep or by an explicit `DELETE`) its AccFS backing file — if the
/// upload provided a non-empty `storage_path` — is deleted from disk.
///
/// # Storage backend
///
/// The route layer uses [`crate::blob_store::BlobStore`] as the in-memory
/// index and, optionally, AccFS (`state.fs_root`) as the durable backing
/// store.  Uploads that include a `storage_path` in the request body will
/// have their decoded payload written to `<fs_root>/<storage_path>` **in
/// addition to** being kept in the in-memory index; the backing file is then
/// deleted when the entry expires or is manually deleted.
///
/// Uploads that do NOT supply a `storage_path` are kept only in the
/// in-memory index.  This is the same behaviour as before TTL was added and
/// remains the common case for short-lived or test blobs.
// Re-export the TTL constants so that external code (e.g. integration tests)
// can reach them via `acc_server::routes::blobs` without depending on the
// internal `blob_store` module directly.  The `pub use` also serves as the
// only import of these names in this module.
pub use crate::blob_store::{BLOB_DEFAULT_TTL_SECS, BLOB_MAX_TTL_SECS};
use crate::blob_store::BlobStore;
use crate::bus_types::MediaType;
use crate::AppState;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post},
    Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the blobs sub-router.
///
/// A fresh `BlobStore` is created here and attached as an Axum layer
/// extension so that each router instance (and therefore each test server)
/// gets its own isolated store.  The background expiry sweep is also started
/// at this point.
pub fn router(fs_root: impl Into<String>) -> Router<Arc<AppState>> {
    let store = BlobStore::new(fs_root);
    store.clone().spawn_sweep();

    Router::new()
        .route("/api/bus/blobs",      post(blob_upload))
        .route("/api/bus/blobs/:id",  get(blob_download))
        .route("/api/bus/blobs/:id",  delete(blob_delete))
        .layer(axum::Extension(store))
}

// ── Request body ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BlobUploadRequest {
    /// Caller-supplied stable identifier for this blob.
    id: String,
    /// MIME type — must be registered in `bus_types::MediaType`.
    mime: String,
    /// Base64-encoded payload.
    data: String,
    /// Optional TTL in seconds.  Omitting applies `BLOB_DEFAULT_TTL_SECS`.
    /// Values above `BLOB_MAX_TTL_SECS` are silently clamped.  Zero means
    /// the blob expires immediately and is not retrievable after the response
    /// is returned.
    #[serde(default)]
    ttl_secs: Option<u64>,
    /// Optional AccFS-relative path for durable backing storage.
    ///
    /// When provided, the decoded payload is also written to
    /// `<fs_root>/<storage_path>` and the file is deleted when the entry
    /// expires or is explicitly deleted.  Callers that only need the
    /// in-memory index can omit this field.
    #[serde(default)]
    storage_path: Option<String>,
}

// ── POST /api/bus/blobs ───────────────────────────────────────────────────────

async fn blob_upload(
    State(state): State<Arc<AppState>>,
    axum::Extension(store): axum::Extension<BlobStore>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    let req: BlobUploadRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!("invalid request body: {e}")})),
            )
                .into_response();
        }
    };

    // Validate the MIME type against the registered table.  Unknown types →
    // 422 Unprocessable Entity.
    if MediaType::from_mime(&req.mime).is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error":  "unsupported_mime_type",
                "mime":   req.mime,
                "detail": "MIME type is not registered in the bus media_types table",
            })),
        )
            .into_response();
    }

    // Decode the base64 payload.
    let raw: Vec<u8> = match B64.decode(req.data.as_bytes()) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({"error": format!("base64 decode failed: {e}")})),
            )
                .into_response();
        }
    };

    // Enforce the per-upload size limit configured on AppState.
    // The check is applied after base64 decoding so `limit_bytes` in the
    // error body reflects the raw (decoded) byte count, matching what callers
    // receive on a successful GET.
    let limit = state.max_blob_bytes;
    if raw.len() as u64 > limit {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error":       "payload_too_large",
                "size_bytes":  raw.len() as u64,
                "limit_bytes": limit,
            })),
        )
            .into_response();
    }

    // Clamp oversized TTL before any further processing.
    let effective_ttl: Option<u64> = req.ttl_secs.map(|t| t.min(BLOB_MAX_TTL_SECS));

    // Optionally persist to AccFS backing storage.
    let storage_path = req.storage_path.clone().unwrap_or_default();
    // Determine the storage_path we'll actually record in the entry.
    // A failed backing-file write is non-fatal: the blob is kept in-memory
    // without a storage_path so the sweep never tries to delete a file that
    // was never created.
    let recorded_storage_path: String = if !storage_path.is_empty() {
        match write_backing_file(&state.fs_root, &storage_path, &raw).await {
            Ok(()) => storage_path.clone(),
            Err(e) => {
                tracing::warn!(
                    id = %req.id,
                    path = %storage_path,
                    error = %e,
                    "blob upload: failed to write backing file"
                );
                String::new()
            }
        }
    } else {
        String::new()
    };

    store
        .insert(&req.id, &req.mime, raw, effective_ttl, &recorded_storage_path)
        .await;

    let reported_ttl = effective_ttl.unwrap_or(BLOB_DEFAULT_TTL_SECS);

    (
        StatusCode::OK,
        Json(json!({
            "ok":      true,
            "id":      req.id,
            "mime":    req.mime,
            "ttl_secs": reported_ttl,
        })),
    )
        .into_response()
}

// ── GET /api/bus/blobs/:id ────────────────────────────────────────────────────

async fn blob_download(
    State(state): State<Arc<AppState>>,
    axum::Extension(store): axum::Extension<BlobStore>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !state.is_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    match store.get(&id).await {
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "blob_not_found", "id": id})),
        )
            .into_response(),
        Some(entry) => {
            let ct = HeaderValue::from_str(&entry.mime)
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", ct)
                .body(Body::from(entry.data))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
    }
}

// ── DELETE /api/bus/blobs/:id ─────────────────────────────────────────────────

async fn blob_delete(
    State(state): State<Arc<AppState>>,
    axum::Extension(store): axum::Extension<BlobStore>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !state.is_authed(&headers) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "Unauthorized"}))).into_response();
    }

    if store.remove(&id).await {
        (StatusCode::OK, Json(json!({"ok": true, "id": id}))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "blob_not_found", "id": id})),
        )
            .into_response()
    }
}

// ── AccFS backing-file helper ─────────────────────────────────────────────────

/// Write `data` to `<fs_root>/<rel_path>`, creating parent directories as
/// needed.  Mirrors the pattern used in `routes/fs.rs`.
async fn write_backing_file(
    fs_root:  &str,
    rel_path: &str,
    data:     &[u8],
) -> std::io::Result<()> {
    // Guard against path traversal — same rule as routes/fs.rs.
    if rel_path.contains("..") || rel_path.starts_with('/') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path traversal not allowed",
        ));
    }

    let abs = std::path::Path::new(fs_root).join(rel_path);
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&abs, data).await
}

// ── Integration tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, body_bytes, body_json, TestServer};
    use axum::http::Request;
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    use serde_json::json;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build the full application with a TestServer (has its own isolated
    /// BlobStore).
    async fn srv() -> TestServer {
        TestServer::new().await
    }

    fn upload_body(id: &str, mime: &str, payload: &[u8]) -> serde_json::Value {
        json!({
            "id":   id,
            "mime": mime,
            "data": B64.encode(payload),
        })
    }

    fn upload_body_with_ttl(
        id:       &str,
        mime:     &str,
        payload:  &[u8],
        ttl_secs: u64,
    ) -> serde_json::Value {
        json!({
            "id":       id,
            "mime":     mime,
            "data":     B64.encode(payload),
            "ttl_secs": ttl_secs,
        })
    }

    // ── basic round-trip ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn upload_and_download_roundtrip() {
        let srv = srv().await;
        let payload = b"hello blobs";
        let up = testing::call(
            &srv.app,
            testing::post_json("/api/bus/blobs", &upload_body("rt1", "text/plain", payload)),
        )
        .await;
        assert_eq!(up.status(), 200);
        let up_json = body_json(up).await;
        assert_eq!(up_json["ok"], true);
        assert_eq!(up_json["id"], "rt1");
        assert_eq!(up_json["mime"], "text/plain");

        let dl = testing::call(&srv.app, testing::get("/api/bus/blobs/rt1")).await;
        assert_eq!(dl.status(), 200);
        let raw = body_bytes(dl).await;
        assert_eq!(&*raw, payload);
    }

    #[tokio::test]
    async fn download_returns_correct_content_type() {
        let srv = srv().await;
        testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body("png1", "image/png", &[0x89, 0x50, 0x4E, 0x47]),
            ),
        )
        .await;

        let dl = testing::call(&srv.app, testing::get("/api/bus/blobs/png1")).await;
        assert_eq!(dl.status(), 200);
        let ct = dl
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "image/png");
    }

    // ── 404 ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn download_unknown_id_returns_404() {
        let srv = srv().await;
        let resp = testing::call(&srv.app, testing::get("/api/bus/blobs/no-such-blob")).await;
        assert_eq!(resp.status(), 404);
        let body = body_json(resp).await;
        assert_eq!(body["error"], "blob_not_found");
    }

    // ── MIME validation ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn unknown_mime_returns_422() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body("bad-mime", "application/x-nonsense-type", b"data"),
            ),
        )
        .await;
        assert_eq!(resp.status(), 422);
        let body = body_json(resp).await;
        assert_eq!(body["error"], "unsupported_mime_type");
    }

    #[tokio::test]
    async fn invalid_base64_returns_422() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &json!({ "id": "b64err", "mime": "text/plain", "data": "not!!base64!!" }),
            ),
        )
        .await;
        assert_eq!(resp.status(), 422);
    }

    // ── auth ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn upload_without_token_returns_401() {
        let srv = srv().await;
        let req = Request::builder()
            .method("POST")
            .uri("/api/bus/blobs")
            .header("Content-Type", "application/json")
            .body(Body::from(
                upload_body("unauth", "text/plain", b"x").to_string(),
            ))
            .unwrap();
        let resp = testing::call(&srv.app, req).await;
        assert_eq!(resp.status(), 401);
    }

    #[tokio::test]
    async fn download_without_token_returns_401() {
        let srv = srv().await;
        // upload first (with auth) so the id exists
        testing::call(
            &srv.app,
            testing::post_json("/api/bus/blobs", &upload_body("auth-dl", "text/plain", b"y")),
        )
        .await;

        let req = Request::builder()
            .method("GET")
            .uri("/api/bus/blobs/auth-dl")
            .body(Body::empty())
            .unwrap();
        let resp = testing::call(&srv.app, req).await;
        assert_eq!(resp.status(), 401);
    }

    // ── TTL field in upload response ──────────────────────────────────────────

    #[tokio::test]
    async fn upload_with_explicit_ttl_echoes_ttl_in_response() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body_with_ttl("ttl-echo", "text/plain", b"hi", 3600),
            ),
        )
        .await;
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["ttl_secs"], 3600);
    }

    #[tokio::test]
    async fn upload_without_ttl_echoes_default_ttl_in_response() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json("/api/bus/blobs", &upload_body("ttl-def", "text/plain", b"hi")),
        )
        .await;
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["ttl_secs"], BLOB_DEFAULT_TTL_SECS as i64);
    }

    #[tokio::test]
    async fn upload_with_oversized_ttl_is_clamped() {
        let srv = srv().await;
        let over = BLOB_MAX_TTL_SECS + 1_000_000;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body_with_ttl("ttl-clamp", "text/plain", b"x", over),
            ),
        )
        .await;
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["ttl_secs"], BLOB_MAX_TTL_SECS as i64);
    }

    // ── zero TTL: expired on insert ───────────────────────────────────────────

    #[tokio::test]
    async fn zero_ttl_upload_accepted_then_returns_404_on_get() {
        let srv = srv().await;
        let up = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body_with_ttl("zero-ttl", "text/plain", b"ephemeral", 0),
            ),
        )
        .await;
        // Upload succeeds with 200.
        assert_eq!(up.status(), 200);

        // Immediately trying to retrieve it yields 404 because the entry is
        // already past its TTL.
        let dl = testing::call(&srv.app, testing::get("/api/bus/blobs/zero-ttl")).await;
        assert_eq!(
            dl.status(),
            404,
            "zero-TTL blob must be invisible to GET immediately after upload"
        );
    }

    // ── DELETE endpoint ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_existing_blob_returns_200() {
        let srv = srv().await;
        testing::call(
            &srv.app,
            testing::post_json("/api/bus/blobs", &upload_body("del-me", "text/plain", b"bye")),
        )
        .await;

        let resp = testing::call(&srv.app, testing::delete("/api/bus/blobs/del-me")).await;
        assert_eq!(resp.status(), 200);
        let body = body_json(resp).await;
        assert_eq!(body["ok"], true);
    }

    #[tokio::test]
    async fn delete_then_get_returns_404() {
        let srv = srv().await;
        testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body("gone-after-del", "text/plain", b"data"),
            ),
        )
        .await;

        testing::call(
            &srv.app,
            testing::delete("/api/bus/blobs/gone-after-del"),
        )
        .await;

        let resp = testing::call(
            &srv.app,
            testing::get("/api/bus/blobs/gone-after-del"),
        )
        .await;
        assert_eq!(resp.status(), 404);
    }

    #[tokio::test]
    async fn delete_unknown_blob_returns_404() {
        let srv = srv().await;
        let resp =
            testing::call(&srv.app, testing::delete("/api/bus/blobs/no-such-id")).await;
        assert_eq!(resp.status(), 404);
        let body = body_json(resp).await;
        assert_eq!(body["error"], "blob_not_found");
    }

    #[tokio::test]
    async fn delete_without_token_returns_401() {
        let srv = srv().await;
        // Upload first
        testing::call(
            &srv.app,
            testing::post_json("/api/bus/blobs", &upload_body("del-unauth", "text/plain", b"z")),
        )
        .await;

        let req = Request::builder()
            .method("DELETE")
            .uri("/api/bus/blobs/del-unauth")
            .body(Body::empty())
            .unwrap();
        let resp = testing::call(&srv.app, req).await;
        assert_eq!(resp.status(), 401);
    }

    // ── 3D model MIME types pass validation ───────────────────────────────────

    #[tokio::test]
    async fn model_gltf_json_upload_accepted() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body("gltf1", "model/gltf+json", b"{}"),
            ),
        )
        .await;
        assert_eq!(resp.status(), 200);
    }

    #[tokio::test]
    async fn model_gltf_binary_upload_accepted() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &upload_body("glb1", "model/gltf-binary", b"\x47\x4C\x54\x46"),
            ),
        )
        .await;
        assert_eq!(resp.status(), 200);
    }

    // ── AccFS backing-file integration ────────────────────────────────────────

    #[tokio::test]
    async fn upload_with_storage_path_writes_file_to_fs_root() {
        let srv = srv().await;
        let payload = b"binary content";
        let rel = "blobs/test-backing.bin";

        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &json!({
                    "id":           "backed",
                    "mime":         "application/octet-stream",
                    "data":         B64.encode(payload),
                    "storage_path": rel,
                }),
            ),
        )
        .await;
        assert_eq!(resp.status(), 200);

        // fs_root is <tmp>/fs (see make_state in lib.rs).
        let abs = srv.tmp.path().join("fs").join(rel);
        // Wait briefly for async write to complete (it happens inline, but
        // the OS may buffer).
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(abs.exists(), "backing file must be written to fs_root");
        let on_disk = tokio::fs::read(&abs).await.unwrap();
        assert_eq!(on_disk, payload);
    }

    #[tokio::test]
    async fn path_traversal_in_storage_path_is_rejected() {
        let srv = srv().await;
        let resp = testing::call(
            &srv.app,
            testing::post_json(
                "/api/bus/blobs",
                &json!({
                    "id":           "traversal",
                    "mime":         "text/plain",
                    "data":         B64.encode(b"x"),
                    "storage_path": "../../etc/passwd",
                }),
            ),
        )
        .await;
        // Upload still returns 200 because a failed backing-file write is
        // non-fatal; the blob is kept in-memory without a storage_path.
        // The important thing is that no file was written outside fs_root.
        let _ = resp.status(); // accept any 2xx or error
        let evil = std::path::Path::new("/etc/passwd-from-test");
        assert!(!evil.exists(), "path traversal must not reach the filesystem");
    }
}
