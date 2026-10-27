//! Reading the value to store: a hidden prompt, or piped stdin.

use std::io::{self, BufRead};

use crate::vault::Secret;

/// Asks the user for a line without echoing it.
pub trait Prompt {
    /// Shows `label` and reads one line, newline removed.
    fn read_hidden(&mut self, label: &str) -> io::Result<String>;
}

/// Why no value was read.
#[derive(Debug, thiserror::Error)]
pub enum ValueError {
    /// The two entries differ.
    #[error("the two entries differ")]
    Mismatch,
    /// Nothing was entered.
    #[error("the value is empty")]
    Empty,
    /// The value has a NUL byte.
    #[error("the value contains NUL")]
    Nul,
    #[error("cannot read the value: {0}")]
    Io(#[from] io::Error),
}

/// The prompt on the controlling terminal, with echo off while it reads.
#[derive(Debug, Default)]
pub struct TtyPrompt;

impl Prompt for TtyPrompt {
    fn read_hidden(&mut self, label: &str) -> io::Result<String> {
        todo!("{label}")
    }
}

/// Reads the value: twice from `prompt` on a terminal, else all of `stdin`
/// with one trailing newline removed.
pub fn read_value(
    stdin_is_tty: bool,
    prompt: &mut dyn Prompt,
    stdin: &mut dyn BufRead,
) -> Result<Secret, ValueError> {
    let _ = (stdin_is_tty, prompt, stdin);
    todo!()
}
