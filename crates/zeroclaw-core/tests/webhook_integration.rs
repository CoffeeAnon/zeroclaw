//! Integration tests for Sam's A2A push-notification webhook.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tower::ServiceExt;
use zeroclaw_a2a_outbox::migrate;
use zeroclaw_core::a2a::wake_channel;
use zeroclaw_core::a2a::webhook::{build_webhook_router, WebhookState};

struct Ctx {
    pool: PgPool,
    _container: testcontainers::ContainerAsync<Postgres>,
}

async fn setup() -> Ctx {
    let container = Postgres::default()
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("map host port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPool::connect(&url).await.expect("connect");
    migrate::apply(&pool).await.expect("migrate");
    Ctx {
        pool,
        _container: container,
    }
}

#[tokio::test]
async fn webhook_rejects_missing_bearer() {
    let ctx = setup().await;
    let (tx, _rx) = wake_channel::channel();
    let state = WebhookState::new(ctx.pool.clone(), tx);
    let app = build_webhook_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/a2a-notify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"statusUpdate":{"taskId":"t"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_rejects_token_mismatch() {
    let ctx = setup().await;
    sqlx::query(
        "INSERT INTO push_notification_configs (task_id, url, token) VALUES ($1, $2, $3)",
    )
    .bind("t-seeded")
    .bind("http://ignored")
    .bind("tok-valid")
    .execute(&ctx.pool)
    .await
    .unwrap();

    let (tx, _rx) = wake_channel::channel();
    let state = WebhookState::new(ctx.pool.clone(), tx);
    let app = build_webhook_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/a2a-notify")
                .header("content-type", "application/json")
                .header("authorization", "Bearer wrong-token")
                .body(Body::from(r#"{"statusUpdate":{"taskId":"t-seeded"}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_persists_and_wakes_on_valid_token() {
    let ctx = setup().await;
    sqlx::query(
        "INSERT INTO push_notification_configs (task_id, url, token) VALUES ($1, $2, $3)",
    )
    .bind("t-valid")
    .bind("http://ignored")
    .bind("tok-ok")
    .execute(&ctx.pool)
    .await
    .unwrap();

    let (tx, mut rx) = wake_channel::channel();
    let state = WebhookState::new(ctx.pool.clone(), tx);
    let app = build_webhook_router(state);

    let body = r#"{"statusUpdate":{"taskId":"t-valid","contextId":"c1","status":{"state":"TASK_STATE_COMPLETED"}}}"#;
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/webhook/a2a-notify")
                .header("content-type", "application/json")
                .header("authorization", "Bearer tok-ok")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let _ = resp.into_body().collect().await;

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox_events WHERE task_id = $1")
        .bind("t-valid")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "one inbox_events row persisted");

    let wake_received = tokio::time::timeout(Duration::from_millis(100), rx.recv())
        .await
        .expect("wake signal arrived before timeout")
        .is_some();
    assert!(wake_received);
}

#[tokio::test]
async fn webhook_is_idempotent_on_repeated_delivery() {
    let ctx = setup().await;
    sqlx::query(
        "INSERT INTO push_notification_configs (task_id, url, token) VALUES ($1, $2, $3)",
    )
    .bind("t-dup")
    .bind("http://ignored")
    .bind("tok")
    .execute(&ctx.pool)
    .await
    .unwrap();

    let (tx, _rx) = wake_channel::channel();
    let state = WebhookState::new(ctx.pool.clone(), tx);

    for _ in 0..2 {
        let app = build_webhook_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/webhook/a2a-notify")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer tok")
                    .body(Body::from(r#"{"statusUpdate":{"taskId":"t-dup"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM inbox_events WHERE task_id = $1")
        .bind("t-dup")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "ON CONFLICT DO NOTHING keeps it at one row");
}
