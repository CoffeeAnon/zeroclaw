//! Hand-rolled migration runner.
//!
//! The sqlx `migrate` feature is disabled in the workspace (see plan
//! amendments: it would activate `sqlx-sqlite` and collide with rusqlite).
//! We apply compile-time-embedded SQL in version order, tracked via a
//! `_schema_migrations` table. Each migration runs in its own transaction.

use sqlx::PgPool;

/// Ordered list of (version, sql) pairs. Versions sort lexicographically.
/// Add new migrations here in date-prefixed order.
static MIGRATIONS: &[(&str, &str)] = &[(
    "20260419000001_create_outbox",
    include_str!("../migrations/20260419000001_create_outbox.sql"),
)];

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

        let mut tx = pool.begin().await?;
        sqlx::raw_sql(sql).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO _schema_migrations (version) VALUES ($1)")
            .bind(version)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    }
    Ok(())
}
