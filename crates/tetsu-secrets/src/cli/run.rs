//! Running a parsed command against the broker.

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::ExitCode;

use super::parse::Command;
use super::value::Prompt;

/// What a command reads and writes.
pub struct Io<'a> {
    /// Standard input.
    pub stdin: &'a mut dyn BufRead,
    /// Whether standard input is a terminal.
    pub stdin_is_tty: bool,
    /// The hidden prompt.
    pub prompt: &'a mut dyn Prompt,
    /// Standard output.
    pub out: &'a mut dyn Write,
    /// Standard error.
    pub err: &'a mut dyn Write,
}

/// Runs `command` against the broker at `socket`; the exit code is 0 for
/// done and 1 for failed.
pub async fn run(command: Command, socket: &Path, io: &mut Io<'_>) -> u8 {
    todo!("{command:?} {socket:?} {}", io.stdin_is_tty)
}

/// The entry point of `tetsu secrets`: arguments after `secrets`. Exit
/// codes: 0 done, 1 failed, 2 usage.
pub fn main(args: &[String]) -> ExitCode {
    todo!("{args:?}")
}
