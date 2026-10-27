//! The command line of `tetsu-agent`: argument parsing and the text it
//! prints, both pure. The binary only wires them to discovery and the
//! runner.
//!
//! `call` is a model-free path to one tool, through the same gate: it is
//! no wider than `varlinkctl` and serves the human checks.

use std::fmt;

use serde_json::{Value, json};
use tetsu_bus::access::MethodKind;

use crate::discovery::Catalog;
use crate::runner::Outcome;
use crate::tools::REASON;

/// The exit code of a usage error (`EX_USAGE`).
pub const USAGE_EXIT: u8 = 64;

/// The usage text.
pub const USAGE: &str =
    "usage: tetsu-agent [tools [--json] | call <tool-id> [<json>] [--reason <text>]]

  tools   list the tools found on the bus
  call    run one tool through the capability gate, without a model";

/// What was asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// No subcommand: no backend is configured, the rules act alone.
    Idle,
    /// List the tools.
    Tools {
        /// Print JSON.
        json: bool,
    },
    /// Run one tool. A reason is already part of `input` as `_reason`.
    Call {
        /// The tool id.
        tool: String,
        /// The input.
        input: Value,
    },
}

/// A usage error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError(pub String);

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

fn usage(why: impl Into<String>) -> CliError {
    CliError(why.into())
}

/// Parses the arguments after the program name.
pub fn parse(args: &[String]) -> Result<Command, CliError> {
    let Some((command, rest)) = args.split_first() else {
        return Ok(Command::Idle);
    };
    match command.as_str() {
        "tools" => parse_tools(rest),
        "call" => parse_call(rest),
        other => Err(usage(format!("unknown command {other}"))),
    }
}

fn parse_tools(rest: &[String]) -> Result<Command, CliError> {
    match rest {
        [] => Ok(Command::Tools { json: false }),
        [flag] if flag == "--json" => Ok(Command::Tools { json: true }),
        [other, ..] => Err(usage(format!("unexpected {other}"))),
    }
}

fn parse_call(rest: &[String]) -> Result<Command, CliError> {
    let mut positional = Vec::new();
    let mut reason = None;
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        if arg == "--reason" {
            let text = rest.next().ok_or_else(|| usage("--reason needs a text"))?;
            reason = Some(text.clone());
        } else if arg.starts_with("--") {
            return Err(usage(format!("unknown flag {arg}")));
        } else {
            positional.push(arg);
        }
    }
    let (tool, json_text) = match positional.as_slice() {
        [tool] => (*tool, None),
        [tool, json_text] => (*tool, Some(*json_text)),
        [] => return Err(usage("call needs a tool id")),
        [_, _, extra, ..] => return Err(usage(format!("unexpected {extra}"))),
    };
    let mut input = match json_text {
        None => json!({}),
        Some(text) => {
            serde_json::from_str(text).map_err(|e| usage(format!("the input is not JSON: {e}")))?
        }
    };
    if let Some(reason) = reason {
        let object = input
            .as_object_mut()
            .ok_or_else(|| usage("--reason needs the input to be a JSON object"))?;
        object.insert(REASON.to_owned(), Value::String(reason));
    }
    Ok(Command::Call {
        tool: tool.clone(),
        input,
    })
}

fn kind_name(kind: MethodKind) -> &'static str {
    match kind {
        MethodKind::Free => "free",
        MethodKind::Gated => "gated",
    }
}

/// The tools found and what was left out, as text or JSON.
pub fn render_tools(catalog: &Catalog, as_json: bool) -> String {
    if as_json {
        let tools: Vec<Value> = catalog
            .tools
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "wire_name": t.wire_name,
                    "kind": kind_name(t.kind),
                    "class": t.class,
                    "destructive": t.destructive,
                })
            })
            .collect();
        let skipped: Vec<Value> = catalog
            .skipped
            .iter()
            .map(|s| json!({ "subject": s.subject, "reason": s.reason }))
            .collect();
        return json!({ "tools": tools, "skipped": skipped }).to_string();
    }
    let mut lines: Vec<String> = catalog
        .tools
        .iter()
        .map(|t| {
            format!(
                "{}\t{}\t{}\t{}",
                t.id,
                kind_name(t.kind),
                t.class.as_deref().unwrap_or("-"),
                if t.destructive { "destructive" } else { "-" }
            )
        })
        .collect();
    lines.extend(
        catalog
            .skipped
            .iter()
            .map(|s| format!("skipped: {}: {}", s.subject, s.reason)),
    );
    lines.join("\n")
}

/// An outcome as one line.
pub fn render_outcome(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Ok(value) => format!("ok {value}"),
        Outcome::Denied => "denied".to_owned(),
        Outcome::Error { error, parameters } => format!("error {error} {parameters}"),
        Outcome::Unavailable(reason) => format!("unavailable {reason}"),
    }
}

/// 0 for `ok`, 1 for anything else.
pub fn exit_code(outcome: &Outcome) -> u8 {
    u8::from(!matches!(outcome, Outcome::Ok(_)))
}
