//! `tetsu secrets add|rm|list|rotate|log`.
//!
//! The logic lives here; the `tetsu` binary only forwards its arguments.
//! A value is read from a no-echo prompt or from piped stdin, never from
//! argv or the environment, and no output ever shows one.

mod parse;
mod run;
mod value;

pub use parse::{Command, USAGE, log_argv, parse};
pub use run::{Io, main, run};
pub use value::{Prompt, TtyPrompt, ValueError, read_value};
