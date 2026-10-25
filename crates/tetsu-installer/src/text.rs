//! The line-prompt frontend: plain lines in, plain lines out. No cursor
//! movement and no colours, so it reads fine on a serial console or in a
//! captured log.

use std::io::{BufRead, Write};

use tetsu_installer_core::error::InstallError;
use tetsu_installer_core::frontend::{Frontend, Validate};
use tetsu_installer_core::unlock::recovery::render_qr;

use crate::tty::EchoOff;

/// Frontend over any line reader and writer.
pub struct TextFrontend<R, W> {
    input: R,
    out: W,
    hide_secrets: bool,
}

impl<R: BufRead, W: Write> TextFrontend<R, W> {
    /// Secrets are read as plain lines.
    pub fn new(input: R, out: W) -> Self {
        Self {
            input,
            out,
            hide_secrets: false,
        }
    }

    /// Turn terminal echo off while a secret is typed. Only meaningful when
    /// `input` is the process's standard input and it is a terminal.
    pub fn hide_secrets(mut self, hide: bool) -> Self {
        self.hide_secrets = hide;
        self
    }

    /// Give the reader and the writer back.
    pub fn into_parts(self) -> (R, W) {
        (self.input, self.out)
    }

    fn print(&mut self, text: &str) -> Result<(), InstallError> {
        write!(self.out, "{text}")
            .and_then(|()| self.out.flush())
            .map_err(console_error)
    }

    fn line(&mut self, text: &str) -> Result<(), InstallError> {
        self.print(&format!("{text}\n"))
    }

    /// One line without its terminator; end of input aborts the install.
    fn read_line(&mut self) -> Result<String, InstallError> {
        let mut buf = String::new();
        match self.input.read_line(&mut buf) {
            Ok(0) => Err(InstallError::Aborted),
            Ok(_) => Ok(buf.trim_end_matches(['\n', '\r']).to_owned()),
            Err(e) => Err(console_error(e)),
        }
    }
}

fn console_error(e: std::io::Error) -> InstallError {
    InstallError::Failed(format!("console: {e}"))
}

impl<R: BufRead, W: Write> Frontend for TextFrontend<R, W> {
    fn ask(&mut self, prompt: &str, validate: Validate<'_>) -> Result<String, InstallError> {
        loop {
            self.print(&format!("{prompt}: "))?;
            let answer = self.read_line()?;
            match validate(&answer) {
                Ok(()) => return Ok(answer),
                Err(msg) => self.line(&msg)?,
            }
        }
    }

    fn ask_secret(&mut self, prompt: &str) -> Result<String, InstallError> {
        self.print(&format!("{prompt} (not shown): "))?;
        let guard = self.hide_secrets.then(EchoOff::new).flatten();
        let answer = self.read_line();
        drop(guard);
        answer
    }

    fn choose(&mut self, prompt: &str, options: &[String]) -> Result<usize, InstallError> {
        self.line(&format!("{prompt}:"))?;
        let width = options.len().to_string().len();
        for (i, option) in options.iter().enumerate() {
            self.line(&format!("  {:>width$}) {option}", i + 1))?;
        }
        loop {
            self.print(&format!("Choice [1-{}]: ", options.len()))?;
            let answer = self.read_line()?;
            match answer.trim().parse::<usize>() {
                Ok(n) if (1..=options.len()).contains(&n) => return Ok(n - 1),
                _ => self.line(&format!("Enter a number from 1 to {}.", options.len()))?,
            }
        }
    }

    fn confirm(&mut self, prompt: &str) -> Result<bool, InstallError> {
        loop {
            self.print(&format!("{prompt} [y/n]: "))?;
            match self.read_line()?.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return Ok(true),
                "n" | "no" => return Ok(false),
                _ => self.line("Answer y or n.")?,
            }
        }
    }

    fn progress(&mut self, step: &str, msg: &str) {
        let _ = self.line(&format!("[{step}] {msg}"));
    }

    fn message(&mut self, text: &str) {
        let _ = self.line(text);
    }

    fn show_qr(&mut self, payload: &str) -> Result<(), InstallError> {
        let qr = render_qr(payload)?;
        self.line(&qr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fe(input: &str) -> TextFrontend<&[u8], Vec<u8>> {
        TextFrontend::new(input.as_bytes(), Vec::new())
    }

    fn output(fe: TextFrontend<&[u8], Vec<u8>>) -> String {
        String::from_utf8(fe.out).unwrap()
    }

    fn not_empty(s: &str) -> Result<(), String> {
        if s.is_empty() {
            Err("empty".to_owned())
        } else {
            Ok(())
        }
    }

    #[test]
    fn ask_reprompts_after_a_validation_error() {
        let mut fe = fe("\nmarco\r\n");
        assert_eq!(fe.ask("User name", &not_empty).unwrap(), "marco");
        assert_eq!(output(fe), "User name: empty\nUser name: ");
    }

    #[test]
    fn choose_numbers_the_options_and_rejects_bad_input() {
        let options: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
        let mut fe = fe("0\nx\n4\n 2 \n");
        assert_eq!(fe.choose("Pick", &options).unwrap(), 1);
        let out = output(fe);
        assert!(out.starts_with("Pick:\n  1) a\n  2) b\n  3) c\n"));
        assert_eq!(out.matches("Enter a number from 1 to 3.").count(), 3);
    }

    #[test]
    fn choose_pads_long_lists_to_one_width() {
        let options: Vec<String> = (0..10).map(|i| format!("o{i}")).collect();
        let mut fe = fe("10\n");
        assert_eq!(fe.choose("Pick", &options).unwrap(), 9);
        let out = output(fe);
        assert!(out.contains("   1) o0") && out.contains("  10) o9"));
    }

    #[test]
    fn confirm_wants_an_explicit_answer() {
        let mut fe = fe("\nmaybe\nYES\nn\n");
        assert!(fe.confirm("Go?").unwrap());
        assert!(!fe.confirm("Again?").unwrap());
        assert_eq!(output(fe).matches("Answer y or n.").count(), 2);
    }

    #[test]
    fn secrets_are_plain_lines_when_not_on_a_tty() {
        let mut fe = fe("s3cret pw\n");
        assert_eq!(fe.ask_secret("Passphrase").unwrap(), "s3cret pw");
        assert_eq!(output(fe), "Passphrase (not shown): ");
    }

    #[test]
    fn end_of_input_aborts() {
        let mut fe = fe("");
        assert!(matches!(fe.confirm("Go?"), Err(InstallError::Aborted)));
        let mut fe = self::fe("only\n");
        fe.ask("a", &|_| Ok(())).unwrap();
        assert!(matches!(fe.ask_secret("b"), Err(InstallError::Aborted)));
    }

    #[test]
    fn progress_message_and_qr_are_plain_lines() {
        let mut fe = fe("");
        fe.progress("disk", "partitioning");
        fe.message("hello");
        fe.show_qr("cccicubg-cccicubg").unwrap();
        let out = output(fe);
        assert!(out.starts_with("[disk] partitioning\nhello\n"));
        assert!(out.lines().count() > 10);
        assert!(!out.contains('\x1b'));
    }
}
