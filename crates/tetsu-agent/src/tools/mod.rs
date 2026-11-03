//! The catalog: one [`Tool`] per method of the contract (literal 1:1).
//!
//! Everything a tool says (id, schema, description) is built from the
//! contract shipped with this build, so no server can write text that
//! reaches the model. `org.varlink.service.*` is not part of the contract
//! and so not a tool.

mod schema;
mod wire;

use std::fmt;

use serde_json::{Value, json};
use tetsu_bus::access::{MethodKind, method_kind};
use tetsu_bus_contract::{Contract, Interface, Method};

pub use schema::{SchemaError, prose};
pub use wire::{MAX_WIRE_LEN, WireError, from_wire, wire_name};

/// The reserved optional property of every gated tool: why the model makes
/// the call. The runner strips it and sends it to capd as the `Intent`.
/// Varlink field names cannot start with `_`, so it never collides.
pub const REASON: &str = "_reason";

/// Streaming methods that never end by themselves and whose first reply
/// is the useful one: the runner keeps their connection open.
pub const HOLD: [&str; 1] = ["org.tetsu.notify.v1.Show"];

/// What the tool description tells about a gated call.
const MAY_ASK: &str = "May ask the user for approval.";
/// What the tool description tells about a destructive call.
const ALWAYS_ASKS: &str = "Always asks the user for approval (destructive).";
/// The description of [`REASON`].
const REASON_TEXT: &str =
    "Why you make this call, in one sentence. It is shown to the user with the approval prompt.";

/// How the runner reads the replies of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// Not streaming: the one reply.
    Single,
    /// A `Watch*` method: the first reply (`initial`), then close.
    First,
    /// A streaming method that ends: wait for the last reply, return it
    /// with the number of replies before it.
    Collect,
    /// Return the first reply and keep the connection open, because closing
    /// it would undo the call.
    Hold,
}

/// One bus call as a tool.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    /// The fully qualified method, `<interface>.<Method>`.
    pub id: String,
    /// The interface.
    pub interface: String,
    /// The method name.
    pub method: String,
    /// The name sent to a provider; maps back to `id`.
    pub wire_name: String,
    /// Free or gated, by the rule of the gate.
    pub kind: MethodKind,
    /// The `@capability` class, when tagged.
    pub class: Option<String>,
    /// The `scope=` parameter of the tag.
    pub scope: Option<String>,
    /// Whether the method is `@destructive`.
    pub destructive: bool,
    /// How replies are read.
    pub stream: StreamMode,
    /// JSON Schema of the input.
    pub schema: Value,
    /// Doc prose, then one line about approval.
    pub description: String,
    /// Paths of the `@untrusted` output fields, as data for the fencing.
    pub untrusted_outputs: Vec<String>,
}

/// Why the catalog cannot be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogError {
    /// An id that cannot be a wire name.
    Wire(WireError),
    /// A method whose input has no schema.
    Schema {
        /// The method.
        id: String,
        /// Why.
        error: SchemaError,
    },
    /// A malformed `@capability` tag.
    Tag {
        /// The method.
        id: String,
        /// Why.
        reason: String,
    },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "tool name: {e}"),
            Self::Schema { id, error } => write!(f, "{id}: {error}"),
            Self::Tag { id, reason } => write!(f, "{id}: {reason}"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// One tool per method of the contract, sorted by id.
pub fn build(contract: &Contract) -> Result<Vec<Tool>, CatalogError> {
    let mut tools = Vec::new();
    for (name, interface) in &contract.interfaces {
        for (method_name, method) in &interface.methods {
            tools.push(tool(contract, name, interface, method_name, method)?);
        }
    }
    tools.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(tools)
}

fn tool(
    contract: &Contract,
    interface_name: &str,
    interface: &Interface,
    method_name: &str,
    method: &Method,
) -> Result<Tool, CatalogError> {
    let id = format!("{interface_name}.{method_name}");
    let wire_name = wire_name(&id).map_err(CatalogError::Wire)?;
    let kind = method_kind(&id);
    let capability = match method.doc.capability() {
        None => None,
        Some(Ok(c)) => Some(c),
        Some(Err(reason)) => return Err(CatalogError::Tag { id, reason }),
    };
    let destructive = method.doc.has_tag("@destructive");
    let mut schema =
        schema::fields_schema(interface, &method.inputs).map_err(|error| CatalogError::Schema {
            id: id.clone(),
            error,
        })?;
    if kind == MethodKind::Gated {
        schema["properties"][REASON] = json!({ "type": "string", "description": REASON_TEXT });
    }
    Ok(Tool {
        stream: stream_mode(contract, interface_name, method_name),
        description: description(&method.doc, kind, destructive),
        untrusted_outputs: schema::untrusted_paths(interface, &method.outputs),
        class: capability.as_ref().map(|c| c.class.clone()),
        scope: capability.and_then(|c| c.scope),
        interface: interface_name.to_owned(),
        method: method_name.to_owned(),
        id,
        wire_name,
        kind,
        destructive,
        schema,
    })
}

fn stream_mode(contract: &Contract, interface: &str, method: &str) -> StreamMode {
    if !contract.is_streaming(interface, method) {
        StreamMode::Single
    } else if HOLD.contains(&format!("{interface}.{method}").as_str()) {
        StreamMode::Hold
    } else if method.starts_with("Watch") {
        StreamMode::First
    } else {
        StreamMode::Collect
    }
}

fn description(doc: &tetsu_bus_contract::Doc, kind: MethodKind, destructive: bool) -> String {
    let mut text = prose(doc);
    let line = match (kind, destructive) {
        (_, true) => ALWAYS_ASKS,
        (MethodKind::Gated, false) => MAY_ASK,
        (MethodKind::Free, false) => return text,
    };
    if !text.is_empty() {
        text.push_str("\n\n");
    }
    text.push_str(line);
    text
}
