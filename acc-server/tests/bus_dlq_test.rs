mod helpers;

use axum::http::StatusCode;
use serde_json::json;

// ─────────────────────────────────────────────────────────────────────────────
// GET /api/bus/dlq  — list
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dlq_list_empty_on_fresh_state() {
    let srv = helpers::TestServer::new().await;
    let resp = helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    let entries = body["entries"].as_array().expect("entries array");
    assert!(entries.is_empty(), "fresh DLQ must be empty");
}

#[tokio::test]
async fn dlq_list_requires_auth() {
    let srv = helpers::TestServer::new().await;
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/api/bus/dlq")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = helpers::call(&srv.app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dlq_list_returns_written_entries() {
    let srv = helpers::TestServer::new().await;

    // Write two entries directly via dlq_write so we control exact contents.
    acc_server::routes::bus::dlq_write(
        &srv.state.dlq_path,
        json!({"type": "ping", "from": "boris"}),
        "connection_refused",
        0,
    )
    .await;
    acc_server::routes::bus::dlq_write(
        &srv.state.dlq_path,
        json!({"type": "memo", "from": "natasha"}),
        "timeout",
        1,
    )
    .await;

    let resp = helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2, "must return both entries");

    // Verify required fields are present on every entry.
    for e in entries {
        assert!(e["id"].as_str().is_some(), "id must be a string");
        assert!(e["ts"].as_str().is_some(), "ts must be a string");
        assert!(e["error"].as_str().is_some(), "error must be a string");
        assert!(e["message"].is_object(), "message must be an object");
        assert!(e["retry_count"].is_number(), "retry_count must be a number");
    }
}

#[tokio::test]
async fn dlq_list_max_age_excludes_old_entries() {
    let srv = helpers::TestServer::new().await;

    // Write one entry with a timestamp 2 hours ago by directly appending to
    // the DLQ file with a hand-crafted past timestamp.
    let old_ts = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let old_entry = json!({
        "id":          "dlq-old-entry-111",
        "ts":          old_ts,
        "error":       "ancient failure",
        "message":     {"type": "ping"},
        "retry_count": 0,
    });
    let path = &srv.state.dlq_path;
    tokio::fs::create_dir_all(
        std::path::Path::new(path).parent().unwrap()
    ).await.unwrap();
    tokio::fs::write(path, format!("{}
", old_entry.to_string()))
        .await
        .unwrap();

    // Write a fresh entry via the helper.
    acc_server::routes::bus::dlq_write(
        path,
        json!({"type": "text", "from": "rocky"}),
        "recent failure",
        0,
    )
    .await;

    // max_age_seconds=300 (5 min) must return only the recent entry.
    let resp = helpers::call(
        &srv.app,
        helpers::get("/api/bus/dlq?max_age_seconds=300"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1, "only the recent entry must pass the age filter");
    assert_ne!(
        entries[0]["id"].as_str().unwrap(),
        "dlq-old-entry-111",
        "old entry must be excluded"
    );
}

#[tokio::test]
async fn dlq_list_no_max_age_returns_all_entries() {
    let srv = helpers::TestServer::new().await;

    // Write one old and one new entry.
    let old_ts = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let old_entry = json!({
        "id":          "dlq-old-222",
        "ts":          old_ts,
        "error":       "old",
        "message":     {"type": "ping"},
        "retry_count": 0,
    });
    let path = &srv.state.dlq_path;
    tokio::fs::create_dir_all(
        std::path::Path::new(path).parent().unwrap()
    ).await.unwrap();
    tokio::fs::write(path, format!("{}
", old_entry.to_string()))
        .await
        .unwrap();

    acc_server::routes::bus::dlq_write(
        path,
        json!({"type": "text", "from": "rocky"}),
        "recent",
        0,
    )
    .await;

    // No filter — both entries must come back.
    let resp = helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await;
    let body = helpers::body_json(resp).await;
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 2, "without max_age both entries must be returned");
}

#[tokio::test]
async fn dlq_list_skips_malformed_lines() {
    let srv = helpers::TestServer::new().await;

    let path = &srv.state.dlq_path;
    tokio::fs::create_dir_all(
        std::path::Path::new(path).parent().unwrap()
    ).await.unwrap();

    // Mix valid and malformed lines.
    let valid = json!({
        "id": "dlq-valid-333", "ts": chrono::Utc::now().to_rfc3339(),
        "error": "ok", "message": {"type": "ping"}, "retry_count": 0,
    });
    let content = format!(
        "not json at all
{}
{{broken
",
        valid.to_string()
    );
    tokio::fs::write(path, content).await.unwrap();

    let resp = helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    let entries = body["entries"].as_array().expect("entries array");
    assert_eq!(entries.len(), 1, "only the valid line must be returned");
    assert_eq!(entries[0]["id"].as_str().unwrap(), "dlq-valid-333");
}

// ─────────────────────────────────────────────────────────────────────────────
// POST /api/bus/dlq/redeliver  — single-entry mode
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dlq_redeliver_single_returns_ok_and_seq() {
    let srv = helpers::TestServer::new().await;

    acc_server::routes::bus::dlq_write(
        &srv.state.dlq_path,
        json!({"type": "ping", "from": "boris", "to": "rocky"}),
        "test_error",
        0,
    )
    .await;

    // Read back the id that was assigned.
    let list = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await,
    )
    .await;
    let dlq_id = list["entries"][0]["id"]
        .as_str()
        .expect("dlq_id")
        .to_string();

    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/dlq/redeliver", &json!({"dlq_id": dlq_id})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["redelivered"], json!(true));
    assert!(body["seq"].is_number(), "seq must be present");
}

#[tokio::test]
async fn dlq_redeliver_single_requires_auth() {
    let srv = helpers::TestServer::new().await;

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/bus/dlq/redeliver")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(json!({"dlq_id": "any"}).to_string()))
        .unwrap();
    let resp = helpers::call(&srv.app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dlq_redeliver_unknown_dlq_id_returns_404() {
    let srv = helpers::TestServer::new().await;
    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/dlq/redeliver",
            &json!({"dlq_id": "dlq-does-not-exist-999"}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["error"].as_str().unwrap(), "dlq_entry_not_found");
}

#[tokio::test]
async fn dlq_redeliver_redelivered_message_appears_in_bus_history() {
    let srv = helpers::TestServer::new().await;

    let unique_subject = "dlq-redeliver-subject-probe-99991";

    acc_server::routes::bus::dlq_write(
        &srv.state.dlq_path,
        json!({"type": "text", "from": "natasha", "subject": unique_subject}),
        "dispatch_error",
        0,
    )
    .await;

    let list = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await,
    )
    .await;
    let dlq_id = list["entries"][0]["id"]
        .as_str()
        .expect("dlq_id")
        .to_string();

    helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/dlq/redeliver", &json!({"dlq_id": dlq_id})),
    )
    .await;

    // Allow async log write to flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().expect("messages array");

    let found = arr.iter().any(|m| {
        m["dlq_redelivered"].as_bool() == Some(true)
            && m["subject"].as_str() == Some(unique_subject)
    });
    assert!(found, "redelivered message must appear in bus/messages with dlq_redelivered=true");
}

#[tokio::test]
async fn dlq_redeliver_message_carries_dlq_id_field() {
    let srv = helpers::TestServer::new().await;

    acc_server::routes::bus::dlq_write(
        &srv.state.dlq_path,
        json!({"type": "ping", "from": "boris"}),
        "err",
        0,
    )
    .await;

    let list = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await,
    )
    .await;
    let dlq_id = list["entries"][0]["id"]
        .as_str()
        .expect("dlq_id")
        .to_string();

    helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/dlq/redeliver", &json!({"dlq_id": dlq_id})),
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().expect("bus messages array");
    let m = arr
        .iter()
        .find(|m| m["dlq_id"].as_str() == Some(&dlq_id))
        .expect("redelivered message with matching dlq_id must be in history");

    assert_eq!(m["dlq_redelivered"], json!(true));
    assert!(m["seq"].is_number(), "redelivered message must have a seq");
    assert!(m["ts"].as_str().is_some(), "redelivered message must have a fresh ts");
}

#[tokio::test]
async fn dlq_redeliver_single_with_max_age_excludes_too_old() {
    let srv = helpers::TestServer::new().await;

    // Write an entry timestamped 2 hours ago.
    let old_ts = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let old_entry = json!({
        "id":          "dlq-old-age-555",
        "ts":          old_ts,
        "error":       "old",
        "message":     {"type": "ping"},
        "retry_count": 0,
    });
    let path = &srv.state.dlq_path;
    tokio::fs::create_dir_all(std::path::Path::new(path).parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, format!("{}
", old_entry.to_string()))
        .await
        .unwrap();

    // Request single redeliver with max_age_seconds=300 — entry is 2 h old → 404.
    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/dlq/redeliver",
            &json!({"dlq_id": "dlq-old-age-555", "max_age_seconds": 300}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ─────────────────────────────────────────────────────────────────────────────
// POST /api/bus/dlq/redeliver  — bulk mode (no dlq_id)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dlq_redeliver_bulk_empty_queue_returns_zero_counts() {
    let srv = helpers::TestServer::new().await;
    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/dlq/redeliver", &json!({})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["total"], json!(0));
    assert_eq!(body["succeeded"], json!(0));
}

#[tokio::test]
async fn dlq_redeliver_bulk_replays_all_entries() {
    let srv = helpers::TestServer::new().await;

    for i in 0..3u32 {
        acc_server::routes::bus::dlq_write(
            &srv.state.dlq_path,
            json!({"type": "ping", "index": i}),
            "bulk_test_error",
            0,
        )
        .await;
    }

    let resp = helpers::call(
        &srv.app,
        helpers::post_json("/api/bus/dlq/redeliver", &json!({})),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["total"], json!(3));
    assert_eq!(body["succeeded"], json!(3));

    // All 3 redelivered messages must appear in bus history.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().expect("bus messages");
    let redelivered_count = arr
        .iter()
        .filter(|m| m["dlq_redelivered"].as_bool() == Some(true))
        .count();
    assert_eq!(redelivered_count, 3, "all 3 entries must appear as redelivered");
}

#[tokio::test]
async fn dlq_redeliver_bulk_max_age_filters_candidates() {
    let srv = helpers::TestServer::new().await;

    // One old entry (2 h ago).
    let old_ts = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let old_entry = json!({
        "id": "dlq-bulk-old-666", "ts": old_ts,
        "error": "old", "message": {"type": "ping"}, "retry_count": 0,
    });
    let path = &srv.state.dlq_path;
    tokio::fs::create_dir_all(std::path::Path::new(path).parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, format!("{}
", old_entry.to_string()))
        .await
        .unwrap();

    // Two fresh entries.
    for _ in 0..2u32 {
        acc_server::routes::bus::dlq_write(
            path,
            json!({"type": "text", "from": "rocky"}),
            "recent",
            0,
        )
        .await;
    }

    // Bulk redeliver with max_age=300 s — only the 2 fresh entries qualify.
    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/dlq/redeliver",
            &json!({"max_age_seconds": 300}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["total"], json!(2), "only 2 recent entries must be candidates");
    assert_eq!(body["succeeded"], json!(2));
}

// ─────────────────────────────────────────────────────────────────────────────
// dlq_write helper (unit-level)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dlq_write_creates_file_and_appends_valid_jsonl() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bus-dlq.jsonl");
    let path_str = path.to_string_lossy().to_string();

    acc_server::routes::bus::dlq_write(
        &path_str,
        json!({"type": "ping", "from": "boris"}),
        "test_error",
        0,
    )
    .await;

    let content = tokio::fs::read_to_string(&path_str).await.unwrap();
    let line = content.lines().next().expect("at least one line");
    let parsed: serde_json::Value = serde_json::from_str(line).expect("valid JSON");

    assert!(parsed["id"].as_str().unwrap().starts_with("dlq-"));
    assert!(parsed["ts"].as_str().is_some());
    assert_eq!(parsed["error"].as_str().unwrap(), "test_error");
    assert_eq!(parsed["retry_count"], json!(0));
    assert_eq!(parsed["message"]["type"].as_str().unwrap(), "ping");
}

#[tokio::test]
async fn dlq_write_increments_retry_count_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bus-dlq.jsonl");
    let path_str = path.to_string_lossy().to_string();

    acc_server::routes::bus::dlq_write(
        &path_str,
        json!({"type": "ping"}),
        "err",
        3,
    )
    .await;

    let content = tokio::fs::read_to_string(&path_str).await.unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(parsed["retry_count"], json!(3));
}

#[tokio::test]
async fn dlq_write_appends_multiple_entries_each_on_own_line() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bus-dlq.jsonl");
    let path_str = path.to_string_lossy().to_string();

    for i in 0..5u32 {
        acc_server::routes::bus::dlq_write(
            &path_str,
            json!({"index": i}),
            &format!("err-{i}"),
            i,
        )
        .await;
    }

    let content = tokio::fs::read_to_string(&path_str).await.unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 5);

    for (i, line) in lines.iter().enumerate() {
        let v: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|_| panic!("line {i} must be valid JSON"));
        assert_eq!(v["retry_count"], json!(i as u32));
        assert_eq!(v["message"]["index"], json!(i as u32));
    }
}

#[tokio::test]
async fn dlq_write_creates_missing_parent_directories() {
    let tmp = tempfile::tempdir().unwrap();
    // Nested path that does not exist yet.
    let path_str = tmp
        .path()
        .join("a")
        .join("b")
        .join("c")
        .join("bus-dlq.jsonl")
        .to_string_lossy()
        .to_string();

    // Must not panic; parent dirs must be created automatically.
    acc_server::routes::bus::dlq_write(
        &path_str,
        json!({"type": "ping"}),
        "err",
        0,
    )
    .await;

    assert!(
        tokio::fs::read_to_string(&path_str).await.is_ok(),
        "DLQ file must exist after write"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// DLQ entry schema validation
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn dlq_entry_schema_has_all_required_fields() {
    let srv = helpers::TestServer::new().await;

    acc_server::routes::bus::dlq_write(
        &srv.state.dlq_path,
        json!({"type": "rcc.exec", "from": "rocky", "to": "boris", "body": "cmd"}),
        "unhandled_panic",
        2,
    )
    .await;

    let resp = helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await;
    let body = helpers::body_json(resp).await;
    let e = &body["entries"][0];

    // id: "dlq-<uuid>" prefix
    assert!(
        e["id"].as_str().unwrap().starts_with("dlq-"),
        "id must start with dlq-"
    );
    // ts: parseable ISO-8601
    assert!(
        chrono::DateTime::parse_from_rfc3339(e["ts"].as_str().unwrap()).is_ok(),
        "ts must be a valid ISO-8601 timestamp"
    );
    // error: non-empty string
    assert!(!e["error"].as_str().unwrap().is_empty(), "error must be non-empty");
    // message: the original payload
    assert_eq!(e["message"]["type"].as_str().unwrap(), "rcc.exec");
    // retry_count: matches what we passed in
    assert_eq!(e["retry_count"], json!(2));
}

// ─────────────────────────────────────────────────────────────────────────────
// Bus send — normal dispatch still works (regression guard)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bus_send_succeeds_and_message_in_history() {
    let srv = helpers::TestServer::new().await;

    let unique = "dlq-regression-subject-77771";
    let resp = helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({"from": "rocky", "to": "all", "type": "ping", "subject": unique}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], json!(true));

    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    let msgs = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/messages")).await,
    )
    .await;
    let arr = msgs.as_array().expect("bus messages array");
    assert!(
        arr.iter().any(|m| m["subject"].as_str() == Some(unique)),
        "sent message must appear in bus history"
    );
}

#[tokio::test]
async fn bus_send_does_not_write_to_dlq_on_normal_success() {
    let srv = helpers::TestServer::new().await;

    helpers::call(
        &srv.app,
        helpers::post_json(
            "/api/bus/send",
            &json!({"from": "boris", "to": "all", "type": "ping"}),
        ),
    )
    .await;

    // DLQ must remain empty after a successful send.
    let list = helpers::body_json(
        helpers::call(&srv.app, helpers::get("/api/bus/dlq")).await,
    )
    .await;
    let entries = list["entries"].as_array().expect("entries array");
    assert!(
        entries.is_empty(),
        "DLQ must be empty after a normal successful send; got {} entries",
        entries.len()
    );
}
