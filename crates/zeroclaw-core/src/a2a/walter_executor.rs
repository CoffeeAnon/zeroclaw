//! Walter's `AgentExecutor` — real cluster-health-survey path.
//!
//! Wires the A2A request through an injected [`AgentRunner`] (typically a
//! shim over the main binary's `agent::run()`). The inbound A2A message's
//! text is forwarded verbatim as the user prompt; Walter's skill ConfigMap
//! (`cluster-health-monitor.md`) guides the LLM from there.
//!
//! A `TaskStatusUpdateEvent` with state `Working` is emitted first so any
//! subscriber sees progress immediately; the final `Task` snapshot is sent
//! on completion, failure, or timeout. `ZEROCLAW_A2A_TASK_TIMEOUT_SECS`
//! caps wall time (default 300 s).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ra2a::server::{AgentExecutor, Event, EventQueue, RequestContext};
use ra2a::types::{
    Message, Part, StreamResponse, Task, TaskState, TaskStatus, TaskStatusUpdateEvent,
};
use ra2a::Result;

use super::runner::AgentRunner;

pub struct WalterAgentExecutor {
    runner: Arc<dyn AgentRunner>,
}

impl WalterAgentExecutor {
    pub fn new(runner: Arc<dyn AgentRunner>) -> Self {
        Self { runner }
    }

    fn timeout_secs() -> u64 {
        std::env::var("ZEROCLAW_A2A_TASK_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300)
    }
}

impl AgentExecutor for WalterAgentExecutor {
    fn execute<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // 1. Announce working so subscribers see progress.
            queue.send(StreamResponse::StatusUpdate(TaskStatusUpdateEvent::new(
                &ctx.task_id,
                &ctx.context_id,
                TaskStatus::new(TaskState::Working),
            )))?;

            // 2. Extract the user prompt from the inbound message.
            let prompt = ctx
                .message
                .as_ref()
                .and_then(|m| m.text_content())
                .unwrap_or_default();

            if prompt.trim().is_empty() {
                let mut task = Task::new(&ctx.task_id, &ctx.context_id);
                task.status = TaskStatus::with_message(
                    TaskState::Failed,
                    Message::agent(vec![Part::text(
                        "A2A message arrived with no text parts; nothing to run.",
                    )]),
                );
                queue.send(Event::Task(task))?;
                return Ok(());
            }

            // 3. Run the agent loop with a wall-clock cap.
            let timeout = Duration::from_secs(Self::timeout_secs());
            let result = tokio::time::timeout(timeout, self.runner.run(&prompt)).await;

            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = match result {
                Ok(Ok(report)) => TaskStatus::with_message(
                    TaskState::Completed,
                    Message::agent(vec![Part::text(report)]),
                ),
                Ok(Err(e)) => TaskStatus::with_message(
                    TaskState::Failed,
                    Message::agent(vec![Part::text(format!("agent run failed: {e}"))]),
                ),
                Err(_) => TaskStatus::with_message(
                    TaskState::Canceled,
                    Message::agent(vec![Part::text(format!(
                        "timeout_exceeded after {}s",
                        timeout.as_secs()
                    ))]),
                ),
            };
            queue.send(Event::Task(task))?;
            Ok(())
        })
    }

    fn cancel<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = TaskStatus::new(TaskState::Canceled);
            queue.send(Event::Task(task))?;
            Ok(())
        })
    }
}
