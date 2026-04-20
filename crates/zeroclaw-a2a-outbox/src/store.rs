//! Outbox enqueue + claim API.
//!
//! Uses runtime-checked `sqlx::query*` (the `macros` / `query!` family is
//! unavailable under the minimal sqlx feature set — see plan amendments).
//! UUIDs are generated server-side via `gen_random_uuid()` and read back as
//! raw bytes (the sqlx `uuid` feature is also disabled, so `Uuid` is neither
//! `Encode<Postgres>` nor `Decode<Postgres>`).

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Row};
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

    /// Inserts a pending outbox row. Idempotent on `(task_id, sequence)`:
    /// a repeated enqueue returns the original row's id without mutating it.
    pub async fn enqueue(
        &self,
        task_id: &str,
        sequence: i32,
        target_url: &str,
        auth_token: Option<&str>,
        payload: Value,
    ) -> Result<Uuid, sqlx::Error> {
        let row = sqlx::query(
            r#"
            INSERT INTO outbox (id, task_id, sequence, target_url, auth_token, payload_json)
            VALUES (gen_random_uuid(), $1, $2, $3, $4, $5)
            ON CONFLICT (task_id, sequence) DO UPDATE SET task_id = outbox.task_id
            RETURNING id
            "#,
        )
        .bind(task_id)
        .bind(sequence)
        .bind(target_url)
        .bind(auth_token)
        .bind(payload)
        .fetch_one(&self.pool)
        .await?;

        let bytes: Vec<u8> = row.try_get_unchecked("id")?;
        Uuid::from_slice(&bytes).map_err(|e| sqlx::Error::ColumnDecode {
            index: "id".to_string(),
            source: e.into(),
        })
    }

    /// Atomically claim up to `limit` pending rows whose `next_attempt_at` is
    /// in the past. Increments `attempts` and returns the claimed rows. Uses
    /// `FOR UPDATE SKIP LOCKED` so multiple workers can run concurrently.
    pub async fn claim_due(&self, limit: i64) -> Result<Vec<OutboxRecord>, sqlx::Error> {
        sqlx::query_as::<_, OutboxRecord>(
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
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mark_delivered(&self, id: Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE outbox SET status = 'delivered', delivered_at = NOW() WHERE id = $1::uuid",
        )
        .bind(id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_deadletter(&self, id: Uuid, reason: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE outbox SET status = 'deadletter', last_error = $2 WHERE id = $1::uuid",
        )
        .bind(id.to_string())
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn reschedule(
        &self,
        id: Uuid,
        at: DateTime<Utc>,
        reason: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE outbox SET next_attempt_at = $2::timestamptz, last_error = $3 WHERE id = $1::uuid",
        )
        .bind(id.to_string())
        .bind(at.to_rfc3339())
        .bind(reason)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
