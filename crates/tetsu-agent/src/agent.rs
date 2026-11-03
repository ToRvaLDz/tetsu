//! The agent: a backend (or none), the tools and the runner, asked one
//! message at a time.
//!
//! With no backend configured the agent only reports it
//! ([`AgentError::NoBackend`]): the rules act alone (#24 item 1). The rules
//! engine is not part of the core.

use std::fmt;

use crate::backend::{Backend, Event, SessionConfig, ToolSpec};
use crate::runner::{LogSink, ToolRunner};
use crate::tools::Tool;
use crate::turn::{self, Context, Limits, TurnOutcome};

/// Why an ask did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// No backend is configured: rules only.
    NoBackend,
    /// The session could not start.
    Backend(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBackend => f.write_str("no backend is configured; only the rules act"),
            Self::Backend(why) => write!(f, "the backend did not start: {why}"),
        }
    }
}

impl std::error::Error for AgentError {}

/// The agent.
pub struct Agent<B, R> {
    backend: Option<B>,
    runner: R,
    tools: Vec<Tool>,
    config: SessionConfig,
    limits: Limits,
    log: LogSink,
    started: bool,
}

impl<B: Backend, R: ToolRunner> Agent<B, R> {
    /// An agent over `tools`, run by `runner`. `backend` is `None` for
    /// rules only.
    pub fn new(
        backend: Option<B>,
        runner: R,
        tools: Vec<Tool>,
        config: SessionConfig,
        log: LogSink,
    ) -> Self {
        Self {
            backend,
            runner,
            tools,
            config,
            limits: Limits::default(),
            log,
            started: false,
        }
    }

    /// The same agent with other turn limits.
    pub fn with_limits(self, limits: Limits) -> Self {
        Self { limits, ..self }
    }

    /// The runner.
    pub fn runner(&self) -> &R {
        &self.runner
    }

    /// Asks one message. The session starts on the first ask, with every
    /// tool.
    pub async fn ask(
        &mut self,
        text: String,
        observe: &mut dyn FnMut(&Event),
    ) -> Result<TurnOutcome, AgentError> {
        let backend = self.backend.as_mut().ok_or(AgentError::NoBackend)?;
        if !self.started {
            let specs: Vec<ToolSpec> = self.tools.iter().map(ToolSpec::from).collect();
            backend
                .start(&self.config, &specs)
                .await
                .map_err(|e| AgentError::Backend(e.message))?;
            self.started = true;
        }
        let ctx = Context {
            runner: &self.runner,
            tools: &self.tools,
            limits: self.limits,
            log: &self.log,
        };
        Ok(turn::run(backend, &ctx, text, observe).await)
    }
}
