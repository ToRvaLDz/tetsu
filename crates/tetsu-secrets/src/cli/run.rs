//! Running a parsed command against the broker.

use std::io::{self, BufRead, Write};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;
use tetsu_bus::secrets::{
    AddParams, Client, ClientError, RotateParams, SecretInfo, SecretKind, Sensitive,
};

use super::parse::{Command, USAGE, log_argv, parse};
use super::value::{Prompt, TtyPrompt, read_value};
use crate::paths::socket;

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

impl Io<'_> {
    fn fail(&mut self, message: impl std::fmt::Display) -> u8 {
        let _ = writeln!(self.err, "tetsu secrets: {message}");
        1
    }
}

/// Runs `command` against the broker at `socket`; the exit code is 0 for
/// done and 1 for failed.
pub async fn run(command: Command, socket: &Path, io: &mut Io<'_>) -> u8 {
    match command {
        Command::Add {
            id,
            kind,
            host,
            allow,
        } => {
            let value = match read_value(io.stdin_is_tty, io.prompt, io.stdin) {
                Ok(value) => value,
                Err(e) => return io.fail(e),
            };
            let (wire_kind, header_name) = kind.to_wire();
            let params = AddParams {
                id: id.to_string(),
                kind: wire_kind,
                header_name,
                host: host.to_string(),
                allowed_tools: (!allow.is_empty())
                    .then(|| allow.iter().map(ToString::to_string).collect()),
                value: Sensitive::new(value.expose()),
            };
            let mut client = match connect(socket).await {
                Ok(client) => client,
                Err(message) => return io.fail(message),
            };
            match client.add(&params).await {
                Ok(info) => {
                    let _ = writeln!(io.out, "added {} ({})", info.id, describe_kind(&info));
                    0
                }
                Err(e) => io.fail(explain(&e, socket)),
            }
        }
        Command::Rotate { id } => {
            let value = match read_value(io.stdin_is_tty, io.prompt, io.stdin) {
                Ok(value) => value,
                Err(e) => return io.fail(e),
            };
            let params = RotateParams {
                id: id.to_string(),
                value: Sensitive::new(value.expose()),
            };
            let mut client = match connect(socket).await {
                Ok(client) => client,
                Err(message) => return io.fail(message),
            };
            match client.rotate(&params).await {
                Ok(info) => {
                    let _ = writeln!(io.out, "rotated {}", info.id);
                    0
                }
                Err(e) => io.fail(explain(&e, socket)),
            }
        }
        Command::Rm { id } => {
            let mut client = match connect(socket).await {
                Ok(client) => client,
                Err(message) => return io.fail(message),
            };
            match client.remove(id.as_str()).await {
                Ok(()) => {
                    let _ = writeln!(io.out, "removed {id}");
                    0
                }
                Err(e) => io.fail(explain(&e, socket)),
            }
        }
        Command::List => {
            let mut client = match connect(socket).await {
                Ok(client) => client,
                Err(message) => return io.fail(message),
            };
            match client.list().await {
                Ok(secrets) => {
                    let _ = io.out.write_all(table(&secrets).as_bytes());
                    0
                }
                Err(e) => io.fail(explain(&e, socket)),
            }
        }
        // `log` is journalctl, run by `main`; nothing talks to the broker.
        Command::Log { .. } => io.fail("log is run by journalctl"),
    }
}

async fn connect(socket: &Path) -> Result<Client, String> {
    Client::connect(socket).await.map_err(|e| {
        format!(
            "cannot reach the secrets broker at {}: {e} (is tetsu-secrets.service running?)",
            socket.display()
        )
    })
}

/// A readable message for a failed call; never a value.
fn explain(error: &ClientError, socket: &Path) -> String {
    let ClientError::Remote { name, parameters } = error else {
        return match error {
            ClientError::Io(e) => format!(
                "the secrets broker at {} stopped answering: {e}",
                socket.display()
            ),
            other => other.to_string(),
        };
    };
    let id = parameters["id"].as_str().unwrap_or("?");
    match name.rsplit('.').next().unwrap_or(name) {
        "SecretNotFound" => format!("no secret `{id}`"),
        "SecretExists" => format!("a secret `{id}` exists already"),
        "VaultUnavailable" => {
            "the vault or the audit journal is not available; nothing changed".to_owned()
        }
        "PermissionDenied" => {
            "not allowed: secrets are changed by the user, not the agent".to_owned()
        }
        "InvalidParameter" => {
            let parameter = text(&parameters["parameter"]);
            format!("the broker refused the {parameter}")
        }
        other => format!("the broker answered {other}"),
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("?")
}

fn describe_kind(info: &SecretInfo) -> String {
    match (info.kind, &info.header_name) {
        (SecretKind::Bearer, _) => "bearer".to_owned(),
        (SecretKind::Basic, _) => "basic".to_owned(),
        (SecretKind::Header, Some(name)) => format!("header:{name}"),
        (SecretKind::Header, None) => "header".to_owned(),
        (SecretKind::Unknown, _) => "?".to_owned(),
    }
}

/// An aligned table of names and bindings; no value is in it.
fn table(secrets: &[SecretInfo]) -> String {
    let mut rows = vec![[
        "ID".to_owned(),
        "KIND".to_owned(),
        "HOST".to_owned(),
        "ALLOWED".to_owned(),
        "CREATED".to_owned(),
        "LAST USED".to_owned(),
        "USES".to_owned(),
    ]];
    for s in secrets {
        let allowed = if s.allowed_tools.is_empty() {
            "-".to_owned()
        } else {
            s.allowed_tools.join(",")
        };
        rows.push([
            s.id.clone(),
            describe_kind(s),
            s.host.clone(),
            allowed,
            s.created_at.clone(),
            s.last_used_at.clone().unwrap_or_else(|| "-".to_owned()),
            s.use_count.to_string(),
        ]);
    }
    let widths: Vec<usize> = (0..7)
        .map(|c| rows.iter().map(|r| r[c].chars().count()).max().unwrap_or(0))
        .collect();
    let mut out = String::new();
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

fn usage_failure(message: &str) -> ExitCode {
    eprintln!("tetsu secrets: {message}\n\n{USAGE}");
    ExitCode::from(2)
}

/// The entry point of `tetsu secrets`: arguments after `secrets`. Exit
/// codes: 0 done, 1 failed, 2 usage.
pub fn main(args: &[String]) -> ExitCode {
    let command = match parse(args) {
        Ok(command) => command,
        Err(message) => return usage_failure(&message),
    };
    if let Command::Log { follow, lines } = command {
        // Only returns when journalctl could not be started.
        let e = std::process::Command::new("journalctl")
            .args(log_argv(follow, lines))
            .exec();
        eprintln!("tetsu secrets: cannot run journalctl: {e}");
        return ExitCode::FAILURE;
    }
    let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
        eprintln!("tetsu secrets: XDG_RUNTIME_DIR is not set");
        return ExitCode::FAILURE;
    };
    let socket = socket(&PathBuf::from(runtime_dir));
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("tetsu secrets: {e}");
            return ExitCode::FAILURE;
        }
    };
    let stdin_is_tty = rustix::termios::isatty(io::stdin());
    let mut stdin = io::stdin().lock();
    let (mut out, mut err) = (io::stdout().lock(), io::stderr().lock());
    let mut prompt = TtyPrompt;
    let mut io = Io {
        stdin: &mut stdin,
        stdin_is_tty,
        prompt: &mut prompt,
        out: &mut out,
        err: &mut err,
    };
    ExitCode::from(runtime.block_on(run(command, &socket, &mut io)))
}
