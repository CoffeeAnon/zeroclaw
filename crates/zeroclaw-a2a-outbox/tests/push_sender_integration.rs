//! Integration tests for `OutboxBackedPushSender`.
//!
//! Verifies that invoking the ra2a `PushSender` surface results in a durable
//! `outbox` row matching the config URL, token, and serialized task.

use ra2a::server::PushSender;
use ra2a::types::{PushNotificationConfig, Task, TaskState, TaskStatus};
use sqlx::{PgPool, Row};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use zeroclaw_a2a_outbox::{migrate, OutboxBackedPushSender};

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
async fn push_sender_enqueues_outbox_row() {
    let ctx = setup().await;
    let sender = OutboxBackedPushSender::new(ctx.pool.clone());

    let cfg = PushNotificationConfig {
        id: None,
        url: "http://example.test/webhook".to_string(),
        token: Some("tok".to_string()),
        authentication: None,
    };
    let mut task = Task::new("task-xyz", "ctx-abc");
    task.status = TaskStatus::new(TaskState::Completed);

    sender.send_push(&cfg, &task).await.unwrap();

    let row = sqlx::query(
        "SELECT target_url, auth_token, status, payload_json FROM outbox WHERE task_id = $1",
    )
    .bind("task-xyz")
    .fetch_one(&ctx.pool)
    .await
    .unwrap();

    let target_url: String = row.try_get("target_url").unwrap();
    let auth_token: Option<String> = row.try_get("auth_token").unwrap();
    let status: String = row.try_get("status").unwrap();
    let payload: serde_json::Value = row.try_get("payload_json").unwrap();

    assert_eq!(target_url, "http://example.test/webhook");
    assert_eq!(auth_token.as_deref(), Some("tok"));
    assert_eq!(status, "pending");
    assert_eq!(
        payload.get("id").and_then(|v| v.as_str()),
        Some("task-xyz"),
        "payload should be the serialized Task"
    );
}

#[tokio::test]
async fn push_sender_is_idempotent_for_repeated_task() {
    let ctx = setup().await;
    let sender = OutboxBackedPushSender::new(ctx.pool.clone());

    let cfg = PushNotificationConfig::new("http://example.test/webhook");
    let task = Task::new("dup-task", "ctx");

    sender.send_push(&cfg, &task).await.unwrap();
    sender.send_push(&cfg, &task).await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE task_id = $1")
        .bind("dup-task")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "(task_id, sequence=0) must dedupe on second send");
}
