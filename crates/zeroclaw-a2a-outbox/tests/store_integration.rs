//! Integration tests for outbox enqueue semantics.
//!
//! Spins up an ephemeral Postgres via testcontainers — no external DB needed.
//! Requires Docker available on the host.

use serde_json::json;
use sqlx::{PgPool, Row};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use zeroclaw_a2a_outbox::{migrate, OutboxStore};

struct Ctx {
    pool: PgPool,
    // Container must stay alive for pool to remain valid; hold via field.
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
    let pool = PgPool::connect(&url).await.expect("connect to postgres");
    migrate::apply(&pool).await.expect("apply migrations");
    Ctx {
        pool,
        _container: container,
    }
}

#[tokio::test]
async fn enqueue_creates_pending_row() {
    let ctx = setup().await;
    let store = OutboxStore::new(ctx.pool.clone());

    let id = store
        .enqueue(
            "task-123",
            0,
            "http://example.test/webhook",
            Some("tok-abc"),
            json!({"hello": "world"}),
        )
        .await
        .unwrap();

    let row = sqlx::query("SELECT status, attempts FROM outbox WHERE task_id = $1")
        .bind("task-123")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    let status: String = row.try_get("status").unwrap();
    let attempts: i32 = row.try_get("attempts").unwrap();
    assert_eq!(status, "pending");
    assert_eq!(attempts, 0);
    assert_ne!(id, uuid::Uuid::nil());
}

#[tokio::test]
async fn enqueue_is_idempotent_on_task_and_sequence() {
    let ctx = setup().await;
    let store = OutboxStore::new(ctx.pool.clone());

    let a = store
        .enqueue("task-1", 0, "u", None, json!({}))
        .await
        .unwrap();
    let b = store
        .enqueue("task-1", 0, "u", None, json!({}))
        .await
        .unwrap();
    assert_eq!(
        a, b,
        "second enqueue returns existing id, does not insert duplicate"
    );

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE task_id = $1")
        .bind("task-1")
        .fetch_one(&ctx.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}
