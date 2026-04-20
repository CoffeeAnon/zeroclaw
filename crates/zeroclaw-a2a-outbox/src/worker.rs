//! Outbox worker: claims due rows, POSTs them, and handles retry / dead-letter.
//!
//! 2xx → `delivered`. 4xx → `deadletter` (non-retryable). 5xx or transport
//! error → retry with exponential backoff until `RetryPolicy::max_attempts`
//! is exceeded, then dead-letter.

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
                self.store
                    .mark_deadletter(rec.id, &format!("HTTP {code}"))
                    .await?;
            }
            Ok(resp) => {
                self.handle_retryable(rec, format!("HTTP {}", resp.status()))
                    .await?;
            }
            Err(e) => {
                self.handle_retryable(rec, format!("transport: {e}")).await?;
            }
        }
        Ok(())
    }

    async fn handle_retryable(
        &self,
        rec: OutboxRecord,
        reason: String,
    ) -> Result<(), sqlx::Error> {
        // `attempts` was already incremented in `claim_due`, so it equals the
        // number of deliveries attempted so far (the one that just failed is
        // counted). `delay_for(attempts)` returns the delay before the next
        // attempt, or None when `attempts >= max_attempts` — the cap.
        let attempts_used = u32::try_from(rec.attempts).unwrap_or(u32::MAX);
        match self.policy.delay_for(attempts_used) {
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
