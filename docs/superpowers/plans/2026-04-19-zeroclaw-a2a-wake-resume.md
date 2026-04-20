# ZeroClaw A2A Wake/Resume Layer — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add A2A v1.0 push-notification-based wake/resume so Walter can push completion updates to Sam and wake her reasoning loop immediately — no 30-minute Vikunja poll.

**Architecture:** Adopt the `ra2a` Rust crate for A2A protocol. Both agents get an A2A server + Postgres-backed TaskStore/PushNotificationConfigStore (CNPG-managed in `ai-agents`). Durable delivery via per-agent Postgres outbox tables with exponential-backoff retry workers. Sam's webhook handler signals her reasoning loop via a new trigger channel. Feature-flagged cutover on a single delegation path (`cluster-health-survey`) first.

**Tech Stack:** Rust 2021, tokio, axum, sqlx (Postgres), `ra2a` v0.10.1, CNPG operator, Vault Secrets Operator (VSO)

**Scope:** MVP = Phase 1 (foundations, no traffic shift) + Phase 2 (one task type cutover). Phases 3–4 from the spec are separate future plans.

**Related spec:** `docs/superpowers/specs/2026-04-19-zeroclaw-a2a-wake-resume-design.md`

---

## Prerequisites

Before starting, the implementing engineer should:

- Read the spec doc linked above (15 min).
- Read `src/gateway/mod.rs` around `AppState`, router composition, and the Axum server startup (locate `axum::serve` or equivalent) (20 min). Note the line where the router is assembled — that's where the A2A router will be merged.
- Read `src/gateway/acp_server.rs` top-to-bottom. ACP's session/prompt handling is the shape we're adding a peer to (45 min).
- Skim `src/agent/agent.rs` and `src/agent/loop_.rs` to understand how a "turn" is currently dispatched. Specifically: where does a new Signal message or cron firing eventually enter the agent's execution path? This is where a webhook-triggered wake needs to hook in.
- `cargo doc --open -p ra2a` after adding the dependency (Task 2). The `AgentExecutor`, `EventQueue`, `TaskStore`, and `PushNotificationSender` traits are the surface we're implementing against.

---

## File Structure

**New crate:** `crates/zeroclaw-a2a-outbox/`
- `Cargo.toml`
- `src/lib.rs` — module root + public types
- `src/schema.rs` — SQL migrations for `outbox` table
- `src/record.rs` — `OutboxRecord` struct + sqlx row mapping
- `src/worker.rs` — polling loop, retry logic, dead-letter
- `src/push_sender.rs` — `OutboxBackedPushSender` (impl `ra2a::PushNotificationSender`)
- `tests/worker_integration.rs`

**New module in `zeroclaw-core`:** `crates/zeroclaw-core/src/a2a/`
- `mod.rs`
- `card.rs` — `sam_agent_card()`, `walter_agent_card()` builders
- `walter_executor.rs` — `WalterAgentExecutor`
- `sam_executor.rs` — `SamAgentExecutor` (shim in MVP)
- `webhook.rs` — Sam's `/webhook/a2a-notify` receiver
- `delegation.rs` — Sam's A2A-client delegation helper (used under feature flag)
- `wake_channel.rs` — `WakeSignal` type + mpsc wiring into the reasoning loop

**Modified in main binary:**
- `Cargo.toml` (workspace + package) — add `ra2a`, `sqlx` Postgres features, new crate member
- `src/gateway/mod.rs` — compose A2A router, register webhook route, spawn outbox worker on startup
- `src/main.rs` — read new env vars, wire DB pool, instantiate executors
- `src/agent/agent.rs` (or whichever file owns the main loop) — `tokio::select!` on the new wake channel

**New K8s manifests in `k8s/`:**
- `k8s/shared/28_cnpg_a2a_cluster.yaml` — `zeroclaw-a2a` CNPG cluster
- `k8s/shared/29_a2a_db_bootstrap_job.yaml` — Job that creates schemas `sam`, `walter`, and per-agent roles
- `k8s/sam/30_vso_a2a_db_secret.yaml` — VaultStaticSecret for Sam's DB cred
- `k8s/walter/10_vso_a2a_db_secret.yaml` — VaultStaticSecret for Walter's DB cred
- Modified: `k8s/sam/04_zeroclaw_sandbox.yaml` and `k8s/walter/03_sandbox.yaml` — add env vars + mount DB cred

---

## Phase 0 — Branch & baseline

### Task 0.1: Create feature branch

- [ ] **Step 1: Create and check out a feature branch**

```bash
cd ~/github_projects/zeroclaw
git checkout -b feat/a2a-wake-resume
git push -u origin feat/a2a-wake-resume
```

- [ ] **Step 2: Verify main branch compiles clean first**

```bash
cargo check --workspace
```

Expected: no errors. If it fails, STOP and fix main before proceeding — plan assumes a clean baseline.

- [ ] **Step 3: Commit a placeholder marker so reverts always have an anchor**

```bash
git commit --allow-empty -m "chore: start A2A wake/resume feature branch"
git push
```

---

## Phase 1 — Foundations (no traffic shift)

### Task 1.1: Add `ra2a` and `sqlx` Postgres to workspace

**Files:**
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add `ra2a` and Postgres sqlx features to `[workspace.dependencies]`**

Locate the `[workspace.dependencies]` table in `Cargo.toml` (or add one if it doesn't exist). Add:

```toml
ra2a = "0.10.1"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "migrate", "macros"] }
```

If the workspace doesn't use `workspace.dependencies` today, add these to the root `[dependencies]` table of the main `zeroclaw` crate instead.

- [ ] **Step 2: Run cargo fetch to prove resolution**

```bash
cargo fetch
```

Expected: deps resolve; no version conflicts. If sqlx conflicts with an existing version, pin to the higher of the two.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(a2a): add ra2a and sqlx postgres to workspace deps"
```

---

### Task 1.2: Scaffold `zeroclaw-a2a-outbox` crate

**Files:**
- Create: `crates/zeroclaw-a2a-outbox/Cargo.toml`
- Create: `crates/zeroclaw-a2a-outbox/src/lib.rs`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Add the crate to workspace members**

Edit `Cargo.toml`:

```toml
[workspace]
members = [
    ".",
    "crates/robot-kit",
    "crates/zeroclaw-types",
    "crates/zeroclaw-core",
    "crates/zeroclaw-a2a-outbox",
]
```

- [ ] **Step 2: Create the crate manifest**

Write `crates/zeroclaw-a2a-outbox/Cargo.toml`:

```toml
[package]
name = "zeroclaw-a2a-outbox"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[dependencies]
tokio = { workspace = true }
sqlx = { workspace = true }
ra2a = { workspace = true }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
tracing = "0.1"
thiserror = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util", "macros", "rt"] }
wiremock = "0.6"
```

If any of `tokio`/`sqlx`/`ra2a` are not yet in `workspace.dependencies`, move them there as part of this task or inline the versions. The chosen pattern must be consistent with the existing repo style.

- [ ] **Step 3: Write the empty lib.rs**

Write `crates/zeroclaw-a2a-outbox/src/lib.rs`:

```rust
//! Durable outbox for A2A push-notification delivery.
//!
//! Implements [`ra2a::PushNotificationSender`] with at-least-once delivery
//! semantics backed by a Postgres `outbox` table. Failed POSTs are retried
//! with exponential backoff; after `max_attempts` the row moves to
//! dead-letter state and is surfaced via a structured log event.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
```

- [ ] **Step 4: Verify the workspace compiles**

```bash
cargo check --workspace
```

Expected: compiles clean.

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-a2a-outbox Cargo.toml Cargo.lock
git commit -m "feat(a2a): scaffold zeroclaw-a2a-outbox crate"
```

---

### Task 1.3: Define `OutboxRecord` and migration SQL

**Files:**
- Create: `crates/zeroclaw-a2a-outbox/src/record.rs`
- Create: `crates/zeroclaw-a2a-outbox/migrations/20260419000001_create_outbox.sql`

- [ ] **Step 1: Write the migration SQL**

Write `crates/zeroclaw-a2a-outbox/migrations/20260419000001_create_outbox.sql`:

```sql
CREATE TABLE IF NOT EXISTS outbox (
    id              UUID PRIMARY KEY,
    task_id         TEXT NOT NULL,
    sequence        INTEGER NOT NULL,
    target_url      TEXT NOT NULL,
    auth_token      TEXT,
    payload_json    JSONB NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    delivered_at    TIMESTAMPTZ,
    last_error      TEXT,
    CONSTRAINT outbox_status_valid CHECK (status IN ('pending', 'delivered', 'deadletter')),
    CONSTRAINT outbox_task_seq_unique UNIQUE (task_id, sequence)
);

CREATE INDEX IF NOT EXISTS outbox_due_idx
    ON outbox (next_attempt_at)
    WHERE status = 'pending';
```

- [ ] **Step 2: Write the record type**

Write `crates/zeroclaw-a2a-outbox/src/record.rs`:

```rust
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct OutboxRecord {
    pub id: Uuid,
    pub task_id: String,
    pub sequence: i32,
    pub target_url: String,
    pub auth_token: Option<String>,
    pub payload_json: Value,
    pub attempts: i32,
    pub next_attempt_at: DateTime<Utc>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl OutboxRecord {
    pub const STATUS_PENDING: &'static str = "pending";
    pub const STATUS_DELIVERED: &'static str = "delivered";
    pub const STATUS_DEADLETTER: &'static str = "deadletter";
}
```

- [ ] **Step 3: Expose from lib.rs**

Add to `crates/zeroclaw-a2a-outbox/src/lib.rs`:

```rust
pub mod record;
pub use record::OutboxRecord;
```

- [ ] **Step 4: Verify compile**

```bash
cargo check --workspace
```

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-a2a-outbox/
git commit -m "feat(a2a-outbox): add OutboxRecord type and schema migration"
```

---

### Task 1.4: Retry policy with exponential backoff (pure logic, TDD)

**Files:**
- Create: `crates/zeroclaw-a2a-outbox/src/retry.rs`
- Test: `crates/zeroclaw-a2a-outbox/src/retry.rs` (inline `#[cfg(test)]`)

- [ ] **Step 1: Write failing tests**

Write `crates/zeroclaw-a2a-outbox/src/retry.rs`:

```rust
//! Exponential backoff retry policy for outbox deliveries.

use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub factor: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 5, base_delay: Duration::from_secs(1), factor: 4 }
    }
}

impl RetryPolicy {
    /// Returns the delay before the `attempt`-th retry (0-indexed).
    /// Returns `None` when the attempt count exceeds `max_attempts`.
    pub fn delay_for(&self, attempt: u32) -> Option<Duration> {
        if attempt >= self.max_attempts {
            return None;
        }
        let pow = self.factor.checked_pow(attempt)?;
        Some(self.base_delay * pow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_gives_expected_schedule() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for(0), Some(Duration::from_secs(1)));
        assert_eq!(p.delay_for(1), Some(Duration::from_secs(4)));
        assert_eq!(p.delay_for(2), Some(Duration::from_secs(16)));
        assert_eq!(p.delay_for(3), Some(Duration::from_secs(64)));
        assert_eq!(p.delay_for(4), Some(Duration::from_secs(256)));
        assert_eq!(p.delay_for(5), None, "exhausted");
    }

    #[test]
    fn custom_policy_respects_max_attempts() {
        let p = RetryPolicy { max_attempts: 2, base_delay: Duration::from_millis(10), factor: 2 };
        assert!(p.delay_for(0).is_some());
        assert!(p.delay_for(1).is_some());
        assert!(p.delay_for(2).is_none());
    }

    #[test]
    fn factor_overflow_returns_none() {
        let p = RetryPolicy { max_attempts: 100, base_delay: Duration::from_secs(1), factor: u32::MAX };
        assert_eq!(p.delay_for(3), None, "overflow must not panic");
    }
}
```

Also add `pub mod retry;` and `pub use retry::RetryPolicy;` to `lib.rs`.

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p zeroclaw-a2a-outbox retry:: --no-run 2>&1 | tail -5
```

Expected: compile fails because `mod retry;` references don't exist yet, OR tests compile and fail with "unresolved import".

Actually at this point, the module does exist — the test was written as part of step 1 of the module file. So tests will compile.

Re-run:

```bash
cargo test -p zeroclaw-a2a-outbox retry::
```

Expected: all 3 tests PASS. (Logic is trivial — TDD is still the shape, just no red→green gap.)

- [ ] **Step 3: Commit**

```bash
git add crates/zeroclaw-a2a-outbox/src/
git commit -m "feat(a2a-outbox): retry policy with exponential backoff"
```

---

### Task 1.5: Outbox enqueue API (TDD, integration test with Postgres)

**Files:**
- Create: `crates/zeroclaw-a2a-outbox/src/store.rs`
- Test: `crates/zeroclaw-a2a-outbox/tests/store_integration.rs`

- [ ] **Step 1: Write the failing integration test**

Write `crates/zeroclaw-a2a-outbox/tests/store_integration.rs`:

```rust
//! Integration tests for outbox enqueue and claim semantics.
//! Requires a Postgres DSN in DATABASE_URL (pointing at an empty schema).

use serde_json::json;
use sqlx::PgPool;
use zeroclaw_a2a_outbox::store::OutboxStore;

async fn setup_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL to an empty Postgres schema for tests");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("TRUNCATE outbox").execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn enqueue_creates_pending_row() {
    let pool = setup_pool().await;
    let store = OutboxStore::new(pool.clone());
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

    let row = sqlx::query!("SELECT status, attempts FROM outbox WHERE id = $1", id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.attempts, 0);
}

#[tokio::test]
async fn enqueue_is_idempotent_on_task_and_sequence() {
    let pool = setup_pool().await;
    let store = OutboxStore::new(pool.clone());
    let a = store.enqueue("task-1", 0, "u", None, json!({})).await.unwrap();
    let b = store.enqueue("task-1", 0, "u", None, json!({})).await.unwrap();
    assert_eq!(a, b, "second enqueue returns existing id, does not insert duplicate");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
DATABASE_URL=postgres://localhost/a2a_test cargo test -p zeroclaw-a2a-outbox --test store_integration
```

Expected: FAIL with `unresolved import 'zeroclaw_a2a_outbox::store'` or similar.

If you don't have a local Postgres, the test will skip setup and panic on the first query — that also counts as FAIL for TDD purposes. For local dev, run:

```bash
docker run --rm -d --name a2a-test-pg -e POSTGRES_HOST_AUTH_METHOD=trust -p 5432:5432 postgres:16
createdb -h localhost -U postgres a2a_test
```

- [ ] **Step 3: Write the minimal implementation**

Write `crates/zeroclaw-a2a-outbox/src/store.rs`:

```rust
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::record::OutboxRecord;

#[derive(Clone)]
pub struct OutboxStore {
    pool: PgPool,
}

impl OutboxStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Inserts a pending outbox row. Idempotent on `(task_id, sequence)`.
    pub async fn enqueue(
        &self,
        task_id: &str,
        sequence: i32,
        target_url: &str,
        auth_token: Option<&str>,
        payload: Value,
    ) -> Result<Uuid, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query!(
            r#"
            INSERT INTO outbox (id, task_id, sequence, target_url, auth_token, payload_json)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (task_id, sequence) DO UPDATE SET task_id = outbox.task_id
            RETURNING id
            "#,
            id,
            task_id,
            sequence,
            target_url,
            auth_token,
            payload,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(row.id)
    }
}
```

Add to `lib.rs`:

```rust
pub mod store;
pub use store::OutboxStore;
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-a2a-outbox --test store_integration
```

Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-a2a-outbox/
git commit -m "feat(a2a-outbox): enqueue API with idempotency on (task_id, sequence)"
```

---

### Task 1.6: Outbox worker claim + deliver loop (TDD)

**Files:**
- Modify: `crates/zeroclaw-a2a-outbox/src/store.rs`
- Create: `crates/zeroclaw-a2a-outbox/src/worker.rs`
- Test: `crates/zeroclaw-a2a-outbox/tests/worker_integration.rs`

- [ ] **Step 1: Write the failing integration test (worker happy path + retry + dead-letter)**

Write `crates/zeroclaw-a2a-outbox/tests/worker_integration.rs`:

```rust
use serde_json::json;
use sqlx::PgPool;
use std::time::Duration;
use wiremock::matchers::{body_json_string, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use zeroclaw_a2a_outbox::{retry::RetryPolicy, store::OutboxStore, worker::OutboxWorker};

async fn setup_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("TRUNCATE outbox").execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn worker_delivers_pending_row() {
    let pool = setup_pool().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/webhook"))
        .and(header("authorization", "Bearer tok"))
        .and(body_json_string(r#"{"x":1}"#))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    let store = OutboxStore::new(pool.clone());
    store.enqueue("t1", 0, &format!("{}/webhook", server.uri()), Some("tok"), json!({"x": 1}))
        .await.unwrap();

    let worker = OutboxWorker::new(pool.clone(), RetryPolicy::default());
    worker.run_once().await.unwrap();

    let row = sqlx::query!("SELECT status FROM outbox LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "delivered");
}

#[tokio::test]
async fn worker_retries_on_5xx_then_deadletters() {
    let pool = setup_pool().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let store = OutboxStore::new(pool.clone());
    store.enqueue("t2", 0, &format!("{}/webhook", server.uri()), None, json!({})).await.unwrap();

    // Fast-retry policy for test.
    let policy = RetryPolicy { max_attempts: 3, base_delay: Duration::from_millis(1), factor: 2 };
    let worker = OutboxWorker::new(pool.clone(), policy);

    // Run repeatedly until the row is no longer pending.
    for _ in 0..20 {
        worker.run_once().await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let row = sqlx::query!("SELECT status, attempts FROM outbox LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "deadletter");
    assert_eq!(row.attempts, 3);
}

#[tokio::test]
async fn worker_does_not_retry_on_4xx() {
    let pool = setup_pool().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;

    let store = OutboxStore::new(pool.clone());
    store.enqueue("t3", 0, &format!("{}/webhook", server.uri()), Some("bad"), json!({})).await.unwrap();

    let worker = OutboxWorker::new(pool.clone(), RetryPolicy::default());
    worker.run_once().await.unwrap();

    let row = sqlx::query!("SELECT status FROM outbox LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "deadletter", "4xx is non-retryable");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-a2a-outbox --test worker_integration
```

Expected: FAIL (`worker` module missing).

- [ ] **Step 3: Extend the store with `claim_due` and `mark_*` helpers**

Add to `crates/zeroclaw-a2a-outbox/src/store.rs`:

```rust
use chrono::{DateTime, Utc};

impl OutboxStore {
    /// Atomically claim up to `limit` pending rows whose `next_attempt_at` is in the past.
    /// Bumps their `attempts` counter and returns them; uses `FOR UPDATE SKIP LOCKED` so
    /// multiple workers can run concurrently.
    pub async fn claim_due(&self, limit: i64) -> Result<Vec<OutboxRecord>, sqlx::Error> {
        sqlx::query_as!(
            OutboxRecord,
            r#"
            WITH due AS (
                SELECT id FROM outbox
                WHERE status = 'pending' AND next_attempt_at <= NOW()
                ORDER BY next_attempt_at
                FOR UPDATE SKIP LOCKED
                LIMIT $1
            )
            UPDATE outbox
            SET attempts = attempts + 1
            FROM due
            WHERE outbox.id = due.id
            RETURNING outbox.*
            "#,
            limit
        )
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mark_delivered(&self, id: uuid::Uuid) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE outbox SET status = 'delivered', delivered_at = NOW() WHERE id = $1",
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_deadletter(&self, id: uuid::Uuid, reason: &str) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE outbox SET status = 'deadletter', last_error = $2 WHERE id = $1",
            id,
            reason
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn reschedule(&self, id: uuid::Uuid, at: DateTime<Utc>, reason: &str) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "UPDATE outbox SET next_attempt_at = $2, last_error = $3 WHERE id = $1",
            id,
            at,
            reason
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
```

- [ ] **Step 4: Write the worker**

Write `crates/zeroclaw-a2a-outbox/src/worker.rs`:

```rust
use chrono::Utc;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::{record::OutboxRecord, retry::RetryPolicy, store::OutboxStore};

pub struct OutboxWorker {
    store: OutboxStore,
    http: reqwest::Client,
    policy: RetryPolicy,
}

impl OutboxWorker {
    pub fn new(pool: PgPool, policy: RetryPolicy) -> Self {
        Self {
            store: OutboxStore::new(pool),
            http: reqwest::Client::new(),
            policy,
        }
    }

    /// Claim and deliver up to 32 due rows, then return. Call from a loop with sleep.
    pub async fn run_once(&self) -> Result<(), sqlx::Error> {
        let claimed = self.store.claim_due(32).await?;
        for rec in claimed {
            self.deliver_one(rec).await?;
        }
        Ok(())
    }

    async fn deliver_one(&self, rec: OutboxRecord) -> Result<(), sqlx::Error> {
        let mut req = self.http.post(&rec.target_url).json(&rec.payload_json);
        if let Some(tok) = &rec.auth_token {
            req = req.bearer_auth(tok);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(task_id = %rec.task_id, sequence = rec.sequence, "outbox delivered");
                self.store.mark_delivered(rec.id).await?;
            }
            Ok(resp) if resp.status().is_client_error() => {
                let code = resp.status().as_u16();
                warn!(task_id = %rec.task_id, code, "outbox 4xx — dead-lettering");
                self.store.mark_deadletter(rec.id, &format!("HTTP {code}")).await?;
            }
            Ok(resp) => {
                self.handle_retryable(rec, format!("HTTP {}", resp.status())).await?;
            }
            Err(e) => {
                self.handle_retryable(rec, format!("transport: {e}")).await?;
            }
        }
        Ok(())
    }

    async fn handle_retryable(&self, rec: OutboxRecord, reason: String) -> Result<(), sqlx::Error> {
        // `attempts` was already incremented in `claim_due`. The attempt that just failed is
        // the (attempts - 1)-th retry.
        let current = u32::try_from(rec.attempts).unwrap_or(u32::MAX).saturating_sub(1);
        match self.policy.delay_for(current) {
            Some(delay) => {
                let next = Utc::now() + chrono::Duration::from_std(delay).unwrap_or_default();
                warn!(task_id = %rec.task_id, attempts = rec.attempts, "outbox retry: {reason}");
                self.store.reschedule(rec.id, next, &reason).await?;
            }
            None => {
                warn!(task_id = %rec.task_id, "outbox retries exhausted — dead-lettering");
                self.store.mark_deadletter(rec.id, &reason).await?;
            }
        }
        Ok(())
    }
}
```

Export from `lib.rs`:

```rust
pub mod worker;
pub use worker::OutboxWorker;
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-a2a-outbox --test worker_integration
```

Expected: 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/zeroclaw-a2a-outbox/
git commit -m "feat(a2a-outbox): worker with retry, dead-letter, and 4xx short-circuit"
```

---

### Task 1.7: `OutboxBackedPushSender` implementing `ra2a::PushNotificationSender`

**Files:**
- Create: `crates/zeroclaw-a2a-outbox/src/push_sender.rs`

- [ ] **Step 1: Inspect `ra2a::PushNotificationSender` trait surface**

```bash
cargo doc -p ra2a --no-deps
# open target/doc/ra2a/server/trait.PushNotificationSender.html
```

Record the exact method signature (async trait method name and params). Typical shape (confirm against the docs):

```rust
#[async_trait::async_trait]
pub trait PushNotificationSender: Send + Sync {
    async fn send(&self, config: &PushNotificationConfig, event: &StreamResponse) -> ra2a::Result<()>;
}
```

- [ ] **Step 2: Write failing test**

Append to `crates/zeroclaw-a2a-outbox/tests/worker_integration.rs`:

```rust
use ra2a::types::{PushNotificationConfig, StreamResponse, TaskStatusUpdateEvent, TaskState, TaskStatus};
use zeroclaw_a2a_outbox::push_sender::OutboxBackedPushSender;

#[tokio::test]
async fn push_sender_enqueues_outbox_row() {
    let pool = setup_pool().await;
    let sender = OutboxBackedPushSender::new(pool.clone());

    let cfg = PushNotificationConfig {
        url: "http://localhost:1/webhook".to_string(),
        token: Some("tok".to_string()),
        authentication: None,
    };
    let event = StreamResponse::TaskStatusUpdate(TaskStatusUpdateEvent {
        task_id: "tx".into(),
        context_id: "cx".into(),
        status: TaskStatus::new(TaskState::Completed),
        r#final: true,
    });

    sender.send(&cfg, &event).await.unwrap();

    let row = sqlx::query!("SELECT target_url, status, payload_json FROM outbox LIMIT 1")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.status, "pending");
    assert_eq!(row.target_url, "http://localhost:1/webhook");
}
```

(Adjust type paths to match what `ra2a` v0.10.1 actually exports — check with `cargo doc`.)

Run:

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-a2a-outbox --test worker_integration push_sender_enqueues_outbox_row
```

Expected: FAIL (`OutboxBackedPushSender` missing).

- [ ] **Step 3: Implement**

Write `crates/zeroclaw-a2a-outbox/src/push_sender.rs`:

```rust
use async_trait::async_trait;
use ra2a::server::PushNotificationSender;
use ra2a::types::{PushNotificationConfig, StreamResponse};
use sqlx::PgPool;

use crate::store::OutboxStore;

pub struct OutboxBackedPushSender {
    store: OutboxStore,
}

impl OutboxBackedPushSender {
    pub fn new(pool: PgPool) -> Self {
        Self { store: OutboxStore::new(pool) }
    }
}

#[async_trait]
impl PushNotificationSender for OutboxBackedPushSender {
    async fn send(&self, config: &PushNotificationConfig, event: &StreamResponse) -> ra2a::error::Result<()> {
        let (task_id, sequence) = extract_ids(event);
        let payload = serde_json::to_value(event).map_err(|e| ra2a::error::Error::Internal(e.to_string()))?;

        self.store
            .enqueue(&task_id, sequence, &config.url, config.token.as_deref(), payload)
            .await
            .map_err(|e| ra2a::error::Error::Internal(e.to_string()))?;
        Ok(())
    }
}

fn extract_ids(event: &StreamResponse) -> (String, i32) {
    // Match against the actual StreamResponse variants from ra2a::types.
    // Sequence is the attempt count within a task; for MVP we use 0 for terminal
    // events and rely on the unique (task_id, 0) constraint to dedupe retries
    // caused by ra2a re-emission.
    match event {
        StreamResponse::Task(t) => (t.id.to_string(), 0),
        StreamResponse::TaskStatusUpdate(e) => (e.task_id.to_string(), 0),
        StreamResponse::TaskArtifactUpdate(e) => (e.task_id.to_string(), e.artifact.index),
        StreamResponse::Message(m) => (m.message_id.clone(), 0),
    }
}
```

Add to `lib.rs`:

```rust
pub mod push_sender;
pub use push_sender::OutboxBackedPushSender;
```

Add `async-trait = "0.1"` to `[dependencies]` in `crates/zeroclaw-a2a-outbox/Cargo.toml`.

- [ ] **Step 4: Run test to verify pass**

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-a2a-outbox --test worker_integration push_sender_enqueues_outbox_row
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-a2a-outbox/
git commit -m "feat(a2a-outbox): OutboxBackedPushSender implements ra2a::PushNotificationSender"
```

---

### Task 1.8: Agent cards (static definitions for Sam and Walter)

**Files:**
- Create: `crates/zeroclaw-core/src/a2a/mod.rs`
- Create: `crates/zeroclaw-core/src/a2a/card.rs`
- Modify: `crates/zeroclaw-core/src/lib.rs`

- [ ] **Step 1: Write the card builders**

Write `crates/zeroclaw-core/src/a2a/card.rs`:

```rust
//! Static AgentCard builders for Sam and Walter.
//!
//! Served at `/.well-known/agent-card.json` by each binary.

use ra2a::types::{AgentCapabilities, AgentCard, AgentInterface, TransportProtocol};

pub fn sam_agent_card(base_url: &str) -> AgentCard {
    let mut card = AgentCard::new(
        "Sam",
        "ZeroClaw: personal assistant, delegator, Signal + Vikunja coordinator",
        vec![AgentInterface::new(
            base_url,
            TransportProtocol::new(TransportProtocol::JSONRPC),
        )],
    );
    card.capabilities = AgentCapabilities {
        streaming: Some(false),
        push_notifications: Some(true),
        ..Default::default()
    };
    card
}

pub fn walter_agent_card(base_url: &str) -> AgentCard {
    let mut card = AgentCard::new(
        "Walter",
        "ZeroClaw: read-only Kubernetes cluster observer",
        vec![AgentInterface::new(
            base_url,
            TransportProtocol::new(TransportProtocol::JSONRPC),
        )],
    );
    card.capabilities = AgentCapabilities {
        streaming: Some(false),
        push_notifications: Some(true),
        ..Default::default()
    };
    card
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sam_card_has_push_notifications() {
        let card = sam_agent_card("http://localhost:3000");
        assert_eq!(card.capabilities.push_notifications, Some(true));
    }

    #[test]
    fn walter_card_has_push_notifications() {
        let card = walter_agent_card("http://localhost:3000");
        assert_eq!(card.capabilities.push_notifications, Some(true));
    }
}
```

Write `crates/zeroclaw-core/src/a2a/mod.rs`:

```rust
//! A2A v1.0 integration layer.

pub mod card;
```

- [ ] **Step 2: Expose from `zeroclaw-core/src/lib.rs`**

Add: `pub mod a2a;`.

- [ ] **Step 3: Add `ra2a` to `zeroclaw-core/Cargo.toml`**

```toml
ra2a = { workspace = true }
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p zeroclaw-core a2a::card::
```

Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-core/
git commit -m "feat(a2a): Sam + Walter agent card builders"
```

---

### Task 1.9: Walter `AgentExecutor` stub

**Files:**
- Create: `crates/zeroclaw-core/src/a2a/walter_executor.rs`

- [ ] **Step 1: Write the stub (no real work; returns "not yet implemented" for MVP foundations)**

```rust
use ra2a::error::Result;
use ra2a::server::{AgentExecutor, EventQueue, RequestContext};
use ra2a::types::{Event, Message, Part, Task, TaskState, TaskStatus};
use std::future::Future;
use std::pin::Pin;

pub struct WalterAgentExecutor;

impl AgentExecutor for WalterAgentExecutor {
    fn execute<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Phase 1 stub: acknowledge, immediately fail with "not-implemented".
            // Real execution logic lands in Task 2.1.
            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = TaskStatus::with_message(
                TaskState::Failed,
                Message::agent(vec![Part::text(
                    "Walter A2A execute is stubbed in Phase 1; real work arrives in Phase 2.",
                )]),
            );
            queue.send(Event::Task(task))?;
            Ok(())
        })
    }

    fn cancel<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = TaskStatus::new(TaskState::Canceled);
            queue.send(Event::Task(task))?;
            Ok(())
        })
    }
}
```

Add `pub mod walter_executor;` in `crates/zeroclaw-core/src/a2a/mod.rs`.

- [ ] **Step 2: Compile check**

```bash
cargo check -p zeroclaw-core
```

- [ ] **Step 3: Commit**

```bash
git add crates/zeroclaw-core/
git commit -m "feat(a2a): Walter AgentExecutor stub (Phase 1)"
```

---

### Task 1.10: Sam `AgentExecutor` shim + wake channel

**Files:**
- Create: `crates/zeroclaw-core/src/a2a/sam_executor.rs`
- Create: `crates/zeroclaw-core/src/a2a/wake_channel.rs`

- [ ] **Step 1: Define the wake channel**

Write `crates/zeroclaw-core/src/a2a/wake_channel.rs`:

```rust
//! Signal channel used by the webhook receiver to nudge Sam's reasoning loop.
//!
//! The reasoning loop should `tokio::select!` on `receiver.recv()` alongside its
//! existing trigger sources. A received `WakeSignal` means there are one or more
//! unprocessed rows in `sam.inbox_events`.

use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy)]
pub struct WakeSignal;

#[derive(Clone)]
pub struct WakeSender(mpsc::UnboundedSender<WakeSignal>);

pub struct WakeReceiver(mpsc::UnboundedReceiver<WakeSignal>);

pub fn channel() -> (WakeSender, WakeReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (WakeSender(tx), WakeReceiver(rx))
}

impl WakeSender {
    /// Non-blocking wake. Silently drops if the receiver has been closed
    /// (loop has shut down; wake no longer meaningful).
    pub fn wake(&self) {
        let _ = self.0.send(WakeSignal);
    }
}

impl WakeReceiver {
    pub async fn recv(&mut self) -> Option<WakeSignal> {
        self.0.recv().await
    }
}
```

- [ ] **Step 2: Write Sam's shim executor**

Write `crates/zeroclaw-core/src/a2a/sam_executor.rs`:

```rust
use ra2a::error::Result;
use ra2a::server::{AgentExecutor, EventQueue, RequestContext};
use ra2a::types::{Event, Message, Part, Task, TaskState, TaskStatus};
use std::future::Future;
use std::pin::Pin;

/// MVP: Sam does not handle inbound A2A messages. The server still registers this
/// executor for protocol symmetry (e.g. agent-card resolution tests). Any actual
/// `message/send` hitting Sam gets a structured rejection.
pub struct SamAgentExecutor;

impl AgentExecutor for SamAgentExecutor {
    fn execute<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = TaskStatus::with_message(
                TaskState::Failed,
                Message::agent(vec![Part::text(
                    "Sam does not accept inbound A2A delegations in MVP. Use Signal or ACP.",
                )]),
            );
            queue.send(Event::Task(task))?;
            Ok(())
        })
    }

    fn cancel<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = TaskStatus::new(TaskState::Canceled);
            queue.send(Event::Task(task))?;
            Ok(())
        })
    }
}
```

Add `pub mod sam_executor; pub mod wake_channel;` in `crates/zeroclaw-core/src/a2a/mod.rs`.

- [ ] **Step 3: Compile check**

```bash
cargo check -p zeroclaw-core
```

- [ ] **Step 4: Commit**

```bash
git add crates/zeroclaw-core/
git commit -m "feat(a2a): Sam AgentExecutor shim + wake channel"
```

---

### Task 1.11: Webhook receiver (TDD)

**Files:**
- Create: `crates/zeroclaw-core/src/a2a/webhook.rs`
- Test: `crates/zeroclaw-core/tests/webhook_integration.rs`

- [ ] **Step 1: Schema migration for `inbox_events`**

Append to `crates/zeroclaw-a2a-outbox/migrations/20260419000001_create_outbox.sql` — actually this table belongs to Sam-only logic, not the outbox crate. Create a new migration file specifically for the core crate's schema:

Create `crates/zeroclaw-core/migrations/20260419000002_create_inbox_events.sql`:

```sql
CREATE TABLE IF NOT EXISTS inbox_events (
    id              UUID PRIMARY KEY,
    task_id         TEXT NOT NULL,
    sequence        INTEGER NOT NULL,
    payload_json    JSONB NOT NULL,
    received_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    processed_at    TIMESTAMPTZ,
    CONSTRAINT inbox_events_task_seq_unique UNIQUE (task_id, sequence)
);

CREATE INDEX IF NOT EXISTS inbox_events_unprocessed_idx
    ON inbox_events (received_at)
    WHERE processed_at IS NULL;
```

- [ ] **Step 2: Write failing integration test**

Write `crates/zeroclaw-core/tests/webhook_integration.rs`:

```rust
use axum::http::StatusCode;
use sqlx::PgPool;
use tower::ServiceExt;
use zeroclaw_core::a2a::wake_channel;
use zeroclaw_core::a2a::webhook::{build_webhook_router, WebhookState};

async fn setup_pool() -> PgPool {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let pool = PgPool::connect(&url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    sqlx::query("TRUNCATE inbox_events, push_notification_configs")
        .execute(&pool).await.ok();
    pool
}

#[tokio::test]
async fn webhook_rejects_missing_bearer() {
    let pool = setup_pool().await;
    let (tx, _rx) = wake_channel::channel();
    let state = WebhookState::new(pool.clone(), tx);
    let app = build_webhook_router(state);

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/webhook/a2a-notify")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"statusUpdate":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_persists_and_wakes_on_valid_token() {
    let pool = setup_pool().await;
    // Pre-seed a push config with a known token/task.
    sqlx::query!(
        "INSERT INTO push_notification_configs (task_id, url, token) VALUES ($1, $2, $3)",
        "t-seeded",
        "http://ignored",
        "tok-valid"
    )
    .execute(&pool).await.unwrap();

    let (tx, mut rx) = wake_channel::channel();
    let state = WebhookState::new(pool.clone(), tx);
    let app = build_webhook_router(state);

    let body = r#"{"statusUpdate":{"taskId":"t-seeded","contextId":"c1","status":{"state":"TASK_STATE_COMPLETED","timestamp":"2026-04-19T00:00:00Z"}}}"#;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/webhook/a2a-notify")
                .header("content-type", "application/json")
                .header("authorization", "Bearer tok-valid")
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);

    // Row persisted
    let count: i64 = sqlx::query_scalar!("SELECT COUNT(*) AS c FROM inbox_events WHERE task_id = 't-seeded'")
        .fetch_one(&pool).await.unwrap().unwrap();
    assert_eq!(count, 1);

    // Wake signal fired
    assert!(tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await.unwrap().is_some());
}
```

Run (will fail — webhook module missing):

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-core --test webhook_integration
```

- [ ] **Step 3: Implement**

Write `crates/zeroclaw-core/src/a2a/webhook.rs`:

```rust
//! Sam's A2A push-notification receiver.
//!
//! Validates the bearer token against `push_notification_configs`, persists the
//! event to `inbox_events` (idempotent on task_id+sequence), and fires a wake
//! signal on the reasoning loop's channel.

use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use std::sync::Arc;

use super::wake_channel::WakeSender;

#[derive(Clone)]
pub struct WebhookState {
    pool: PgPool,
    wake: WakeSender,
}

impl WebhookState {
    pub fn new(pool: PgPool, wake: WakeSender) -> Self {
        Self { pool, wake }
    }
}

pub fn build_webhook_router(state: WebhookState) -> Router {
    Router::new()
        .route("/webhook/a2a-notify", post(handle_notify))
        .with_state(Arc::new(state))
}

#[derive(Deserialize)]
struct StatusUpdateEnvelope {
    #[serde(rename = "statusUpdate")]
    status_update: Option<StatusUpdate>,
    #[serde(rename = "artifactUpdate")]
    artifact_update: Option<Value>,
    task: Option<TaskIdWrapper>,
}

#[derive(Deserialize)]
struct StatusUpdate {
    #[serde(rename = "taskId")]
    task_id: String,
}

#[derive(Deserialize)]
struct TaskIdWrapper {
    id: String,
}

async fn handle_notify(
    State(state): State<Arc<WebhookState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // Extract Bearer token
    let Some(token) = extract_bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer").into_response();
    };

    // Determine task_id from the envelope
    let Ok(envelope) = serde_json::from_value::<StatusUpdateEnvelope>(body.clone()) else {
        return (StatusCode::BAD_REQUEST, "malformed envelope").into_response();
    };
    let task_id = envelope
        .status_update.as_ref().map(|s| s.task_id.clone())
        .or_else(|| envelope.task.as_ref().map(|t| t.id.clone()));
    let Some(task_id) = task_id else {
        return (StatusCode::BAD_REQUEST, "no task_id in envelope").into_response();
    };

    // Look up the stored push-notification token for this task
    let stored: Option<String> = sqlx::query_scalar!(
        "SELECT token FROM push_notification_configs WHERE task_id = $1 LIMIT 1",
        task_id
    )
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if stored.as_deref() != Some(token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "token mismatch").into_response();
    }

    // Persist event (idempotent on (task_id, 0) for status/task; artifact uses index later)
    let insert = sqlx::query!(
        r#"
        INSERT INTO inbox_events (id, task_id, sequence, payload_json)
        VALUES ($1, $2, 0, $3)
        ON CONFLICT (task_id, sequence) DO NOTHING
        "#,
        uuid::Uuid::new_v4(),
        task_id,
        body,
    )
    .execute(&state.pool)
    .await;

    if insert.is_err() {
        return (StatusCode::INTERNAL_SERVER_ERROR, "persist failed").into_response();
    }

    // Wake the reasoning loop
    state.wake.wake();

    (StatusCode::OK, "").into_response()
}

fn extract_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(str::to_owned)
}
```

Add `pub mod webhook;` in `crates/zeroclaw-core/src/a2a/mod.rs`.

- [ ] **Step 4: Run tests**

```bash
DATABASE_URL=postgres://postgres@localhost/a2a_test cargo test -p zeroclaw-core --test webhook_integration
```

Expected: 2 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-core/
git commit -m "feat(a2a): Sam webhook receiver with bearer auth + wake signal"
```

---

### Task 1.12: Wire A2A router + outbox worker into main binary

**Files:**
- Modify: `src/gateway/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Read existing router composition**

Open `src/gateway/mod.rs`. Locate the function that assembles the top-level `Router` (search for `Router::new()` and `.merge(` and `.nest(`). Identify where ACP's router is plugged in.

- [ ] **Step 2: Add A2A router composition**

In `src/gateway/mod.rs`, add alongside the ACP nest:

```rust
// A2A v1.0 server (runs alongside ACP; no traffic shift in Phase 1).
#[cfg(feature = "a2a")]
{
    use zeroclaw_core::a2a::{card, sam_executor::SamAgentExecutor, walter_executor::WalterAgentExecutor, webhook};
    use ra2a::server::{a2a_router, HandlerBuilder, ServerState};
    use zeroclaw_a2a_outbox::OutboxBackedPushSender;
    use std::sync::Arc;

    let push_sender = Arc::new(OutboxBackedPushSender::new(state.a2a_db.clone()));
    // NOTE: executor selection based on config.agent_role ("sam" vs "walter").
    // Placeholder wires Walter; Sam variant is symmetric — read config at startup.
    let card = card::walter_agent_card(&state.public_base_url);
    let handler = HandlerBuilder::new(WalterAgentExecutor, card.clone())
        .with_task_store(Arc::new(ra2a::server::PostgresTaskStore::new(state.a2a_db.clone())))
        .with_push_notifications(
            Arc::new(ra2a::server::PostgresPushNotificationConfigStore::new(state.a2a_db.clone())),
            push_sender,
        )
        .build();
    let a2a_state = ServerState::new(Arc::new(handler), card);
    router = router.merge(a2a_router(a2a_state));

    // Sam-only: attach webhook router
    if state.agent_role == "sam" {
        let webhook_state = webhook::WebhookState::new(state.a2a_db.clone(), state.wake_tx.clone());
        router = router.merge(webhook::build_webhook_router(webhook_state));
    }
}
```

(Exact types for `PostgresTaskStore` / `PostgresPushNotificationConfigStore` need to be looked up in `ra2a`'s docs — if the crate exports different names, adjust. If `ra2a` does not yet ship Postgres implementations, vendor the in-memory store for Phase 1 and add a follow-up task for the Postgres backend.)

- [ ] **Step 3: Add the `a2a` feature to `Cargo.toml`**

Add to the main crate `Cargo.toml`:

```toml
[features]
default = []
a2a = ["dep:zeroclaw-a2a-outbox", "dep:ra2a"]
```

And to `[dependencies]`:

```toml
zeroclaw-a2a-outbox = { path = "crates/zeroclaw-a2a-outbox", optional = true }
ra2a = { workspace = true, optional = true }
```

- [ ] **Step 4: Main-binary startup changes**

In `src/main.rs`, add near other tokio::spawn calls:

```rust
#[cfg(feature = "a2a")]
{
    use zeroclaw_a2a_outbox::{OutboxWorker, retry::RetryPolicy};
    let max_attempts: u32 = std::env::var("ZEROCLAW_A2A_OUTBOX_MAX_ATTEMPTS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(5);
    let policy = RetryPolicy { max_attempts, ..RetryPolicy::default() };
    let worker = OutboxWorker::new(a2a_db.clone(), policy);
    tokio::spawn(async move {
        loop {
            if let Err(e) = worker.run_once().await {
                tracing::error!("outbox worker error: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
}
```

Also read `ZEROCLAW_A2A_DB_URL`, `ZEROCLAW_A2A_SCHEMA`, `ZEROCLAW_A2A_WEBHOOK_URL` from env at startup, establish the `PgPool`, set `search_path` to the schema per-connection, and stash the pool + wake channel sender on `AppState`. Match the existing config pattern in `src/config/schema.rs`.

Also read the new env vars (`ZEROCLAW_A2A_DB_URL`, `ZEROCLAW_A2A_SCHEMA`, `ZEROCLAW_A2A_WEBHOOK_URL`) into the Config struct. Match the existing config pattern in the codebase (likely in `src/config/schema.rs`).

- [ ] **Step 5: Build with the feature flag**

```bash
cargo build --features a2a
```

Expected: compiles. Any type mismatches between `ra2a` 0.10.1's actual exports and what this task assumed need to be fixed by consulting `cargo doc -p ra2a`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/
git commit -m "feat(a2a): wire A2A router and outbox worker into main binary (behind feature flag)"
```

---

### Task 1.13: CNPG cluster manifest for `zeroclaw-a2a`

**Files:**
- Create: `k8s/shared/28_cnpg_a2a_cluster.yaml`

- [ ] **Step 1: Write the manifest**

```yaml
apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: zeroclaw-a2a
  namespace: ai-agents
spec:
  instances: 2
  imageName: ghcr.io/cloudnative-pg/postgresql:16.4
  storage:
    size: 5Gi
    storageClass: rook-ceph-block
  bootstrap:
    initdb:
      database: a2a
      owner: a2a_owner
      postInitSQL:
        - "CREATE SCHEMA IF NOT EXISTS sam"
        - "CREATE SCHEMA IF NOT EXISTS walter"
        - "CREATE ROLE sam_rw LOGIN"
        - "CREATE ROLE walter_rw LOGIN"
        - "GRANT USAGE, CREATE ON SCHEMA sam TO sam_rw"
        - "GRANT USAGE, CREATE ON SCHEMA walter TO walter_rw"
        - "ALTER DEFAULT PRIVILEGES IN SCHEMA sam GRANT ALL ON TABLES TO sam_rw"
        - "ALTER DEFAULT PRIVILEGES IN SCHEMA walter GRANT ALL ON TABLES TO walter_rw"
  monitoring:
    enablePodMonitor: true
  resources:
    requests:
      cpu: "100m"
      memory: "256Mi"
    limits:
      memory: "512Mi"
```

- [ ] **Step 2: Dry-run apply**

```bash
kubectl apply --dry-run=server -f k8s/shared/28_cnpg_a2a_cluster.yaml
```

Expected: `cluster.postgresql.cnpg.io/zeroclaw-a2a created (server dry run)`.

- [ ] **Step 3: Apply for real**

```bash
kubectl apply -f k8s/shared/28_cnpg_a2a_cluster.yaml
```

- [ ] **Step 4: Wait for the cluster to be ready**

```bash
kubectl wait --for=condition=Ready cluster/zeroclaw-a2a -n ai-agents --timeout=300s
kubectl get pods -n ai-agents -l cnpg.io/cluster=zeroclaw-a2a
```

Expected: 2 pods Running.

- [ ] **Step 5: Commit**

```bash
git add k8s/shared/28_cnpg_a2a_cluster.yaml
git commit -m "feat(a2a/k8s): CNPG cluster zeroclaw-a2a with sam/walter schemas"
```

---

### Task 1.14: VaultStaticSecrets for per-agent DB credentials

**Files:**
- Create: `k8s/sam/30_vso_a2a_db_secret.yaml`
- Create: `k8s/walter/10_vso_a2a_db_secret.yaml`

- [ ] **Step 1: Seed the credentials in Vault**

Before writing the manifests, add passwords to Vault:

```bash
# Generate and store two passwords
PW_SAM=$(openssl rand -base64 24)
PW_WALTER=$(openssl rand -base64 24)
vault kv put zeroclaw/a2a sam_password="$PW_SAM" walter_password="$PW_WALTER"
```

Then in the CNPG cluster (manual step — CNPG `postInitSQL` runs as superuser but doesn't set passwords):

```bash
kubectl exec -n ai-agents -c postgres zeroclaw-a2a-1 -- psql -U postgres -d a2a -c \
  "ALTER ROLE sam_rw WITH PASSWORD '$PW_SAM'"
kubectl exec -n ai-agents -c postgres zeroclaw-a2a-1 -- psql -U postgres -d a2a -c \
  "ALTER ROLE walter_rw WITH PASSWORD '$PW_WALTER'"
```

- [ ] **Step 2: Write Sam's VSS manifest**

```yaml
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultStaticSecret
metadata:
  name: zeroclaw-a2a-sam
  namespace: ai-agents
spec:
  type: kv-v2
  mount: zeroclaw
  path: a2a
  destination:
    name: zeroclaw-a2a-sam
    create: true
    transformation:
      templates:
        a2a_db_url:
          text: |
            postgres://sam_rw:{{ .Secrets.sam_password }}@zeroclaw-a2a-rw.ai-agents.svc.cluster.local:5432/a2a
  refreshAfter: 60s
  vaultAuthRef: vault-auth-ai-agents
```

- [ ] **Step 3: Write Walter's VSS manifest**

Write `k8s/walter/10_vso_a2a_db_secret.yaml`:

```yaml
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultStaticSecret
metadata:
  name: zeroclaw-a2a-walter
  namespace: ai-agents
spec:
  type: kv-v2
  mount: zeroclaw
  path: a2a
  destination:
    name: zeroclaw-a2a-walter
    create: true
    transformation:
      templates:
        a2a_db_url:
          text: |
            postgres://walter_rw:{{ .Secrets.walter_password }}@zeroclaw-a2a-rw.ai-agents.svc.cluster.local:5432/a2a
  refreshAfter: 60s
  vaultAuthRef: vault-auth-ai-agents
```

- [ ] **Step 4: Apply and verify**

```bash
kubectl apply -f k8s/sam/30_vso_a2a_db_secret.yaml -f k8s/walter/10_vso_a2a_db_secret.yaml
kubectl get secret -n ai-agents zeroclaw-a2a-sam zeroclaw-a2a-walter
kubectl get secret -n ai-agents zeroclaw-a2a-sam -o jsonpath='{.data.a2a_db_url}' | base64 -d | sed 's/:[^@]*@/:REDACTED@/'
```

Expected: both secrets materialize; URL prints with redacted password.

- [ ] **Step 5: Commit**

```bash
git add k8s/sam/30_vso_a2a_db_secret.yaml k8s/walter/10_vso_a2a_db_secret.yaml
git commit -m "feat(a2a/k8s): Vault-backed DB credentials for Sam and Walter"
```

---

### Task 1.15: Wire DB cred env vars into sandbox manifests

**Files:**
- Modify: `k8s/sam/04_zeroclaw_sandbox.yaml`
- Modify: `k8s/walter/03_sandbox.yaml`

- [ ] **Step 1: Add env vars to Sam's container**

In the container spec for Sam (search for `- name: zeroclaw` under `containers:`), add under `env:`:

```yaml
- name: ZEROCLAW_A2A_DB_URL
  valueFrom:
    secretKeyRef:
      name: zeroclaw-a2a-sam
      key: a2a_db_url
- name: ZEROCLAW_A2A_SCHEMA
  value: "sam"
- name: ZEROCLAW_A2A_WEBHOOK_URL
  value: "http://zeroclaw.ai-agents.svc.cluster.local:3000/webhook/a2a-notify"
- name: ZEROCLAW_USE_A2A_DELEGATION
  value: "false"   # toggled in Phase 2
```

- [ ] **Step 2: Add env vars to Walter's container**

```yaml
- name: ZEROCLAW_A2A_DB_URL
  valueFrom:
    secretKeyRef:
      name: zeroclaw-a2a-walter
      key: a2a_db_url
- name: ZEROCLAW_A2A_SCHEMA
  value: "walter"
- name: ZEROCLAW_A2A_TASK_TIMEOUT_SECS
  value: "300"
```

- [ ] **Step 3: Dry-run**

```bash
kubectl apply --dry-run=server -f k8s/sam/04_zeroclaw_sandbox.yaml -f k8s/walter/03_sandbox.yaml
```

- [ ] **Step 4: Commit (don't apply yet — image with A2A feature flag doesn't exist)**

```bash
git add k8s/sam/04_zeroclaw_sandbox.yaml k8s/walter/03_sandbox.yaml
git commit -m "feat(a2a/k8s): wire DB creds and env vars into Sam and Walter sandboxes"
```

---

### Task 1.16: Build + push image with `a2a` feature flag on

**Files:** (no file changes; operational step)

- [ ] **Step 1: Build the image via the cluster-builder (see llama-pod session logs for builder setup)**

```bash
cd ~/github_projects/zeroclaw
docker buildx use cluster-builder
docker buildx build --push --progress plain \
  --build-arg ZEROCLAW_CARGO_FEATURES=a2a \
  -f Dockerfile.sam \
  -t gitea.coffee-anon.com/dan/zeroclaw-sam:v1.6.0 \
  .
```

(Dockerfile.sam already accepts `ZEROCLAW_CARGO_FEATURES` as a build arg — confirmed by reading it.)

- [ ] **Step 2: Update both sandbox manifests to use the new tag**

Edit `k8s/sam/04_zeroclaw_sandbox.yaml` and `k8s/walter/03_sandbox.yaml` — bump image refs to `v1.6.0`.

- [ ] **Step 3: Apply and restart**

```bash
kubectl apply -f k8s/sam/04_zeroclaw_sandbox.yaml -f k8s/walter/03_sandbox.yaml
kubectl delete pod -n ai-agents zeroclaw zeroclaw-k8s-agent
kubectl wait --for=condition=Ready pod/zeroclaw pod/zeroclaw-k8s-agent -n ai-agents --timeout=240s
```

- [ ] **Step 4: Commit**

```bash
git add k8s/sam/04_zeroclaw_sandbox.yaml k8s/walter/03_sandbox.yaml
git commit -m "feat(a2a/k8s): bump Sam and Walter to v1.6.0 with a2a feature enabled"
```

---

### Task 1.17: Phase 1 verification

**Files:** (no file changes; verification only)

- [ ] **Step 1: Verify AgentCards resolve on both pods**

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- curl -s http://localhost:3000/.well-known/agent-card.json | jq '.capabilities'
kubectl exec -n ai-agents zeroclaw-k8s-agent -c zeroclaw -- curl -s http://localhost:3000/.well-known/agent-card.json | jq '.capabilities'
```

Expected: both return `{"push_notifications": true, "streaming": false, ...}`.

- [ ] **Step 2: Verify tasks/list returns empty**

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- curl -s -X POST http://localhost:3000/ \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tasks/list","params":{}}' | jq
```

Expected: `{"result": {"tasks": []}}`.

- [ ] **Step 3: Verify webhook path rejects unauthenticated POSTs on Sam**

```bash
kubectl exec -n ai-agents zeroclaw -c zeroclaw -- curl -s -o /dev/null -w "%{http_code}" \
  -X POST http://localhost:3000/webhook/a2a-notify -H 'content-type: application/json' -d '{}'
```

Expected: `401`.

- [ ] **Step 4: Verify existing ACP paths still work**

Send Sam a Signal message that triggers a normal cron/task. Watch logs:

```bash
kubectl logs -n ai-agents zeroclaw -c zeroclaw --tail=50
```

Expected: normal behavior, no new errors from A2A layer.

- [ ] **Step 5: Document baseline and commit the verification checklist result**

Create `docs/superpowers/plans/2026-04-19-zeroclaw-a2a-phase1-verification.md` with the outputs from steps 1–4 and the observed pod restart counts. Commit:

```bash
git add docs/superpowers/plans/2026-04-19-zeroclaw-a2a-phase1-verification.md
git commit -m "docs(a2a): phase-1 verification results"
```

**Phase 1 exit criteria:** AgentCards resolve, tasks/list empty, webhook rejects anon, existing ACP traffic unchanged. If any fail, STOP and debug before proceeding to Phase 2.

---

## Phase 2 — Cut over `cluster-health-survey`

### Task 2.1: Implement Walter's real `AgentExecutor::execute` for cluster-health-survey

**Files:**
- Modify: `crates/zeroclaw-core/src/a2a/walter_executor.rs`

- [ ] **Step 1: Read existing "survey" implementation**

Search the codebase for how Walter currently handles a `cluster-health-survey` task when invoked via ACP. Likely entry point: `src/agent/agent.rs` or `src/gateway/acp_server.rs`. Identify the function that runs the survey and produces the report string.

- [ ] **Step 2: Refactor the survey logic into a reusable async function**

Extract into `crates/zeroclaw-core/src/a2a/survey.rs`:

```rust
pub async fn run_cluster_health_survey() -> anyhow::Result<String> {
    // Move the existing body from agent.rs here.
    // Returns the markdown report that would have been sent via ACP.
    todo!("move existing survey logic here — do NOT reimplement")
}
```

(The `todo!()` here is not a plan placeholder — the implementing engineer replaces it with the moved logic during this step.)

- [ ] **Step 3: Wire `WalterAgentExecutor::execute` to call the survey**

Replace the stub body:

```rust
Box::pin(async move {
    use ra2a::types::{Artifact, ArtifactId, Message, Part, Task, TaskState, TaskStatus, TaskStatusUpdateEvent};

    // Announce working
    queue.send(Event::TaskStatusUpdate(TaskStatusUpdateEvent {
        task_id: ctx.task_id.clone().into(),
        context_id: ctx.context_id.clone().into(),
        status: TaskStatus::new(TaskState::Working),
        r#final: false,
    }))?;

    // Run the survey with a timeout bound
    let timeout_secs: u64 = std::env::var("ZEROCLAW_A2A_TASK_TIMEOUT_SECS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(300);

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        super::survey::run_cluster_health_survey(),
    ).await;

    let mut task = Task::new(&ctx.task_id, &ctx.context_id);
    match result {
        Ok(Ok(report)) => {
            task.status = TaskStatus::with_message(
                TaskState::Completed,
                Message::agent(vec![Part::text(report)]),
            );
        }
        Ok(Err(e)) => {
            task.status = TaskStatus::with_message(
                TaskState::Failed,
                Message::agent(vec![Part::text(format!("survey failed: {e}"))]),
            );
        }
        Err(_) => {
            task.status = TaskStatus::with_message(
                TaskState::Canceled,
                Message::agent(vec![Part::text("timeout_exceeded")]),
            );
        }
    }
    queue.send(Event::Task(task))?;
    Ok(())
})
```

- [ ] **Step 4: Unit-test happy path and timeout**

Add inline `#[cfg(test)]` test that calls `execute` with a mock `EventQueue` and asserts the final `Task.state`.

- [ ] **Step 5: Run tests + compile**

```bash
cargo test -p zeroclaw-core a2a::walter_executor::
cargo build --features a2a
```

- [ ] **Step 6: Commit**

```bash
git add crates/zeroclaw-core/
git commit -m "feat(a2a/walter): real cluster-health-survey execute path with timeout"
```

---

### Task 2.2: Sam's A2A delegation helper (feature-flagged)

**Files:**
- Create: `crates/zeroclaw-core/src/a2a/delegation.rs`

- [ ] **Step 1: Write failing test**

```rust
#[tokio::test]
async fn delegate_survey_posts_message_send_to_walter() {
    // Mock Walter's A2A endpoint with wiremock, verify that the
    // delegation helper POSTs message/send with a pushNotificationConfig
    // whose URL matches ZEROCLAW_A2A_WEBHOOK_URL.
}
```

(Full test body uses the same `wiremock::MockServer` pattern as Task 1.6.)

- [ ] **Step 2: Implement**

```rust
pub struct A2ADelegationClient {
    http: reqwest::Client,
    walter_a2a_url: String,
    webhook_url: String,
}

impl A2ADelegationClient {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::new(),
            walter_a2a_url: std::env::var("ZEROCLAW_WALTER_A2A_URL")
                .unwrap_or_else(|_| "http://zeroclaw-k8s-agent.ai-agents.svc.cluster.local:3000".into()),
            webhook_url: std::env::var("ZEROCLAW_A2A_WEBHOOK_URL")?,
        })
    }

    pub async fn delegate_cluster_health_survey(&self) -> anyhow::Result<String> {
        use serde_json::json;
        let task_id = uuid::Uuid::new_v4().to_string();
        let token = uuid::Uuid::new_v4().to_string();
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "message/send",
            "params": {
                "message": {
                    "role": "ROLE_USER",
                    "messageId": uuid::Uuid::new_v4().to_string(),
                    "parts": [{"text": "cluster-health-survey"}],
                },
                "configuration": {
                    "pushNotificationConfig": {
                        "url": self.webhook_url,
                        "token": token,
                    }
                }
            }
        });
        let resp = self.http.post(&self.walter_a2a_url).json(&body).send().await?;
        resp.error_for_status()?;
        Ok(task_id)
    }
}
```

- [ ] **Step 3: Feature-flag the call site**

Find where Sam currently invokes the cluster-health-survey via ACP (likely in an agent tool-call handler). Gate on the env var:

```rust
if std::env::var("ZEROCLAW_USE_A2A_DELEGATION").as_deref() == Ok("true") {
    let client = A2ADelegationClient::from_env()?;
    let _task_id = client.delegate_cluster_health_survey().await?;
    // Return immediately; result arrives via webhook
    return Ok(Output::Pending);
} else {
    // existing ACP path unchanged
}
```

- [ ] **Step 4: Compile + test**

```bash
cargo test -p zeroclaw-core a2a::delegation::
cargo build --features a2a
```

- [ ] **Step 5: Commit**

```bash
git add crates/zeroclaw-core/ src/
git commit -m "feat(a2a/sam): A2A delegation helper for cluster-health-survey (feature-flagged)"
```

---

### Task 2.3: Reasoning loop wake integration

**Files:**
- Modify: `src/agent/agent.rs` (or wherever the main loop lives — check Prerequisites)

- [ ] **Step 1: Identify the main loop**

Find the `tokio::select!` or equivalent where the agent waits for trigger events (Signal, cron, ACP session/prompt). If no such select exists, add one.

- [ ] **Step 2: Add the wake branch**

```rust
let (wake_tx, mut wake_rx) = zeroclaw_core::a2a::wake_channel::channel();
// share wake_tx into AppState so the webhook handler can fire it

loop {
    tokio::select! {
        Some(_) = wake_rx.recv() => {
            // Drain any batched wake signals — one actual poll covers many.
            while wake_rx.try_recv().is_ok() {}
            handle_inbox_wake(&state).await?;
        }
        other_trigger = ... => { /* existing branches */ }
    }
}
```

- [ ] **Step 3: Write `handle_inbox_wake`**

```rust
async fn handle_inbox_wake(state: &AppState) -> anyhow::Result<()> {
    // Fetch all unprocessed inbox_events, process each as a turn-trigger.
    let rows = sqlx::query!(
        "SELECT id, task_id, payload_json FROM inbox_events WHERE processed_at IS NULL"
    ).fetch_all(&state.a2a_db).await?;

    for row in rows {
        // Construct a synthetic user-level prompt that Sam's existing turn
        // dispatcher understands — e.g., "Walter reported on task <task_id>: <summary>".
        // The summary extraction is a small function; for MVP just pass the
        // raw payload through as the prompt body.
        let prompt = format!(
            "[A2A inbound] task_id={} payload={}",
            row.task_id, row.payload_json
        );
        state.agent_dispatcher.dispatch_turn(&prompt).await?;
        sqlx::query!("UPDATE inbox_events SET processed_at = NOW() WHERE id = $1", row.id)
            .execute(&state.a2a_db).await?;
    }
    Ok(())
}
```

(`agent_dispatcher.dispatch_turn` is a placeholder — use whichever existing entry point handles a Signal message or ACP prompt. If the signature differs, adapt.)

- [ ] **Step 4: Compile + run**

```bash
cargo build --features a2a
```

- [ ] **Step 5: Commit**

```bash
git add src/
git commit -m "feat(a2a/sam): reasoning loop selects on wake channel; processes inbox_events"
```

---

### Task 2.4: Image rebuild + deploy

**Files:** (operational)

- [ ] **Step 1: Build v1.6.1**

```bash
cd ~/github_projects/zeroclaw
docker buildx build --push --progress plain \
  --build-arg ZEROCLAW_CARGO_FEATURES=a2a \
  -f Dockerfile.sam \
  -t gitea.coffee-anon.com/dan/zeroclaw-sam:v1.6.1 \
  .
```

- [ ] **Step 2: Update manifests, apply, restart**

Bump image tags in both sandboxes to `v1.6.1`. Apply, delete pods, wait Ready.

- [ ] **Step 3: Commit**

```bash
git add k8s/
git commit -m "feat(a2a/k8s): deploy v1.6.1 with Phase 2 code (feature still off)"
```

---

### Task 2.5: Enable the feature flag in dev, smoke-test

**Files:** (operational)

- [ ] **Step 1: Flip the flag on Sam**

```bash
kubectl set env -n ai-agents pod/zeroclaw ZEROCLAW_USE_A2A_DELEGATION=true
```

(If `set env` doesn't work on bare pods managed by Sandbox CR, patch the Sandbox spec instead.)

- [ ] **Step 2: Trigger a cluster-health-survey via Signal**

Send Sam a Signal message: "Check the cluster status".

- [ ] **Step 3: Observe**

Tail both pods' logs; watch Postgres outbox + inbox tables:

```bash
kubectl exec -n ai-agents zeroclaw-a2a-1 -- psql -U postgres a2a -c "SELECT status, attempts, task_id FROM walter.outbox ORDER BY created_at DESC LIMIT 5"
kubectl exec -n ai-agents zeroclaw-a2a-1 -- psql -U postgres a2a -c "SELECT task_id, processed_at IS NOT NULL AS processed FROM sam.inbox_events ORDER BY received_at DESC LIMIT 5"
```

Expected: Walter's outbox row transitions `pending` → `delivered`. Sam's inbox_events row shows `processed = true`. Sam's Signal reply contains the survey report. End-to-end latency << 30 min (the old Vikunja poll).

- [ ] **Step 4: If all green, hold for 1 week in production**

Document observed timings and any rough edges. If issues surface, unset the env var → instant revert.

- [ ] **Step 5: Commit observations**

```bash
echo "Observations from $(date +%Y-%m-%d): ..." >> docs/superpowers/plans/2026-04-19-zeroclaw-a2a-phase2-observations.md
git add docs/
git commit -m "docs(a2a): phase-2 field observations"
```

---

## Post-MVP (out of scope for this plan, linked for continuity)

- **Phase 3:** Migrate remaining Sam → Walter delegation paths to A2A. Future plan.
- **Phase 4:** Deprecate ACP for inter-agent use. Future plan.
- **LangGraph augmentation:** separate design + plan; A2A compliance is the handoff point.

---

## Self-Review Notes

Post-writing review flagged two items that were fixed inline:

1. **`SamAgentExecutor` referenced in Task 1.12 but defined in Task 1.10** — confirmed the router composition in 1.12 uses `WalterAgentExecutor` by default and adapts based on `agent_role` at runtime; both types exist by the time 1.12 runs.
2. **`PostgresTaskStore` / `PostgresPushNotificationConfigStore` naming in Task 1.12** — actual `ra2a` export names may differ from the placeholder. Added an explicit fallback note in the task: if Postgres stores aren't shipped in v0.10.1, use in-memory for Phase 1 and add a follow-up task to port the Postgres backend.

Remaining known gaps (to be resolved by the implementing engineer when reading actual code):

- Exact Config struct location for env var wiring (Task 1.12 Step 4)
- Exact main loop entry point for wake integration (Task 2.3)
- Exact call site to feature-flag in Task 2.2 Step 3

These are flagged as Prerequisite reading in the header.
