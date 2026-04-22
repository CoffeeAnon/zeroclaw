//! A2A inbox drain — Sam-only supervised component.
//!
//! Polls `inbox_events WHERE processed_at IS NULL` every
//! `ZEROCLAW_A2A_INBOX_POLL_SECS` (default 5 s), and for each unprocessed
//! row dispatches the payload into the agent reasoning loop exactly the
//! same way cron jobs and heartbeat tasks do — synthetic user prompt with
//! an `[A2A inbound …]` prefix and an isolated session id.
//!
//! On success the row is marked `processed_at = NOW()`. On dispatch error
//! we log and leave the row unprocessed so the next tick retries.
//!
//! Only started when `ZEROCLAW_A2A_AGENT_ROLE=sam` — Walter writes to
//! outbox but has no inbox_events traffic.
//!
//! The `WakeSignal` channel plumbed in Phase 1 is *not* consumed yet.
//! Polling at 5 s is ~6000× faster than the 30-minute Vikunja poll this
//! whole layer replaces, so the latency gain from wake-signal `select!`
//! is negligible for MVP. Worth revisiting only if someone sees stale
//! inbox rows lingering under load.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::config::Config;

pub async fn run(config: Config) -> Result<()> {
    let role = std::env::var("ZEROCLAW_A2A_AGENT_ROLE").unwrap_or_default();
    if role != "sam" {
        tracing::info!(
            role = %role,
            "a2a-inbox-drain: role is not `sam`, idling forever"
        );
        std::future::pending::<()>().await;
        return Ok(());
    }

    let Ok(db_url) = std::env::var("ZEROCLAW_A2A_DB_URL") else {
        tracing::info!("ZEROCLAW_A2A_DB_URL unset; a2a-inbox-drain idle");
        std::future::pending::<()>().await;
        return Ok(());
    };
    let schema = std::env::var("ZEROCLAW_A2A_SCHEMA").unwrap_or_else(|_| "public".to_string());

    let pool = build_pool(&db_url, &schema)
        .await
        .context("failed to connect a2a-inbox-drain pool")?;

    let poll_secs: u64 = std::env::var("ZEROCLAW_A2A_INBOX_POLL_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let mut tick = tokio::time::interval(Duration::from_secs(poll_secs));
    tracing::info!(poll_secs, schema = %schema, "a2a-inbox-drain: running");

    loop {
        tick.tick().await;
        if let Err(e) = drain_once(&config, &pool).await {
            tracing::warn!("a2a-inbox-drain iteration failed: {e:#}");
        }
    }
}

async fn drain_once(config: &Config, pool: &PgPool) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, task_id, payload_json
         FROM inbox_events
         WHERE processed_at IS NULL
         ORDER BY received_at
         LIMIT 16",
    )
    .fetch_all(pool)
    .await
    .context("select unprocessed inbox_events")?;

    for row in rows {
        let row_id_bytes: Vec<u8> = row.try_get_unchecked("id")?;
        let row_id = uuid::Uuid::from_slice(&row_id_bytes)
            .map_err(|e| anyhow::anyhow!("inbox row id not a valid UUID: {e}"))?;
        let task_id: String = row.try_get("task_id")?;
        let payload: serde_json::Value = row.try_get("payload_json")?;

        dispatch_one(config, pool, row_id, &task_id, &payload).await;
    }
    Ok(())
}

#[derive(Default)]
struct DelegationBinding {
    session_id: Option<String>,
    channel: Option<String>,
    sender: Option<String>,
}

async fn dispatch_one(
    config: &Config,
    pool: &PgPool,
    row_id: uuid::Uuid,
    task_id: &str,
    payload: &serde_json::Value,
) {
    // Resume the original delegation's session id so Sam's memory for
    // this conversation is in scope during the reply turn. Fall back to
    // a fresh per-row session if no correlation exists (inbound event
    // didn't originate from our ask_walter tool).
    let binding = lookup_delegation(pool, task_id).await;

    let pretty = serde_json::to_string_pretty(payload).unwrap_or_else(|_| payload.to_string());
    let prelude = if binding.session_id.is_some() {
        format!(
            "[A2A reply to your earlier delegation — task {task_id}]\n\
             Review your recent conversation for the original request. Walter's \
             response payload follows:\n\n{pretty}\n\n\
             Summarize Walter's answer in your own words and address it to the \
             original asker. Your reply text will be delivered back to them \
             automatically — do NOT attempt to call a channel-send tool yourself."
        )
    } else {
        format!("[A2A inbound from task {task_id}]\n{pretty}")
    };
    let session_id = binding
        .session_id
        .clone()
        .or_else(|| Some(format!("a2a_inbox_{row_id}")));

    let run_result = crate::agent::loop_::run(
        config.clone(),
        Some(prelude),
        None, // provider_override
        None, // model_override
        config.default_temperature,
        vec![], // peripheral_overrides
        false,  // interactive
        None,   // hooks
        session_id,
        None, // cancellation_token (inbox dispatch is not user-cancelable)
    )
    .await;

    match run_result {
        Ok(output) => {
            tracing::info!(%task_id, %row_id, output_len = output.len(), "a2a inbox event processed");
            deliver_to_channel(task_id, row_id, &binding, &output).await;
            if let Err(e) = mark_processed(pool, row_id).await {
                // Loud error: we just ran the agent turn successfully but can't
                // persist "done", so the next poll will re-dispatch. This risks
                // double-processing. Escalate.
                tracing::error!(
                    %task_id, %row_id,
                    "a2a inbox event ran successfully but mark_processed failed: {e:#} — next tick will re-dispatch this row"
                );
            }
        }
        Err(e) => {
            tracing::warn!(%task_id, %row_id, "a2a inbox dispatch failed: {e:#}");
            // Leave processed_at NULL; next tick retries.
        }
    }
}

async fn lookup_delegation(pool: &PgPool, task_id: &str) -> DelegationBinding {
    let row = sqlx::query(
        "SELECT session_id, channel, sender FROM a2a_delegations WHERE task_id = $1",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await;

    match row {
        Ok(Some(r)) => DelegationBinding {
            session_id: r
                .try_get::<Option<String>, _>("session_id")
                .ok()
                .flatten(),
            channel: r.try_get::<Option<String>, _>("channel").ok().flatten(),
            sender: r.try_get::<Option<String>, _>("sender").ok().flatten(),
        },
        Ok(None) => DelegationBinding::default(),
        Err(e) => {
            tracing::warn!(%task_id, "a2a_delegations lookup failed: {e:#}");
            DelegationBinding::default()
        }
    }
}

async fn deliver_to_channel(
    task_id: &str,
    row_id: uuid::Uuid,
    binding: &DelegationBinding,
    output: &str,
) {
    let (Some(channel_name), Some(recipient)) = (binding.channel.as_deref(), binding.sender.as_deref())
    else {
        return;
    };

    let trimmed = output.trim();
    if trimmed.is_empty() {
        tracing::warn!(%task_id, %row_id, channel = %channel_name,
            "a2a reply had empty agent output; nothing to send to channel");
        return;
    }

    let Some(channel) = crate::channels::get_live_channel(channel_name) else {
        tracing::warn!(%task_id, %row_id, channel = %channel_name,
            "no live channel registered; dropping reply delivery");
        return;
    };

    let message = crate::channels::traits::SendMessage::new(trimmed, recipient);
    match channel.send(&message).await {
        Ok(()) => tracing::info!(%task_id, %row_id, channel = %channel_name, recipient = %recipient,
            "a2a reply delivered to channel"),
        Err(e) => tracing::error!(%task_id, %row_id, channel = %channel_name, recipient = %recipient,
            "a2a reply delivery failed: {e:#}"),
    }
}

async fn mark_processed(pool: &PgPool, row_id: uuid::Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE inbox_events SET processed_at = NOW() WHERE id = $1::uuid")
        .bind(row_id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}


async fn build_pool(dsn: &str, schema: &str) -> Result<PgPool> {
    // Same pattern as gateway::a2a::build_pool — libpq `options` query
    // parameter bakes in the per-connection search_path so this crate
    // doesn't need its own `after_connect` closure (which trips sqlx's
    // Executor HRTB — see plan amendment E).
    let encoded = format!("-c search_path={schema},public")
        .replace(' ', "%20")
        .replace('=', "%3D")
        .replace(',', "%2C");
    let separator = if dsn.contains('?') { '&' } else { '?' };
    let dsn = format!("{dsn}{separator}options={encoded}");

    PgPoolOptions::new()
        .max_connections(4)
        .connect(&dsn)
        .await
        .context("connect a2a-inbox-drain pool")
}
