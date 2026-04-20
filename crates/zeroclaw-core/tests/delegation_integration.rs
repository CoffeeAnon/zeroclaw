//! Integration tests for the A2A delegation client.
//!
//! Uses wiremock to stand in for Walter's `:3001/` JSON-RPC endpoint and
//! verifies the outgoing request shape.

use serde_json::Value;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zeroclaw_core::a2a::delegation::A2ADelegationClient;

#[tokio::test]
async fn delegate_posts_message_send_with_push_config() {
    let walter = MockServer::start().await;
    let webhook_url = "http://sam.ai-agents.svc.cluster.local:3001/webhook/a2a-notify";

    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("content-type", "application/json"))
        .and(body_partial_json(serde_json::json!({
            "jsonrpc": "2.0",
            "method": "message/send",
            "params": {
                "message": {
                    "role": "ROLE_USER",
                    "parts": [{"text": "please check cluster health"}],
                },
                "configuration": {
                    "pushNotificationConfig": {
                        "url": webhook_url,
                    }
                }
            }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {"id": "walter-assigned-task-id"}
        })))
        .expect(1)
        .mount(&walter)
        .await;

    let client = A2ADelegationClient::new(
        reqwest::Client::new(),
        walter.uri(),
        webhook_url.to_string(),
    );

    let handle = client
        .delegate("please check cluster health")
        .await
        .expect("delegation call succeeds");

    // Sam-generated IDs, returned so the caller can persist them.
    assert!(!handle.task_id.is_empty());
    assert!(!handle.token.is_empty());
    assert_ne!(handle.task_id, handle.token);
}

#[tokio::test]
async fn delegate_surfaces_upstream_failure() {
    let walter = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string("walter on fire"))
        .mount(&walter)
        .await;

    let client = A2ADelegationClient::new(
        reqwest::Client::new(),
        walter.uri(),
        "http://sam/webhook/a2a-notify".to_string(),
    );

    let err = client
        .delegate("hello")
        .await
        .expect_err("500 must propagate");
    let msg = format!("{err:#}");
    assert!(msg.contains("500"), "error should mention status: {msg}");
}

#[tokio::test]
async fn delegate_sends_task_id_in_message() {
    let walter = MockServer::start().await;

    // Capture the incoming request via an always-match mock that records the body.
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {}
        })))
        .mount(&walter)
        .await;

    let client = A2ADelegationClient::new(
        reqwest::Client::new(),
        walter.uri(),
        "http://sam/webhook/a2a-notify".to_string(),
    );

    let handle = client.delegate("hi").await.unwrap();

    // Pull the recorded request body and assert the task_id in the message
    // matches the one we got back (same UUID).
    let received = &walter.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&received.body).unwrap();
    let msg_task_id = body["params"]["message"]["taskId"].as_str().unwrap();
    assert_eq!(msg_task_id, handle.task_id);
}
