//! Hand-rolled migration runner.
//!
//! The sqlx `migrate` feature is disabled in the workspace (see plan
//! amendments: it would activate `sqlx-sqlite` and collide with rusqlite).
//! We apply compile-time-embedded SQL in version order, tracked via a
//! `_schema_migrations` table.
//!
//! Migrations run without a wrapping transaction on purpose — holding a
//! `&mut Transaction` across await points trips sqlx 0.8's `Executor<'_>`
//! HRTB bound, which then propagates into the gateway's spawned-future Send
//! requirement. Each migration's DDL is idempotent (`CREATE TABLE IF NOT
//! EXISTS`, `CREATE EXTENSION IF NOT EXISTS`, etc.), so a crash between the
//! DDL and the `_schema_migrations` insert is recoverable on next startup:
//! the DDL becomes a no-op and the insert completes.

use sqlx::PgPool;

/// Ordered list of (version, sql) pairs. Versions sort lexicographically.
/// Add new migrations here in date-prefixed order.
static MIGRATIONS: &[(&str, &str)] = &[
    (
        "20260419000001_create_outbox",
        include_str!("../migrations/20260419000001_create_outbox.sql"),
    ),
    (
        "20260419000002_create_inbox_events",
        include_str!("../migrations/20260419000002_create_inbox_events.sql"),
    ),
    (
        "20260419000003_create_push_notification_configs",
        include_str!("../migrations/20260419000003_create_push_notification_configs.sql"),
    ),
    (
        "20260420000001_create_a2a_delegations",
        include_str!("../migrations/20260420000001_create_a2a_delegations.sql"),
    ),
    (
        "20260421000001_add_a2a_delegations_channel_sender",
        include_str!("../migrations/20260421000001_add_a2a_delegations_channel_sender.sql"),
    ),
];

pub async fn apply(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )",
    )
    .execute(pool)
    .await?;

    for (version, sql) in MIGRATIONS {
        let already: Option<String> =
            sqlx::query_scalar("SELECT version FROM _schema_migrations WHERE version = $1")
                .bind(version)
                .fetch_optional(pool)
                .await?;
        if already.is_some() {
            continue;
        }

        sqlx::raw_sql(sql).execute(pool).await?;
        sqlx::query("INSERT INTO _schema_migrations (version) VALUES ($1)")
            .bind(version)
            .execute(pool)
            .await?;
    }
    Ok(())
}
