//! `ra2a::server::PushSender` implementation backed by the durable outbox.
//!
//! When ra2a finalizes a task, it calls `send_push(config, task)`. Instead of
//! POSTing directly, we enqueue a row in the `outbox` table and let the worker
//! (Task 1.6) deliver it asynchronously with retries. This gives at-least-once
//! semantics across restarts — the whole point of the A2A wake/resume layer.
//!
//! Payload shape: the full `Task` snapshot serialized as JSON. `sequence` is
//! always 0 in MVP; terminal-state dedup relies on `UNIQUE (task_id, sequence)`
//! so repeated emissions of the same task id are idempotent.

use std::future::Future;
use std::pin::Pin;

use ra2a::server::PushSender;
use ra2a::types::{PushNotificationConfig, Task};
use ra2a::{A2AError, Result};
use sqlx::PgPool;

use crate::store::OutboxStore;

pub struct OutboxBackedPushSender {
    store: OutboxStore,
}

impl OutboxBackedPushSender {
    pub fn new(pool: PgPool) -> Self {
        Self {
            store: OutboxStore::new(pool),
        }
    }
}

impl PushSender for OutboxBackedPushSender {
    fn send_push<'a>(
        &'a self,
        config: &'a PushNotificationConfig,
        task: &'a Task,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let payload = serde_json::to_value(task)
                .map_err(|e| A2AError::Other(format!("serialize task: {e}")))?;
            self.store
                .enqueue(
                    task.id.as_str(),
                    0,
                    &config.url,
                    config.token.as_deref(),
                    payload,
                )
                .await
                .map_err(|e| A2AError::Database(e.to_string()))?;
            Ok(())
        })
    }
}
