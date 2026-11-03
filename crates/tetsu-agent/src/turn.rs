//! The turn loop: one user message, as many model steps as it takes.
//!
//! Pure over two traits. Each step the [`Backend`] reports events; the
//! tool calls among them are mapped back to their tool by wire name, run
//! by the [`ToolRunner`], and their results go back to the model, until a
//! step ends with no tool call or the step limit is reached.
//!
//! A denied ask is a normal outcome: the model gets an error result that
//! says the call was refused, the tool is recorded in
//! [`TurnOutcome::denied`], and a second call of the same tool in the turn
//! is answered the same way without touching the bus, so the model cannot
//! ask again in a loop.
//!
//! The log carries no model text, input or result.

use std::collections::BTreeSet;

use serde_json::json;

use crate::backend::{Backend, Event, Input, StopReason, ToolResult};
use crate::runner::{LogSink, Outcome, ToolRunner};
use crate::tools::Tool;

/// Backend steps a turn may take unless told otherwise.
pub const DEFAULT_MAX_STEPS: usize = 20;

/// Bounds of a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most sends to the backend in one turn, the user's message
    /// included.
    pub max_steps: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_steps: DEFAULT_MAX_STEPS,
        }
    }
}

/// What a turn needs besides the backend.
pub struct Context<'a, R> {
    /// Runs the tools.
    pub runner: &'a R,
    /// The tools the model may call.
    pub tools: &'a [Tool],
    /// The bounds.
    pub limits: Limits,
    /// Where log lines go.
    pub log: &'a LogSink,
}

/// Why a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The model finished, or a backend that runs tools itself did.
    Done(StopReason),
    /// The step limit was reached with calls still wanted.
    StepLimit,
    /// The backend failed.
    BackendError {
        /// Whether trying again may work.
        retryable: bool,
    },
    /// The backend closed without ending the step.
    Closed,
}

/// One tool call of the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRun {
    /// The tool id (never a name the model made up).
    pub tool: String,
    /// The kind of the outcome.
    pub outcome: &'static str,
}

/// Tokens and cost summed over the turn.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// Input tokens.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Tokens read from the cache.
    pub cache_read: u64,
    /// Tokens written to the cache.
    pub cache_write: u64,
    /// The known cost in USD.
    pub cost_usd: f64,
    /// Whether any part is an estimate.
    pub is_estimate: bool,
}

/// What a turn came to.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnOutcome {
    /// The answer text of every step, joined.
    pub text: String,
    /// Why it ended.
    pub stop: Stop,
    /// The tool ids whose call was refused, once each.
    pub denied: Vec<String>,
    /// Every tool call, in order.
    pub runs: Vec<ToolRun>,
    /// The usage.
    pub usage: Usage,
}

/// How one step ended.
enum Step {
    Done(StopReason),
    Failed { retryable: bool },
    Closed,
}

struct Call {
    id: String,
    name: String,
    input: serde_json::Value,
}

/// Runs one turn for `user`. `observe` sees every event of the backend.
pub async fn run<B: Backend, R: ToolRunner>(
    backend: &mut B,
    ctx: &Context<'_, R>,
    user: String,
    observe: &mut dyn FnMut(&Event),
) -> TurnOutcome {
    let own_tools = !backend.flags().executes_tools_itself;
    let mut outcome = TurnOutcome {
        text: String::new(),
        stop: Stop::Closed,
        denied: Vec::new(),
        runs: Vec::new(),
        usage: Usage::default(),
    };
    let mut refused: BTreeSet<String> = BTreeSet::new();
    let mut input = Input::User(user);
    let mut sends = 0usize;
    loop {
        if let Err(e) = backend.send(input).await {
            outcome.stop = Stop::BackendError {
                retryable: e.retryable,
            };
            return outcome;
        }
        sends += 1;
        let mut calls = Vec::new();
        match read_step(backend, observe, &mut outcome, &mut calls, own_tools).await {
            Step::Failed { retryable } => {
                outcome.stop = Stop::BackendError { retryable };
                return outcome;
            }
            Step::Closed => {
                outcome.stop = Stop::Closed;
                return outcome;
            }
            Step::Done(reason) => {
                if calls.is_empty() {
                    outcome.stop = Stop::Done(reason);
                    return outcome;
                }
            }
        }
        if sends >= ctx.limits.max_steps {
            outcome.stop = Stop::StepLimit;
            return outcome;
        }
        let mut results = Vec::new();
        for call in calls {
            results.push(run_call(ctx, call, &mut refused, &mut outcome).await);
        }
        input = Input::ToolResults(results);
    }
}

/// Reads events until the step ends, collecting text, usage and calls.
async fn read_step<B: Backend>(
    backend: &mut B,
    observe: &mut dyn FnMut(&Event),
    outcome: &mut TurnOutcome,
    calls: &mut Vec<Call>,
    own_tools: bool,
) -> Step {
    while let Some(event) = backend.next_event().await {
        observe(&event);
        match event {
            Event::TextDelta(text) => outcome.text.push_str(&text),
            Event::ToolCall { id, name, input } if own_tools => {
                calls.push(Call { id, name, input });
            }
            Event::Usage {
                input,
                output,
                cache_read,
                cache_write,
                cost_usd,
                is_estimate,
            } => {
                let usage = &mut outcome.usage;
                usage.input += input;
                usage.output += output;
                usage.cache_read += cache_read;
                usage.cache_write += cache_write;
                usage.cost_usd += cost_usd.unwrap_or(0.0);
                usage.is_estimate |= is_estimate;
            }
            Event::Error { retryable, .. } => return Step::Failed { retryable },
            Event::TurnDone { stop_reason } => return Step::Done(stop_reason),
            // Observed only: thinking, tools the backend ran, permission asks.
            Event::ThinkingDelta(_)
            | Event::ToolCall { .. }
            | Event::ToolResult { .. }
            | Event::PermissionRequest { .. } => {}
        }
    }
    Step::Closed
}

fn refusal_text(tool_id: &str) -> String {
    format!(
        "{tool_id} was refused by the user or the capability policy; do not retry it in this turn"
    )
}

fn failure(call_id: String, content: String) -> ToolResult {
    ToolResult {
        call_id,
        content,
        is_error: true,
    }
}

async fn run_call<R: ToolRunner>(
    ctx: &Context<'_, R>,
    call: Call,
    refused: &mut BTreeSet<String>,
    outcome: &mut TurnOutcome,
) -> ToolResult {
    let Some(tool) = ctx.tools.iter().find(|t| t.wire_name == call.name) else {
        // The name is model text: only its length is logged.
        (ctx.log)(&format!(
            "tetsu-agent: call of an unknown tool ({} bytes)",
            call.name.len()
        ));
        return failure(call.id, "no such tool".to_owned());
    };
    if refused.contains(&tool.id) {
        record(outcome, tool, "denied");
        return failure(call.id, refusal_text(&tool.id));
    }
    let result = ctx.runner.run(tool, call.input).await;
    record(outcome, tool, result.kind());
    match result {
        Outcome::Ok(value) => ToolResult {
            call_id: call.id,
            content: value.to_string(),
            is_error: false,
        },
        Outcome::Denied => {
            refused.insert(tool.id.clone());
            outcome.denied.push(tool.id.clone());
            failure(call.id, refusal_text(&tool.id))
        }
        Outcome::Error { error, parameters } => failure(
            call.id,
            json!({ "error": error, "parameters": parameters }).to_string(),
        ),
        Outcome::Unavailable(reason) => {
            failure(call.id, format!("the call could not be made: {reason}"))
        }
    }
}

fn record(outcome: &mut TurnOutcome, tool: &Tool, kind: &'static str) {
    outcome.runs.push(ToolRun {
        tool: tool.id.clone(),
        outcome: kind,
    });
}
