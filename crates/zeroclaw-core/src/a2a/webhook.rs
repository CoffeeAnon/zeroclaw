//! Sam's A2A push-notification receiver.
//!
//! 1. Validates the Bearer token against `push_notification_configs`.
//! 2. Persists the event to `inbox_events` (idempotent on task_id+sequence).
//! 3. Fires a wake signal on the reasoning loop's channel.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;

use super::wake_channel::WakeSender;

#[derive(Clone)]
pub struct WebhookState {
    pool: PgPool,
    wake: WakeSender,
}

impl WebhookState {
    pub fn new(pool: PgPool, wake: WakeSender) -> Self {
        Self { pool, wake }
    }
}

pub fn build_webhook_router(state: WebhookState) -> Router {
    Router::new()
        .route("/webhook/a2a-notify", post(handle_notify))
        .with_state(Arc::new(state))
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "statusUpdate")]
    status_update: Option<TaskRef>,
    task: Option<TaskRef>,
    #[serde(rename = "artifactUpdate")]
    artifact_update: Option<TaskRef>,
}

#[derive(Deserialize)]
struct TaskRef {
    #[serde(rename = "taskId", alias = "id")]
    task_id: Option<String>,
}

async fn handle_notify(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let Some(token) = extract_bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer").into_response();
    };

    let Ok(envelope) = serde_json::from_value::<Envelope>(body.clone()) else {
        return (StatusCode::BAD_REQUEST, "malformed envelope").into_response();
    };
    let task_id = envelope
        .status_update
        .and_then(|r| r.task_id)
        .or_else(|| envelope.task.and_then(|r| r.task_id))
        .or_else(|| envelope.artifact_update.and_then(|r| r.task_id));
    let Some(task_id) = task_id else {
        return (StatusCode::BAD_REQUEST, "no task_id in envelope").into_response();
    };

    let stored: Option<String> =
        match sqlx::query_scalar("SELECT token FROM push_notification_configs WHERE task_id = $1 LIMIT 1")
            .bind(&task_id)
            .fetch_optional(&state.pool)
            .await
        {
            Ok(v) => v.flatten(),
            Err(e) => {
                tracing::error!(error = %e, "push config lookup failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "config lookup failed")
                    .into_response();
            }
        };

    if stored.as_deref() != Some(token.as_str()) {
        return (StatusCode::UNAUTHORIZED, "token mismatch").into_response();
    }

    let insert = sqlx::query(
        r#"
        INSERT INTO inbox_events (id, task_id, sequence, payload_json)
        VALUES (gen_random_uuid(), $1, 0, $2)
        ON CONFLICT (task_id, sequence) DO NOTHING
        "#,
    )
    .bind(&task_id)
    .bind(&body)
    .execute(&state.pool)
    .await;

    if let Err(e) = insert {
        tracing::error!(error = %e, "inbox_events persist failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, "persist failed").into_response();
    }

    state.wake.wake();
    (StatusCode::OK, "").into_response()
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    v.strip_prefix("Bearer ").map(str::to_owned)
}
