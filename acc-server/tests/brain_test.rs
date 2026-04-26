mod helpers;

use axum::http::{Request, StatusCode};
use axum::body::Body;
use serde_json::json;

// ── Status ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_brain_status_no_auth_required() {
    let ts = helpers::TestServer::new().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/brain/status")
        .body(Body::empty())
        .unwrap();
    let resp = helpers::call(&ts.app, req).await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_brain_status_shape() {
    let ts = helpers::TestServer::new().await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/brain/status")
        .body(Body::empty())
        .unwrap();
    let body = helpers::body_json(helpers::call(&ts.app, req).await).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["queueDepth"], 0);
    assert_eq!(body["completedCount"], 0);
    assert!(body["backend"].is_string());
}

// ── Brain request ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_brain_request_requires_auth() {
    let ts = helpers::TestServer::new().await;
    let req = Request::builder()
        .method("POST")
        .uri("/api/brain/request")
        .header("Content-Type", "application/json")
        .body(Body::from(json!({"messages": []}).to_string()))
        .unwrap();
    let resp = helpers::call(&ts.app, req).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_brain_request_requires_messages_field() {
    let ts = helpers::TestServer::new().await;
    let resp = helpers::call(
        &ts.app,
        helpers::post_json("/api/brain/request", &json!({})),
    ).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_brain_request_accepted_and_queued() {
    let ts = helpers::TestServer::new().await;
    let resp = helpers::call(
        &ts.app,
        helpers::post_json("/api/brain/request", &json!({
            "messages": [{"role": "user", "content": "hello"}]
        })),
    ).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = helpers::body_json(resp).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["status"], "queued");
    assert!(body["requestId"].as_str().unwrap().starts_with("brain-"));
}

#[tokio::test]
async fn test_brain_request_increments_queue_depth() {
    let ts = helpers::TestServer::new().await;
    helpers::call(
        &ts.app,
        helpers::post_json("/api/brain/request", &json!({
            "messages": [{"role": "user", "content": "task 1"}]
        })),
    ).await;
    helpers::call(
        &ts.app,
        helpers::post_json("/api/brain/request", &json!({
            "messages": [{"role": "user", "content": "task 2"}]
        })),
    ).await;

    let status_req = Request::builder()
        .method("GET")
        .uri("/api/brain/status")
        .body(Body::empty())
        .unwrap();
    let body = helpers::body_json(helpers::call(&ts.app, status_req).await).await;
    assert_eq!(body["queueDepth"], 2);
}

#[tokio::test]
async fn test_brain_request_with_priority() {
    let ts = helpers::TestServer::new().await;
    let resp = helpers::call(
        &ts.app,
        helpers::post_json("/api/brain/request", &json!({
            "messages": [{"role": "user", "content": "urgent"}],
            "priority": "high",
            "maxTokens": 512,
        })),
    ).await;
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = helpers::body_json(resp).await;
    assert!(body["requestId"].is_string());
}

#[tokio::test]
async fn test_brain_request_ids_are_unique() {
    let ts = helpers::TestServer::new().await;
    let msg = json!({"messages": [{"role": "user", "content": "x"}]});
    let r1 = helpers::body_json(
        helpers::call(&ts.app, helpers::post_json("/api/brain/request", &msg)).await,
    ).await;
    let r2 = helpers::body_json(
        helpers::call(&ts.app, helpers::post_json("/api/brain/request", &msg)).await,
    ).await;
    assert_ne!(r1["requestId"], r2["requestId"]);
}

// ── classify_error unit tests ─────────────────────────────────────────────────

mod classify_error_tests {
    use acc_server::brain::{classify_error, ErrorKind};

    #[test]
    fn status_0_is_transient() {
        // Network / connection failure — no HTTP response received at all.
        assert_eq!(classify_error(0, ""), ErrorKind::Transient);
    }

    #[test]
    fn status_429_is_transient() {
        // Rate-limited responses should be retried.
        assert_eq!(classify_error(429, "rate limit exceeded"), ErrorKind::Transient);
    }

    #[test]
    fn status_500_is_transient() {
        assert_eq!(classify_error(500, "internal server error"), ErrorKind::Transient);
    }

    #[test]
    fn status_502_is_transient() {
        assert_eq!(classify_error(502, "bad gateway"), ErrorKind::Transient);
    }

    #[test]
    fn status_503_is_transient() {
        assert_eq!(classify_error(503, "service unavailable"), ErrorKind::Transient);
    }

    #[test]
    fn status_599_is_transient() {
        // Top of the 5xx range should still be transient.
        assert_eq!(classify_error(599, ""), ErrorKind::Transient);
    }

    #[test]
    fn status_401_is_hard() {
        // Bad credentials — retrying with the same key will not help.
        assert_eq!(classify_error(401, "unauthorized"), ErrorKind::Hard);
    }

    #[test]
    fn status_403_is_hard() {
        // Forbidden — the credential is valid but access is denied.
        assert_eq!(classify_error(403, "forbidden"), ErrorKind::Hard);
    }

    #[test]
    fn status_400_is_hard() {
        // Malformed request — the payload itself is wrong.
        assert_eq!(classify_error(400, "bad request"), ErrorKind::Hard);
    }

    #[test]
    fn status_404_is_hard() {
        // Unknown endpoint — not recoverable by retrying.
        assert_eq!(classify_error(404, "not found"), ErrorKind::Hard);
    }

    #[test]
    fn status_422_is_hard() {
        // Unprocessable entity — the request was syntactically valid but
        // semantically rejected.
        assert_eq!(classify_error(422, "unprocessable entity"), ErrorKind::Hard);
    }

    #[test]
    fn unexpected_status_200_is_hard() {
        // 200 should never reach classify_error in normal flow; if it does,
        // the fallback must be Hard so we don't loop forever.
        assert_eq!(classify_error(200, ""), ErrorKind::Hard);
    }

    #[test]
    fn unexpected_status_301_is_hard() {
        // Redirects are not retried.
        assert_eq!(classify_error(301, "moved permanently"), ErrorKind::Hard);
    }

    #[test]
    fn body_content_does_not_change_classification() {
        // The body parameter is reserved for future use; passing arbitrary
        // content must not flip the classification.
        assert_eq!(
            classify_error(503, r#"{"error":"overloaded","retry_after":60}"#),
            ErrorKind::Transient,
        );
        assert_eq!(
            classify_error(401, r#"{"error":"invalid_api_key"}"#),
            ErrorKind::Hard,
        );
    }
}

// ── failure_reason integration tests ─────────────────────────────────────────

mod failure_reason_tests {
    use acc_server::brain::{BrainQueue, BrainRequest, BrainState};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// Spawn a minimal axum HTTP server on an OS-assigned port and return its
    /// base URL together with a shutdown handle.  The single handler always
    /// responds with the given status code and body string.
    async fn spawn_mock_tokenhub(
        status: u16,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{routing::post, Router};
        use axum::response::IntoResponse;

        let handler = move || async move {
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                body,
            )
                .into_response()
        };

        let app = Router::new().route("/v1/chat/completions", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://127.0.0.1:{}", addr.port());

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        (url, handle)
    }

    /// Build a BrainQueue that is wired to `tokenhub_url` and has its tick
    /// interval set to a large value so the background worker never fires
    /// spontaneously during the test.
    fn make_brain(tokenhub_url: String) -> Arc<BrainQueue> {
        Arc::new(BrainQueue {
            state: RwLock::new(BrainState::default()),
            state_path: "/dev/null".to_string(),
            tokenhub_url,
            tokenhub_key: String::new(),
            models: vec!["test-model".to_string()],
            tick_ms: 3_600_000, // 1 h — never fires on its own
            notify: tokio::sync::Notify::new(),
        })
    }

    fn make_request(id: &str) -> BrainRequest {
        BrainRequest {
            id: id.to_string(),
            messages: vec![json!({"role": "user", "content": "hello"})],
            max_tokens: 64,
            priority: "normal".to_string(),
            created: chrono::Utc::now().to_rfc3339(),
            attempts: vec![],
            status: "pending".to_string(),
            result: None,
            completed_at: None,
            callback_url: None,
            metadata: json!({}),
            failure_reason: None,
        }
    }

    // ── hard error short-circuit ──────────────────────────────────────────────

    /// When the first model returns a hard error (401) and a second model would
    /// have returned a transient error (503), call_model must short-circuit on
    /// the first hard error.  The request must be marked "failed" (not left
    /// "pending" for retry) and failure_reason must reference the hard-error
    /// status, not the transient one.
    ///
    /// This exercises the bug where `last_err` was silently overwritten by each
    /// subsequent model's error string, so process_request would call
    /// classify_error on the *last* model's transient 503 instead of the *first*
    /// model's hard 401.
    #[tokio::test]
    async fn hard_error_from_first_model_short_circuits_loop() {
        use axum::{routing::post, Router};
        use axum::response::IntoResponse;
        use std::sync::{Arc as StdArc, atomic::{AtomicUsize, Ordering}};

        // Counter so we can verify the second model endpoint was never reached.
        let call_count = StdArc::new(AtomicUsize::new(0));
        let call_count_clone = StdArc::clone(&call_count);

        // First model: always returns 401 (hard error).
        let hard_handler = || async {
            (axum::http::StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
        };

        // Second model: would return 503 (transient) — but should never be reached.
        let transient_handler = move || {
            let cc = StdArc::clone(&call_count_clone);
            async move {
                cc.fetch_add(1, Ordering::SeqCst);
                (axum::http::StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable").into_response()
            }
        };

        // Both models are served from the same tokenhub URL but distinguished
        // by the "model" field in the request body.  We route them via a single
        // handler that reads the model name.
        let cc2 = StdArc::clone(&call_count);
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |body: axum::extract::Json<serde_json::Value>| {
                let cc = StdArc::clone(&cc2);
                async move {
                    let model = body.0["model"].as_str().unwrap_or("").to_string();
                    if model == "hard-model" {
                        hard_handler().await
                    } else {
                        // This is the second model — count the call so we can assert it never happens.
                        cc.fetch_add(1, Ordering::SeqCst);
                        transient_handler().await
                    }
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://127.0.0.1:{}", addr.port());
        let _server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        // Two-model brain: hard-model first, transient-model second.
        let brain = Arc::new(BrainQueue {
            state: RwLock::new(BrainState::default()),
            state_path: "/dev/null".to_string(),
            tokenhub_url: url,
            tokenhub_key: String::new(),
            models: vec!["hard-model".to_string(), "transient-model".to_string()],
            tick_ms: 3_600_000,
            notify: tokio::sync::Notify::new(),
        });

        let client = reqwest::Client::new();
        brain.enqueue(make_request("req-short-circuit-1")).await;
        brain.tick(&client).await;

        // The second (transient) model must never have been called.
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            0,
            "call_model must short-circuit on the first hard error and never reach the second model"
        );

        let state = brain.state.read().await;
        assert!(
            state.queue.is_empty(),
            "queue must be empty — request should not be left pending for retry"
        );
        assert_eq!(state.completed.len(), 1);

        let entry = &state.completed[0];
        assert_eq!(
            entry.status, "failed",
            "status must be 'failed', not 'pending' (which would mean retry was scheduled)"
        );
        assert!(
            entry.failure_reason.is_some(),
            "failure_reason must be set after a hard error short-circuit"
        );
        let reason = entry.failure_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("401"),
            "failure_reason must reference the hard-error status (401), not the transient one (503); got: {reason:?}"
        );
    }

    // ── transient error / retry cap ───────────────────────────────────────────

    /// When tokenhub consistently returns a transient error (e.g. 429) the
    /// request must eventually be escalated to `failed` once the attempt count
    /// reaches the cap, rather than looping in the queue forever.
    #[tokio::test]
    async fn transient_error_escalates_to_failed_after_max_attempts() {
        let (url, _server) = spawn_mock_tokenhub(429, "Too Many Requests").await;

        // Use a cap of 3 so the test completes quickly.
        std::env::set_var("BRAIN_MAX_ATTEMPTS", "3");

        let brain = make_brain(url);
        let client = reqwest::Client::new();
        brain.enqueue(make_request("req-cap-1")).await;

        // Drive ticks until the request is drained from the queue.
        // The request should NOT stay in the queue forever.
        for _ in 0..5 {
            brain.tick(&client).await;
            let state = brain.state.read().await;
            if state.queue.is_empty() {
                break;
            }
        }

        std::env::remove_var("BRAIN_MAX_ATTEMPTS");

        let state = brain.state.read().await;
        assert!(
            state.queue.is_empty(),
            "queue must be empty once the retry cap is reached; request must not loop forever"
        );
        assert_eq!(
            state.completed.len(),
            1,
            "the capped request must appear in completed"
        );

        let entry = &state.completed[0];
        assert_eq!(
            entry.status, "failed",
            "status must be 'failed' after exceeding the retry cap, got: {:?}",
            entry.status
        );
        assert!(
            entry.failure_reason.is_some(),
            "failure_reason must be set when the retry cap is exceeded"
        );
        let reason = entry.failure_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("max retry attempts"),
            "failure_reason should describe the cap being exceeded; got: {reason:?}"
        );
        // The attempts array must have at least `max_attempts` entries.
        assert!(
            entry.attempts.len() >= 3,
            "attempts array must record all retry attempts; got {} entries",
            entry.attempts.len()
        );
    }

    /// Each tick that ends in a transient error must still add an entry to the
    /// `attempts` array, providing a full audit trail of every retry.
    #[tokio::test]
    async fn transient_retries_are_recorded_in_attempts() {
        let (url, _server) = spawn_mock_tokenhub(503, "Service Unavailable").await;

        std::env::set_var("BRAIN_MAX_ATTEMPTS", "2");

        let brain = make_brain(url);
        let client = reqwest::Client::new();
        brain.enqueue(make_request("req-cap-attempts")).await;

        // Run enough ticks to exhaust the cap.
        for _ in 0..4 {
            brain.tick(&client).await;
        }

        std::env::remove_var("BRAIN_MAX_ATTEMPTS");

        let state = brain.state.read().await;
        // Should have escalated to failed.
        let entry = state.completed.iter().find(|r| r.id == "req-cap-attempts");
        let entry = entry.expect("request should appear in completed after retry cap");
        assert_eq!(entry.status, "failed");
        // Every attempt — including all the transient retries — must be logged.
        assert!(
            entry.attempts.len() >= 2,
            "all transient retry attempts must be recorded; got {} entries",
            entry.attempts.len()
        );
        // Each attempt entry must carry a timestamp.
        for attempt in &entry.attempts {
            assert!(
                attempt["ts"].is_string(),
                "each attempt entry must have a 'ts' timestamp field"
            );
        }
    }

    /// A transient error that is below the cap must leave the request in the
    /// queue (status = "pending"), not move it to failed/completed prematurely.
    #[tokio::test]
    async fn transient_error_below_cap_stays_pending() {
        let (url, _server) = spawn_mock_tokenhub(503, "Service Unavailable").await;

        // Cap of 5: after 1 tick the request has 1 attempt — still below cap.
        std::env::set_var("BRAIN_MAX_ATTEMPTS", "5");

        let brain = make_brain(url);
        let client = reqwest::Client::new();
        brain.enqueue(make_request("req-below-cap")).await;

        // Single tick — should record one failure and re-queue as pending.
        brain.tick(&client).await;

        std::env::remove_var("BRAIN_MAX_ATTEMPTS");

        let state = brain.state.read().await;
        assert!(
            state.completed.is_empty(),
            "request must not be completed/failed after fewer attempts than the cap"
        );
        let queued = state.queue.iter().find(|r| r.id == "req-below-cap");
        let queued = queued.expect("request should still be in the queue below the cap");
        assert_eq!(
            queued.status, "pending",
            "status must remain 'pending' while below the retry cap"
        );
        assert_eq!(
            queued.attempts.len(),
            1,
            "one attempt must have been recorded"
        );
    }

    // ── hard error ────────────────────────────────────────────────────────────

    /// When tokenhub returns 401 (hard error), the completed entry must carry a
    /// non-null `failure_reason` and the queue must be empty afterwards.
    #[tokio::test]
    async fn hard_error_populates_failure_reason() {
        let (url, _server) = spawn_mock_tokenhub(401, "Unauthorized").await;
        let brain = make_brain(url);
        let client = reqwest::Client::new();

        brain.enqueue(make_request("req-hard-1")).await;

        // Drive one tick manually (no background worker running).
        brain.tick(&client).await;

        let state = brain.state.read().await;
        // The request must have moved from the queue to the completed list.
        assert!(
            state.queue.is_empty(),
            "queue should be empty after a hard error"
        );
        assert_eq!(
            state.completed.len(),
            1,
            "completed list should contain the failed entry"
        );

        let entry = &state.completed[0];
        assert_eq!(entry.status, "failed", "status must be 'failed'");
        assert!(
            entry.failure_reason.is_some(),
            "failure_reason must be populated on a hard error, got None"
        );
        // The reason should reference the HTTP status so it is actionable.
        let reason = entry.failure_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("401"),
            "failure_reason should mention the HTTP status; got: {reason:?}"
        );
        // A failed request must not carry a successful result.
        assert!(
            entry.result.is_none(),
            "result must be None for a failed request"
        );
    }

    /// A 403 Forbidden is also a hard error and must set failure_reason.
    #[tokio::test]
    async fn forbidden_response_populates_failure_reason() {
        let (url, _server) = spawn_mock_tokenhub(403, "Forbidden").await;
        let brain = make_brain(url);
        let client = reqwest::Client::new();

        brain.enqueue(make_request("req-hard-403")).await;
        brain.tick(&client).await;

        let state = brain.state.read().await;
        let entry = &state.completed[0];
        assert_eq!(entry.status, "failed");
        assert!(
            entry.failure_reason.is_some(),
            "failure_reason must be set for a 403 response"
        );
        let reason = entry.failure_reason.as_deref().unwrap_or("");
        assert!(
            reason.contains("403"),
            "failure_reason should mention the HTTP status; got: {reason:?}"
        );
    }

    // ── success ───────────────────────────────────────────────────────────────

    /// When tokenhub returns a well-formed completion, `failure_reason` must be
    /// absent (None) and `result` must be populated.
    #[tokio::test]
    async fn successful_request_has_no_failure_reason() {
        let ok_body = r#"{
            "choices": [{"message": {"role": "assistant", "content": "pong"}}],
            "usage": {"total_tokens": 7}
        }"#;

        // We need the mock server to return JSON with the correct Content-Type
        // so that reqwest's `.json()` call in call_model_once succeeds.
        use axum::{routing::post, Json, Router};

        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async {
                Json(serde_json::json!({
                    "choices": [{"message": {"role": "assistant", "content": "pong"}}],
                    "usage": {"total_tokens": 7}
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("http://127.0.0.1:{}", addr.port());
        let _server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("mock server");
        });

        let _ = ok_body; // suppress unused warning; the real body comes from the handler

        let brain = make_brain(url);
        let client = reqwest::Client::new();

        brain.enqueue(make_request("req-ok-1")).await;
        brain.tick(&client).await;

        let state = brain.state.read().await;
        assert!(
            state.queue.is_empty(),
            "queue should be empty after a successful completion"
        );
        assert_eq!(state.completed.len(), 1);

        let entry = &state.completed[0];
        assert_eq!(entry.status, "completed", "status must be 'completed'");
        assert!(
            entry.failure_reason.is_none(),
            "failure_reason must be None on a successful request, got: {:?}",
            entry.failure_reason
        );
        assert_eq!(
            entry.result.as_deref(),
            Some("pong"),
            "result must carry the model's reply"
        );
    }
}
