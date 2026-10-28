//! The compositor under test, started headless as a child process.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A hung compositor is killed after this.
const WATCHDOG: Duration = Duration::from_secs(60);

/// The text of each config layer file to write before the start.
#[derive(Default, Clone, Copy)]
pub struct Layers<'a> {
    pub defaults: Option<&'a str>,
    pub managed: Option<&'a str>,
    pub user: Option<&'a str>,
}

/// The defaults layer of a test that passes none: no border, so the size
/// fixtures stay exact. A test of the built-in border passes `Some("")`.
pub const UNDECORATED: &str = "[decorations]\nwidth = 0\n";

/// Points the three layers at paths under `root`, so a test never reads
/// or writes the developer's own `~/.config/tetsu` or `~/.local/state/tetsu`.
pub fn isolate(command: &mut Command, root: &Path) {
    command
        .env("XDG_CONFIG_HOME", root.join("xdg-config"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("TETSU_COMP_DEFAULTS_FILE", root.join("defaults.toml"))
        .env("HOME", root.join("home"));
}

/// A running compositor, killed when dropped or when it hangs.
pub struct Comp {
    child: Child,
    runtime: tempfile::TempDir,
    display: String,
    x11: Option<String>,
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
        Self::spawn(args, config, false, &[], Layers::default())
    }

    /// Starts with the layer files written first.
    pub fn start_layers(args: &[&str], layers: Layers<'_>) -> Self {
        Self::spawn(args, None, false, &[], layers)
    }

    /// Like `start_with`, with extra environment variables (they win over
    /// the harness's own). The helpers the compositor starts inherit them.
    pub fn start_with_env(args: &[&str], config: Option<&str>, env: &[(&str, &str)]) -> Self {
        Self::spawn(args, config, false, env, Layers::default())
    }

    /// Starts with `--headless-hotplug` and a piped stdin; `control` writes
    /// its commands.
    pub fn start_hotplug(args: &[&str], config: Option<&str>) -> Self {
        let mut all = vec!["--headless-hotplug"];
        all.extend_from_slice(args);
        Self::spawn(&all, config, true, &[], Layers::default())
    }

    /// Like `start_hotplug`, with extra environment variables.
    pub fn start_hotplug_with_env(
        args: &[&str],
        config: Option<&str>,
        env: &[(&str, &str)],
    ) -> Self {
        let mut all = vec!["--headless-hotplug"];
        all.extend_from_slice(args);
        Self::spawn(&all, config, true, env, Layers::default())
    }

    /// Like `start_hotplug`, with the layer files written first.
    pub fn start_hotplug_layers(args: &[&str], layers: Layers<'_>) -> Self {
        let mut all = vec!["--headless-hotplug"];
        all.extend_from_slice(args);
        Self::spawn(&all, None, true, &[], layers)
    }

    /// Sends one hotplug command line.
    pub fn control(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("started with start_hotplug");
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn spawn(
        args: &[&str],
        config: Option<&str>,
        piped_stdin: bool,
        env: &[(&str, &str)],
        layers: Layers<'_>,
    ) -> Self {
        let runtime = tempfile::tempdir().unwrap();
        for (path, text) in [
            (
                runtime.path().join("defaults.toml"),
                layers.defaults.or(Some(UNDECORATED)),
            ),
            (managed_path(runtime.path()), layers.managed),
            (user_path(runtime.path()), layers.user),
        ] {
            if let Some(text) = text {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_tetsu-comp"));
        command.args(["--backend", "headless"]).args(args);
        isolate(&mut command, runtime.path());
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
            .envs(env.iter().copied())
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
        let x11 = parse_x11_display(&line);
        // The compositor writes nothing else; keep the pipe open.
        std::mem::forget(stdout);
        let stdin = child.stdin.take();
        Self {
            stdin,
            child,
            runtime,
            display,
            x11,
            bus,
            cancel,
        }
    }

    /// The compositor's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// `:<n>` when the compositor serves X11 (the ready line says so).
    pub fn x11_display(&self) -> Option<&str> {
        self.x11.as_deref()
    }

    /// The user layer file, which may not exist yet.
    pub fn user_file(&self) -> PathBuf {
        user_path(self.runtime.path())
    }

    /// The managed layer file, which may not exist yet.
    pub fn managed_file(&self) -> PathBuf {
        managed_path(self.runtime.path())
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
        if let Some(display) = &self.x11 {
            remove_x11_files(display, self.child.id());
        }
    }
}

/// A killed compositor leaves its X11 lock and socket in `/tmp`: removes
/// them, but only while the lock still holds that (dead) compositor's pid.
fn remove_x11_files(display: &str, pid: u32) {
    let Some(n) = display.strip_prefix(':') else {
        return;
    };
    let lock = PathBuf::from(format!("/tmp/.X{n}-lock"));
    let owner = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok());
    if owner == Some(pid) {
        let _ = std::fs::remove_file(format!("/tmp/.X11-unix/X{n}"));
        let _ = std::fs::remove_file(&lock);
    }
}

/// The optional `DISPLAY=:<n>` after the two fixed words of the ready line.
pub fn parse_x11_display(line: &str) -> Option<String> {
    line.split_whitespace()
        .nth(3)
        .and_then(|w| w.strip_prefix("DISPLAY="))
        .map(str::to_owned)
}

/// `ready WAYLAND_DISPLAY=<n> TETSU_COMP_SOCKET=<p> [DISPLAY=:<n>]`
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

fn user_path(root: &Path) -> PathBuf {
    root.join("xdg-config/tetsu/comp.toml")
}

fn managed_path(root: &Path) -> PathBuf {
    root.join("xdg-state/tetsu/comp.toml")
}
