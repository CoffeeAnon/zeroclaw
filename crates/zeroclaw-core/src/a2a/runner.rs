//! Abstraction over the agent's LLM reasoning loop.
//!
//! `WalterAgentExecutor` lives in `zeroclaw-core` but its real work runs
//! through `agent::run()` in the main binary — which depends on `Config`,
//! `Provider`, tool registry, memory, and skills that `zeroclaw-core`
//! can't hold without pulling the whole binary back in as a dep.
//!
//! Dependency-inject the runner instead: define a minimal trait here,
//! implement it in the main binary, pass it into the executor at startup.

use std::future::Future;
use std::pin::Pin;

/// Runs a single agent turn with the given user message and returns
/// the assistant's final text response.
///
/// Implementations typically wrap `agent::run()` (or similar) and are
/// owned by `Arc<dyn AgentRunner>` so a single instance is shared
/// across all A2A requests on the pod.
pub trait AgentRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        message: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<String>> + Send + 'a>>;
}
