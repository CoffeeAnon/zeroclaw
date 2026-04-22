//! Sam-side A2A delegation helper.
//!
//! Wraps the JSON-RPC `message/send` POST to Walter's `:3001/` endpoint.
//! The call runs in blocking mode — ra2a 0.10.1's `returnImmediately: true`
//! path drops its broadcast receiver before the executor's terminal event
//! arrives, so the push callback never fires. See
//! `wiki/services/ra2a-limitations.md` for the full trace.
//!
//! Instead, we hold the HTTP connection open for the full agent run,
//! read Walter's completion out of the synchronous response, and let the
//! caller shim that back into Sam's inbox so the wake/resume semantics
//! are preserved at the Sam-side (see `src/tools/a2a_delegate.rs`).

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// Result of a completed delegation. Carries the server-minted task id
/// alongside Walter's final task snapshot so the caller can persist it
/// to Sam's inbox without any further round-trips.
#[derive(Debug, Clone)]
pub struct DelegationHandle {
    pub task_id: String,
    /// Random bearer token registered with Walter's push config. Kept on
    /// the handle so Sam's `push_notification_configs` row stays in sync
    /// with what Walter would push *if* the push path ever gets used in
    /// this direction. Not currently consumed by the sync-to-inbox shim.
    pub token: String,
    /// Walter's response task object (`result.task` from the JSON-RPC
    /// envelope). Forwarded verbatim into `inbox_events.payload_json` so
    /// the inbox drain can hand it to Sam's next agent turn.
    pub task: Value,
}

/// Thin A2A client configured for the Sam → Walter delegation path.
pub struct A2ADelegationClient {
    http: reqwest::Client,
    walter_a2a_url: String,
    webhook_url: String,
}

impl A2ADelegationClient {
    pub fn new(http: reqwest::Client, walter_a2a_url: String, webhook_url: String) -> Self {
        Self {
            http,
            walter_a2a_url,
            webhook_url,
        }
    }

    /// Construct from environment:
    ///
    /// - `ZEROCLAW_WALTER_A2A_URL` — defaults to Walter's in-cluster service.
    /// - `ZEROCLAW_A2A_WEBHOOK_URL` — required; the URL Walter posts back to.
    pub fn from_env() -> Result<Self> {
        let walter_a2a_url = std::env::var("ZEROCLAW_WALTER_A2A_URL").unwrap_or_else(|_| {
            "http://zeroclaw-k8s-agent.ai-agents.svc.cluster.local:3001".to_string()
        });
        let webhook_url = std::env::var("ZEROCLAW_A2A_WEBHOOK_URL")
            .context("ZEROCLAW_A2A_WEBHOOK_URL must be set for A2A delegation")?;
        Ok(Self::new(reqwest::Client::new(), walter_a2a_url, webhook_url))
    }

    /// Send `prompt` to Walter and block until the agent run completes.
    ///
    /// Returns Walter's final `result.task` value alongside the server-
    /// minted task id. The HTTP connection is held for the full duration
    /// of the run (5-minute default cap enforced by Walter's executor);
    /// callers should spawn this on a background task so Sam's main
    /// reasoning loop isn't blocked.
    pub async fn delegate(&self, prompt: &str) -> Result<DelegationHandle> {
        // Wire-format notes (see wiki/services/ra2a-limitations.md):
        //   - `message.taskId` MUST be omitted. A client-provided id is
        //     treated as a reference to an existing task and ra2a returns
        //     `-32603 task not found`.
        //   - Push config key is `taskPushNotificationConfig`. We keep it
        //     on the request so Walter's push store stays populated — if
        //     we later move off the sync shim, the push path is already
        //     wired from Sam's side.
        //   - `returnImmediately` is left at its default (`false`). The
        //     early-return path is broken in ra2a 0.10.1 and drops the
        //     terminal event on the floor.
        let token = uuid::Uuid::new_v4().to_string();
        let message_id = uuid::Uuid::new_v4().to_string();

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "message/send",
            "params": {
                "message": {
                    "role": "ROLE_USER",
                    "messageId": message_id,
                    "parts": [{"text": prompt}],
                },
                "configuration": {
                    "taskPushNotificationConfig": {
                        "url": self.webhook_url,
                        "token": token,
                    }
                }
            }
        });

        let resp = self
            .http
            .post(&self.walter_a2a_url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {} failed", self.walter_a2a_url))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .with_context(|| format!("read body from {}", self.walter_a2a_url))?;

        if !status.is_success() {
            anyhow::bail!("Walter A2A returned HTTP {status}: {text}");
        }

        // JSON-RPC errors come back with HTTP 200 and an `error` field;
        // checking only the HTTP status silently accepts them.
        let envelope: Value = serde_json::from_str(&text)
            .with_context(|| format!("parse JSON-RPC envelope from {}: {text}", self.walter_a2a_url))?;

        if let Some(err) = envelope.get("error") {
            anyhow::bail!("Walter A2A JSON-RPC error: {err}");
        }

        let task = envelope
            .pointer("/result/task")
            .cloned()
            .with_context(|| format!("missing result.task in Walter response: {text}"))?;

        let task_id = task
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .with_context(|| format!("missing result.task.id in Walter response: {text}"))?;

        Ok(DelegationHandle {
            task_id,
            token,
            task,
        })
    }
}
