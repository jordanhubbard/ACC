mod helpers;

// Chunked blob upload tests.
//
// The bus API accepts blobs in a single POST to /api/bus/send.  "Chunked
// upload" here refers to the pattern where a large payload is split by the
// caller into multiple sequential messages — each carrying a chunk of
// base64-encoded data and metadata that lets the receiver reassemble them.
//
// These tests verify:
//   • Individual chunk messages are accepted and stored.
//   • Each chunk carries the fields required for reassembly
//     (chunk_index, chunk_total, upload_id).
//   • All chunks for one upload_id appear in history.
//   • Non-blob messages are not affected by chunk metadata.
//
// NOTE: AppState does not implement Clone.  Do NOT call .clone() on it.
// Each test constructs its own isolated server via helpers::TestServer::new().

use axum::http::StatusCode;
use serde_json::json;

// ── Single-chunk upload ───────────────────────────────────────────────────────

/// A single-part blob (chunk_index=0, chunk_total=1) is the degenerate case
/// of chunked upload and must round-trip through the bus without modification
/// to the chunk fields.
#[tokio::test]
async fn single_chunk_blob_is_accepted() {
    let srv = helpers::TestServer::new().await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":        "uploader-agent",
                "to":          "all",
                "type":        "blob",
                "mime":        "image/png",
                "enc":         "base64",
                "subject":     "chunked-single-part",
                "upload_id":   "upload-abc-001",
                "chunk_index": 0,
                "chunk_total": 1,
                // 1×1 transparent PNG
                "body": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVQI12NgAAIABQ==",
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "single-chunk blob must be accepted"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
}

/// The stored chunk message must carry all three reassembly fields
/// (upload_id, chunk_index, chunk_total) when they were sent.
#[tokio::test]
async fn single_chunk_blob_preserves_reassembly_fields() {
    let srv = helpers::TestServer::new().await;

    helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":        "uploader-agent",
                "to":          "all",
                "type":        "blob",
                "mime":        "image/jpeg",
                "enc":         "base64",
                "subject":     "chunked-fields-check",
                "upload_id":   "upload-field-test-001",
                "chunk_index": 0,
                "chunk_total": 1,
                "body": "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAA==",
            }),
        ),
    )
    .await;

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    assert_eq!(list.status(), StatusCode::OK);
    let msgs = helpers::body_json(list).await;
    let msgs = msgs.as_array().expect("expected JSON array");

    let msg = msgs
        .iter()
        .find(|m| m["subject"] == json!("chunked-fields-check"))
        .expect("single-chunk blob must appear in /bus/messages");

    assert_eq!(
        msg["upload_id"],
        json!("upload-field-test-001"),
        "upload_id must be preserved in stored message"
    );
    assert_eq!(
        msg["chunk_index"],
        json!(0),
        "chunk_index must be preserved in stored message"
    );
    assert_eq!(
        msg["chunk_total"],
        json!(1),
        "chunk_total must be preserved in stored message"
    );
}

// ── Multi-chunk upload ────────────────────────────────────────────────────────

/// A three-part upload: chunks 0, 1, 2 each posted separately.  All three
/// must appear in /bus/messages and carry the correct chunk_index values.
#[tokio::test]
async fn multi_chunk_blob_all_parts_stored() {
    let srv = helpers::TestServer::new().await;
    let upload_id = "upload-multi-001";

    // Stub base64 payload split across three "chunks".
    let chunks = [
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCA==", // chunk 0
        "YAAAAfFcSJAAAAC0lEQVQI12NgAAIABQ==",   // chunk 1
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==",   // chunk 2
    ];

    for (idx, chunk_body) in chunks.iter().enumerate() {
        let resp = helpers::call(
            &srv.app,
            helpers::post_json(
                "/api/bus/send",
                &json!({
                    "from":        "uploader-agent",
                    "to":          "assembler-agent",
                    "type":        "blob",
                    "mime":        "application/octet-stream",
                    "enc":         "base64",
                    "subject":     format!("multi-chunk-{}", idx),
                    "upload_id":   upload_id,
                    "chunk_index": idx,
                    "chunk_total": chunks.len(),
                    "body": chunk_body,
                }),
            ),
        )
        .await;

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "chunk {idx} of multi-chunk upload must be accepted"
        );
    }

    // All three chunks must appear in history
    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    assert_eq!(list.status(), StatusCode::OK);
    let msgs = helpers::body_json(list).await;
    let msgs = msgs.as_array().expect("expected JSON array");

    let upload_msgs: Vec<&serde_json::Value> = msgs
        .iter()
        .filter(|m| m["upload_id"] == json!(upload_id))
        .collect();

    assert_eq!(
        upload_msgs.len(),
        chunks.len(),
        "all {chunk_count} chunks must be stored; found {found}",
        chunk_count = chunks.len(),
        found = upload_msgs.len()
    );

    // Every chunk index 0..N must be present exactly once
    for expected_idx in 0..chunks.len() {
        assert!(
            upload_msgs
                .iter()
                .any(|m| m["chunk_index"] == json!(expected_idx)),
            "chunk_index={expected_idx} must appear in stored messages"
        );
    }
}

/// Each chunk in a multi-part upload must record the same chunk_total value.
#[tokio::test]
async fn multi_chunk_blob_chunk_total_consistent() {
    let srv = helpers::TestServer::new().await;
    let upload_id = "upload-total-check-001";
    let total = 2u64;

    for idx in 0..total {
        helpers::call(
            &srv.app,
            helpers::post_json(
                "/api/bus/send",
                &json!({
                    "from":        "uploader-agent",
                    "to":          "all",
                    "type":        "blob",
                    "mime":        "image/png",
                    "enc":         "base64",
                    "subject":     format!("total-check-chunk-{}", idx),
                    "upload_id":   upload_id,
                    "chunk_index": idx,
                    "chunk_total": total,
                    "body": "AAAA",
                }),
            ),
        )
        .await;
    }

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msgs = msgs.as_array().unwrap();

    let upload_msgs: Vec<&serde_json::Value> = msgs
        .iter()
        .filter(|m| m["upload_id"] == json!(upload_id))
        .collect();

    for msg in &upload_msgs {
        assert_eq!(
            msg["chunk_total"],
            json!(total),
            "every chunk must carry chunk_total={total}; got {}",
            msg["chunk_total"]
        );
    }
}

// ── Isolation between uploads ─────────────────────────────────────────────────

/// Two concurrent uploads with different upload_ids must not interfere with
/// each other.  The server-side `?upload_id=` filter must return only the
/// chunks that belong to the requested upload, making isolation a server
/// guarantee rather than a client-side concern.
#[tokio::test]
async fn two_uploads_are_isolated_by_upload_id() {
    let srv = helpers::TestServer::new().await;

    let upload_a = "upload-iso-aaa";
    let upload_b = "upload-iso-bbb";

    // Post one chunk for upload A and two for upload B.
    for (uid, idx, total, subject) in [
        (upload_a, 0u64, 1u64, "iso-a-chunk-0"),
        (upload_b, 0u64, 2u64, "iso-b-chunk-0"),
        (upload_b, 1u64, 2u64, "iso-b-chunk-1"),
    ] {
        helpers::call(
            &srv.app,
            helpers::post_json(
                "/api/bus/send",
                &json!({
                    "from":        "uploader-agent",
                    "to":          "all",
                    "type":        "blob",
                    "mime":        "application/octet-stream",
                    "enc":         "base64",
                    "subject":     subject,
                    "upload_id":   uid,
                    "chunk_index": idx,
                    "chunk_total": total,
                    "body":        "AAEC",
                }),
            ),
        )
        .await;
    }

    // ── server-side filter: ?upload_id=upload-iso-aaa ──────────────────────
    let resp_a = helpers::call(
        &srv.app,
        helpers::get(&format!("/api/bus/messages?upload_id={upload_a}")),
    )
    .await;
    assert_eq!(resp_a.status(), StatusCode::OK);
    let msgs_a = helpers::body_json(resp_a).await;
    let msgs_a = msgs_a.as_array().expect("expected JSON array for upload_a filter");

    assert_eq!(
        msgs_a.len(),
        1,
        "server-side ?upload_id={upload_a} must return exactly 1 chunk; got {}",
        msgs_a.len()
    );
    assert_eq!(
        msgs_a[0]["upload_id"],
        json!(upload_a),
        "returned chunk must belong to upload_a"
    );

    // ── server-side filter: ?upload_id=upload-iso-bbb ──────────────────────
    let resp_b = helpers::call(
        &srv.app,
        helpers::get(&format!("/api/bus/messages?upload_id={upload_b}")),
    )
    .await;
    assert_eq!(resp_b.status(), StatusCode::OK);
    let msgs_b = helpers::body_json(resp_b).await;
    let msgs_b = msgs_b.as_array().expect("expected JSON array for upload_b filter");

    assert_eq!(
        msgs_b.len(),
        2,
        "server-side ?upload_id={upload_b} must return exactly 2 chunks; got {}",
        msgs_b.len()
    );
    for msg in msgs_b {
        assert_eq!(
            msg["upload_id"],
            json!(upload_b),
            "every returned message must belong to {upload_b}"
        );
    }

    // ── in-memory cross-check (belt-and-suspenders) ────────────────────────
    // Fetch all blob messages and verify counts match the server-filtered results.
    let all_resp = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let all_msgs = helpers::body_json(all_resp).await;
    let all_msgs = all_msgs.as_array().unwrap();

    let count_a = all_msgs
        .iter()
        .filter(|m| m["upload_id"] == json!(upload_a))
        .count();
    let count_b = all_msgs
        .iter()
        .filter(|m| m["upload_id"] == json!(upload_b))
        .count();

    assert_eq!(count_a, 1, "upload_id={upload_a} must have exactly 1 chunk in full history");
    assert_eq!(count_b, 2, "upload_id={upload_b} must have exactly 2 chunks in full history");
}

/// The server-side `?upload_id=` filter must return only the chunks that
/// belong to the requested upload, with no bleed-through from other uploads
/// or from non-blob messages present in the log.
#[tokio::test]
async fn upload_id_filter_excludes_other_uploads_and_non_blob_messages() {
    let srv = helpers::TestServer::new().await;

    let target_upload = "upload-filter-target";
    let other_upload  = "upload-filter-other";

    // Two chunks for the target upload.
    for idx in 0u64..2 {
        helpers::call(
            &srv.app,
            helpers::post_json(
                "/api/bus/send",
                &json!({
                    "from":        "uploader-agent",
                    "to":          "all",
                    "type":        "blob",
                    "mime":        "image/png",
                    "enc":         "base64",
                    "subject":     format!("target-chunk-{idx}"),
                    "upload_id":   target_upload,
                    "chunk_index": idx,
                    "chunk_total": 2,
                    "body":        "iVBORw0KGgo=",
                }),
            ),
        )
        .await;
    }

    // One chunk for a different upload — must not appear in filtered results.
    helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":        "uploader-agent",
                "to":          "all",
                "type":        "blob",
                "mime":        "image/jpeg",
                "enc":         "base64",
                "subject":     "other-chunk-0",
                "upload_id":   other_upload,
                "chunk_index": 0,
                "chunk_total": 1,
                "body":        "/9j/4AAQ==",
            }),
        ),
    )
    .await;

    // A plain text message — must also not appear in filtered results.
    helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "chat-agent",
                "to":      "all",
                "type":    "text",
                "subject": "noise-text-message",
                "body":    "this is not a blob",
            }),
        ),
    )
    .await;

    // Server-side filter by target upload_id.
    let resp = helpers::call(
        &srv.app,
        helpers::get(&format!("/api/bus/messages?upload_id={target_upload}")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let msgs = helpers::body_json(resp).await;
    let msgs = msgs.as_array().expect("expected JSON array");

    assert_eq!(
        msgs.len(),
        2,
        "?upload_id={target_upload} must return exactly 2 messages; got {}",
        msgs.len()
    );
    for msg in msgs {
        assert_eq!(
            msg["upload_id"],
            json!(target_upload),
            "every message in filtered response must carry upload_id={target_upload}"
        );
        assert_eq!(
            msg["type"],
            json!("blob"),
            "every message in upload_id-filtered response must be type=blob"
        );
    }
}

/// Querying with an `?upload_id=` that does not match any stored message
/// must return an empty array, not an error.
#[tokio::test]
async fn upload_id_filter_returns_empty_for_unknown_id() {
    let srv = helpers::TestServer::new().await;

    // Post a blob with a known upload_id so the bus log is non-empty.
    helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":        "uploader-agent",
                "to":          "all",
                "type":        "blob",
                "mime":        "image/png",
                "enc":         "base64",
                "subject":     "known-upload-chunk",
                "upload_id":   "upload-known-001",
                "chunk_index": 0,
                "chunk_total": 1,
                "body":        "iVBORw0KGgo=",
            }),
        ),
    )
    .await;

    // Query for a completely different upload_id that was never posted.
    let resp = helpers::call(
        &srv.app,
        helpers::get("/api/bus/messages?upload_id=upload-does-not-exist"),
    )
    .await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "?upload_id= with no matches must return 200, not an error"
    );
    let body = helpers::body_json(resp).await;
    let msgs = body
        .as_array()
        .expect("response must be a JSON array even when empty");
    assert!(
        msgs.is_empty(),
        "unknown upload_id must yield an empty array; got {} messages",
        msgs.len()
    );
}

// ── blob_meta enrichment on chunks ────────────────────────────────────────────

/// Chunk messages are still `type=blob`, so blob_meta must be injected by the
/// server on the /bus/messages response — even when chunk_index / chunk_total
/// are present.
#[tokio::test]
async fn chunked_blob_message_gets_blob_meta_injected() {
    let srv = helpers::TestServer::new().await;

    helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":        "uploader-agent",
                "to":          "all",
                "type":        "blob",
                "mime":        "image/png",
                "enc":         "base64",
                "subject":     "chunk-with-blob-meta",
                "upload_id":   "upload-meta-check",
                "chunk_index": 0,
                "chunk_total": 1,
                "body": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAC0lEQVQI12NgAAIABQ==",
            }),
        ),
    )
    .await;

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("chunk-with-blob-meta"))
        .expect("chunk blob message must appear in history");

    let meta = msg
        .get("blob_meta")
        .expect("blob_meta must be injected on chunk messages");

    assert_eq!(meta["mime"], json!("image/png"));
    assert_eq!(meta["enc"], json!("base64"));
    assert_eq!(meta["render_as"], json!("image"),
        "image/png chunk must have render_as=image");
    assert!(
        meta["size_bytes"].as_u64().unwrap_or(0) > 0,
        "size_bytes must be > 0 for a non-empty chunk body"
    );
}

// ── Auth guard ────────────────────────────────────────────────────────────────

/// Posting a chunk without a valid Bearer token must be rejected with 401.
#[tokio::test]
async fn chunked_blob_upload_requires_auth() {
    use axum::body::Body;
    use axum::http::Request;

    let srv = helpers::TestServer::new().await;

    let body = json!({
        "from":        "rogue-agent",
        "to":          "all",
        "type":        "blob",
        "mime":        "image/png",
        "enc":         "base64",
        "subject":     "unauth-chunk",
        "upload_id":   "upload-unauth",
        "chunk_index": 0,
        "chunk_total": 1,
        "body":        "AAAA",
    });

    let req = Request::builder()
        .method("POST")
        .uri("/api/bus/send")
        .header("Content-Type", "application/json")
        // Deliberately omit Authorization header
        .body(Body::from(body.to_string()))
        .unwrap();

    let resp = helpers::call(&srv.app, req).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "chunk upload without auth must return 401"
    );
}

// ── 3-D asset MIME types ──────────────────────────────────────────────────────
//
// The `model/*` top-level type covers 3-D assets.  All six MIME types below
// must be accepted by /api/bus/send (HTTP 200) and must be stored on the bus
// log exactly like any other blob message.  The server does not render model/*
// assets inline — `blob_meta.render_as` must be "download" for all of them.

/// model/gltf+json — GL Transmission Format, JSON variant.
#[tokio::test]
async fn blob_gltf_json_is_accepted() {
    let srv = helpers::TestServer::new().await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "3d-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "model/gltf+json",
                "enc":     "base64",
                "subject": "test-gltf-json",
                // Minimal valid glTF 2.0 JSON, base64-encoded
                "body":    "eyJhc3NldCI6eyJ2ZXJzaW9uIjoiMi4wIn19",
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "model/gltf+json blob must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    // Verify storage and blob_meta enrichment
    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("test-gltf-json"))
        .expect("model/gltf+json blob must appear in message history");

    let meta = msg.get("blob_meta").expect("blob_meta must be injected");
    assert_eq!(meta["mime"], json!("model/gltf+json"));
    assert_eq!(
        meta["render_as"], json!("download"),
        "model/* must fall back to render_as=download"
    );
}

/// model/gltf-binary — GL Transmission Format, binary (.glb) variant.
#[tokio::test]
async fn blob_gltf_binary_is_accepted() {
    let srv = helpers::TestServer::new().await;

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "3d-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "model/gltf-binary",
                "enc":     "base64",
                "subject": "test-gltf-binary",
                // Stub GLB magic bytes (0x46546C67 = "glTF"), base64-encoded
                "body":    "Z2xURgIAAAA=",
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "model/gltf-binary blob must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("test-gltf-binary"))
        .expect("model/gltf-binary blob must appear in message history");

    let meta = msg.get("blob_meta").expect("blob_meta must be injected");
    assert_eq!(meta["mime"], json!("model/gltf-binary"));
    assert_eq!(meta["render_as"], json!("download"),
        "model/* must fall back to render_as=download");
}

/// model/obj — Wavefront OBJ mesh format.
#[tokio::test]
async fn blob_obj_is_accepted() {
    let srv = helpers::TestServer::new().await;

    // Minimal OBJ file (single vertex), base64-encoded
    let obj_b64 = base64_encode(b"v 0.0 0.0 0.0\n");

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "3d-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "model/obj",
                "enc":     "base64",
                "subject": "test-obj",
                "body":    obj_b64,
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "model/obj blob must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("test-obj"))
        .expect("model/obj blob must appear in message history");

    let meta = msg.get("blob_meta").expect("blob_meta must be injected");
    assert_eq!(meta["mime"], json!("model/obj"));
    assert_eq!(meta["render_as"], json!("download"),
        "model/* must fall back to render_as=download");
}

/// model/stl — Stereolithography mesh format.
#[tokio::test]
async fn blob_stl_is_accepted() {
    let srv = helpers::TestServer::new().await;

    // Minimal ASCII STL header, base64-encoded
    let stl_b64 = base64_encode(b"solid test\nendsolid test\n");

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "3d-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "model/stl",
                "enc":     "base64",
                "subject": "test-stl",
                "body":    stl_b64,
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "model/stl blob must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("test-stl"))
        .expect("model/stl blob must appear in message history");

    let meta = msg.get("blob_meta").expect("blob_meta must be injected");
    assert_eq!(meta["mime"], json!("model/stl"));
    assert_eq!(meta["render_as"], json!("download"),
        "model/* must fall back to render_as=download");
}

/// model/ply — Polygon File Format / Stanford Triangle Format.
#[tokio::test]
async fn blob_ply_is_accepted() {
    let srv = helpers::TestServer::new().await;

    // Minimal ASCII PLY header, base64-encoded
    let ply_b64 = base64_encode(b"ply\nformat ascii 1.0\nend_header\n");

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "3d-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "model/ply",
                "enc":     "base64",
                "subject": "test-ply",
                "body":    ply_b64,
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "model/ply blob must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("test-ply"))
        .expect("model/ply blob must appear in message history");

    let meta = msg.get("blob_meta").expect("blob_meta must be injected");
    assert_eq!(meta["mime"], json!("model/ply"));
    assert_eq!(meta["render_as"], json!("download"),
        "model/* must fall back to render_as=download");
}

/// model/vnd.usdz+zip — Universal Scene Description, USDZ package format.
#[tokio::test]
async fn blob_usdz_is_accepted() {
    let srv = helpers::TestServer::new().await;

    // Stub USDZ payload (ZIP magic bytes PK\x03\x04), base64-encoded
    let usdz_b64 = base64_encode(b"PK\x03\x04");

    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({
                "from":    "3d-agent",
                "to":      "all",
                "type":    "blob",
                "mime":    "model/vnd.usdz+zip",
                "enc":     "base64",
                "subject": "test-usdz",
                "body":    usdz_b64,
            }),
        ),
    )
    .await;

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "model/vnd.usdz+zip blob must be accepted with 200"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    let list = helpers::call(&srv.app, helpers::get("/api/bus/messages?type=blob")).await;
    let msgs = helpers::body_json(list).await;
    let msg = msgs
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["subject"] == json!("test-usdz"))
        .expect("model/vnd.usdz+zip blob must appear in message history");

    let meta = msg.get("blob_meta").expect("blob_meta must be injected");
    assert_eq!(meta["mime"], json!("model/vnd.usdz+zip"));
    assert_eq!(meta["render_as"], json!("download"),
        "model/* must fall back to render_as=download");
}

// ── test helpers ──────────────────────────────────────────────────────────────

/// Minimal base64 encoder (standard alphabet, with padding) used by the 3-D
/// asset tests to produce deterministic payloads without adding a dependency.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        let combined = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(combined >> 18) & 0x3F] as char);
        out.push(ALPHABET[(combined >> 12) & 0x3F] as char);
        out.push(if chunk.len() > 1 { ALPHABET[(combined >> 6) & 0x3F] as char } else { '=' });
        out.push(if chunk.len() > 2 { ALPHABET[combined & 0x3F] as char } else { '=' });
    }
    out
}
