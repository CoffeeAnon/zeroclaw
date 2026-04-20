//! A2A wake/resume integration layer.
//!
//! Runs on a *separate* HTTP listener from the main gateway. The reason is
//! state: ra2a's `a2a_router` and our webhook router both bake their own
//! state via `.with_state`, producing `Router<()>`. Axum 0.8 does not let
//! `Router<()>` merge into a `Router<AppState>` parent (no `From` impl), so
//! the two cleanest options are (a) separate listener, (b) rebuild routes by
//! hand. We pick (a). This also gives us per-listener body limits / timeouts
//! tuned for A2A traffic without polluting the main gateway.
//!
//! Toggled entirely by the `a2a` Cargo feature.
//!
//! ## Env vars
//!
//! - `ZEROCLAW_A2A_DB_URL` — Postgres DSN for the A2A DB (required when enabled)
//! - `ZEROCLAW_A2A_SCHEMA` — schema name, defaults to `public`. Per-agent
//!   schemas (`sam`, `walter`) are wired by appending a libpq `options=-c
//!   search_path=<schema>,public` to the DSN; this survives pool reconnects
//!   without needing an `after_connect` closure (which would trip sqlx 0.8's
//!   `Executor<'_>` HRTB and cascade Send-ness issues into `spawn_component_supervisor`).
//! - `ZEROCLAW_A2A_AGENT_ROLE` — `sam` or `walter`. Determines which executor
//!   and agent card to mount; Sam additionally gets the `/webhook/a2a-notify`
//!   route.
//! - `ZEROCLAW_A2A_PUBLIC_BASE_URL` — base URL advertised in the agent card
//!   (e.g. `https://sam.ai-agents.svc.cluster.local:3001`).
//! - `ZEROCLAW_A2A_BIND_ADDR` — listener address, defaults to `0.0.0.0:3001`.
//! - `ZEROCLAW_A2A_OUTBOX_MAX_ATTEMPTS` — retry cap for the outbox worker,
//!   defaults to `5`.

use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use axum::Router;
use ra2a::server::{
    a2a_router, AgentExecutor, HandlerBuilder, InMemoryPushNotificationConfigStore,
    InMemoryTaskStore, ServerState,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use zeroclaw_a2a_outbox::{migrate, OutboxBackedPushSender, OutboxWorker, RetryPolicy};
use zeroclaw_core::a2a::{
    card::{sam_agent_card, walter_agent_card},
    sam_executor::SamAgentExecutor,
    wake_channel,
    walter_executor::WalterAgentExecutor,
    webhook::{build_webhook_router, WebhookState},
};

/// Spawns the A2A listener + outbox worker in the background.
///
/// Returns `Ok(())` even if `ZEROCLAW_A2A_DB_URL` is unset — an empty feature
/// knob, treated as "A2A compiled in but not configured for this pod". This
/// lets the same binary run in pods that don't participate in A2A.
pub async fn setup() -> Result<()> {
    let Ok(db_url) = env::var("ZEROCLAW_A2A_DB_URL") else {
        tracing::info!("A2A feature compiled in but ZEROCLAW_A2A_DB_URL unset; skipping setup");
        return Ok(());
    };

    let schema = env::var("ZEROCLAW_A2A_SCHEMA").unwrap_or_else(|_| "public".to_string());
    let bind_addr = env::var("ZEROCLAW_A2A_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:3001".to_string());
    let role = env::var("ZEROCLAW_A2A_AGENT_ROLE")
        .context("ZEROCLAW_A2A_AGENT_ROLE must be set to `sam` or `walter`")?;
    let base_url = env::var("ZEROCLAW_A2A_PUBLIC_BASE_URL")
        .context("ZEROCLAW_A2A_PUBLIC_BASE_URL must be set when A2A is enabled")?;
    let max_attempts: u32 = env::var("ZEROCLAW_A2A_OUTBOX_MAX_ATTEMPTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let pool = build_pool(&db_url, &schema).await?;
    migrate::apply(&pool)
        .await
        .context("failed to apply A2A migrations")?;

    let (wake_tx, wake_rx) = wake_channel::channel();
    let _ = wake_rx; // reasoning-loop integration lands in Task 2.3; hold for now.

    let push_sender = Arc::new(OutboxBackedPushSender::new(pool.clone()));

    // HandlerBuilder::new takes `impl AgentExecutor + 'static` by value, so we
    // branch on role and construct with the concrete executor type each time.
    let a2a_router = match role.as_str() {
        "sam" => build_a2a_router(SamAgentExecutor, sam_agent_card(&base_url), push_sender.clone()),
        "walter" => {
            build_a2a_router(WalterAgentExecutor, walter_agent_card(&base_url), push_sender.clone())
        }
        other => {
            return Err(anyhow!(
                "ZEROCLAW_A2A_AGENT_ROLE must be `sam` or `walter`, got `{other}`"
            ));
        }
    };

    let webhook_router = if role == "sam" {
        let webhook_state = WebhookState::new(pool.clone(), wake_tx);
        Some(build_webhook_router(webhook_state))
    } else {
        drop(wake_tx);
        None
    };

    spawn_outbox_worker(pool, max_attempts);

    // Compose the listener router. Both sub-routers are Router<()> so they
    // merge freely here.
    let mut app: Router = a2a_router;
    if let Some(webhook) = webhook_router {
        app = app.merge(webhook);
    }

    let addr: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("invalid ZEROCLAW_A2A_BIND_ADDR: {bind_addr}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("bind A2A listener on {addr}"))?;
    tracing::info!(%addr, role = %role, schema = %schema, "A2A wake/resume layer listening");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app.into_make_service()).await {
            tracing::error!("A2A listener exited: {e}");
        }
    });

    Ok(())
}

fn build_a2a_router<E>(
    executor: E,
    card: ra2a::types::AgentCard,
    push_sender: Arc<OutboxBackedPushSender>,
) -> Router
where
    E: AgentExecutor + 'static,
{
    let handler = HandlerBuilder::new(executor, card.clone())
        .with_task_store(Arc::new(InMemoryTaskStore::new()))
        .with_push_notifications(
            Arc::new(InMemoryPushNotificationConfigStore::new()),
            push_sender,
        )
        .build();
    let server_state = ServerState::new(Arc::new(handler), card);
    a2a_router(server_state)
}

async fn build_pool(dsn: &str, schema: &str) -> Result<PgPool> {
    // Bake the per-connection search_path into the DSN as a libpq `options`
    // query parameter. This survives pool reconnects without needing an
    // after_connect closure (which hits HRTB lifetime issues in sqlx 0.8).
    //
    // URL-encoded form: `options=-c%20search_path%3D<schema>%2Cpublic`.
    let encoded = format!("-c search_path={schema},public");
    let encoded =
        encoded.replace(' ', "%20").replace('=', "%3D").replace(',', "%2C");
    let separator = if dsn.contains('?') { '&' } else { '?' };
    let dsn = format!("{dsn}{separator}options={encoded}");

    PgPoolOptions::new()
        .max_connections(8)
        .connect(&dsn)
        .await
        .with_context(|| "connect A2A Postgres".to_string())
}

fn spawn_outbox_worker(pool: PgPool, max_attempts: u32) {
    let policy = RetryPolicy {
        max_attempts,
        ..RetryPolicy::default()
    };
    let worker = OutboxWorker::new(pool, policy);
    tokio::spawn(async move {
        loop {
            if let Err(e) = worker.run_once().await {
                tracing::error!("A2A outbox worker error: {e}");
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
}
