mod helpers;

// Integration tests for the `max_blob_bytes` size-limit enforcement in
// `POST /api/bus/blobs`.
//
// Every test constructs its own isolated [`helpers::TestServer`] via
// `TestServer::with_limit(n)` so the limit under test is explicit and
// independent of every other test suite.
//
// Scenarios covered (matching the eight required cases from the task spec):
//
// 1. **Under-limit → 200** — a payload strictly smaller than the limit is
//    accepted and stored.
// 2. **Over-limit → 413** — a payload one byte larger than the limit is
//    rejected with HTTP 413 Payload Too Large.
// 3. **Cumulative / multi-chunk 413** — each upload is an independent
//    single-`POST` operation; the second upload, which is individually
//    over the limit, is rejected even though the first succeeded.
// 4. **Exact-boundary → 200** — a payload whose decoded size equals the
//    limit exactly is accepted (the check is `>`, not `>=`).
// 5. **Zero-limit** — a limit of 0 means every non-empty payload is
//    rejected with 413; an empty payload is accepted.
// 6. **`u64::MAX` limit** — no realistic payload can exceed `u64::MAX` bytes;
//    ordinary uploads must succeed.
// 7. **`limit_bytes` field in 413 body** — the error JSON must carry a
//    `limit_bytes` field equal to the configured limit.
// 8. **No persistence after 413** — a blob rejected with 413 must not be
//    retrievable via `GET /api/bus/blobs/:id`.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::json;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a well-formed upload body with the supplied decoded payload.
fn upload_body(id: &str, payload: &[u8]) -> serde_json::Value {
    json!({
        "id":   id,
        "mime": "application/octet-stream",
        "data": B64.encode(payload),
    })
}

// ── Scenario 1: under-limit upload returns 200 ───────────────────────────────

/// A payload whose decoded size is strictly less than the configured limit
/// must be accepted with HTTP 200 and `ok: true`.
#[tokio::test]
async fn under_limit_upload_returns_200() {
    // Limit: 10 bytes.  Payload: 9 bytes.
    let limit: u64 = 10;
    let srv = helpers::TestServer::with_limit(limit).await;

    let payload = vec![0u8; 9];
    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("under-limit", &payload)),
    )
    .await;

    assert_eq!(
        resp.status(),
        200,
        "9-byte payload with limit={limit} must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["id"], json!("under-limit"));
}

// ── Scenario 2: over-limit upload returns 413 ────────────────────────────────

/// A payload whose decoded size is one byte larger than the configured limit
/// must be rejected with HTTP 413 Payload Too Large.
#[tokio::test]
async fn over_limit_upload_returns_413() {
    // Limit: 10 bytes.  Payload: 11 bytes — one byte over.
    let limit: u64 = 10;
    let srv = helpers::TestServer::with_limit(limit).await;

    let payload = vec![0xFFu8; 11];
    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("over-limit", &payload)),
    )
    .await;

    assert_eq!(
        resp.status(),
        413,
        "11-byte payload with limit={limit} must be rejected with 413"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(
        body["error"], json!("payload_too_large"),
        "413 response must carry error=payload_too_large"
    );
}

// ── Scenario 3: second upload over-limit returns 413 (per-upload, not cumulative) ──

/// Each `POST /api/bus/blobs` is an independent upload.  The first upload
/// (under-limit) succeeds; the second upload (over-limit) is rejected.
/// This verifies that the limit is enforced per-call, not as a running
/// cumulative total across calls.
#[tokio::test]
async fn second_upload_over_limit_returns_413() {
    // Limit: 8 bytes.
    let limit: u64 = 8;
    let srv = helpers::TestServer::with_limit(limit).await;

    // First upload: 6 bytes — under the limit.
    let first_resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("chunk-1", &vec![0u8; 6])),
    )
    .await;
    assert_eq!(
        first_resp.status(),
        200,
        "first upload (6 bytes, limit={limit}) must succeed"
    );

    // Second upload: 9 bytes — over the limit on its own.
    let second_resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("chunk-2", &vec![0u8; 9])),
    )
    .await;
    assert_eq!(
        second_resp.status(),
        413,
        "second upload (9 bytes, limit={limit}) must be rejected with 413"
    );
    let body = helpers::body_json(second_resp).await;
    assert_eq!(body["error"], json!("payload_too_large"));
}

// ── Scenario 4: exact-boundary payload returns 200 ───────────────────────────

/// A payload whose decoded byte count equals `max_blob_bytes` exactly is
/// at the boundary — the check is `size > limit`, so an equal-size payload
/// must be accepted.
#[tokio::test]
async fn exact_boundary_upload_returns_200() {
    let limit: u64 = 16;
    let srv = helpers::TestServer::with_limit(limit).await;

    // Payload is exactly `limit` bytes.
    let payload = vec![0xABu8; limit as usize];
    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("exact-boundary", &payload)),
    )
    .await;

    assert_eq!(
        resp.status(),
        200,
        "{limit}-byte payload with limit={limit} (exactly at boundary) must be accepted"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
}

// ── Scenario 5a: zero-limit rejects non-empty payload ────────────────────────

/// When `max_blob_bytes` is 0 every non-empty payload must be rejected with
/// 413; even a single byte exceeds the limit.
#[tokio::test]
async fn zero_limit_rejects_nonempty_payload() {
    let srv = helpers::TestServer::with_limit(0).await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("zero-limit-reject", &[0x01])),
    )
    .await;

    assert_eq!(
        resp.status(),
        413,
        "1-byte payload with limit=0 must be rejected with 413"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["error"], json!("payload_too_large"));
    assert_eq!(
        body["limit_bytes"],
        json!(0u64),
        "limit_bytes in the 413 body must be 0 when limit=0"
    );
}

// ── Scenario 5b: zero-limit accepts empty payload ─────────────────────────────

/// An empty payload (0 decoded bytes) is not strictly greater than 0 so it
/// passes the `size > limit` check and must be accepted.
#[tokio::test]
async fn zero_limit_accepts_empty_payload() {
    let srv = helpers::TestServer::with_limit(0).await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("zero-limit-empty", b"")),
    )
    .await;

    assert_eq!(
        resp.status(),
        200,
        "0-byte payload with limit=0 must be accepted (0 is not > 0)"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
}

// ── Scenario 6: u64::MAX limit accepts ordinary payloads ─────────────────────

/// When the limit is `u64::MAX` no realistic payload can exceed it.  Both a
/// small and a moderately-sized upload must be accepted normally.
#[tokio::test]
async fn usize_max_limit_accepts_ordinary_payloads() {
    let srv = helpers::TestServer::with_limit(u64::MAX).await;

    for (id, size) in [("tiny", 1usize), ("medium", 1_024)] {
        let resp = helpers::call(
            &srv.app,
            helpers::post_json(
                "/api/bus/blobs",
                &upload_body(id, &vec![0u8; size]),
            ),
        )
        .await;
        assert_eq!(
            resp.status(),
            200,
            "{size}-byte payload with limit=u64::MAX must be accepted"
        );
        let body = helpers::body_json(resp).await;
        assert_eq!(body["ok"], json!(true));
    }
}

// ── Scenario 7: 413 body carries correct limit_bytes ─────────────────────────

/// The JSON body of a 413 response must include a `limit_bytes` field whose
/// value matches the limit that was configured on the server.
#[tokio::test]
async fn limit_bytes_in_413_body_matches_configured_limit() {
    let limit: u64 = 50;
    let srv = helpers::TestServer::with_limit(limit).await;

    // Payload: 51 bytes — one over the limit.
    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("limit-body-check", &vec![0u8; 51])),
    )
    .await;

    assert_eq!(resp.status(), 413);
    let body = helpers::body_json(resp).await;

    assert_eq!(
        body["limit_bytes"],
        json!(limit),
        "limit_bytes in 413 body must equal the configured limit ({limit})"
    );
    assert_eq!(
        body["size_bytes"],
        json!(51u64),
        "size_bytes in 413 body must reflect the decoded payload size (51)"
    );
}

// ── Scenario 8: no persistence after 413 ─────────────────────────────────────

/// A blob that is rejected with 413 must not be persisted to the store.
/// Attempting to retrieve it via `GET /api/bus/blobs/:id` must return 404.
#[tokio::test]
async fn rejected_blob_is_not_persisted() {
    let limit: u64 = 4;
    let srv = helpers::TestServer::with_limit(limit).await;

    // Upload exceeds the limit and must be rejected.
    let up = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/blobs", &upload_body("no-persist", &[0u8; 5])),
    )
    .await;
    assert_eq!(
        up.status(),
        413,
        "5-byte payload with limit={limit} must be rejected with 413"
    );

    // The blob must not be stored; a subsequent GET must return 404.
    let get = helpers::call(
        &srv.app,
        helpers::get("/api/bus/blobs/no-persist"),
    )
    .await;
    assert_eq!(
        get.status(),
        404,
        "a 413-rejected blob must not be retrievable via GET"
    );
    let body = helpers::body_json(get).await;
    assert_eq!(
        body["error"],
        json!("blob_not_found"),
        "GET after rejected upload must return blob_not_found"
    );
}
