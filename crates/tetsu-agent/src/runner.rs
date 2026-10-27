//! The runner: one tool call through the bus facade.
//!
//! A gated call first tells capd why (`cap.v1.Intent`, over the fixed,
//! verified capd endpoint), then calls the method on its server. The
//! gate in front of the server decides; a refusal comes back as
//! `<interface>.PermissionDenied` and is an [`Outcome::Denied`], not an
//! error of the run.
//!
//! The log names only the tool id, the kind of the outcome and a duration:
//! never arguments, results, reasons or error text.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;

use serde_json::{Map, Value, json};
use tetsu_bus::cap::{self, IntentParams};
use tetsu_bus::dynamic::{Client, ClientError, DynStream};
use tetsu_bus::endpoint::Endpoint;

use crate::discovery::Route;
use crate::tools::{REASON, StreamMode, Tool};
use tetsu_bus::access::MethodKind;

/// A serialized result over this many bytes is replaced by its head.
pub const MAX_RESULT_BYTES: usize = 64 * 1024;
/// How much of an oversized result is kept.
const HEAD_BYTES: usize = 2048;
/// The longest error name or unavailable text sent to the model.
const MAX_TEXT_BYTES: usize = 256;
/// The longest `_reason` sent to capd.
const MAX_REASON_BYTES: usize = 1024;
/// Streams kept open for [`StreamMode::Hold`]; the oldest is dropped first.
pub const MAX_HELD: usize = 8;
/// The error of an input the runner refuses before any bus call.
pub const INVALID_INPUT: &str = "org.tetsu.agent.InvalidInput";
/// The service-wide refusal every Varlink service may answer.
const VARLINK_DENIED: &str = "org.varlink.service.PermissionDenied";

/// Where log lines go.
pub type LogSink = Rc<dyn Fn(&str)>;

/// The sink of the production binary: standard error.
pub fn stderr_sink() -> LogSink {
    Rc::new(|line| eprintln!("{line}"))
}

/// What a tool call came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The reply (bounded).
    Ok(Value),
    /// The user or the capability policy refused the call.
    Denied,
    /// The server answered with an error.
    Error {
        /// Fully qualified error name.
        error: String,
        /// The error parameters.
        parameters: Value,
    },
    /// The server could not be reached or the connection broke.
    Unavailable(String),
}

impl Outcome {
    /// The kind, for the log and the report.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ok(_) => "ok",
            Self::Denied => "denied",
            Self::Error { .. } => "error",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

/// Runs one tool call.
pub trait ToolRunner {
    /// Runs `tool` with the model's `input`. Dropping the future cancels the
    /// call: the connection closes, and the gate withdraws a pending ask.
    fn run(&self, tool: &Tool, input: Value) -> impl Future<Output = Outcome>;
}

/// The runner over the bus.
pub struct BusRunner {
    routes: BTreeMap<String, Route>,
    capd: Endpoint,
    log: LogSink,
    held: RefCell<VecDeque<DynStream>>,
}

impl BusRunner {
    /// A runner with the routes discovery found, the endpoint of capd (for
    /// the reasons) and a log sink.
    pub fn new(routes: BTreeMap<String, Route>, capd: Endpoint, log: LogSink) -> Self {
        Self {
            routes,
            capd,
            log,
            held: RefCell::new(VecDeque::new()),
        }
    }

    async fn connect(&self, tool: &Tool) -> Result<Client, Outcome> {
        let route = self
            .routes
            .get(&tool.interface)
            .ok_or_else(|| Outcome::Unavailable("the interface is not served".to_owned()))?;
        let connected = match route {
            Route::Scanned(path) => Client::connect(path).await,
            Route::Pinned(endpoint) => Client::connect_to(endpoint).await,
        };
        connected.map_err(|e| Outcome::Unavailable(short_text(&e.to_string())))
    }

    /// Tells capd why. A failure is logged and the call goes on: the ask is
    /// then shown without a reason, and capd still decides.
    async fn intent(&self, tool: &Tool, reason: String) {
        let sent = async {
            let mut client = cap::Client::connect_to(&self.capd).await?;
            client
                .intent(&IntentParams {
                    method: tool.id.clone(),
                    reason,
                })
                .await
        };
        if sent.await.is_err() {
            (self.log)(&format!("tetsu-agent: {} intent not delivered", tool.id));
        }
    }

    async fn call(&self, tool: &Tool, params: Value) -> Outcome {
        let client = match self.connect(tool).await {
            Ok(client) => client,
            Err(outcome) => return outcome,
        };
        let result = match tool.stream {
            StreamMode::Single => single(client, tool, &params).await,
            StreamMode::First => first(client, tool, &params).await.map(|(v, _)| v),
            StreamMode::Collect => collect(client, tool, &params).await,
            StreamMode::Hold => match first(client, tool, &params).await {
                Ok((value, rest)) => {
                    self.hold(rest);
                    Ok(value)
                }
                Err(e) => Err(e),
            },
        };
        match result {
            Ok(value) => Outcome::Ok(bounded(value)),
            Err(e) => outcome_of_error(tool, e),
        }
    }

    /// Keeps a stream open, dropping (closing) the oldest past the limit.
    fn hold(&self, stream: Option<DynStream>) {
        let Some(stream) = stream else { return };
        let mut held = self.held.borrow_mut();
        held.push_back(stream);
        while held.len() > MAX_HELD {
            held.pop_front();
        }
    }
}

impl ToolRunner for BusRunner {
    async fn run(&self, tool: &Tool, input: Value) -> Outcome {
        let started = Instant::now();
        let outcome = match split_input(input) {
            Err(outcome) => outcome,
            Ok((params, reason)) => {
                if let (MethodKind::Gated, Some(reason)) = (tool.kind, reason) {
                    self.intent(tool, reason).await;
                }
                self.call(tool, params).await
            }
        };
        (self.log)(&format!(
            "tetsu-agent: {} -> {} in {} ms",
            tool.id,
            outcome.kind(),
            started.elapsed().as_millis()
        ));
        outcome
    }
}

/// The params object without `_reason`, and the reason when it is a
/// non-blank string. No input reads as an empty object.
fn split_input(input: Value) -> Result<(Value, Option<String>), Outcome> {
    let mut object: Map<String, Value> = match input {
        Value::Null => Map::new(),
        Value::Object(object) => object,
        _ => {
            return Err(Outcome::Error {
                error: INVALID_INPUT.to_owned(),
                parameters: json!({}),
            });
        }
    };
    let reason = match object.remove(REASON) {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(cut(text, MAX_REASON_BYTES)),
        _ => None,
    };
    Ok((Value::Object(object), reason))
}

async fn single(mut client: Client, tool: &Tool, params: &Value) -> Result<Value, ClientError> {
    client.call(&tool.id, params).await
}

/// The first reply, and the rest of the stream when it continues.
async fn first(
    client: Client,
    tool: &Tool,
    params: &Value,
) -> Result<(Value, Option<DynStream>), ClientError> {
    let mut stream = client.call_more(&tool.id, params).await?;
    match stream.next().await? {
        Some(item) if item.continues => Ok((item.value, Some(stream))),
        Some(item) => Ok((item.value, None)),
        None => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
    }
}

/// Waits for the last reply: `{final, intermediate_count}`.
async fn collect(client: Client, tool: &Tool, params: &Value) -> Result<Value, ClientError> {
    let mut stream = client.call_more(&tool.id, params).await?;
    let mut intermediate = 0u64;
    let mut last = None;
    while let Some(item) = stream.next().await? {
        if item.continues {
            intermediate += 1;
        }
        last = Some(item.value);
    }
    match last {
        Some(value) => Ok(json!({ "final": value, "intermediate_count": intermediate })),
        None => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
    }
}

fn outcome_of_error(tool: &Tool, error: ClientError) -> Outcome {
    match error {
        ClientError::Remote { error, parameters } => {
            if error == format!("{}.PermissionDenied", tool.interface) || error == VARLINK_DENIED {
                Outcome::Denied
            } else {
                Outcome::Error {
                    error: short_text(&error),
                    parameters: bounded(parameters),
                }
            }
        }
        ClientError::Io(e) => Outcome::Unavailable(short_text(&e.to_string())),
    }
}

/// The value itself, or `{truncated, bytes, head}` when its JSON is over
/// [`MAX_RESULT_BYTES`].
fn bounded(value: Value) -> Value {
    let text = value.to_string();
    if text.len() <= MAX_RESULT_BYTES {
        return value;
    }
    let mut end = HEAD_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    json!({ "truncated": true, "bytes": text.len(), "head": &text[..end] })
}

/// `text` cut to at most `max` bytes, at a char boundary.
fn cut(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Text from a server or the system for the model: control characters
/// dropped, cut to [`MAX_TEXT_BYTES`].
fn short_text(text: &str) -> String {
    cut(
        text.chars().filter(|c| !c.is_control()).collect(),
        MAX_TEXT_BYTES,
    )
}
