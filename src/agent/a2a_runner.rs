//! Main-binary implementation of [`zeroclaw_core::a2a::runner::AgentRunner`].
//!
//! Wraps `crate::agent::loop_::run()` so Walter's A2A executor — which lives
//! in `zeroclaw-core` and can't depend on `Config`/`Provider`/tool registry —
//! can invoke a full reasoning turn via the shared trait.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use zeroclaw_core::a2a::runner::AgentRunner;

use crate::config::Config;

pub struct AgentRunnerImpl {
    config: Arc<Config>,
}

impl AgentRunnerImpl {
    pub fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

impl AgentRunner for AgentRunnerImpl {
    fn run<'a>(
        &'a self,
        message: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        let config = (*self.config).clone();
        let temperature = config.default_temperature;
        let message = message.to_owned();
        let session_id = Some(format!("a2a-{}", uuid::Uuid::new_v4()));
        Box::pin(async move {
            crate::agent::loop_::run(
                config,
                Some(message),
                None,               // provider_override
                None,               // model_override
                temperature,
                vec![],             // peripheral_overrides
                false,              // interactive
                None,               // hooks
                session_id,
                None,               // cancellation_token (A2A timeout
                                    // is enforced by the executor via
                                    // tokio::time::timeout)
            )
            .await
        })
    }
}
