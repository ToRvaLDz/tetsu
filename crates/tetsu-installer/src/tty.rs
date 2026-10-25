//! Terminal echo control for secret prompts.

use std::io::stdin;
use std::os::fd::AsFd;

use rustix::termios::{LocalModes, OptionalActions, Termios, isatty, tcgetattr, tcsetattr};

/// Whether standard input is a terminal (false for serial logs, pipes and
/// tests).
pub fn stdin_is_tty() -> bool {
    isatty(stdin().as_fd())
}

/// Echo is off while the guard lives; dropping it restores the terminal.
pub struct EchoOff {
    saved: Termios,
}

impl EchoOff {
    /// Turn echo off on standard input; `None` when it is not a terminal.
    pub fn new() -> Option<Self> {
        let fd = stdin();
        let saved = tcgetattr(fd.as_fd()).ok()?;
        let mut quiet = saved.clone();
        quiet.local_modes.remove(LocalModes::ECHO);
        // The newline the user types is not echoed either; keep it so the
        // next line starts clean.
        quiet.local_modes.insert(LocalModes::ECHONL);
        tcsetattr(fd.as_fd(), OptionalActions::Flush, &quiet).ok()?;
        Some(Self { saved })
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        let _ = tcsetattr(stdin().as_fd(), OptionalActions::Flush, &self.saved);
    }
}
