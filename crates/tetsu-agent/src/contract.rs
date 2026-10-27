//! The contract shipped with this build: the IDL of `tetsu-bus`, embedded.
//!
//! Tool names, schemas and descriptions come from here and never from a
//! live server (the text of a tool definition reaches the model, so a
//! server must not be able to write it). Discovery decides only which
//! interfaces are present and where. A test compares [`SOURCES`] with the
//! IDL directory, so a new file cannot be forgotten.

use tetsu_bus_contract::{Contract, LoadError};

/// `(file name, IDL text)` of every interface, sorted by file name.
pub const SOURCES: &[(&str, &str)] = &[
    (
        "org.tetsu.cap.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.cap.v1.varlink"),
    ),
    (
        "org.tetsu.comp.clipboard.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.clipboard.v1.varlink"),
    ),
    (
        "org.tetsu.comp.config.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.config.v1.varlink"),
    ),
    (
        "org.tetsu.comp.output.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.output.v1.varlink"),
    ),
    (
        "org.tetsu.comp.render.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.render.v1.varlink"),
    ),
    (
        "org.tetsu.comp.session.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.session.v1.varlink"),
    ),
    (
        "org.tetsu.comp.window.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.window.v1.varlink"),
    ),
    (
        "org.tetsu.comp.workspace.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.comp.workspace.v1.varlink"),
    ),
    (
        "org.tetsu.notify.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.notify.v1.varlink"),
    ),
    (
        "org.tetsu.tx.v1.varlink",
        include_str!("../../tetsu-bus/idl/org.tetsu.tx.v1.varlink"),
    ),
];

/// The streaming (`more`) method list, one `<interface>.<Method>` per line.
pub const STREAMING_LIST: &str = include_str!("../../tetsu-bus/idl/streaming.list");

/// Parses the embedded contract.
pub fn load() -> Result<Contract, LoadError> {
    Contract::from_sources(SOURCES, STREAMING_LIST)
}
