//! A2A delegation tool — Sam-side "ask Walter asynchronously".
//!
//! When invoked, this tool:
//!
//! 1. Generates a `(task_id, token)` pair and POSTs `message/send` to
//!    Walter's `:3001/` with a `pushNotificationConfig` pointing at Sam's
//!    webhook (`ZEROCLAW_A2A_WEBHOOK_URL`).
//! 2. Persists the token into `push_notification_configs` so Sam's
//!    webhook handler can validate the bearer when Walter pushes back.
//! 3. Persists correlation metadata (task_id, session_id, prompt) into
//!    `a2a_delegations` so the follow-up inbox-drain turn can reuse the
//!    original session id and Sam has memory continuity from the delegate
//!    call to the eventual reply.
//! 4. Returns immediately with a short status string — the LLM is
//!    expected to tell the user it's following up and end its turn.
//!    Walter's answer arrives later as a synthetic `[A2A inbound …]`
//!    prompt to a fresh reasoning turn scoped to the same session.
//!
//! Registration is gated on the `a2a` Cargo feature AND
//! `ZEROCLAW_USE_A2A_DELEGATION=true` at startup.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::OnceCell;
use zeroclaw_core::a2a::delegation::A2ADelegationClient;

use crate::agent::turn_context::current_session_id;
use crate::tools::traits::{Tool, ToolResult};

pub struct A2ADelegateTool {
    http: reqwest::Client,
    pool: OnceCell<PgPool>,
}

impl A2ADelegateTool {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::new(),
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
            .context("ask_walter requires a non-empty `prompt` string argument")?;

        let webhook_url = std::env::var("ZEROCLAW_A2A_WEBHOOK_URL")
            .context("ZEROCLAW_A2A_WEBHOOK_URL must be set for A2A delegation")?;
        let walter_a2a_url = std::env::var("ZEROCLAW_WALTER_A2A_URL").unwrap_or_else(|_| {
            "http://zeroclaw-k8s-agent.ai-agents.svc.cluster.local:3001".to_string()
        });

        let client =
            A2ADelegationClient::new(self.http.clone(), walter_a2a_url, webhook_url.clone());
        let handle = client
            .delegate(prompt)
            .await
            .context("A2A delegation POST to Walter failed")?;

        let pool = self.pool().await.context("A2A delegation DB pool")?;
        let session_id = current_session_id();

        // push_notification_configs: bearer-token store Sam's webhook
        // validates against when Walter pushes the reply back.
        sqlx::query(
            "INSERT INTO push_notification_configs (task_id, config_id, url, token)
             VALUES ($1, '', $2, $3)
             ON CONFLICT (task_id, config_id) DO UPDATE
               SET url = EXCLUDED.url, token = EXCLUDED.token",
        )
        .bind(&handle.task_id)
        .bind(&webhook_url)
        .bind(&handle.token)
        .execute(pool)
        .await
        .context("persist push_notification_configs")?;

        // a2a_delegations: correlation so the follow-up inbox turn can
        // resume Sam's original session (memory continuity).
        sqlx::query(
            "INSERT INTO a2a_delegations (task_id, session_id, prompt)
             VALUES ($1, $2, $3)
             ON CONFLICT (task_id) DO NOTHING",
        )
        .bind(&handle.task_id)
        .bind(&session_id)
        .bind(prompt)
        .execute(pool)
        .await
        .context("persist a2a_delegations")?;

        tracing::info!(
            task_id = %handle.task_id,
            session_id = ?session_id,
            prompt_len = prompt.len(),
            "ask_walter: delegated to Walter"
        );

        Ok(ToolResult {
            success: true,
            output: format!(
                "Delegated to Walter as task `{}`. His response will arrive as a \
                 follow-up message to you, typically within 1–5 minutes. Tell the \
                 user you've asked Walter and will follow up when he replies, then \
                 end your turn.",
                handle.task_id
            ),
            error: None,
        })
    }
}
