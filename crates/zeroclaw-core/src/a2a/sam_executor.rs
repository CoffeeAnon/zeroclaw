//! Sam's `AgentExecutor` — MVP shim.
//!
//! Sam does not accept inbound A2A delegations in the MVP (Signal + ACP are
//! her inbound channels). The executor is still registered for protocol
//! symmetry so agent-card resolution and ra2a's handler pipeline work; any
//! `message/send` that reaches Sam gets a structured Failed reply.

use std::future::Future;
use std::pin::Pin;

use ra2a::server::{AgentExecutor, Event, EventQueue, RequestContext};
use ra2a::types::{Message, Part, Task, TaskState, TaskStatus};
use ra2a::Result;

pub struct SamAgentExecutor;

impl AgentExecutor for SamAgentExecutor {
    fn execute<'a>(
        &'a self,
        ctx: &'a RequestContext,
        queue: &'a EventQueue,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let mut task = Task::new(&ctx.task_id, &ctx.context_id);
            task.status = TaskStatus::with_message(
                TaskState::Failed,
                Message::agent(vec![Part::text(
                    "Sam does not accept inbound A2A delegations in MVP. Use Signal or ACP.",
                )]),
            );
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
