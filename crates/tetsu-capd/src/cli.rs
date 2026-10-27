//! `tetsu cap`: list and change grants through `org.tetsu.cap.v1`.

use std::io::Write;

use tetsu_bus::cap::{self, Capability, ClientError, Grant, SetParams};

/// The usage text.
pub const USAGE: &str = "\
usage: tetsu cap list
       tetsu cap set <class> <automatic|notify|ask|off> [--scope <scope>]
       tetsu cap revoke <class> [--scope <scope>]";

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `-h`.
    Help,
    /// `list`.
    List,
    /// `set`.
    Set {
        /// The class.
        class: String,
        /// The scope.
        scope: Option<String>,
        /// The new state.
        state: Grant,
    },
    /// `revoke`: removes the managed value.
    Revoke {
        /// The class.
        class: String,
        /// The scope.
        scope: Option<String>,
    },
}

/// Parses the arguments after `cap`. The error is the usage message.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let _ = args;
    unimplemented!()
}

/// What the command needs from the service.
pub trait CapApi {
    /// `ListCapabilities`.
    fn list(&mut self) -> impl Future<Output = Result<Vec<Capability>, ClientError>>;

    /// `Set`.
    fn set(&mut self, params: &SetParams) -> impl Future<Output = Result<(), ClientError>>;
}

impl CapApi for cap::Client {
    async fn list(&mut self) -> Result<Vec<Capability>, ClientError> {
        cap::Client::list(self).await
    }

    async fn set(&mut self, params: &SetParams) -> Result<(), ClientError> {
        cap::Client::set(self, params).await
    }
}

/// Runs a command; returns the exit status (0 done, 1 failed).
pub async fn run<A: CapApi>(
    command: Command,
    api: &mut A,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let _ = (command, api, out, err);
    unimplemented!()
}
