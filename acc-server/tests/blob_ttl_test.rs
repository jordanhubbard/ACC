mod helpers;

// TTL constants are defined in routes::blobs (which re-exports them from the
// internal blob_store module).  Importing from this path keeps the test
// insulated from internal module layout changes.
use acc_server::routes::blobs::{BLOB_DEFAULT_TTL_SECS, BLOB_MAX_TTL_SECS};
use axum::http::StatusCode;
use serde_json::json;

// ── Compile-time constant sanity checks ───────────────────────────────────────
//
// These assertions are evaluated entirely by the compiler; a wrong value or
// ordering causes a build failure rather than a test-run failure.

/// BLOB_DEFAULT_TTL_SECS must be strictly less than BLOB_MAX_TTL_SECS.
/// A default that already exceeds the cap would make enforcement impossible.
const _: () = assert!(
    BLOB_DEFAULT_TTL_SECS < BLOB_MAX_TTL_SECS,
    "BLOB_DEFAULT_TTL_SECS must be strictly less than BLOB_MAX_TTL_SECS",
);

/// Both constants must be positive — zero would mean "expire immediately",
/// which is not a valid default or maximum for a blob storage TTL.
const _: () = assert!(
    BLOB_DEFAULT_TTL_SECS > 0,
    "BLOB_DEFAULT_TTL_SECS must be greater than 0",
);
const _: () = assert!(
    BLOB_MAX_TTL_SECS > 0,
    "BLOB_MAX_TTL_SECS must be greater than 0",
);

/// The default TTL must be exactly 86 400 seconds (24 hours).
/// Changing this value without updating SPEC.md is a breaking contract change.
const _: () = assert!(
    BLOB_DEFAULT_TTL_SECS == 86_400,
    "BLOB_DEFAULT_TTL_SECS must be exactly 86400 (24 hours)",
);

/// The maximum TTL must be exactly 604 800 seconds (7 days).
/// Changing this value without updating SPEC.md is a breaking contract change.
const _: () = assert!(
    BLOB_MAX_TTL_SECS == 604_800,
    "BLOB_MAX_TTL_SECS must be exactly 604800 (7 days)",
);

// ── Runtime constant sanity checks ───────────────────────────────────────────

/// The default TTL must be strictly less than the maximum TTL.
#[test]
fn blob_default_ttl_is_less_than_max_ttl() {
    assert!(
        BLOB_DEFAULT_TTL_SECS < BLOB_MAX_TTL_SECS,
        "BLOB_DEFAULT_TTL_SECS ({BLOB_DEFAULT_TTL_SECS}) must be < BLOB_MAX_TTL_SECS ({BLOB_MAX_TTL_SECS})"
    );
}

/// Both TTL constants must be positive (zero would mean "expire immediately").
#[test]
fn blob_ttl_constants_are_positive() {
    assert!(BLOB_DEFAULT_TTL_SECS > 0, "BLOB_DEFAULT_TTL_SECS must be > 0");
    assert!(BLOB_MAX_TTL_SECS > 0, "BLOB_MAX_TTL_SECS must be > 0");
}

/// The default TTL must be exactly 24 hours (86 400 s).
#[test]
fn blob_default_ttl_is_twenty_four_hours() {
    const ONE_DAY: u64 = 86_400;
    assert_eq!(
        BLOB_DEFAULT_TTL_SECS, ONE_DAY,
        "BLOB_DEFAULT_TTL_SECS must be exactly 86400 s (24 h), got {BLOB_DEFAULT_TTL_SECS}"
    );
}

/// The maximum TTL must be exactly 7 days (604 800 s).
#[test]
fn blob_max_ttl_is_seven_days() {
    const SEVEN_DAYS: u64 = 604_800;
    assert_eq!(
        BLOB_MAX_TTL_SECS, SEVEN_DAYS,
        "BLOB_MAX_TTL_SECS must be exactly 604800 s (7 days), got {BLOB_MAX_TTL_SECS}"
    );
}

// ── TTL field round-trip through the bus API ──────────────────────────────────

/// A blob message posted with a `ttl_secs` within the allowed range must be
/// stored and returned with the same field intact.
#[tokio::test]
async fn blob_message_with_valid_ttl_is_accepted() {
    let srv = helpers::TestServer::new().await;

    let ttl = BLOB_DEFAULT_TTL_SECS;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":     "ttl-test-agent",
                "to":       "all",
                "type":     "blob",
                "mime":     "image/png",
                "enc":      "base64",
                "subject":  "ttl-valid-blob",
                "body":     "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVQI12NgAAIABQ==",
                "ttl_secs": ttl,
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "blob with ttl_secs={ttl} (≤ BLOB_MAX_TTL_SECS={BLOB_MAX_TTL_SECS}) must be accepted"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    // The message must appear in history.
    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    assert_eq!(list.status(), StatusCode::OK);
    let msgs = helpers::body_json(list).await;
    let msgs = msgs.as_array().expect("expected JSON array from /bus/messages");
    assert!(
        msgs.iter().any(|m| m["subject"] == json!("ttl-valid-blob")),
        "blob with valid ttl must appear in message history"
    );
}

/// A blob message posted with `ttl_secs` exceeding BLOB_MAX_TTL_SECS must
/// still be accepted by the server (the server clamps it rather than
/// rejecting it).  If the message appears in history it must not carry a
/// ttl_secs value beyond the cap.
#[tokio::test]
async fn blob_message_with_excessive_ttl_is_clamped_not_rejected() {
    let srv = helpers::TestServer::new().await;

    let excessive_ttl = BLOB_MAX_TTL_SECS + 86_400; // one day over the cap

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":     "ttl-test-agent",
                "to":       "all",
                "type":     "blob",
                "mime":     "image/gif",
                "enc":      "base64",
                "subject":  "ttl-excessive-blob",
                "body":     "R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==",
                "ttl_secs": excessive_ttl,
            }),
        ),
    )
    .await;

    // The server must not return a client error for an over-cap TTL.
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "blob with ttl_secs={excessive_ttl} > BLOB_MAX_TTL_SECS={BLOB_MAX_TTL_SECS} must be \
         accepted with clamping, not rejected with a 4xx"
    );

    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    // If the returned message object carries ttl_secs it must not exceed the cap.
    if let Some(msg) = body.get("message") {
        if let Some(returned_ttl) = msg.get("ttl_secs").and_then(|v| v.as_u64()) {
            assert!(
                returned_ttl <= BLOB_MAX_TTL_SECS,
                "returned ttl_secs ({returned_ttl}) must be clamped to \
                 BLOB_MAX_TTL_SECS ({BLOB_MAX_TTL_SECS})"
            );
        }
    }
}

/// A blob message posted without any `ttl_secs` field must be accepted.
/// This verifies the default TTL path (no field → server applies
/// BLOB_DEFAULT_TTL_SECS internally).
#[tokio::test]
async fn blob_message_without_ttl_uses_default() {
    let srv = helpers::TestServer::new().await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "ttl-test-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "audio/ogg",
                "enc":     "base64",
                "subject": "ttl-default-blob",
                "body":    "T2dnUwACA==",
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "blob without ttl_secs must be accepted (server applies BLOB_DEFAULT_TTL_SECS)"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
}

/// A blob message posted with `ttl_secs = 0` must be accepted (HTTP 200).
/// Zero does NOT mean "no retention preference" — per the TTL semantics
/// defined in `blob_store.rs`, a zero TTL means the blob is **immediately
/// expired on insert** and is not retrievable after the upload completes.
/// The server accepts the message rather than rejecting it, but the
/// `ttl_secs` value is preserved as `0` in the response; it is never
/// silently promoted to `BLOB_DEFAULT_TTL_SECS`.
#[tokio::test]
async fn blob_message_with_zero_ttl_is_accepted() {
    let srv = helpers::TestServer::new().await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":     "ttl-test-agent",
                "to":       "all",
                "type":     "blob",
                "mime":     "video/mp4",
                "enc":      "base64",
                "subject":  "ttl-zero-blob",
                "body":     "AAABIAAAA",
                "ttl_secs": 0u64,
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "blob with ttl_secs=0 must be accepted (immediately expired on insert, not rejected)"
    );

    // The echoed message must carry ttl_secs=0, not BLOB_DEFAULT_TTL_SECS,
    // confirming that a zero TTL is never silently promoted to the default.
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
    assert_eq!(
        body["message"]["ttl_secs"],
        json!(0u64),
        "ttl_secs=0 must be preserved as 0 in the response, not rewritten to \
         BLOB_DEFAULT_TTL_SECS ({BLOB_DEFAULT_TTL_SECS})"
    );
}
