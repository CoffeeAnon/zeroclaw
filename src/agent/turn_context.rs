//! Per-turn context exposed to tool implementations via a task-local.
//!
//! The `Tool::execute(args)` trait takes only JSON args, by design — tools
//! shouldn't depend on ambient context in general. But a handful of
//! capability-bridging tools (e.g. the A2A delegation tool) need to know
//! which agent session they're running inside so they can persist
//! correlation metadata for eventual async replies. Rather than widen the
//! `Tool` trait for every implementation, we surface a narrow task-local
//! that tools may opt into reading.
//!
//! `agent::loop_::run()` wraps its body in `TURN_CONTEXT.scope(...)` so
//! anything it (transitively) awaits — including `tool.execute(...)` —
//! sees the current turn's context.

use std::borrow::Cow;

tokio::task_local! {
    pub(crate) static TURN_CONTEXT: TurnContext;
}

#[derive(Clone, Default)]
pub struct TurnContext {
    pub session_id: Option<String>,
    /// Channel the current turn was initiated on (e.g. "signal"). Used
    /// by the A2A delegation tool to correlate async replies back to
    /// the right conversation. `None` when the turn wasn't started by
    /// a channel (CLI, tests, A2A inbox drain, etc.).
    pub channel: Option<String>,
    /// Sender id (phone number, group id, user id, …) for the channel
    /// above. Interpreted by the channel — the A2A layer just stores
    /// and forwards it verbatim.
    pub sender: Option<String>,
}

impl TurnContext {
    pub fn new(session_id: Option<String>) -> Self {
        Self {
            session_id,
            channel: None,
            sender: None,
        }
    }

    pub fn with_channel(mut self, channel: Option<String>, sender: Option<String>) -> Self {
        self.channel = channel;
        self.sender = sender;
        self
    }
}

/// Returns the current turn's session id if the caller is running inside
/// an `agent::loop_::run()` scope. Returns `None` when called from a
/// context that didn't set one (tests, direct tool invocation, etc.).
pub fn current_session_id() -> Option<String> {
    TURN_CONTEXT
        .try_with(|ctx| ctx.session_id.clone())
        .ok()
        .flatten()
}

/// Returns the current turn's `(channel, sender)` pair, if the turn was
/// initiated by a channel message and the dispatcher threaded them
/// through. `(None, None)` for non-channel callers.
pub fn current_channel_binding() -> (Option<String>, Option<String>) {
    TURN_CONTEXT
        .try_with(|ctx| (ctx.channel.clone(), ctx.sender.clone()))
        .unwrap_or((None, None))
}

/// Run `fut` inside a turn-context scope. Convenience wrapper so callers
/// don't have to import the task-local name directly.
pub async fn with_turn<F, R>(session_id: Option<String>, fut: F) -> R
where
    F: std::future::Future<Output = R>,
{
    TURN_CONTEXT.scope(TurnContext::new(session_id), fut).await
}

/// `with_turn` variant that also threads the originating channel + sender
/// through the task-local. Used by the channel dispatcher so async
/// tools (like `ask_walter`) can remember where to reply.
pub async fn with_turn_full<F, R>(
    session_id: Option<String>,
    channel: Option<String>,
    sender: Option<String>,
    fut: F,
) -> R
where
    F: std::future::Future<Output = R>,
{
    let ctx = TurnContext::new(session_id).with_channel(channel, sender);
    TURN_CONTEXT.scope(ctx, fut).await
}

/// Lazy debug-display of the current session id (or `"<no-session>"`).
/// Cheap to construct for use inside `tracing` fields.
#[allow(dead_code)]
pub fn current_session_id_display() -> Cow<'static, str> {
    match current_session_id() {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed("<no-session>"),
    }
}
