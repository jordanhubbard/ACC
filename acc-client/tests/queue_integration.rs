//! Integration tests for the queue + items + heartbeat API surface.

use acc_client::{model::HeartbeatRequest, Client, Error};
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn client_for(server: &MockServer) -> Client {
    Client::new(server.uri(), "test-token").unwrap()
}

#[tokio::test]
async fn queue_list_accepts_bare_array_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"id": "wq-1", "status": "pending", "priority": "normal", "title": "item one"},
            {"id": "wq-2", "status": "pending", "priority": "urgent", "title": "item two",
             "journal": [{"ts":"2026-04-23T00:00:00Z","text":"hi"}],
             "branch": "main"}
        ])))
        .mount(&server)
        .await;

    let client = client_for(&server).await;
    let items = client.queue().list().await.unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, "wq-1");
    // Unknown fields preserved on the second item
    assert!(items[1].extra.contains_key("journal"));
    assert!(items[1].extra.contains_key("branch"));
    // Priority stays a string so unexpected values like "urgent" don't break us
    assert_eq!(items[1].priority.as_deref(), Some("urgent"));
}

#[tokio::test]
async fn queue_list_accepts_wrapped_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {"id": "wq-1", "status": "in-progress"}
            ]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server).await;
    let items = client.queue().list().await.unwrap();
    assert_eq!(items.len(), 1);
    // Hyphenated status flows through unchanged
    assert_eq!(items[0].status.as_deref(), Some("in-progress"));
}

#[tokio::test]
async fn item_claim_conflict_maps_to_typed_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/item/wq-9/claim"))
        .and(body_partial_json(json!({"agent": "agent-a"})))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({"error": "already_claimed"})))
        .mount(&server)
        .await;

    let client = client_for(&server).await;
    let err = client
        .items()
        .claim("wq-9", "agent-a", None)
        .await
        .unwrap_err();
    match err {
        Error::Conflict(body) => assert_eq!(body.error, "already_claimed"),
        other => panic!("expected Conflict, got {other:?}"),
    }
}

#[tokio::test]
async fn item_complete_sends_result_and_resolution() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/item/wq-1/complete"))
        .and(body_partial_json(
            json!({"agent": "a", "result": "done", "resolution": "fixed"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;

    let client = client_for(&server).await;
    client
        .items()
        .complete("wq-1", "a", Some("done"), Some("fixed"))
        .await
        .unwrap();
}

#[tokio::test]
async fn heartbeat_posts_to_named_agent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/heartbeat/agent-a"))
        .and(body_partial_json(json!({"status": "ok"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;

    let client = client_for(&server).await;
    let hb = HeartbeatRequest {
        status: Some("ok".into()),
        ..Default::default()
    };
    client.items().heartbeat("agent-a", &hb).await.unwrap();
}
