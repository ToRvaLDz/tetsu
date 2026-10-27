//! The compositor under test, started headless as a child process.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A hung compositor is killed after this.
const WATCHDOG: Duration = Duration::from_secs(60);

/// A running compositor, killed when dropped or when it hangs.
pub struct Comp {
    child: Child,
    runtime: tempfile::TempDir,
    display: String,
    pub bus: PathBuf,
    cancel: mpsc::Sender<()>,
    stdin: Option<ChildStdin>,
}

impl Comp {
    pub fn start() -> Self {
        Self::start_with(&[], None)
    }

    /// Starts with extra arguments and, if given, a config file.
    pub fn start_with(args: &[&str], config: Option<&str>) -> Self {
        Self::spawn(args, config, false)
    }

    /// Starts with `--headless-hotplug` and a piped stdin; `control` writes
    /// its commands.
    pub fn start_hotplug(args: &[&str], config: Option<&str>) -> Self {
        let mut all = vec!["--headless-hotplug"];
        all.extend_from_slice(args);
        Self::spawn(&all, config, true)
    }

    /// Sends one hotplug command line.
    pub fn control(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("started with start_hotplug");
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn spawn(args: &[&str], config: Option<&str>, piped_stdin: bool) -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tetsu-comp"));
        command.args(["--backend", "headless"]).args(args);
        if let Some(text) = config {
            let path = runtime.path().join("config.toml");
            std::fs::write(&path, text).unwrap();
            command.arg("--config").arg(path);
        }
        if piped_stdin {
            command.stdin(Stdio::piped());
        }
        let mut child = command
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let (cancel, cancelled) = mpsc::channel();
        std::thread::spawn(move || {
            if cancelled.recv_timeout(WATCHDOG).is_err() {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        });
        let mut line = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout.read_line(&mut line).unwrap();
        let (display, bus) = parse_ready(&line);
        // The compositor writes nothing else; keep the pipe open.
        std::mem::forget(stdout);
        let stdin = child.stdin.take();
        Self {
            stdin,
            child,
            runtime,
            display,
            bus,
            cancel,
        }
    }

    pub fn wayland_socket(&self) -> PathBuf {
        self.runtime.path().join(&self.display)
    }
}

impl Drop for Comp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.cancel.send(());
    }
}

/// `ready WAYLAND_DISPLAY=<n> TETSU_COMP_SOCKET=<p>`
pub fn parse_ready(line: &str) -> (String, PathBuf) {
    let mut words = line.split_whitespace();
    assert_eq!(words.next(), Some("ready"), "readiness line: {line:?}");
    let display = words
        .next()
        .and_then(|w| w.strip_prefix("WAYLAND_DISPLAY="))
        .unwrap_or_else(|| panic!("no WAYLAND_DISPLAY in {line:?}"));
    let socket = words
        .next()
        .and_then(|w| w.strip_prefix("TETSU_COMP_SOCKET="))
        .unwrap_or_else(|| panic!("no TETSU_COMP_SOCKET in {line:?}"));
    (display.to_owned(), PathBuf::from(socket))
}
