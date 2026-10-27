//! The command line of `tetsu secrets`.

use crate::model::{Host, KindSpec, SecretId, ToolId};

/// The usage text.
pub const USAGE: &str = "usage:
  tetsu secrets add <id> --kind bearer|header:<name>|basic --host <host>[:port] [--allow <tool>]...
  tetsu secrets rotate <id>
  tetsu secrets rm <id>
  tetsu secrets list
  tetsu secrets log [-f] [-n <lines>]

The value is read from a hidden prompt, or from stdin when it is piped.";

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Add a secret.
    Add {
        /// The name.
        id: SecretId,
        /// How the value is presented.
        kind: KindSpec,
        /// The only host it may go to.
        host: Host,
        /// Tool ids that may receive it.
        allow: Vec<ToolId>,
    },
    /// Replace a value.
    Rotate {
        /// The name.
        id: SecretId,
    },
    /// Delete a secret.
    Rm {
        /// The name.
        id: SecretId,
    },
    /// List the secrets.
    List,
    /// Show the audit trail.
    Log {
        /// Keep following.
        follow: bool,
        /// How many lines.
        lines: Option<u32>,
    },
}

/// Parses the arguments after `secrets`; an error is a usage message.
pub fn parse(args: &[String]) -> Result<Command, String> {
    todo!("{args:?}")
}

/// The `journalctl` arguments that show the audit trail.
pub fn log_argv(follow: bool, lines: Option<u32>) -> Vec<String> {
    todo!("{follow} {lines:?}")
}
