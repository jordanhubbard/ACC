//! Integration tests for the `max_blob_bytes` size-limit enforcement on
//! POST /api/bus/blobs/upload.
//!
//! These tests exercise the per-blob accumulated-size guard that is checked
//! after each chunk is decoded.  They use a small custom limit (well below
//! 100 MiB) so that the test suite can run quickly in CI without allocating
//! large buffers.
//!
//! Test plan (8 tests):
//!
//!  1.  Single-chunk payload **exactly at** the limit → 200 OK, blob complete.
//!  2.  Single-chunk payload one byte **above** the limit → 413 Payload Too Large
//!      with `{"error":"blob_size_limit_exceeded"}`.
//!  3.  Single-chunk payload **one byte below** the limit → 200 OK, blob complete.
//!  4.  Multi-chunk upload whose **combined** size exceeds the limit → 413 on the
//!      chunk that pushes the total over, `{"error":"blob_size_limit_exceeded"}`.
//!  5.  Multi-chunk upload whose combined size equals the limit → all chunks
//!      accepted, blob marked complete.
//!  6.  After a 413 the in-progress session is cleaned up: a follow-up chunk
//!      referencing the same blob_id returns 422 `orphan_chunk` (session gone).
//!  7.  A fresh upload to the *same* blob_id after a rejected session succeeds
//!      when it fits within the limit.
//!  8.  The default limit (100 MiB) is honoured when no override is set: a
//!      1-byte payload on a standard test server succeeds.

mod helpers;

use acc_server::routes::blobs::b64_encode;
use serde_json::{json, Value};

// ── constants ─────────────────────────────────────────────────────────────────

/// Byte limit used by every size-limit test server in this file.
/// 1 KiB — small enough to exercise over- and at-limit cases cheaply.
const TEST_LIMIT: u64 = 1024;

// ── shared upload helper ──────────────────────────────────────────────────────

/// Upload one chunk of `data` to POST /api/bus/blobs/upload and return
/// `(HTTP status code, parsed JSON body)`.
///
/// * `blob_id` — when `Some`, the supplied blob_id is forwarded so that
///   subsequent chunks can reference the same in-progress session.
/// * `chunk_index` / `total_chunks` — standard chunking fields.
async fn upload_chunk(
    srv: &helpers::TestServer,
    blob_id: Option<&str>,
    data: &[u8],
    chunk_index: u64,
    total_chunks: u64,
) -> (u16, Value) {
    let mut body = json!({
        "mime_type":    "application/octet-stream",
        "enc":          "base64",
        "data":         b64_encode(data),
        "chunk_index":  chunk_index,
        "total_chunks": total_chunks,
    });
    if let Some(id) = blob_id {
        body["blob_id"] = json!(id);
    }
    let resp =
        helpers::call(&srv.app, helpers::post_json("/api/bus/blobs/upload", &body)).await;
    let status = resp.status().as_u16();
    (status, helpers::body_json(resp).await)
}

/// Convenience wrapper: upload `size` zero-bytes as a complete single-chunk blob.
async fn upload_single(srv: &helpers::TestServer, size: usize) -> (u16, Value) {
    let data = vec![0xBBu8; size];
    upload_chunk(srv, None, &data, 0, 1).await
}

// ── 1. Exactly at the limit → 200 ────────────────────────────────────────────

#[tokio::test]
async fn payload_exactly_at_limit_is_accepted() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;
    let at_limit = TEST_LIMIT as usize;

    let (status, body) = upload_single(&srv, at_limit).await;

    assert_eq!(
        status, 200,
        "payload exactly at limit ({TEST_LIMIT} bytes) must return 200; got {status}: {body}"
    );
    assert_eq!(
        body["complete"], json!(true),
        "blob at exactly the limit must be marked complete"
    );
    assert_eq!(
        body["ok"], json!(true),
        "response must include ok:true"
    );
    assert_eq!(
        body["chunks_received"].as_u64().unwrap(),
        1u64,
        "single-chunk upload must show chunks_received = 1"
    );
}

// ── 2. One byte above the limit → 413 ────────────────────────────────────────

#[tokio::test]
async fn payload_one_byte_above_limit_is_rejected_with_413() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;
    let over = TEST_LIMIT as usize + 1;

    let (status, body) = upload_single(&srv, over).await;

    assert_eq!(
        status, 413,
        "payload {} bytes (limit {TEST_LIMIT}) must return 413; got {status}: {body}",
        over
    );
    assert_eq!(
        body["error"].as_str().unwrap(),
        "blob_size_limit_exceeded",
        "error field must be 'blob_size_limit_exceeded'; got {body}"
    );
}

// ── 3. One byte below the limit → 200 ────────────────────────────────────────

#[tokio::test]
async fn payload_one_byte_below_limit_is_accepted() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;
    let under = TEST_LIMIT as usize - 1;

    let (status, body) = upload_single(&srv, under).await;

    assert_eq!(
        status, 200,
        "payload one byte under limit ({under} bytes) must return 200; got {status}: {body}"
    );
    assert_eq!(
        body["complete"], json!(true),
        "blob one byte under the limit must be marked complete"
    );
    assert_eq!(
        body["ok"], json!(true),
        "response must include ok:true"
    );
}

// ── 4. Multi-chunk: combined size exceeds limit → 413 on the offending chunk ──

#[tokio::test]
async fn multi_chunk_combined_size_above_limit_returns_413() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;

    // Chunk 0: exactly half the limit — must be accepted.
    let half = TEST_LIMIT as usize / 2;
    let (s0, b0) = upload_chunk(&srv, None, &vec![0xAAu8; half], 0, 2).await;
    assert_eq!(
        s0, 200,
        "first chunk (half of limit) must succeed; got {s0}: {b0}"
    );
    let blob_id = b0["blob_id"]
        .as_str()
        .expect("blob_id must be present in chunk-0 response")
        .to_string();
    assert_eq!(
        b0["complete"], json!(false),
        "blob must not be complete after chunk 0 of 2"
    );

    // Chunk 1: half + 1 bytes — combined total = TEST_LIMIT + 1 > TEST_LIMIT.
    let (s1, b1) = upload_chunk(
        &srv,
        Some(&blob_id),
        &vec![0xBBu8; half + 1],
        1,
        2,
    )
    .await;
    assert_eq!(
        s1, 413,
        "second chunk that pushes combined total above limit must return 413; \
         got {s1}: {b1}"
    );
    assert_eq!(
        b1["error"].as_str().unwrap(),
        "blob_size_limit_exceeded",
        "error field must be 'blob_size_limit_exceeded'; got {b1}"
    );
}

// ── 5. Multi-chunk: combined size equals limit → all accepted ─────────────────

#[tokio::test]
async fn multi_chunk_combined_size_at_limit_is_accepted() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;

    // Split evenly: each chunk is exactly half the limit.
    // Combined total = TEST_LIMIT, which must not trigger the guard.
    let half = TEST_LIMIT as usize / 2;

    let (s0, b0) = upload_chunk(&srv, None, &vec![0xCCu8; half], 0, 2).await;
    assert_eq!(s0, 200, "first chunk must succeed; got {s0}: {b0}");
    assert_eq!(
        b0["complete"], json!(false),
        "blob must not be complete after chunk 0 of 2"
    );
    let blob_id = b0["blob_id"]
        .as_str()
        .expect("blob_id must be present after chunk 0")
        .to_string();

    let (s1, b1) = upload_chunk(
        &srv,
        Some(&blob_id),
        &vec![0xDDu8; half],
        1,
        2,
    )
    .await;
    assert_eq!(
        s1, 200,
        "second chunk bringing combined total to exactly {TEST_LIMIT} bytes must succeed; \
         got {s1}: {b1}"
    );
    assert_eq!(
        b1["complete"], json!(true),
        "blob at exactly the limit across two chunks must be marked complete"
    );
    assert_eq!(
        b1["chunks_received"].as_u64().unwrap(),
        2u64,
        "two chunks must be recorded"
    );
}

// ── 6. Session evicted after 413 → follow-up chunk becomes an orphan ──────────

#[tokio::test]
async fn session_evicted_after_413_follow_up_chunk_returns_orphan_error() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;

    // Chunk 0: half the limit — accepted, session created.
    let half = TEST_LIMIT as usize / 2;
    let (s0, b0) = upload_chunk(&srv, None, &vec![0x11u8; half], 0, 3).await;
    assert_eq!(s0, 200, "chunk 0 must succeed; got {s0}: {b0}");
    let blob_id = b0["blob_id"]
        .as_str()
        .expect("blob_id must be present after chunk 0")
        .to_string();

    // Chunk 1: a full TEST_LIMIT bytes — combined total = half + TEST_LIMIT > TEST_LIMIT,
    // so 413 is returned and the session is evicted from the store.
    let (s1, b1) = upload_chunk(
        &srv,
        Some(&blob_id),
        &vec![0x22u8; TEST_LIMIT as usize],
        1,
        3,
    )
    .await;
    assert_eq!(
        s1, 413,
        "overflow chunk must return 413; got {s1}: {b1}"
    );
    assert_eq!(
        b1["error"].as_str().unwrap(),
        "blob_size_limit_exceeded",
        "413 error field must be 'blob_size_limit_exceeded'; got {b1}"
    );

    // Chunk 2: session no longer exists → orphan_chunk (422).
    let (s2, b2) = upload_chunk(&srv, Some(&blob_id), &vec![0x33u8; 1], 2, 3).await;
    assert_eq!(
        s2, 422,
        "follow-up chunk after session eviction must return 422 orphan_chunk; \
         got {s2}: {b2}"
    );
    assert_eq!(
        b2["error"].as_str().unwrap(),
        "orphan_chunk",
        "error field after session eviction must be 'orphan_chunk'; got {b2}"
    );
}

// ── 7. Same blob_id reused in a fresh upload after a rejected session ──────────

#[tokio::test]
async fn same_blob_id_can_be_reused_after_rejected_session() {
    let srv = helpers::TestServer::with_limit(TEST_LIMIT).await;

    // Use a caller-supplied blob_id so we can refer to it explicitly in both
    // upload attempts.
    let explicit_id = "reuse-after-rejection-test-blob-id";

    // First attempt: single chunk that exceeds the limit → 413, session evicted.
    let (s0, b0) = upload_chunk(
        &srv,
        Some(explicit_id),
        &vec![0xAAu8; TEST_LIMIT as usize + 1],
        0,
        1,
    )
    .await;
    assert_eq!(
        s0, 413,
        "first (over-limit) attempt must return 413; got {s0}: {b0}"
    );
    assert_eq!(
        b0["error"].as_str().unwrap(),
        "blob_size_limit_exceeded",
        "first attempt error must be 'blob_size_limit_exceeded'; got {b0}"
    );

    // Second attempt with the same blob_id but a tiny, well-within-limit payload.
    // The first session was evicted, so a fresh session is started.
    let (s1, b1) = upload_chunk(
        &srv,
        Some(explicit_id),
        &vec![0xBBu8; 16],
        0,
        1,
    )
    .await;
    assert_eq!(
        s1, 200,
        "re-using blob_id after eviction with a small payload must succeed; \
         got {s1}: {b1}"
    );
    assert_eq!(
        b1["blob_id"].as_str().unwrap(),
        explicit_id,
        "server must echo back the supplied blob_id"
    );
    assert_eq!(
        b1["complete"], json!(true),
        "single-chunk re-upload must be complete"
    );
    assert_eq!(
        b1["ok"], json!(true),
        "response must include ok:true"
    );
}

// ── 8. Default 100 MiB limit: a 1-byte upload on a standard server succeeds ──

#[tokio::test]
async fn production_default_limit_allows_small_upload() {
    // Use the default TestServer (100 MiB limit rather than TEST_LIMIT).
    // A 1-byte upload must succeed, proving the default is not zero or
    // misconfigured.
    let srv = helpers::TestServer::new().await;

    let (status, body) = upload_single(&srv, 1).await;

    assert_eq!(
        status, 200,
        "1-byte upload must succeed on the default-limit server; \
         got {status}: {body}"
    );
    assert_eq!(
        body["complete"], json!(true),
        "1-byte single-chunk upload must be marked complete"
    );
    assert_eq!(
        body["ok"], json!(true),
        "response must include ok:true"
    );
}
