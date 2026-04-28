//! Persistent log of messages cron jobs delivered via channels.
//!
//! Cron-announce delivery (`scheduler::deliver_announcement`) pushes the
//! agent's final response straight to the configured channel, bypassing
//! the agent's `history` Vec and the per-sender conversation cache. When
//! the user later replies to that message, the interactive session has
//! no record of having spoken first. This log captures cron-side
//! outbound messages so the channel inbound path can surface them as
//! context on the next turn — see `channels::build_cron_outbound_context`.
//!
//! Schema lives alongside `cron_jobs` / `cron_runs` in `cron/jobs.db`.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct OutboundEntry {
    pub job_id: String,
    pub job_name: Option<String>,
    pub channel: String,
    pub recipient: String,
    pub body: String,
    pub sent_at: DateTime<Utc>,
}

fn open(workspace_dir: &Path) -> Result<Connection> {
    let db_path = workspace_dir.join("cron").join("jobs.db");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cron directory: {}", parent.display()))?;
    }
    let conn = Connection::open(&db_path)
        .with_context(|| format!("Failed to open cron DB: {}", db_path.display()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS cron_outbound (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id      TEXT NOT NULL,
            job_name    TEXT,
            channel     TEXT NOT NULL,
            recipient   TEXT NOT NULL,
            body        TEXT NOT NULL,
            sent_at     TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_cron_outbound_recipient
            ON cron_outbound(channel, recipient, sent_at);",
    )
    .context("Failed to initialize cron_outbound schema")?;
    Ok(conn)
}

pub fn record(
    workspace_dir: &Path,
    job_id: &str,
    job_name: Option<&str>,
    channel: &str,
    recipient: &str,
    body: &str,
) -> Result<()> {
    let conn = open(workspace_dir)?;
    conn.execute(
        "INSERT INTO cron_outbound (job_id, job_name, channel, recipient, body, sent_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            job_id,
            job_name,
            channel,
            recipient,
            body,
            Utc::now().to_rfc3339()
        ],
    )
    .context("Failed to insert cron_outbound row")?;
    Ok(())
}

pub fn recent_for_recipient(
    workspace_dir: &Path,
    channel: &str,
    recipient: &str,
    since: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<OutboundEntry>> {
    let conn = open(workspace_dir)?;
    let mut stmt = conn.prepare(
        "SELECT job_id, job_name, channel, recipient, body, sent_at
         FROM cron_outbound
         WHERE channel = ?1 AND recipient = ?2 AND sent_at >= ?3
         ORDER BY sent_at ASC
         LIMIT ?4",
    )?;
    let rows = stmt
        .query_map(
            params![channel, recipient, since.to_rfc3339(), limit as i64],
            |row| {
                let sent_at_str: String = row.get(5)?;
                let sent_at = DateTime::parse_from_rfc3339(&sent_at_str)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc);
                Ok(OutboundEntry {
                    job_id: row.get(0)?,
                    job_name: row.get(1)?,
                    channel: row.get(2)?,
                    recipient: row.get(3)?,
                    body: row.get(4)?,
                    sent_at,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn record_and_recent_round_trip() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        record(ws, "job-a", Some("Daily Curator"), "signal", "uuid-1", "first body").unwrap();
        record(ws, "job-b", None, "signal", "uuid-1", "second body").unwrap();
        record(ws, "job-c", None, "signal", "uuid-other", "different recipient").unwrap();
        record(ws, "job-d", None, "telegram", "uuid-1", "different channel").unwrap();

        let since = Utc::now() - chrono::Duration::hours(1);
        let rows = recent_for_recipient(ws, "signal", "uuid-1", since, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].body, "first body");
        assert_eq!(rows[0].job_name.as_deref(), Some("Daily Curator"));
        assert_eq!(rows[1].body, "second body");
        assert!(rows[1].job_name.is_none());
    }

    #[test]
    fn recent_filters_by_since_window() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();

        record(ws, "job-a", None, "signal", "uuid-1", "now").unwrap();

        let future = Utc::now() + chrono::Duration::hours(1);
        let rows = recent_for_recipient(ws, "signal", "uuid-1", future, 10).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn recent_respects_limit() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        for i in 0..5 {
            record(ws, &format!("job-{i}"), None, "signal", "uuid-1", "x").unwrap();
        }
        let since = Utc::now() - chrono::Duration::hours(1);
        let rows = recent_for_recipient(ws, "signal", "uuid-1", since, 3).unwrap();
        assert_eq!(rows.len(), 3);
    }
}
