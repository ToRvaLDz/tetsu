//! The boundary to a model: the session interface of #24 (G1), reduced to
//! what the turn loop uses. The core calls no provider and holds no
//! credential: a backend (an API or CLI adapter, #79/#80) implements
//! [`Backend`] and keeps its own secrets behind the secrets proxy.
//!
//! Generic, not `dyn`: `async fn` in a trait, no `Send`, run on a
//! `LocalSet`. `classify` and `resume` join the trait when their callers
//! exist.

pub mod fake;

use std::fmt;

use serde_json::Value;

use crate::tools::Tool;

/// What a backend is, so the loop can adapt (#24 item 10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// The backend runs the tools itself (a CLI agent); the loop only
    /// observes.
    pub executes_tools_itself: bool,
    /// The usage it reports carries the exact cost.
    pub reports_exact_cost: bool,
    /// It asks permission through a callback (`PermissionRequest`).
    pub supports_permission_callback: bool,
    /// It reaches tools through an MCP shim.
    pub needs_mcp: bool,
}

/// How a session starts. No credential field: the provider key is the
/// adapter's business and never passes through the core.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionConfig {
    /// The model, an opaque string (presets are a later decision, #65).
    pub model: String,
    /// The system prompt, when the backend takes one.
    pub system: Option<String>,
}

/// A tool as the model is told about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    /// The wire name.
    pub name: String,
    /// What it does.
    pub description: String,
    /// JSON Schema of the input.
    pub input_schema: Value,
}

impl From<&Tool> for ToolSpec {
    fn from(tool: &Tool) -> Self {
        Self {
            name: tool.wire_name.clone(),
            description: tool.description.clone(),
            input_schema: tool.schema.clone(),
        }
    }
}

/// Why a model stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// It finished its answer.
    EndTurn,
    /// It wants tool results.
    ToolUse,
    /// It ran out of output tokens.
    MaxTokens,
    /// It declined.
    Refusal,
    /// Anything else a provider reports.
    Other,
}

/// The result of one tool call, sent back to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// The id the model gave the call.
    pub call_id: String,
    /// The result as text.
    pub content: String,
    /// Whether the call failed or was refused.
    pub is_error: bool,
}

/// What the loop sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// The user's message.
    User(String),
    /// The results of every call of the last step.
    ToolResults(Vec<ToolResult>),
}

/// What a backend reports, one at a time.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Answer text.
    TextDelta(String),
    /// Reasoning text.
    ThinkingDelta(String),
    /// The model calls a tool; `name` is a wire name.
    ToolCall {
        /// The call id.
        id: String,
        /// The wire name.
        name: String,
        /// The input.
        input: Value,
    },
    /// A tool the backend ran itself finished.
    ToolResult {
        /// The call id.
        id: String,
        /// The result as text.
        content: String,
        /// Whether it failed.
        is_error: bool,
    },
    /// The backend asks permission for something. Observed, never
    /// answered by the loop.
    PermissionRequest {
        /// The request id.
        id: String,
        /// The tool the backend names.
        tool: String,
        /// Its input.
        input: Value,
    },
    /// Tokens and cost so far for this step.
    Usage {
        /// Input tokens.
        input: u64,
        /// Output tokens.
        output: u64,
        /// Tokens read from the cache.
        cache_read: u64,
        /// Tokens written to the cache.
        cache_write: u64,
        /// The cost in USD, when the backend knows.
        cost_usd: Option<f64>,
        /// Whether the numbers are an estimate.
        is_estimate: bool,
    },
    /// The backend failed.
    Error {
        /// What the backend says; never logged by the core.
        message: String,
        /// Whether trying again may work.
        retryable: bool,
    },
    /// The step is over.
    TurnDone {
        /// Why.
        stop_reason: StopReason,
    },
}

/// A failure to start or to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    /// What failed.
    pub message: String,
    /// Whether trying again may work.
    pub retryable: bool,
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BackendError {}

/// A model session.
pub trait Backend {
    /// What this backend is.
    fn flags(&self) -> Flags;

    /// Starts the session with the tools it may call.
    fn start(
        &mut self,
        config: &SessionConfig,
        tools: &[ToolSpec],
    ) -> impl Future<Output = Result<(), BackendError>>;

    /// Sends the next input; events follow through [`Backend::next_event`].
    fn send(&mut self, input: Input) -> impl Future<Output = Result<(), BackendError>>;

    /// The next event of the step, or `None` when the backend closed.
    fn next_event(&mut self) -> impl Future<Output = Option<Event>>;

    /// Cancels the step in progress.
    fn cancel(&mut self) -> impl Future<Output = ()>;
}
