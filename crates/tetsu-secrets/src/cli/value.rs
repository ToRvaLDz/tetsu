//! Reading the value to store: a hidden prompt, or piped stdin.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};

use rustix::termios::{LocalModes, OptionalActions, Termios, tcgetattr, tcsetattr};
use zeroize::Zeroizing;

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

/// Restores the terminal mode when dropped, whatever ends the read.
struct EchoOff<'a> {
    tty: &'a File,
    saved: Termios,
}

impl<'a> EchoOff<'a> {
    fn new(tty: &'a File) -> io::Result<Self> {
        let saved = tcgetattr(tty)?;
        let mut quiet = saved.clone();
        quiet.local_modes.remove(LocalModes::ECHO);
        tcsetattr(tty, OptionalActions::Flush, &quiet)?;
        Ok(Self { tty, saved })
    }
}

impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(self.tty, OptionalActions::Flush, &self.saved);
    }
}

impl Prompt for TtyPrompt {
    fn read_hidden(&mut self, label: &str) -> io::Result<String> {
        let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        tty.write_all(label.as_bytes())?;
        tty.flush()?;
        let mut line = Zeroizing::new(String::new());
        {
            let _quiet = EchoOff::new(&tty)?;
            BufReader::new(&tty).read_line(&mut line)?;
        }
        // The user's Enter was not echoed either.
        tty.write_all(b"\n")?;
        Ok(strip_newline(std::mem::take(&mut *line)))
    }
}

fn strip_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    text
}

fn checked(text: String) -> Result<Secret, ValueError> {
    let text = Zeroizing::new(text);
    if text.is_empty() {
        return Err(ValueError::Empty);
    }
    if text.contains('\0') {
        return Err(ValueError::Nul);
    }
    Ok(Secret::new(text.as_str()))
}

/// Reads the value: twice from `prompt` on a terminal, else all of `stdin`
/// with one trailing newline removed.
pub fn read_value(
    stdin_is_tty: bool,
    prompt: &mut dyn Prompt,
    stdin: &mut dyn BufRead,
) -> Result<Secret, ValueError> {
    if stdin_is_tty {
        let first = Zeroizing::new(prompt.read_hidden("Value: ")?);
        let again = Zeroizing::new(prompt.read_hidden("Again: ")?);
        if first.as_str() != again.as_str() {
            return Err(ValueError::Mismatch);
        }
        return checked(first.to_string());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    stdin.read_to_end(&mut bytes)?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the value is not UTF-8 text"))?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    checked(text.to_owned())
}
