//! Sam-side A2A delegation helper.
//!
//! Wraps the JSON-RPC `message/send` POST to Walter's `:3001/` endpoint,
//! attaching a `pushNotificationConfig` so the result is pushed back to
//! Sam's webhook instead of returned synchronously.
//!
//! The helper only owns the HTTP request. The caller is responsible for:
//!
//! 1. Persisting the returned `(task_id, token)` into Sam's
//!    `push_notification_configs` table before the POST returns, so that
//!    Sam's webhook handler can validate the bearer token when Walter
//!    pushes the completion back. (Order matters: if Walter is fast
//!    enough to push back before the insert lands, the webhook will
//!    reject the first notification as an unknown token.)
//! 2. Blocking / returning to the reasoning loop appropriately while
//!    waiting for the webhook to fire — the delegation call itself
//!    returns as soon as Walter acknowledges the `message/send`.

use anyhow::{Context, Result};
use serde_json::json;

/// Opaque handle to an in-flight delegation. The caller must persist the
/// `(task_id, token)` pair before reading Walter's eventual push.
#[derive(Debug, Clone)]
pub struct DelegationHandle {
    pub task_id: String,
    /// Random bearer token Walter will echo back in the `Authorization`
    /// header when it posts the completion update to Sam's webhook.
    pub token: String,
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

    /// Send `prompt` to Walter as the body of an A2A `message/send` request
    /// and register a push-notification callback pointing at Sam's webhook.
    ///
    /// Returns the generated `(task_id, token)` pair — the caller should
    /// persist these into `push_notification_configs` so the webhook can
    /// validate the eventual inbound bearer.
    pub async fn delegate(&self, prompt: &str) -> Result<DelegationHandle> {
        let task_id = uuid::Uuid::new_v4().to_string();
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
                    "taskId": task_id,
                    "parts": [{"text": prompt}],
                },
                "configuration": {
                    "pushNotificationConfig": {
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
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Walter A2A returned {status}: {text}");
        }

        Ok(DelegationHandle { task_id, token })
    }
}
