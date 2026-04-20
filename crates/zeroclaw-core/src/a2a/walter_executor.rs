//! Walter's `AgentExecutor` — Phase 1 stub.
//!
//! Acknowledges every incoming request with a "not yet implemented" failure.
//! Real execution logic (cluster-health-survey) lands in Task 2.1.

use std::future::Future;
use std::pin::Pin;

use ra2a::server::{AgentExecutor, Event, EventQueue, RequestContext};
use ra2a::types::{Message, Part, Task, TaskState, TaskStatus};
use ra2a::Result;

pub struct WalterAgentExecutor;

impl AgentExecutor for WalterAgentExecutor {
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
                    "Walter A2A execute is stubbed in Phase 1; real work arrives in Phase 2.",
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
