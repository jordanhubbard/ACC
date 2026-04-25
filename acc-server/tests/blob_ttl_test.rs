//! Integration tests for blob TTL (time-to-live) behaviour on
//! POST /api/bus/blobs/upload and GET /api/bus/blobs.
//!
//! Scenarios covered:
//!   1.  BLOB_DEFAULT_TTL_SECS constant is 86 400 (24 h).
//!   2.  BLOB_MAX_TTL_SECS constant is 2 592 000 (30 days).
//!   3.  Upload with no ttl_seconds echoes the default (86 400) in the response.
//!   4.  Upload with explicit ttl_seconds echoes that value in the response.
//!   5.  Upload response includes a non-null `expires_at` RFC-3339 timestamp.
//!   6.  `expires_at` is approximately `uploaded_at + ttl_seconds`.
//!   7.  Upload with ttl_seconds > BLOB_MAX_TTL_SECS returns 422 ttl_seconds_too_large.
//!   8.  The 422 response includes `max_ttl_seconds` in the body.
//!   9.  Upload with ttl_seconds == BLOB_MAX_TTL_SECS is accepted (boundary).
//!  10.  Upload with ttl_seconds == 1 is accepted (minimum non-zero value).
//!  11.  Blob list default (no query params) hides expired blobs.
//!  12.  Blob list ?include_expired=true surfaces expired blobs.
//!  13.  Blob list entries include a `seconds_remaining` field.
//!  14.  `seconds_remaining` is 0 for expired blobs returned via include_expired.
//!  15.  Blob list ?mime_prefix=image/ returns only image/* blobs.
//!  16.  Blob list ?mime_prefix=text/ returns only text/* blobs.
//!  17.  Blob list ?mime_prefix=audio/ returns no results when none uploaded.
//!  18.  Blob list response includes a top-level `count` field.
//!  19.  `count` equals the length of the `blobs` array.
//!  20.  mime_prefix + include_expired can be combined.

mod helpers;

use acc_server::routes::blobs::{BLOB_DEFAULT_TTL_SECS, BLOB_MAX_TTL_SECS};
use serde_json::json;

// ── internal helpers ──────────────────────────────────────────────────────────

/// Upload a single-chunk text blob.  Returns (HTTP status, parsed JSON body).
async fn upload(
    srv: &helpers::TestServer,
    mime_type: &str,
    data: &str,
    ttl_seconds: Option<u64>,
) -> (u16, serde_json::Value) {
    let mut body = json!({
        "mime_type":    mime_type,
        "enc":          "none",
        "data":         data,
        "total_chunks": 1,
    });
    if let Some(ttl) = ttl_seconds {
        body["ttl_seconds"] = json!(ttl);
    }
    let resp = helpers::call(&srv.app, helpers::post_json("/api/bus/blobs/upload", &body)).await;
    let status = resp.status().as_u16();
    (status, helpers::body_json(resp).await)
}

/// GET /api/bus/blobs with optional query string (e.g. "include_expired=true").
async fn list(srv: &helpers::TestServer, qs: &str) -> serde_json::Value {
    let path = if qs.is_empty() {
        "/api/bus/blobs".to_string()
    } else {
        format!("/api/bus/blobs?{qs}")
    };
    helpers::body_json(helpers::call(&srv.app, helpers::get(&path)).await).await
}

// ── 1. BLOB_DEFAULT_TTL_SECS == 86 400 ───────────────────────────────────────

#[tokio::test]
async fn blob_default_ttl_secs_constant_is_86400() {
    assert_eq!(
        BLOB_DEFAULT_TTL_SECS, 86_400,
        "BLOB_DEFAULT_TTL_SECS must be 86 400 (24 hours)"
    );
}

// ── 2. BLOB_MAX_TTL_SECS == 2 592 000 ────────────────────────────────────────

#[tokio::test]
async fn blob_max_ttl_secs_constant_is_30_days() {
    assert_eq!(
        BLOB_MAX_TTL_SECS,
        30 * 24 * 3_600,
        "BLOB_MAX_TTL_SECS must be 2 592 000 (30 days)"
    );
}

// ── 3. No ttl_seconds → default echoed in response ───────────────────────────

#[tokio::test]
async fn upload_without_ttl_seconds_echoes_default_in_response() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = upload(&srv, "text/plain", "default ttl test", None).await;
    assert_eq!(status, 200, "upload failed: {body}");
    assert_eq!(
        body["ttl_seconds"].as_u64().unwrap(),
        BLOB_DEFAULT_TTL_SECS,
        "omitting ttl_seconds must echo BLOB_DEFAULT_TTL_SECS ({BLOB_DEFAULT_TTL_SECS}) in the response"
    );
}

// ── 4. Explicit ttl_seconds echoed back ──────────────────────────────────────

#[tokio::test]
async fn upload_with_explicit_ttl_seconds_echoes_that_value() {
    let srv = helpers::TestServer::new().await;
    let custom_ttl = 3_600u64; // 1 hour
    let (status, body) = upload(&srv, "text/plain", "explicit ttl", Some(custom_ttl)).await;
    assert_eq!(status, 200, "upload failed: {body}");
    assert_eq!(
        body["ttl_seconds"].as_u64().unwrap(),
        custom_ttl,
        "response ttl_seconds must match the value supplied by the caller"
    );
}

// ── 5. Upload response contains expires_at ───────────────────────────────────

#[tokio::test]
async fn upload_response_contains_expires_at() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = upload(&srv, "text/plain", "expires_at test", Some(7_200)).await;
    assert_eq!(status, 200, "upload failed: {body}");
    let expires_at = body["expires_at"].as_str();
    assert!(
        expires_at.is_some(),
        "upload response must include a non-null expires_at field; got: {body}"
    );
    // Must be parseable as RFC-3339.
    chrono::DateTime::parse_from_rfc3339(expires_at.unwrap())
        .expect("expires_at must be a valid RFC-3339 timestamp");
}

// ── 6. expires_at ≈ now + ttl_seconds ────────────────────────────────────────

#[tokio::test]
async fn expires_at_is_approximately_now_plus_ttl_seconds() {
    let srv = helpers::TestServer::new().await;
    let ttl = 3_600u64;
    let before = chrono::Utc::now();
    let (status, body) = upload(&srv, "text/plain", "timestamp test", Some(ttl)).await;
    assert_eq!(status, 200, "upload failed: {body}");
    let after = chrono::Utc::now();

    let expires_at_str = body["expires_at"].as_str().expect("expires_at must be present");
    let expires_at = chrono::DateTime::parse_from_rfc3339(expires_at_str)
        .expect("expires_at must parse as RFC-3339")
        .with_timezone(&chrono::Utc);

    // expires_at must be between (before + ttl - 2s) and (after + ttl + 2s)
    let lower = before + chrono::Duration::seconds(ttl as i64 - 2);
    let upper = after  + chrono::Duration::seconds(ttl as i64 + 2);
    assert!(
        expires_at >= lower && expires_at <= upper,
        "expires_at {expires_at} is not within 2 s of now + {ttl}s (expected [{lower}, {upper}])"
    );
}

// ── 7. ttl_seconds > BLOB_MAX_TTL_SECS → 422 ttl_seconds_too_large ───────────

#[tokio::test]
async fn upload_with_ttl_above_max_returns_422() {
    let srv = helpers::TestServer::new().await;
    let over_max = BLOB_MAX_TTL_SECS + 1;
    let (status, body) = upload(&srv, "text/plain", "too long", Some(over_max)).await;
    assert_eq!(
        status, 422,
        "ttl_seconds > BLOB_MAX_TTL_SECS must return 422; got {status}: {body}"
    );
    assert_eq!(
        body["error"].as_str().unwrap(),
        "ttl_seconds_too_large",
        "error field must be 'ttl_seconds_too_large'"
    );
}

// ── 8. 422 body includes max_ttl_seconds ─────────────────────────────────────

#[tokio::test]
async fn ttl_too_large_response_includes_max_ttl_seconds() {
    let srv = helpers::TestServer::new().await;
    let (_, body) = upload(&srv, "text/plain", "over max", Some(BLOB_MAX_TTL_SECS + 1)).await;
    assert_eq!(
        body["max_ttl_seconds"].as_u64().unwrap(),
        BLOB_MAX_TTL_SECS,
        "422 response must include max_ttl_seconds == BLOB_MAX_TTL_SECS"
    );
}

// ── 9. ttl_seconds == BLOB_MAX_TTL_SECS is accepted (boundary) ───────────────

#[tokio::test]
async fn upload_with_ttl_equal_to_max_is_accepted() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = upload(&srv, "text/plain", "at boundary", Some(BLOB_MAX_TTL_SECS)).await;
    assert_eq!(
        status, 200,
        "ttl_seconds == BLOB_MAX_TTL_SECS must be accepted; got {status}: {body}"
    );
    assert_eq!(
        body["ttl_seconds"].as_u64().unwrap(),
        BLOB_MAX_TTL_SECS
    );
}

// ── 10. ttl_seconds == 1 is accepted ─────────────────────────────────────────

#[tokio::test]
async fn upload_with_ttl_of_one_second_is_accepted() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = upload(&srv, "text/plain", "one second ttl", Some(1)).await;
    assert_eq!(
        status, 200,
        "ttl_seconds=1 must be accepted; got {status}: {body}"
    );
    assert_eq!(body["ttl_seconds"].as_u64().unwrap(), 1u64);
}

// ── 11. Default list hides expired blobs ─────────────────────────────────────

#[tokio::test]
async fn default_list_hides_expired_blobs() {
    let srv = helpers::TestServer::new().await;

    // Upload a blob that will expire in 1 second.
    let (_, b) = upload(&srv, "text/plain", "ephemeral", Some(1)).await;
    let blob_id = b["blob_id"].as_str().unwrap().to_string();

    // Wait for it to expire.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let resp = list(&srv, "").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");
    let found = blobs.iter().any(|b| b["id"].as_str() == Some(&blob_id));
    assert!(
        !found,
        "expired blob {blob_id} must NOT appear in the default list"
    );
}

// ── 12. include_expired=true surfaces expired blobs ──────────────────────────

#[tokio::test]
async fn include_expired_true_surfaces_expired_blobs() {
    let srv = helpers::TestServer::new().await;

    let (_, b) = upload(&srv, "text/plain", "show me when expired", Some(1)).await;
    let blob_id = b["blob_id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let resp = list(&srv, "include_expired=true").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");
    let found = blobs.iter().any(|b| b["id"].as_str() == Some(&blob_id));
    assert!(
        found,
        "expired blob {blob_id} must appear when include_expired=true"
    );
}

// ── 13. List entries include seconds_remaining ───────────────────────────────

#[tokio::test]
async fn list_entries_include_seconds_remaining_field() {
    let srv = helpers::TestServer::new().await;

    let (_, b) = upload(&srv, "text/plain", "seconds remaining test", Some(3_600)).await;
    let blob_id = b["blob_id"].as_str().unwrap().to_string();

    let resp = list(&srv, "").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");
    let entry = blobs
        .iter()
        .find(|b| b["id"].as_str() == Some(&blob_id))
        .expect("blob must appear in list");

    assert!(
        entry.get("seconds_remaining").is_some(),
        "each list entry must include a seconds_remaining field; got: {entry}"
    );
}

// ── 14. seconds_remaining is 0 for expired blobs ─────────────────────────────

#[tokio::test]
async fn seconds_remaining_is_zero_for_expired_blobs() {
    let srv = helpers::TestServer::new().await;

    let (_, b) = upload(&srv, "text/plain", "zero remaining", Some(1)).await;
    let blob_id = b["blob_id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let resp = list(&srv, "include_expired=true").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");
    let entry = blobs
        .iter()
        .find(|b| b["id"].as_str() == Some(&blob_id))
        .expect("expired blob must appear with include_expired=true");

    let secs = entry["seconds_remaining"].as_i64()
        .expect("seconds_remaining must be a number");
    assert_eq!(
        secs, 0,
        "seconds_remaining must be 0 for an already-expired blob; got {secs}"
    );
}

// ── 15. mime_prefix=image/ returns only image/* blobs ────────────────────────

#[tokio::test]
async fn mime_prefix_image_returns_only_image_blobs() {
    let srv = helpers::TestServer::new().await;

    // Upload one image and one text blob.
    let (_, ib) = upload(&srv, "image/svg+xml", "<svg/>", None).await;
    let image_id = ib["blob_id"].as_str().unwrap().to_string();

    let (_, tb) = upload(&srv, "text/plain", "hello", None).await;
    let text_id = tb["blob_id"].as_str().unwrap().to_string();

    let resp = list(&srv, "mime_prefix=image%2F").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");

    let found_image = blobs.iter().any(|b| b["id"].as_str() == Some(&image_id));
    let found_text  = blobs.iter().any(|b| b["id"].as_str() == Some(&text_id));

    assert!(found_image, "image blob {image_id} must appear with mime_prefix=image/");
    assert!(!found_text, "text blob {text_id} must NOT appear with mime_prefix=image/");

    // Every returned blob must have a mime_type starting with "image/".
    for b in blobs {
        let mime = b["mime_type"].as_str().unwrap_or("");
        assert!(
            mime.starts_with("image/"),
            "mime_prefix=image/ filter returned a non-image blob: mime={mime}"
        );
    }
}

// ── 16. mime_prefix=text/ returns only text/* blobs ──────────────────────────

#[tokio::test]
async fn mime_prefix_text_returns_only_text_blobs() {
    let srv = helpers::TestServer::new().await;

    let (_, tb) = upload(&srv, "text/markdown", "# heading", None).await;
    let text_id = tb["blob_id"].as_str().unwrap().to_string();

    let (_, jb) = upload(&srv, "application/json", "{}", None).await;
    let json_id = jb["blob_id"].as_str().unwrap().to_string();

    let resp = list(&srv, "mime_prefix=text%2F").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");

    let found_text = blobs.iter().any(|b| b["id"].as_str() == Some(&text_id));
    let found_json = blobs.iter().any(|b| b["id"].as_str() == Some(&json_id));

    assert!(found_text, "text/markdown blob must appear with mime_prefix=text/");
    assert!(!found_json, "application/json blob must NOT appear with mime_prefix=text/");

    for b in blobs {
        let mime = b["mime_type"].as_str().unwrap_or("");
        assert!(
            mime.starts_with("text/"),
            "mime_prefix=text/ filter returned a non-text blob: mime={mime}"
        );
    }
}

// ── 17. mime_prefix=audio/ returns empty when none uploaded ──────────────────

#[tokio::test]
async fn mime_prefix_with_no_matching_blobs_returns_empty_array() {
    let srv = helpers::TestServer::new().await;

    // Upload only text blobs — no audio.
    upload(&srv, "text/plain", "no audio here", None).await;

    let resp = list(&srv, "mime_prefix=audio%2F").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");
    assert!(
        blobs.is_empty(),
        "mime_prefix=audio/ must return an empty blobs array when no audio blobs exist; got {blobs:?}"
    );
}

// ── 18. List response includes top-level count ───────────────────────────────

#[tokio::test]
async fn list_response_includes_count_field() {
    let srv = helpers::TestServer::new().await;
    upload(&srv, "text/plain", "count test", None).await;

    let resp = list(&srv, "").await;
    assert!(
        resp.get("count").is_some(),
        "list response must include a top-level 'count' field; got: {resp}"
    );
}

// ── 19. count equals blobs array length ──────────────────────────────────────

#[tokio::test]
async fn list_count_equals_blobs_array_length() {
    let srv = helpers::TestServer::new().await;
    upload(&srv, "text/plain",       "first",  None).await;
    upload(&srv, "application/json", "{}",     None).await;
    upload(&srv, "text/markdown",    "# head", None).await;

    let resp = list(&srv, "").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");
    let count = resp["count"].as_u64().expect("count must be a number");

    assert_eq!(
        count,
        blobs.len() as u64,
        "count ({count}) must equal the length of the blobs array ({})",
        blobs.len()
    );
}

// ── 20. mime_prefix + include_expired can be combined ────────────────────────

#[tokio::test]
async fn mime_prefix_and_include_expired_can_be_combined() {
    let srv = helpers::TestServer::new().await;

    // Upload a text blob that expires in 1 second and an image blob that lives.
    let (_, tb) = upload(&srv, "text/plain", "soon gone", Some(1)).await;
    let text_id = tb["blob_id"].as_str().unwrap().to_string();

    let (_, ib) = upload(&srv, "image/svg+xml", "<svg/>", None).await;
    let image_id = ib["blob_id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Ask for expired text blobs.
    let resp = list(&srv, "mime_prefix=text%2F&include_expired=true").await;
    let blobs = resp["blobs"].as_array().expect("blobs must be an array");

    let found_text  = blobs.iter().any(|b| b["id"].as_str() == Some(&text_id));
    let found_image = blobs.iter().any(|b| b["id"].as_str() == Some(&image_id));

    assert!(
        found_text,
        "expired text blob {text_id} must appear when mime_prefix=text/&include_expired=true"
    );
    assert!(
        !found_image,
        "image blob {image_id} must NOT appear with mime_prefix=text/ even with include_expired=true"
    );

    // count must still match the returned array length.
    let count = resp["count"].as_u64().expect("count must be present");
    assert_eq!(count, blobs.len() as u64, "count must equal blobs array length");
}
