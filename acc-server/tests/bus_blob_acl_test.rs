//! Integration tests for blob download access control (allowed_agents).
//!
//! Scenarios covered:
//!   1.  Public blob (empty allowed_agents) — any authenticated caller can download.
//!   2.  Restricted blob — listed agent (token matches agents registry) can download (200).
//!   3.  Restricted blob — unlisted agent token → 403 access_denied.
//!   4.  Restricted blob — unknown / random token → 403 access_denied.
//!   5.  Restricted blob — unauthenticated request → 401.
//!   6.  allowed_agents list is returned in the upload response.
//!   7.  allowed_agents is stored in blob metadata (GET /api/bus/blobs/:id).
//!   8.  Single-agent restriction: exactly one permitted agent.
//!   9.  Multi-agent restriction: multiple permitted agents, all can download.
//!  10.  Multi-agent restriction: agent not in list → 403.
//!  11.  POST /api/bus/blob (binary) via query param ?allowed_agents=boris,natasha.
//!  12.  POST /bus/blob alias respects allowed_agents.
//!  13.  POST /api/bus/blob multipart with allowed_agents form field (comma-separated).
//!  14.  POST /api/bus/blob multipart with allowed_agents form field (JSON array).
//!  15.  POST /api/bus/blobs/upload (JSON+base64) with allowed_agents array.
//!  16.  403 response body includes "allowed_agents" list for diagnostics.
//!  17.  403 response body includes "error":"access_denied".
//!  18.  Public blob after restricted one: no cross-contamination.
//!  19.  Uploader token is not automatically in allowed_agents (no implicit grant).
//!  20.  Empty string ?allowed_agents= treated as public (no restriction).
mod helpers;

use axum::{body::Body, http::{Request, StatusCode}};
use serde_json::json;

// ─────────────────────────────────────────────────────────────────────────────
// Agent names and tokens used throughout.
// Each token must be in auth_tokens (via TestServer::with_agent_tokens) AND
// the agent record must be in the agents store (via srv.seed_agent) so that
// agent_from_token() can resolve Bearer token → agent name.
// ─────────────────────────────────────────────────────────────────────────────
const AGENT_BORIS:    &str = "boris";
const TOKEN_BORIS:    &str = "token-boris-secret";
const AGENT_NATASHA:  &str = "natasha";
const TOKEN_NATASHA:  &str = "token-natasha-secret";
const AGENT_BULLWINK: &str = "bullwinkle";
const TOKEN_BULLWINK: &str = "token-bullwinkle-secret";

/// All agent tokens used — passed to TestServer::with_agent_tokens so they
/// pass is_authed() checks at every endpoint.
const ALL_AGENT_TOKENS: &[&str] = &[TOKEN_BORIS, TOKEN_NATASHA, TOKEN_BULLWINK];

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a GET /api/bus/blobs/:id/download request with a specific bearer token.
fn download_req(blob_id: &str, bearer: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/bus/blobs/{blob_id}/download"))
        .header("Authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap()
}

/// Build a GET download request with NO Authorization header.
fn download_req_no_auth(blob_id: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/bus/blobs/{blob_id}/download"))
        .body(Body::empty())
        .unwrap()
}

/// Upload a blob via POST /api/bus/blob (raw binary) and return the parsed
/// response body. `allowed_agents_qs` is the raw query-string fragment,
/// e.g. `"allowed_agents=boris,natasha"` or `""`.
async fn upload_raw(srv: &helpers::TestServer, allowed_agents_qs: &str) -> serde_json::Value {
    let path = if allowed_agents_qs.is_empty() {
        "/api/bus/blob".to_string()
    } else {
        format!("/api/bus/blob?{allowed_agents_qs}")
    };
    let req = Request::builder()
        .method("POST")
        .uri(&path)
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .header("Content-Type", "text/plain")
        .body(Body::from(b"acl test payload".to_vec()))
        .unwrap();
    helpers::body_json(helpers::call(&srv.app, req).await).await
}

/// Upload a blob via POST /api/bus/blobs/upload (JSON+base64, text/plain)
/// with an explicit allowed_agents list.
async fn upload_json_b64(
    srv: &helpers::TestServer,
    allowed_agents: Vec<&str>,
) -> serde_json::Value {
    let body = json!({
        "mime_type":      "text/plain",
        "enc":            "none",
        "data":           "json upload acl payload",
        "allowed_agents": allowed_agents,
    });
    helpers::body_json(
        helpers::call(&srv.app, helpers::post_json("/api/bus/blobs/upload", &body)).await,
    )
    .await
}

/// Build a multipart/form-data POST /api/bus/blob request with an
/// `allowed_agents` text field set to `allowed_agents_field`.
fn multipart_upload_req(allowed_agents_field: &str) -> Request<Body> {
    let boundary = "aclboundary001";
    let mut body: Vec<u8> = Vec::new();

    // file part
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.txt\"\r\nContent-Type: text/plain\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(b"multipart acl payload");
    body.extend_from_slice(b"\r\n");

    // allowed_agents part
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"allowed_agents\"\r\n\r\n{allowed_agents_field}\r\n"
        )
        .as_bytes(),
    );

    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    Request::builder()
        .method("POST")
        .uri("/api/bus/blob")
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .header(
            "Content-Type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(Body::from(body))
        .unwrap()
}


// ─────────────────────────────────────────────────────────────────────────────
// 1. Public blob — any authenticated caller can download
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn public_blob_any_authed_caller_can_download() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    let up = upload_raw(&srv, "").await;
    assert_eq!(up["ok"], json!(true), "upload failed: {up}");
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    // Download with any registered agent token — must succeed
    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BORIS)).await;
    assert_eq!(
        resp.status(), StatusCode::OK,
        "public blob must be downloadable by any authed caller"
    );
}

#[tokio::test]
async fn public_blob_allowed_agents_is_empty_array_in_upload_response() {
    let srv = helpers::TestServer::new().await;
    let up = upload_raw(&srv, "").await;
    assert_eq!(up["ok"], json!(true));
    assert_eq!(
        up["allowed_agents"], json!([]),
        "public blob must have allowed_agents:[] in upload response"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Restricted blob — listed agent can download (200)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn restricted_blob_listed_agent_can_download() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS, TOKEN_BORIS).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    assert_eq!(up["ok"], json!(true), "upload failed: {up}");
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BORIS)).await;
    assert_eq!(
        resp.status(), StatusCode::OK,
        "listed agent must be able to download the restricted blob"
    );
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Restricted blob — unlisted agent token → 403
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn restricted_blob_unlisted_agent_gets_403() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,   TOKEN_BORIS).await;
    srv.seed_agent(AGENT_NATASHA, TOKEN_NATASHA).await;

    // Upload restricted to boris only
    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    // natasha tries to download — must be denied
    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "unlisted agent must receive 403");
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Restricted blob — unrecognised / random token → 403
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn restricted_blob_unknown_token_gets_403() {
    // A token that passes is_authed (in auth_tokens) but maps to no agent name
    let unknown_token = "completely-random-unknown-token-xyz";
    let srv = helpers::TestServer::with_agent_tokens(&[unknown_token]).await;
    srv.seed_agent(AGENT_BORIS, TOKEN_BORIS).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, unknown_token)).await;
    assert_eq!(
        resp.status(), StatusCode::FORBIDDEN,
        "unrecognised token (no agent record) must be denied on a restricted blob"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. Restricted blob — unauthenticated request → 401
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn restricted_blob_no_auth_header_gets_401() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS, TOKEN_BORIS).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req_no_auth(&blob_id)).await;
    assert_eq!(
        resp.status(), StatusCode::UNAUTHORIZED,
        "request without any auth header must get 401, not 403"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. allowed_agents list is returned in upload response
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn upload_response_contains_allowed_agents() {
    let srv = helpers::TestServer::new().await;
    let up = upload_raw(&srv, "allowed_agents=boris,natasha").await;
    assert_eq!(up["ok"], json!(true), "upload failed: {up}");
    let agents = up["allowed_agents"].as_array().expect("allowed_agents must be an array");
    let names: Vec<&str> = agents.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"boris"),   "boris must be in allowed_agents");
    assert!(names.contains(&"natasha"), "natasha must be in allowed_agents");
    assert_eq!(names.len(), 2, "exactly 2 agents expected");
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. allowed_agents is stored in blob metadata
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn allowed_agents_persisted_in_blob_meta() {
    let srv = helpers::TestServer::new().await;
    let up = upload_raw(&srv, "allowed_agents=boris,natasha").await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let meta = helpers::body_json(
        helpers::call(&srv.app, helpers::get(&format!("/api/bus/blobs/{blob_id}"))).await,
    )
    .await;

    let agents = meta["allowed_agents"]
        .as_array()
        .expect("allowed_agents must be an array in blob meta");
    let names: Vec<&str> = agents.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"boris"),   "boris must persist in meta");
    assert!(names.contains(&"natasha"), "natasha must persist in meta");
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. Single-agent restriction
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn single_agent_restriction_only_that_agent_succeeds() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_NATASHA,  TOKEN_NATASHA).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_NATASHA}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let r1 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(r1.status(), StatusCode::OK, "natasha must be allowed");

    let r2 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(r2.status(), StatusCode::FORBIDDEN, "bullwinkle must be denied");
}

// ─────────────────────────────────────────────────────────────────────────────
// 9. Multi-agent restriction — all listed agents can download
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_agent_restriction_all_listed_can_download() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,   TOKEN_BORIS).await;
    srv.seed_agent(AGENT_NATASHA, TOKEN_NATASHA).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS},{AGENT_NATASHA}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let r1 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BORIS)).await;
    assert_eq!(r1.status(), StatusCode::OK, "boris must be allowed");

    let r2 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(r2.status(), StatusCode::OK, "natasha must be allowed");
}

// ─────────────────────────────────────────────────────────────────────────────
// 10. Multi-agent restriction — agent not in list → 403
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_agent_restriction_unlisted_agent_denied() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,    TOKEN_BORIS).await;
    srv.seed_agent(AGENT_NATASHA,  TOKEN_NATASHA).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS},{AGENT_NATASHA}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "bullwinkle must be denied");
}


// ─────────────────────────────────────────────────────────────────────────────
// 11. POST /api/bus/blob (raw binary) via ?allowed_agents= query param
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn raw_binary_upload_allowed_agents_query_param_enforced() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,   TOKEN_BORIS).await;
    srv.seed_agent(AGENT_NATASHA, TOKEN_NATASHA).await;

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/bus/blob?allowed_agents={AGENT_BORIS}"))
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .header("Content-Type", "text/plain")
        .body(Body::from(b"raw binary with acl".to_vec()))
        .unwrap();
    let up = helpers::body_json(helpers::call(&srv.app, req).await).await;
    assert_eq!(up["ok"], json!(true), "upload failed: {up}");
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let r1 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BORIS)).await;
    assert_eq!(r1.status(), StatusCode::OK, "boris must succeed");

    let r2 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(r2.status(), StatusCode::FORBIDDEN, "natasha must be denied");
}

// ─────────────────────────────────────────────────────────────────────────────
// 12. /bus/blob alias respects allowed_agents
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bus_blob_alias_respects_allowed_agents() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_NATASHA,  TOKEN_NATASHA).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let req = Request::builder()
        .method("POST")
        .uri(format!("/bus/blob?allowed_agents={AGENT_NATASHA}"))
        .header("Authorization", format!("Bearer {}", helpers::TEST_TOKEN))
        .header("Content-Type", "text/plain")
        .body(Body::from(b"alias acl test".to_vec()))
        .unwrap();
    let up = helpers::body_json(helpers::call(&srv.app, req).await).await;
    assert_eq!(up["ok"], json!(true), "alias upload failed: {up}");
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let r1 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(r1.status(), StatusCode::OK, "natasha must be allowed via /bus/blob alias");

    let r2 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(r2.status(), StatusCode::FORBIDDEN, "bullwinkle must be denied via /bus/blob alias");
}

// ─────────────────────────────────────────────────────────────────────────────
// 13. Multipart upload with allowed_agents comma-separated form field
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multipart_allowed_agents_comma_separated_field_enforced() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,   TOKEN_BORIS).await;
    srv.seed_agent(AGENT_NATASHA, TOKEN_NATASHA).await;

    let req = multipart_upload_req(&format!("{AGENT_BORIS},{AGENT_NATASHA}"));
    let up = helpers::body_json(helpers::call(&srv.app, req).await).await;
    assert_eq!(up["ok"], json!(true), "multipart upload failed: {up}");
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let r1 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BORIS)).await;
    assert_eq!(r1.status(), StatusCode::OK, "boris must be allowed (multipart comma)");

    let r2 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(r2.status(), StatusCode::OK, "natasha must be allowed (multipart comma)");
}

#[tokio::test]
async fn multipart_allowed_agents_unlisted_agent_denied() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,    TOKEN_BORIS).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let req = multipart_upload_req(AGENT_BORIS);
    let up = helpers::body_json(helpers::call(&srv.app, req).await).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "unlisted agent must be denied (multipart)");
}

// ─────────────────────────────────────────────────────────────────────────────
// 14. Multipart upload with allowed_agents as JSON array form field
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multipart_allowed_agents_json_array_field_enforced() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,    TOKEN_BORIS).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    // Supply allowed_agents as a JSON array string in the form field
    let req = multipart_upload_req(&format!("[\"{AGENT_BORIS}\"]"));
    let up = helpers::body_json(helpers::call(&srv.app, req).await).await;
    assert_eq!(up["ok"], json!(true), "JSON array multipart upload failed: {up}");
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let r1 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BORIS)).await;
    assert_eq!(r1.status(), StatusCode::OK, "boris must be allowed (JSON array field)");

    let r2 = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(r2.status(), StatusCode::FORBIDDEN, "bullwinkle must be denied (JSON array field)");
}

// ─────────────────────────────────────────────────────────────────────────────
// 15. POST /api/bus/blobs/upload (JSON+base64) with allowed_agents array
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn json_b64_upload_allowed_agents_enforced() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_NATASHA,  TOKEN_NATASHA).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let up = upload_json_b64(&srv, vec![AGENT_NATASHA]).await;
    let blob_id = up["blob_id"].as_str().expect("blob_id missing in JSON upload response");

    let r1 = helpers::call(&srv.app, download_req(blob_id, TOKEN_NATASHA)).await;
    assert_eq!(r1.status(), StatusCode::OK, "natasha must be allowed (json+b64)");

    let r2 = helpers::call(&srv.app, download_req(blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(r2.status(), StatusCode::FORBIDDEN, "bullwinkle must be denied (json+b64)");
}

#[tokio::test]
async fn json_b64_upload_public_when_allowed_agents_empty() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS, TOKEN_BORIS).await;

    let up = upload_json_b64(&srv, vec![]).await;
    let blob_id = up["blob_id"].as_str().expect("blob_id missing");

    let resp = helpers::call(&srv.app, download_req(blob_id, TOKEN_BORIS)).await;
    assert_eq!(
        resp.status(), StatusCode::OK,
        "public json+b64 blob must be downloadable by any authed agent"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 16 & 17. 403 response body shape
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn forbidden_response_body_has_error_access_denied() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,    TOKEN_BORIS).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = helpers::body_json(resp).await;
    assert_eq!(
        body["error"].as_str().unwrap(), "access_denied",
        "error field must be 'access_denied'; got: {body}"
    );
}

#[tokio::test]
async fn forbidden_response_body_includes_allowed_agents_list() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,    TOKEN_BORIS).await;
    srv.seed_agent(AGENT_NATASHA,  TOKEN_NATASHA).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS},{AGENT_NATASHA}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_BULLWINK)).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = helpers::body_json(resp).await;
    let list = body["allowed_agents"]
        .as_array()
        .expect("403 body must include 'allowed_agents' array for diagnostics");
    let names: Vec<&str> = list.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&AGENT_BORIS),   "allowed_agents in 403 must include boris");
    assert!(names.contains(&AGENT_NATASHA), "allowed_agents in 403 must include natasha");
}

// ─────────────────────────────────────────────────────────────────────────────
// 18. Public blob after restricted one — no cross-contamination
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn public_blob_after_restricted_no_contamination() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS,    TOKEN_BORIS).await;
    srv.seed_agent(AGENT_BULLWINK, TOKEN_BULLWINK).await;

    // First upload: restricted to boris
    let up_restricted = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    let restricted_id = up_restricted["blob_id"].as_str().unwrap().to_string();

    // Second upload: public (no restriction)
    let up_public = upload_raw(&srv, "").await;
    let public_id = up_public["blob_id"].as_str().unwrap().to_string();

    // The public blob must be downloadable by bullwinkle
    let r1 = helpers::call(&srv.app, download_req(&public_id, TOKEN_BULLWINK)).await;
    assert_eq!(
        r1.status(), StatusCode::OK,
        "public blob must not inherit restrictions from a previously uploaded restricted blob"
    );

    // The restricted blob must still deny bullwinkle
    let r2 = helpers::call(&srv.app, download_req(&restricted_id, TOKEN_BULLWINK)).await;
    assert_eq!(r2.status(), StatusCode::FORBIDDEN, "restricted blob still denies bullwinkle");
}

// ─────────────────────────────────────────────────────────────────────────────
// 19. Uploader token is NOT automatically in allowed_agents (no implicit grant)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn uploader_not_implicitly_granted_access_to_restricted_blob() {
    // TEST_TOKEN (the upload token) is in auth_tokens so it passes is_authed,
    // but it has no agent record → agent_from_token returns None → empty string,
    // which never matches a named entry.
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_BORIS, TOKEN_BORIS).await;

    let up = upload_raw(&srv, &format!("allowed_agents={AGENT_BORIS}")).await;
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    // The uploader (TEST_TOKEN) is not in allowed_agents — must be denied
    let resp = helpers::call(&srv.app, download_req(&blob_id, helpers::TEST_TOKEN)).await;
    assert_eq!(
        resp.status(), StatusCode::FORBIDDEN,
        "uploader token must not get implicit access on a restricted blob"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 20. Empty string ?allowed_agents= treated as public (no restriction)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_allowed_agents_query_param_means_public() {
    let srv = helpers::TestServer::with_agent_tokens(ALL_AGENT_TOKENS).await;
    srv.seed_agent(AGENT_NATASHA, TOKEN_NATASHA).await;

    let up = upload_raw(&srv, "allowed_agents=").await;
    assert_eq!(up["ok"], json!(true), "upload failed: {up}");
    assert_eq!(
        up["allowed_agents"], json!([]),
        "empty allowed_agents param must produce an empty list (public)"
    );
    let blob_id = up["blob_id"].as_str().unwrap().to_string();

    let resp = helpers::call(&srv.app, download_req(&blob_id, TOKEN_NATASHA)).await;
    assert_eq!(
        resp.status(), StatusCode::OK,
        "public blob (empty param) must be downloadable by any authed agent"
    );
}

