//! A2A delegation tool — Sam-side "ask Walter asynchronously".
//!
//! The ra2a 0.10.1 push path is broken when the handler returns early
//! (see `wiki/services/ra2a-limitations.md`), so we use a sync-to-inbox
//! shim instead:
//!
//! 1. The tool spawns a background tokio task and returns immediately.
//! 2. The background task POSTs `message/send` in blocking mode and
//!    waits for Walter's full agent run to complete (up to Walter's
//!    executor timeout, default 300 s).
//! 3. On success it writes `a2a_delegations` (for session correlation),
//!    `push_notification_configs` (so the existing webhook path still
//!    works if we ever get push delivery working), and `inbox_events`
//!    (the wake/resume trigger the drain polls).
//! 4. The inbox drain picks up the row within ~5 s and dispatches a new
//!    agent turn with Sam's original session id, so memory continuity
//!    from the delegate call through the reply is preserved.
//!
//! On background-task failure we still write an `inbox_events` row with
//! a failure payload so Sam's next turn can tell the user what went
//! wrong instead of silently never replying.
//!
//! Registration is gated on the `a2a` Cargo feature AND
//! `ZEROCLAW_USE_A2A_DELEGATION=true` at startup.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::OnceCell;
use zeroclaw_core::a2a::delegation::{A2ADelegationClient, DelegationHandle};

use crate::agent::turn_context::{current_channel_binding, current_session_id};
use crate::tools::traits::{Tool, ToolResult};

pub struct A2ADelegateTool {
    http: reqwest::Client,
    pool: OnceCell<PgPool>,
}

impl A2ADelegateTool {
    pub fn new() -> Self {
        // Walter's executor caps individual runs at 300 s by default
        // (`ZEROCLAW_A2A_TASK_TIMEOUT_SECS`). We hold the blocking POST
        // open at least that long plus a buffer for HTTP overhead —
        // reqwest's default has no overall timeout, which would leak
        // connections if Walter hung forever.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(360))
            .build()
            .expect("reqwest client with timeout");
        Self {
            http,
            pool: OnceCell::new(),
        }
    }

    async fn pool(&self) -> Result<&PgPool> {
        self.pool
            .get_or_try_init(|| async {
                let dsn = std::env::var("ZEROCLAW_A2A_DB_URL").context(
                    "ZEROCLAW_A2A_DB_URL must be set for ask_walter to reach Walter",
                )?;
                let schema = std::env::var("ZEROCLAW_A2A_SCHEMA")
                    .unwrap_or_else(|_| "public".to_string());
                let encoded = format!("-c search_path={schema},public")
                    .replace(' ', "%20")
                    .replace('=', "%3D")
                    .replace(',', "%2C");
                let separator = if dsn.contains('?') { '&' } else { '?' };
                let dsn = format!("{dsn}{separator}options={encoded}");
                PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&dsn)
                    .await
                    .context("connect ask_walter pool")
            })
            .await
    }
}

impl Default for A2ADelegateTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for A2ADelegateTool {
    fn name(&self) -> &str {
        "ask_walter"
    }

    fn description(&self) -> &str {
        "Delegate a read-only infrastructure or Kubernetes observation task to Walter \
         (the cluster-monitoring agent) asynchronously. Use for cluster health checks, \
         kubectl observations, workload status, log investigation, or any other \
         read-only question about the Kubernetes cluster. \
         \
         IMPORTANT: this tool returns IMMEDIATELY with a task id — Walter's actual \
         answer arrives later as a new message to you, typically within 1–5 minutes. \
         When you call it, tell the user `I've asked Walter and will follow up when \
         he replies.` and end your turn. Do NOT wait or poll for the result inside \
         the same turn. \
         \
         Do NOT use this for: write operations (Walter is read-only), anything that \
         needs a sub-second answer, or anything within your own capabilities."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The natural-language question or task for Walter. \
                                    Include enough context that he doesn't need to ask \
                                    follow-up questions. Good example: `Check the status \
                                    of all pods in the ai-agents namespace and report any \
                                    that are not Ready.` Bad example: `check cluster` \
                                    (too vague)."
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        })
    }

    async fn execute(&self, args: serde_json::Value) -> Result<ToolResult> {
        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .context("ask_walter requires a non-empty `prompt` string argument")?
            .to_string();

        let webhook_url = std::env::var("ZEROCLAW_A2A_WEBHOOK_URL")
            .context("ZEROCLAW_A2A_WEBHOOK_URL must be set for A2A delegation")?;
        let walter_a2a_url = std::env::var("ZEROCLAW_WALTER_A2A_URL").unwrap_or_else(|_| {
            "http://zeroclaw-k8s-agent.ai-agents.svc.cluster.local:3001".to_string()
        });

        // Resolve the pool on the caller's task so failures here surface
        // inline rather than disappearing into a background task.
        let pool = self
            .pool()
            .await
            .context("A2A delegation DB pool")?
            .clone();
        let session_id = current_session_id();
        let (channel, sender) = current_channel_binding();
        let http = self.http.clone();

        tokio::spawn(async move {
            let client = A2ADelegationClient::new(http, walter_a2a_url, webhook_url.clone());
            run_delegation(
                client, pool, webhook_url, session_id, channel, sender, prompt,
            )
            .await;
        });

        Ok(ToolResult {
            success: true,
            output: "Delegated to Walter. His response will arrive as a follow-up \
                    message to you, typically within 1–5 minutes. Tell the user \
                    you've asked Walter and will follow up when he replies, then \
                    end your turn."
                .to_string(),
            error: None,
        })
    }
}

async fn run_delegation(
    client: A2ADelegationClient,
    pool: PgPool,
    webhook_url: String,
    session_id: Option<String>,
    channel: Option<String>,
    sender: Option<String>,
    prompt: String,
) {
    match client.delegate(&prompt).await {
        Ok(handle) => {
            tracing::info!(
                task_id = %handle.task_id,
                session_id = ?session_id,
                channel = ?channel,
                prompt_len = prompt.len(),
                "ask_walter: Walter replied, writing to inbox"
            );
            if let Err(e) = persist_success(
                &pool,
                &webhook_url,
                &session_id,
                &channel,
                &sender,
                &prompt,
                &handle,
            )
            .await
            {
                tracing::error!(
                    task_id = %handle.task_id,
                    error = ?e,
                    "ask_walter: failed to persist Walter's reply; Sam will not be woken"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                session_id = ?session_id,
                channel = ?channel,
                error = ?e,
                "ask_walter: delegation to Walter failed; writing failure inbox event"
            );
            if let Err(persist_err) =
                persist_failure(&pool, &session_id, &channel, &sender, &prompt, &e).await
            {
                tracing::error!(
                    error = ?persist_err,
                    "ask_walter: failed to persist failure inbox event; Sam will not be woken"
                );
            }
        }
    }
}

async fn persist_success(
    pool: &PgPool,
    webhook_url: &str,
    session_id: &Option<String>,
    channel: &Option<String>,
    sender: &Option<String>,
    prompt: &str,
    handle: &DelegationHandle,
) -> Result<()> {
    // Keep the push store consistent — if we ever move off the sync shim,
    // Walter's eventual push would need this row to validate its bearer.
    sqlx::query(
        "INSERT INTO push_notification_configs (task_id, config_id, url, token)
         VALUES ($1, '', $2, $3)
         ON CONFLICT (task_id, config_id) DO UPDATE
           SET url = EXCLUDED.url, token = EXCLUDED.token",
    )
    .bind(&handle.task_id)
    .bind(webhook_url)
    .bind(&handle.token)
    .execute(pool)
    .await
    .context("persist push_notification_configs")?;

    sqlx::query(
        "INSERT INTO a2a_delegations (task_id, session_id, prompt, channel, sender)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (task_id) DO NOTHING",
    )
    .bind(&handle.task_id)
    .bind(session_id)
    .bind(prompt)
    .bind(channel)
    .bind(sender)
    .execute(pool)
    .await
    .context("persist a2a_delegations")?;

    // `payload_json` here mirrors the ra2a push envelope shape the
    // webhook writes — `{ "task": <Task> }` — so the inbox drain's
    // existing dispatch logic doesn't need to branch on source.
    let payload = serde_json::json!({ "task": handle.task.clone() });

    sqlx::query(
        r#"
        INSERT INTO inbox_events (id, task_id, sequence, payload_json)
        VALUES (gen_random_uuid(), $1, 0, $2)
        ON CONFLICT (task_id, sequence) DO NOTHING
        "#,
    )
    .bind(&handle.task_id)
    .bind(&payload)
    .execute(pool)
    .await
    .context("persist inbox_events")?;

    Ok(())
}

async fn persist_failure(
    pool: &PgPool,
    session_id: &Option<String>,
    channel: &Option<String>,
    sender: &Option<String>,
    prompt: &str,
    error: &anyhow::Error,
) -> Result<()> {
    // Mint a synthetic task id so the drain's correlation lookup still
    // works. The delegation row gives the next turn enough context to
    // tell the user what went wrong.
    let task_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO a2a_delegations (task_id, session_id, prompt, channel, sender)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (task_id) DO NOTHING",
    )
    .bind(&task_id)
    .bind(session_id)
    .bind(prompt)
    .bind(channel)
    .bind(sender)
    .execute(pool)
    .await
    .context("persist failure a2a_delegations")?;

    let payload = serde_json::json!({
        "error": format!("{error:#}"),
        "note": "ask_walter delegation failed before Walter replied; no task result available"
    });

    sqlx::query(
        r#"
        INSERT INTO inbox_events (id, task_id, sequence, payload_json)
        VALUES (gen_random_uuid(), $1, 0, $2)
        ON CONFLICT (task_id, sequence) DO NOTHING
        "#,
    )
    .bind(&task_id)
    .bind(&payload)
    .execute(pool)
    .await
    .context("persist failure inbox_events")?;

    Ok(())
}
