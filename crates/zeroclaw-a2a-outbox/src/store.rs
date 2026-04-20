//! Outbox enqueue + claim API.
//!
//! Uses runtime-checked `sqlx::query*` (the `macros` / `query!` family is
//! unavailable under the minimal sqlx feature set — see plan amendments).
//! UUIDs are generated server-side via `gen_random_uuid()` and read back as
//! raw bytes (the sqlx `uuid` feature is also disabled, so `Uuid` is neither
//! `Encode<Postgres>` nor `Decode<Postgres>`).

use serde_json::Value;
use sqlx::{PgPool, Row};
use uuid::Uuid;

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
}
