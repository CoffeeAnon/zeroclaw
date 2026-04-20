//! Outbox record type. sqlx's `uuid`/`chrono` features are disabled in the
//! workspace to avoid a transitive conflict with rusqlite (see plan
//! amendments). We therefore implement [`sqlx::FromRow`] manually, using
//! low-level decode primitives that do not require those facade features.
//!
//! ## Decode strategy
//!
//! - **UUID**: `row.try_get_unchecked::<Vec<u8>, _>()` reads the 16-byte
//!   binary wire representation, then `uuid::Uuid::from_slice` constructs the
//!   value. The type compatibility check is bypassed via `try_get_unchecked`
//!   because the `uuid` sqlx feature (which would register `Uuid` as
//!   `Type<Postgres>`) is disabled.
//!
//! - **TIMESTAMPTZ**: similarly decoded as `i64` microseconds-since-2000-01-01
//!   via `try_get_unchecked`, then converted to `DateTime<Utc>` manually.
//!
//! - **JSONB**: `serde_json::Value` works without any feature flag because
//!   sqlx-postgres always compiles its JSON codec (required for driver
//!   internals); `try_get` (with type check) is safe here.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use sqlx::{postgres::PgRow, FromRow, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
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

impl<'r> FromRow<'r, PgRow> for OutboxRecord {
    fn from_row(row: &'r PgRow) -> sqlx::Result<Self> {
        Ok(Self {
            id: decode_uuid(row, "id")?,
            task_id: row.try_get("task_id")?,
            sequence: row.try_get("sequence")?,
            target_url: row.try_get("target_url")?,
            auth_token: row.try_get("auth_token")?,
            payload_json: row.try_get("payload_json")?,
            attempts: row.try_get("attempts")?,
            next_attempt_at: decode_timestamptz(row, "next_attempt_at")?,
            status: row.try_get("status")?,
            created_at: decode_timestamptz(row, "created_at")?,
            delivered_at: decode_timestamptz_opt(row, "delivered_at")?,
            last_error: row.try_get("last_error")?,
        })
    }
}

/// Decode a Postgres UUID column without the sqlx `uuid` facade feature.
///
/// UUID is transmitted as 16 raw bytes over the Postgres binary protocol.
/// We use `try_get_unchecked` to bypass the type-compatibility check (which
/// would reject `Vec<u8>` for a UUID column), then construct `Uuid` from the
/// byte slice.
fn decode_uuid(row: &PgRow, col: &str) -> sqlx::Result<Uuid> {
    let bytes: Vec<u8> = row.try_get_unchecked(col)?;
    Uuid::from_slice(&bytes).map_err(|e| sqlx::Error::ColumnDecode {
        index: col.to_string(),
        source: e.into(),
    })
}

/// Decode a Postgres TIMESTAMPTZ column as `DateTime<Utc>` without the sqlx
/// `chrono` facade feature.
///
/// TIMESTAMPTZ is transmitted as `i64` microseconds since the Postgres epoch
/// (2000-01-01 UTC) over the binary protocol. We use `try_get_unchecked` to
/// bypass the type-compatibility check, then reconstruct the `DateTime<Utc>`
/// manually.
fn decode_timestamptz(row: &PgRow, col: &str) -> sqlx::Result<DateTime<Utc>> {
    // Postgres binary wire format: i64 µs from 2000-01-01T00:00:00 UTC.
    let micros: i64 = row.try_get_unchecked(col)?;
    pg_micros_to_datetime(micros, col)
}

fn decode_timestamptz_opt(row: &PgRow, col: &str) -> sqlx::Result<Option<DateTime<Utc>>> {
    let raw: Option<i64> = row.try_get_unchecked(col)?;
    raw.map(|micros| pg_micros_to_datetime(micros, col)).transpose()
}

/// Convert Postgres microseconds-since-2000-01-01 to `DateTime<Utc>`.
fn pg_micros_to_datetime(micros: i64, col: &str) -> sqlx::Result<DateTime<Utc>> {
    // Delta between Unix epoch (1970-01-01) and Postgres epoch (2000-01-01) in µs.
    const EPOCH_DELTA_MICROS: i64 = 946_684_800_000_000;
    let unix_micros = micros + EPOCH_DELTA_MICROS;
    let secs = unix_micros / 1_000_000;
    let nanos = ((unix_micros % 1_000_000).unsigned_abs() * 1_000) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .ok_or_else(|| sqlx::Error::ColumnDecode {
            index: col.to_string(),
            source: "invalid timestamptz value".into(),
        })
}
