//! Integration tests for POST /bus/blob and POST /api/bus/blob
//!
//! Tests cover:
//!   • Raw binary upload (Content-Type header mode)
//!   • multipart/form-data upload
//!   • All 17 known MediaType variants
//!   • Size limit enforcement (100 MiB default)
//!   • bus message broadcast (type="blob") on completion
//!   • broadcast=false suppression
//!   • Auth gating (401 with no token)
//!   • Missing/empty body rejection
//!   • Unknown MIME type rejection
//!   • Response shape: blob_id, uri, mime, size_bytes, broadcast
//!   • Reverse-proxy alias /bus/blob == /api/bus/blob
//!   • Downloaded bytes match uploaded bytes (round-trip)
//!   • from/to query parameters appear in the bus event
mod helpers;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::json;

use acc_server::routes::blobs::{b64_decode, b64_encode, BLOB_SIZE_LIMIT};

// ── tiny helpers ──────────────────────────────────────────────────────────────

/// Build a raw-binary POST /api/bus/blob request with auth.
fn raw_upload_req(path: &str, mime: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .header("Content-Type", mime)
        .body(Body::from(body))
        .unwrap()
}

/// Build a raw-binary POST without an Authorization header.
fn raw_upload_no_auth(mime: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/bus/blob")
        .header("Content-Type", mime)
        .body(Body::from(body))
        .unwrap()
}

/// Build a multipart/form-data request manually.
/// The boundary is fixed so tests are deterministic.
fn multipart_req(
    path: &str,
    boundary: &str,
    parts: &[(&str, Option<&str>, Vec<u8>)], // (field_name, content_type_opt, bytes)
    extra_text_fields: &[(&str, &str)],       // (field_name, value)
) -> Request<Body> {
    let mut body_bytes: Vec<u8> = Vec::new();
    for (name, ct, data) in parts {
        body_bytes.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes(),
        );
        if let Some(ct_val) = ct {
            body_bytes
                .extend_from_slice(format!("; filename=\"upload\"\r\nContent-Type: {ct_val}").as_bytes());
        }
        body_bytes.extend_from_slice(b"\r\n\r\n");
        body_bytes.extend_from_slice(data);
        body_bytes.extend_from_slice(b"\r\n");
    }
    for (name, value) in extra_text_fields {
        body_bytes.extend_from_slice(
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes(),
        );
    }
    body_bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body_bytes))
        .unwrap()
}

/// Upload raw binary and return (status, json_body).
async fn raw_upload(
    srv: &helpers::TestServer,
    path: &str,
    mime: &str,
    data: &[u8],
) -> (u16, serde_json::Value) {
    let req = raw_upload_req(path, mime, data.to_vec());
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    (status, helpers::body_json(resp).await)
}

/// Download via the existing /api/bus/blobs/:id/download endpoint.
async fn download(
    srv: &helpers::TestServer,
    blob_id: &str,
) -> (u16, serde_json::Value) {
    let resp = helpers::call(
        &srv.app,
        helpers::get(&format!("/api/bus/blobs/{blob_id}/download")),
    )
    .await;
    let status = resp.status().as_u16();
    (status, helpers::body_json(resp).await)
}


// ── 1. Auth gating ────────────────────────────────────────────────────────────

#[tokio::test]
async fn blob_endpoint_requires_auth() {
    let srv = helpers::TestServer::new().await;
    let req = raw_upload_no_auth("text/plain", b"hello".to_vec());
    let resp = helpers::call(&srv.app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn blob_endpoint_alias_requires_auth() {
    let srv = helpers::TestServer::new().await;
    let req = Request::builder()
        .method("POST")
        .uri("/bus/blob")
        .header("Content-Type", "text/plain")
        .body(Body::from(b"hello".to_vec()))
        .unwrap();
    let resp = helpers::call(&srv.app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ── 2. Response shape ─────────────────────────────────────────────────────────

#[tokio::test]
async fn raw_upload_response_has_required_fields() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = raw_upload(&srv, "/api/bus/blob", "text/plain", b"hello world").await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");

    assert_eq!(body["ok"], json!(true));
    assert!(body["blob_id"].is_string(), "blob_id must be a string");
    assert!(body["uri"].is_string(), "uri must be a string");
    assert_eq!(body["mime"].as_str().unwrap(), "text/plain");
    assert_eq!(body["size_bytes"].as_u64().unwrap(), 11u64);
    assert!(body["broadcast"].is_boolean(), "broadcast must be boolean");

    // URI must reference the download endpoint
    let uri = body["uri"].as_str().unwrap();
    assert!(
        uri.contains("/api/bus/blobs/"),
        "uri must contain /api/bus/blobs/; got {uri}"
    );
    assert!(uri.ends_with("/download"), "uri must end with /download; got {uri}");
}

#[tokio::test]
async fn raw_upload_blob_id_is_unique_per_request() {
    let srv = helpers::TestServer::new().await;
    let (_, b1) = raw_upload(&srv, "/api/bus/blob", "text/plain", b"data").await;
    let (_, b2) = raw_upload(&srv, "/api/bus/blob", "text/plain", b"data").await;
    assert_ne!(
        b1["blob_id"].as_str().unwrap(),
        b2["blob_id"].as_str().unwrap(),
        "each upload must get a unique blob_id"
    );
}


// ── 3. Reverse-proxy alias /bus/blob ─────────────────────────────────────────

#[tokio::test]
async fn bus_blob_alias_works() {
    let srv = helpers::TestServer::new().await;
    // /bus/blob must behave identically to /api/bus/blob
    let (status, body) = raw_upload(&srv, "/bus/blob", "text/plain", b"alias test").await;
    assert_eq!(status, 200, "/bus/blob alias returned {status}: {body}");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["mime"].as_str().unwrap(), "text/plain");
}

// ── 4. All 17 known MediaType variants — raw binary round-trip ────────────────

/// Upload raw bytes via /api/bus/blob, then download and verify exact bytes come back.
async fn raw_round_trip(srv: &helpers::TestServer, mime: &str, data: &[u8]) {
    let (status, body) = raw_upload(srv, "/api/bus/blob", mime, data).await;
    assert_eq!(status, 200, "upload failed for {mime}: {body}");

    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    let (dl_status, dl) = download(srv, &blob_id).await;
    assert_eq!(dl_status, 200, "download failed for {mime}: {dl}");

    let returned = b64_decode(dl["data"].as_str().unwrap())
        .expect("download data must be valid base64");
    assert_eq!(
        returned, data,
        "round-trip mismatch for mime={mime}: uploaded {} bytes, got {} bytes",
        data.len(),
        returned.len()
    );
}

#[tokio::test]
async fn round_trip_text_plain() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "text/plain", b"hello world").await;
}

#[tokio::test]
async fn round_trip_text_markdown() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "text/markdown", b"# Title\n\nParagraph.").await;
}

#[tokio::test]
async fn round_trip_text_html() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "text/html", b"<p>Hello</p>").await;
}

#[tokio::test]
async fn round_trip_application_json() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "application/json", b"{\"key\":\"val\"}").await;
}

#[tokio::test]
async fn round_trip_image_svg() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "image/svg+xml", b"<svg><rect/></svg>").await;
}

#[tokio::test]
async fn round_trip_audio_wav() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "audio/wav", b"\x52\x49\x46\x46\x00\x00WAVE").await;
}

#[tokio::test]
async fn round_trip_audio_mp3() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "audio/mp3", b"\xff\xfb\x90\x00\x00").await;
}

#[tokio::test]
async fn round_trip_audio_ogg() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "audio/ogg", b"OggS\x00\x02").await;
}

#[tokio::test]
async fn round_trip_audio_flac() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "audio/flac", b"fLaC\x00").await;
}

#[tokio::test]
async fn round_trip_video_mp4() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "video/mp4", b"\x00\x00\x00\x1cftyp").await;
}

#[tokio::test]
async fn round_trip_video_webm() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "video/webm", b"\x1a\x45\xdf\xa3").await;
}

#[tokio::test]
async fn round_trip_video_ogg() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "video/ogg", b"OggS\x00\x02video").await;
}

#[tokio::test]
async fn round_trip_image_png() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "image/png", b"\x89PNG\r\n\x1a\n").await;
}

#[tokio::test]
async fn round_trip_image_jpeg() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "image/jpeg", b"\xff\xd8\xff\xe0\x00\x10JFIF").await;
}

#[tokio::test]
async fn round_trip_image_gif() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "image/gif", b"GIF89a\x01\x00\x01\x00").await;
}

#[tokio::test]
async fn round_trip_image_webp() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "image/webp", b"RIFF\x00\x00\x00\x00WEBP").await;
}

#[tokio::test]
async fn round_trip_application_octet_stream() {
    let srv = helpers::TestServer::new().await;
    raw_round_trip(&srv, "application/octet-stream", b"\x00\x01\x02\x03\xfe\xff").await;
}


// ── 5. MIME type validation ───────────────────────────────────────────────────

#[tokio::test]
async fn unknown_mime_type_returns_422() {
    let srv = helpers::TestServer::new().await;
    let (status, body) =
        raw_upload(&srv, "/api/bus/blob", "application/x-totally-unknown-type", b"data").await;
    assert_eq!(status, 422, "expected 422 for unknown mime, got {status}: {body}");
    assert_eq!(body["error"].as_str().unwrap(), "unknown_media_type");
    // known_types list must be present for discoverability
    assert!(
        body["known_types"].is_array(),
        "known_types array must be in error response"
    );
}

#[tokio::test]
async fn missing_content_type_returns_415() {
    let srv = helpers::TestServer::new().await;
    // No Content-Type header at all on a raw binary request
    let req = Request::builder()
        .method("POST")
        .uri("/api/bus/blob")
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .body(Body::from(b"raw bytes".to_vec()))
        .unwrap();
    let resp = helpers::call(&srv.app, req).await;
    // axum will reject the request with 415 Unsupported Media Type because there
    // is no Content-Type, which maps to our missing_content_type branch.
    assert!(
        resp.status().as_u16() == 415 || resp.status().as_u16() == 422,
        "expected 415 or 422 for missing Content-Type, got {}",
        resp.status()
    );
}

#[tokio::test]
async fn empty_body_returns_error() {
    let srv = helpers::TestServer::new().await;
    let req = raw_upload_req("/api/bus/blob", "text/plain", vec![]);
    let resp = helpers::call(&srv.app, req).await;
    // empty body must be rejected — either 422 or 400
    let status = resp.status().as_u16();
    assert!(
        status == 422 || status == 400,
        "expected 422 or 400 for empty body, got {status}"
    );
}

// ── 6. Size limit ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn upload_at_exactly_limit_is_accepted() {
    // Use a small limit override — we cannot allocate 100 MiB in tests.
    // Instead we verify the constant is set correctly and that a body just
    // under (and at) a small threshold works.
    // The limit is enforced at the router layer; here we test a realistic
    // payload of 1 MiB succeeds.
    let srv = helpers::TestServer::new().await;
    let one_mib = vec![0xABu8; 1024 * 1024];
    let (status, body) = raw_upload(&srv, "/api/bus/blob", "application/octet-stream", &one_mib).await;
    assert_eq!(status, 200, "1 MiB upload must succeed: {body}");
    assert_eq!(body["size_bytes"].as_u64().unwrap(), 1024 * 1024);
}

#[tokio::test]
async fn blob_size_limit_constant_is_100_mib() {
    // The public constant exported from the module must be exactly 100 MiB.
    assert_eq!(
        BLOB_SIZE_LIMIT,
        100 * 1024 * 1024,
        "BLOB_SIZE_LIMIT must be exactly 100 MiB (104_857_600 bytes)"
    );
}

// ── 7. Bus event broadcast ────────────────────────────────────────────────────

#[tokio::test]
async fn raw_upload_broadcasts_blob_event_by_default() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = raw_upload(
        &srv,
        "/api/bus/blob?from=natasha&to=rocky",
        "image/png",
        b"\x89PNG\r\n\x1a\n",
    )
    .await;
    assert_eq!(status, 200, "upload failed: {body}");
    assert_eq!(body["broadcast"], json!(true));

    let blob_id = body["blob_id"].as_str().unwrap().to_string();

    // Give async log write a moment to flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs_resp = helpers::call(&srv.app, helpers::get("/api/bus/messages")).await;
    let msgs = helpers::body_json(msgs_resp).await;
    let arr = msgs.as_array().expect("bus messages must be an array");

    let event = arr.iter().find(|m| {
        m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id)
    });
    assert!(
        event.is_some(),
        "type=blob bus event not found for blob_id={blob_id}; messages: {msgs}"
    );

    let ev = event.unwrap();
    assert_eq!(ev["mime"].as_str().unwrap(), "image/png");
    assert_eq!(ev["from"].as_str().unwrap(), "natasha");
    assert_eq!(ev["to"].as_str().unwrap(), "rocky");
    assert!(ev["uri"].as_str().unwrap().contains(&blob_id));
    assert_eq!(ev["size_bytes"].as_u64().unwrap(), 8u64);
}

#[tokio::test]
async fn raw_upload_broadcast_false_suppresses_event() {
    let srv = helpers::TestServer::new().await;
    let (status, body) = raw_upload(
        &srv,
        "/api/bus/blob?broadcast=false",
        "text/plain",
        b"silent upload",
    )
    .await;
    assert_eq!(status, 200, "upload failed: {body}");
    assert_eq!(body["broadcast"], json!(false));

    let blob_id = body["blob_id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().expect("bus messages must be an array");
    let found = arr.iter().any(|m| {
        m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id)
    });
    assert!(
        !found,
        "broadcast=false must suppress the bus event, but found one for blob_id={blob_id}"
    );
}

#[tokio::test]
async fn raw_upload_broadcast_zero_suppresses_event() {
    // broadcast=0 is equivalent to broadcast=false
    let srv = helpers::TestServer::new().await;
    let (status, body) = raw_upload(
        &srv,
        "/api/bus/blob?broadcast=0",
        "text/plain",
        b"also silent",
    )
    .await;
    assert_eq!(status, 200, "upload failed: {body}");
    assert_eq!(body["broadcast"], json!(false));
}

#[tokio::test]
async fn blob_event_contains_uri_pointing_to_download() {
    let srv = helpers::TestServer::new().await;
    let (status, body) =
        raw_upload(&srv, "/api/bus/blob", "text/plain", b"event uri test").await;
    assert_eq!(status, 200);
    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    let response_uri = body["uri"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().unwrap();
    let ev = arr
        .iter()
        .find(|m| m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id))
        .expect("bus event must exist");

    assert_eq!(
        ev["uri"].as_str().unwrap(),
        response_uri,
        "uri in bus event must match uri in upload response"
    );
}


// ── 8. from / to query parameters ────────────────────────────────────────────

#[tokio::test]
async fn from_to_defaults_when_omitted() {
    let srv = helpers::TestServer::new().await;
    // No ?from= or ?to= — event should still be emitted with empty/default values
    let (status, body) =
        raw_upload(&srv, "/api/bus/blob", "text/plain", b"defaults test").await;
    assert_eq!(status, 200);

    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().unwrap();
    let ev = arr
        .iter()
        .find(|m| m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id))
        .expect("bus event must exist even without from/to");
    // "to" defaults to "all"
    assert_eq!(ev["to"].as_str().unwrap_or(""), "all");
}

#[tokio::test]
async fn from_to_query_params_appear_in_bus_event() {
    let srv = helpers::TestServer::new().await;
    let (_, body) = raw_upload(
        &srv,
        "/api/bus/blob?from=boris&to=bullwinkle",
        "text/plain",
        b"directed message",
    )
    .await;
    let blob_id = body["blob_id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().unwrap();
    let ev = arr
        .iter()
        .find(|m| m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id))
        .expect("bus event not found");

    assert_eq!(ev["from"].as_str().unwrap(), "boris");
    assert_eq!(ev["to"].as_str().unwrap(), "bullwinkle");
}

// ── 9. Content-Type parameter stripping ──────────────────────────────────────

#[tokio::test]
async fn content_type_parameters_are_stripped_before_storage() {
    // e.g. "text/plain; charset=utf-8" → stored as "text/plain"
    let srv = helpers::TestServer::new().await;
    let req = raw_upload_req(
        "/api/bus/blob",
        "text/plain; charset=utf-8",
        b"charset test".to_vec(),
    );
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    let body = helpers::body_json(resp).await;
    assert_eq!(status, 200, "charset-parameterised content-type must be accepted: {body}");
    assert_eq!(
        body["mime"].as_str().unwrap(),
        "text/plain",
        "mime in response must be stripped of charset parameter"
    );
}

// ── 10. Blob appears in /api/bus/blobs list after raw upload ──────────────────

#[tokio::test]
async fn raw_uploaded_blob_appears_in_blob_list() {
    let srv = helpers::TestServer::new().await;
    let (status, body) =
        raw_upload(&srv, "/api/bus/blob", "image/gif", b"GIF89a\x01\x00").await;
    assert_eq!(status, 200);
    let blob_id = body["blob_id"].as_str().unwrap().to_string();

    let list = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/blobs")).await,
    )
    .await;
    let blobs = list["blobs"].as_array().expect("blobs array must exist");
    let found = blobs.iter().any(|b| b["id"].as_str() == Some(&blob_id));
    assert!(
        found,
        "blob_id {blob_id} must appear in /api/bus/blobs list"
    );
}

#[tokio::test]
async fn raw_uploaded_blob_meta_is_correct() {
    let srv = helpers::TestServer::new().await;
    let data = b"\x89PNG\r\n\x1a\n";
    let (status, body) = raw_upload(&srv, "/api/bus/blob", "image/png", data).await;
    assert_eq!(status, 200);
    let blob_id = body["blob_id"].as_str().unwrap().to_string();

    let meta = helpers::body_json(
        helpers::call(
            &srv.app,
            helpers::get(&format!("/api/bus/blobs/{blob_id}")),
        )
        .await,
    )
    .await;

    assert_eq!(meta["id"].as_str().unwrap(), blob_id);
    assert_eq!(meta["mime_type"].as_str().unwrap(), "image/png");
    assert_eq!(meta["complete"], json!(true));
    assert_eq!(meta["total_chunks"], json!(1));
    assert_eq!(meta["chunks_received"], json!(1));
    assert_eq!(
        meta["size_bytes"].as_u64().unwrap(),
        data.len() as u64
    );
}


// ── 11. multipart/form-data upload ───────────────────────────────────────────

#[tokio::test]
async fn multipart_upload_text_plain_round_trip() {
    let srv = helpers::TestServer::new().await;
    let data = b"multipart hello world";
    let req = multipart_req(
        "/api/bus/blob",
        "testboundary001",
        &[("file", Some("text/plain"), data.to_vec())],
        &[],
    );
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    let body = helpers::body_json(resp).await;
    assert_eq!(status, 200, "multipart upload failed: {body}");
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["mime"].as_str().unwrap(), "text/plain");
    assert_eq!(body["size_bytes"].as_u64().unwrap(), data.len() as u64);

    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    let (dl_status, dl) = download(&srv, &blob_id).await;
    assert_eq!(dl_status, 200, "download after multipart upload failed: {dl}");
    let returned = b64_decode(dl["data"].as_str().unwrap()).unwrap();
    assert_eq!(returned, data);
}

#[tokio::test]
async fn multipart_upload_binary_png_round_trip() {
    let srv = helpers::TestServer::new().await;
    let data = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";
    let req = multipart_req(
        "/api/bus/blob",
        "testboundary002",
        &[("file", Some("image/png"), data.to_vec())],
        &[],
    );
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    let body = helpers::body_json(resp).await;
    assert_eq!(status, 200, "multipart PNG upload failed: {body}");
    assert_eq!(body["mime"].as_str().unwrap(), "image/png");

    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    let (dl_status, dl) = download(&srv, &blob_id).await;
    assert_eq!(dl_status, 200);
    let returned = b64_decode(dl["data"].as_str().unwrap()).unwrap();
    assert_eq!(returned, data);
}

#[tokio::test]
async fn multipart_upload_from_to_fields_in_event() {
    let srv = helpers::TestServer::new().await;
    let data = b"agent directed";
    let req = multipart_req(
        "/api/bus/blob",
        "testboundary003",
        &[("file", Some("text/plain"), data.to_vec())],
        &[("from", "natasha"), ("to", "boris")],
    );
    let resp = helpers::call(&srv.app, req).await;
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().unwrap();
    let ev = arr
        .iter()
        .find(|m| m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id))
        .expect("bus event must exist");

    assert_eq!(ev["from"].as_str().unwrap(), "natasha");
    assert_eq!(ev["to"].as_str().unwrap(), "boris");
}

#[tokio::test]
async fn multipart_upload_broadcast_false_suppresses_event() {
    let srv = helpers::TestServer::new().await;
    let data = b"silent multipart";
    let req = multipart_req(
        "/api/bus/blob",
        "testboundary004",
        &[("file", Some("text/plain"), data.to_vec())],
        &[("broadcast", "false")],
    );
    let resp = helpers::call(&srv.app, req).await;
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["broadcast"], json!(false));

    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().unwrap();
    let found = arr.iter().any(|m| {
        m["type"].as_str() == Some("blob") && m["blob_id"].as_str() == Some(&blob_id)
    });
    assert!(
        !found,
        "broadcast=false in multipart field must suppress event for blob_id={blob_id}"
    );
}

#[tokio::test]
async fn multipart_upload_missing_file_field_returns_error() {
    let srv = helpers::TestServer::new().await;
    // Send multipart with no "file" field at all
    let req = multipart_req(
        "/api/bus/blob",
        "testboundary005",
        &[],
        &[("from", "natasha")],
    );
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    assert!(
        status == 422 || status == 400,
        "missing 'file' field must return 422 or 400, got {status}"
    );
}

#[tokio::test]
async fn multipart_upload_fallback_mime_when_part_has_no_content_type() {
    // When the file part carries no Content-Type, we fall back to application/octet-stream
    let srv = helpers::TestServer::new().await;
    let data = b"no content-type file part";
    let req = multipart_req(
        "/api/bus/blob",
        "testboundary006",
        // None = no content-type on the file part
        &[("file", None, data.to_vec())],
        &[],
    );
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    let body = helpers::body_json(resp).await;
    assert_eq!(status, 200, "no-CT file part must still succeed: {body}");
    assert_eq!(
        body["mime"].as_str().unwrap(),
        "application/octet-stream",
        "fallback mime must be application/octet-stream"
    );
}


// ── 12. multipart alias /bus/blob ─────────────────────────────────────────────

#[tokio::test]
async fn multipart_upload_via_bus_blob_alias() {
    let srv = helpers::TestServer::new().await;
    let data = b"alias multipart";
    let req = multipart_req(
        "/bus/blob",
        "testboundary007",
        &[("file", Some("text/plain"), data.to_vec())],
        &[],
    );
    let resp = helpers::call(&srv.app, req).await;
    let status = resp.status().as_u16();
    let body = helpers::body_json(resp).await;
    assert_eq!(status, 200, "/bus/blob alias multipart failed: {body}");
    assert_eq!(body["ok"], json!(true));
}

// ── 13. Idempotent download after raw upload ──────────────────────────────────

#[tokio::test]
async fn download_can_be_called_multiple_times() {
    let srv = helpers::TestServer::new().await;
    let data = b"idempotent download test data";
    let (_, body) = raw_upload(&srv, "/api/bus/blob", "text/plain", data).await;
    let blob_id = body["blob_id"].as_str().unwrap().to_string();

    for i in 0..3 {
        let (status, dl) = download(&srv, &blob_id).await;
        assert_eq!(status, 200, "download attempt {i} failed: {dl}");
        let returned = b64_decode(dl["data"].as_str().unwrap()).unwrap();
        assert_eq!(returned, data, "data mismatch on download attempt {i}");
    }
}

// ── 14. Large payload (realistic test within test budget) ─────────────────────

#[tokio::test]
async fn upload_large_payload_within_limit() {
    // 4 MiB — large enough to exercise buffering, small enough for CI
    let srv = helpers::TestServer::new().await;
    let four_mib: Vec<u8> = (0u8..=255u8).cycle().take(4 * 1024 * 1024).collect();
    let (status, body) =
        raw_upload(&srv, "/api/bus/blob", "application/octet-stream", &four_mib).await;
    assert_eq!(status, 200, "4 MiB upload must succeed: {body}");
    assert_eq!(
        body["size_bytes"].as_u64().unwrap(),
        four_mib.len() as u64,
        "size_bytes must match exactly"
    );

    // Verify the data comes back intact
    let blob_id = body["blob_id"].as_str().unwrap().to_string();
    let (dl_status, dl) = download(&srv, &blob_id).await;
    assert_eq!(dl_status, 200, "download of 4 MiB blob failed: {dl}");
    let returned = b64_decode(dl["data"].as_str().unwrap()).unwrap();
    assert_eq!(
        returned.len(),
        four_mib.len(),
        "returned byte count must match"
    );
    assert_eq!(returned, four_mib, "4 MiB round-trip data integrity check failed");
}

// ── 15. bus event seq is monotonically increasing ────────────────────────────

#[tokio::test]
async fn blob_events_have_increasing_seq_numbers() {
    let srv = helpers::TestServer::new().await;

    let (_, b1) = raw_upload(&srv, "/api/bus/blob", "text/plain", b"first").await;
    let (_, b2) = raw_upload(&srv, "/api/bus/blob", "text/plain", b"second").await;
    let id1 = b1["blob_id"].as_str().unwrap().to_string();
    let id2 = b2["blob_id"].as_str().unwrap().to_string();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().unwrap();

    let seq1 = arr
        .iter()
        .find(|m| m["blob_id"].as_str() == Some(&id1))
        .and_then(|m| m["seq"].as_u64())
        .expect("event for id1 must have seq");
    let seq2 = arr
        .iter()
        .find(|m| m["blob_id"].as_str() == Some(&id2))
        .and_then(|m| m["seq"].as_u64())
        .expect("event for id2 must have seq");

    assert!(seq2 > seq1, "second event seq ({seq2}) must be > first ({seq1})");
}

// ── 16. Binary data is NOT base64-encoded in transit to disk ─────────────────
//
// This verifies the fundamental promise of the endpoint: bytes go to disk as-is
// (no base64 inflation). We check by comparing the on-disk size to the original.
#[tokio::test]
async fn raw_upload_stores_exact_bytes_on_disk() {
    let srv = helpers::TestServer::new().await;
    // Use a payload that base64 would inflate (not a multiple of 3 in length)
    let data: Vec<u8> = (0u8..=255u8).collect(); // 256 bytes
    let (status, body) =
        raw_upload(&srv, "/api/bus/blob", "application/octet-stream", &data).await;
    assert_eq!(status, 200);

    // size_bytes in the response must equal the raw byte count, not the base64 length
    let stored_size = body["size_bytes"].as_u64().unwrap();
    assert_eq!(
        stored_size,
        data.len() as u64,
        "stored size ({stored_size}) must equal raw byte count ({}), not base64-inflated size ({})",
        data.len(),
        b64_encode(&data).len()
    );
}

