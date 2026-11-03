//! Reading the value to store: a hidden prompt, or piped stdin.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, Read, Write};

use rustix::termios::{
    LocalModes, OptionalActions, SpecialCodeIndex, Termios, tcgetattr, tcsetattr,
};
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
    /// The user pressed Ctrl-C (or Ctrl-\\) at the prompt.
    #[error("interrupted")]
    Interrupted,
    #[error("cannot read the value: {0}")]
    Io(#[from] io::Error),
}

/// The prompt on the controlling terminal, with echo off while it reads.
#[derive(Debug, Default)]
pub struct TtyPrompt;

/// Restores the terminal mode when dropped, whatever ends the read.
///
/// Signal generation is off while echo is off (`ISIG`) and so is line
/// buffering (`ICANON`): Ctrl-C arrives as a byte, [`read_secret_line`]
/// sees it, and the process never dies with the terminal still silent.
struct EchoOff<'a> {
    tty: &'a File,
    saved: Termios,
}

impl<'a> EchoOff<'a> {
    fn new(tty: &'a File) -> io::Result<Self> {
        let saved = tcgetattr(tty)?;
        let mut quiet = saved.clone();
        quiet
            .local_modes
            .remove(LocalModes::ECHO | LocalModes::ISIG | LocalModes::ICANON);
        quiet.special_codes[SpecialCodeIndex::VMIN] = 1;
        quiet.special_codes[SpecialCodeIndex::VTIME] = 0;
        tcsetattr(tty, OptionalActions::Flush, &quiet)?;
        Ok(Self { tty, saved })
    }
}

impl Drop for EchoOff<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(self.tty, OptionalActions::Flush, &self.saved);
    }
}

const ETX: u8 = 0x03;
const EOT: u8 = 0x04;
const BACKSPACE: u8 = 0x08;
const FILE_SEPARATOR: u8 = 0x1c;
const KILL_LINE: u8 = 0x15;
const DELETE: u8 = 0x7f;

/// Reads one line from a terminal in raw-ish mode (no echo, no line
/// editing by the driver): Enter ends it, Backspace and Ctrl-U edit it,
/// Ctrl-D ends it too, and Ctrl-C or Ctrl-\\ give `Interrupted`.
fn read_secret_line(mut tty: impl Read) -> io::Result<Zeroizing<String>> {
    // Room up front: a reallocation would leave an unwiped copy behind.
    let mut line = Zeroizing::new(Vec::with_capacity(4096));
    let mut byte = [0u8; 1];
    loop {
        match tty.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        match byte[0] {
            b'\n' | b'\r' | EOT => break,
            ETX | FILE_SEPARATOR => return Err(io::ErrorKind::Interrupted.into()),
            KILL_LINE => line.clear(),
            BACKSPACE | DELETE => {
                // Drop one whole character, not one byte of it.
                while let Some(last) = line.pop() {
                    if last & 0xc0 != 0x80 {
                        break;
                    }
                }
            }
            // Without ICANON the driver no longer eats Ctrl-W, Ctrl-R
            // and the like: a stray control byte must not end up in the
            // value unseen.
            control if control < 0x20 => {}
            other => line.push(other),
        }
    }
    let text = std::str::from_utf8(&line)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the value is not UTF-8 text"))?;
    Ok(Zeroizing::new(text.to_owned()))
}

impl Prompt for TtyPrompt {
    fn read_hidden(&mut self, label: &str) -> io::Result<String> {
        let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        tty.write_all(label.as_bytes())?;
        tty.flush()?;
        let line = {
            let _quiet = EchoOff::new(&tty)?;
            read_secret_line(&tty)
        };
        // The user's Enter was not echoed either.
        tty.write_all(b"\n")?;
        Ok(std::mem::take(&mut *line?))
    }
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

/// One hidden entry; an interrupt is its own error, so the caller can exit
/// as a shell does for Ctrl-C.
fn hidden(prompt: &mut dyn Prompt, label: &str) -> Result<String, ValueError> {
    prompt.read_hidden(label).map_err(|e| {
        if e.kind() == io::ErrorKind::Interrupted {
            ValueError::Interrupted
        } else {
            ValueError::Io(e)
        }
    })
}

/// Reads the value: twice from `prompt` on a terminal, else all of `stdin`
/// with one trailing newline removed.
pub fn read_value(
    stdin_is_tty: bool,
    prompt: &mut dyn Prompt,
    stdin: &mut dyn BufRead,
) -> Result<Secret, ValueError> {
    if stdin_is_tty {
        let first = Zeroizing::new(hidden(prompt, "Value: ")?);
        let again = Zeroizing::new(hidden(prompt, "Again: ")?);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(input: &[u8]) -> io::Result<String> {
        read_secret_line(input).map(|l| l.to_string())
    }

    #[test]
    fn enter_ends_the_line() {
        assert_eq!(line(b"abc\nrest").unwrap(), "abc");
        assert_eq!(line(b"abc\r").unwrap(), "abc");
        assert_eq!(line(b"abc").unwrap(), "abc");
    }

    #[test]
    fn backspace_and_kill_edit_the_line() {
        assert_eq!(line(b"abx\x7fc\n").unwrap(), "abc");
        assert_eq!(line(b"abc\x15xy\n").unwrap(), "xy");
        assert_eq!(line("a\u{e8}\x7fb\n".as_bytes()).unwrap(), "ab");
    }

    #[test]
    fn ctrl_c_and_ctrl_backslash_interrupt() {
        for input in [&b"ab\x03cd\n"[..], &b"\x1c"[..]] {
            let e = line(input).unwrap_err();
            assert_eq!(e.kind(), io::ErrorKind::Interrupted);
        }
    }

    #[test]
    fn other_control_bytes_never_reach_the_value() {
        // Ctrl-W, Ctrl-R, Esc and Tab are dropped, not stored.
        assert_eq!(line(b"a\x17b\x12c\x1b\td\n").unwrap(), "abcd");
    }

    #[test]
    fn ctrl_d_ends_the_line() {
        assert_eq!(line(b"abc\x04rest").unwrap(), "abc");
    }

    #[test]
    fn invalid_utf8_is_refused() {
        let e = line(b"a\xffb\n").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    }
}
