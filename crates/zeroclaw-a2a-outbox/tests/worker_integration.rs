//! Integration tests for the outbox worker (claim + deliver loop).
//!
//! Uses testcontainers for Postgres and wiremock for the target webhook.

use serde_json::json;
use sqlx::{PgPool, Row};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zeroclaw_a2a_outbox::{migrate, OutboxStore, OutboxWorker, RetryPolicy};

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

async fn fetch_status(pool: &PgPool) -> (String, i32) {
    let row = sqlx::query("SELECT status, attempts FROM outbox LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    (row.try_get("status").unwrap(), row.try_get("attempts").unwrap())
}

#[tokio::test]
async fn worker_delivers_pending_row() {
    let ctx = setup().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhook"))
        .and(header("authorization", "Bearer tok"))
        .and(body_json(json!({"x": 1})))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let store = OutboxStore::new(ctx.pool.clone());
    store
        .enqueue(
            "t1",
            0,
            &format!("{}/webhook", server.uri()),
            Some("tok"),
            json!({"x": 1}),
        )
        .await
        .unwrap();

    let worker = OutboxWorker::new(ctx.pool.clone(), RetryPolicy::default());
    worker.run_once().await.unwrap();

    let (status, _) = fetch_status(&ctx.pool).await;
    assert_eq!(status, "delivered");
}

#[tokio::test]
async fn worker_retries_on_5xx_then_deadletters() {
    let ctx = setup().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let store = OutboxStore::new(ctx.pool.clone());
    store
        .enqueue(
            "t2",
            0,
            &format!("{}/webhook", server.uri()),
            None,
            json!({}),
        )
        .await
        .unwrap();

    // Fast-retry policy for test.
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        factor: 2,
    };
    let worker = OutboxWorker::new(ctx.pool.clone(), policy);

    for _ in 0..20 {
        worker.run_once().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (status, attempts) = fetch_status(&ctx.pool).await;
    assert_eq!(status, "deadletter");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn worker_does_not_retry_on_4xx() {
    let ctx = setup().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;

    let store = OutboxStore::new(ctx.pool.clone());
    store
        .enqueue(
            "t3",
            0,
            &format!("{}/webhook", server.uri()),
            Some("bad"),
            json!({}),
        )
        .await
        .unwrap();

    let worker = OutboxWorker::new(ctx.pool.clone(), RetryPolicy::default());
    worker.run_once().await.unwrap();

    let (status, _) = fetch_status(&ctx.pool).await;
    assert_eq!(status, "deadletter", "4xx is non-retryable");
}
