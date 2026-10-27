//! `tetsu comp`: talk to the compositor's bus.

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use tetsu_bus::cap::ClientError;
use tetsu_bus::config;

use self::socket::SocketEnv;

pub mod render;
pub mod socket;

/// Usage of the `comp` group.
pub const USAGE: &str = "\
usage: tetsu comp config [--json]

  config   show the effective compositor config: a table of key, value,
           origin (default, managed or user) and whether the key is
           writable, then the problems found in the user file. Values
           are printed in full; --json prints the reply as the compositor
           sent it, which is easier to read for long values

The compositor is found through TETSU_COMP_SOCKET when set, else the
socket $XDG_RUNTIME_DIR/tetsu/comp/$WAYLAND_DISPLAY, else the only socket
in $XDG_RUNTIME_DIR/tetsu/comp.

exit status: 0 shown, 1 failed, 2 usage error";

/// A parsed `tetsu comp` command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `config [--json]`.
    Config { json: bool },
    /// `-h`, `--help`.
    Help,
}

/// Parse the arguments after `comp`; an error is the usage message.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["-h" | "--help"] | ["config", "-h" | "--help"] => Ok(Command::Help),
        ["config"] => Ok(Command::Config { json: false }),
        ["config", "--json"] => Ok(Command::Config { json: true }),
        _ => Err(format!("tetsu: unknown command\n{USAGE}")),
    }
}

/// How long the compositor has to answer.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// One line telling why the call failed, without the `tetsu: ` prefix.
pub fn describe(error: &ClientError, socket: &Path) -> String {
    match error {
        ClientError::Io(e) => match e.kind() {
            ErrorKind::NotFound | ErrorKind::ConnectionRefused => format!(
                "tetsu-comp is not running (no listener at {})",
                socket.display()
            ),
            ErrorKind::PermissionDenied => format!(
                "permission denied on {}: the socket belongs to another user or session",
                socket.display()
            ),
            _ => format!("cannot talk to tetsu-comp at {}: {e}", socket.display()),
        },
        ClientError::Remote { error, .. } => match error.as_str() {
            config::PERMISSION_DENIED => {
                "not allowed: tetsu-comp refused to show its config to this process".to_owned()
            }
            "org.varlink.service.MethodNotFound" | "org.varlink.service.InterfaceNotFound" => {
                format!(
                    "this tetsu-comp does not serve {} (too old?)",
                    config::INTERFACE
                )
            }
            other => other.to_owned(),
        },
    }
}

/// "5 s", or "200 ms" when it is not a whole number of seconds.
fn show(timeout: Duration) -> String {
    if timeout.subsec_nanos() == 0 {
        format!("{} s", timeout.as_secs())
    } else {
        format!("{} ms", timeout.as_millis())
    }
}

/// Runs a parsed command against the compositor at `socket`. Returns the
/// exit status: 0 shown, 1 failed.
pub async fn run(
    command: Command,
    socket: &Path,
    timeout: Duration,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let json = match command {
        Command::Help => return write_out(out, err, &format!("{USAGE}\n")),
        Command::Config { json } => json,
    };
    let reply = match tokio::time::timeout(timeout, config::get_config(socket)).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(e)) => return fail(err, &describe(&e, socket)),
        Err(_) => {
            let why = format!("tetsu-comp did not answer within {}", show(timeout));
            return fail(err, &why);
        }
    };
    let text = if json {
        render::json(&reply)
    } else {
        render::table(&reply)
    };
    write_out(out, err, &text)
}

fn write_out(out: &mut dyn Write, err: &mut dyn Write, text: &str) -> u8 {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => 0,
        Err(e) => fail(err, &format!("cannot write the output: {e}")),
    }
}

fn fail(err: &mut dyn Write, why: &str) -> u8 {
    // Nothing more can be done if stderr itself is gone.
    let _ = writeln!(err, "tetsu: {why}");
    1
}

/// Entry point of `tetsu comp ...`: exit status 0 shown, 1 failed, 2 usage.
pub fn main(args: Vec<String>) -> ExitCode {
    let command = match parse(&args) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if command == Command::Help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let socket = match socket::discover(&SocketEnv::from_process()) {
        Ok(socket) => socket,
        Err(why) => {
            eprintln!("tetsu: {why}");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("tetsu: cannot start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let status = runtime.block_on(run(
        command,
        &socket,
        CALL_TIMEOUT,
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    ));
    ExitCode::from(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, String> {
        parse(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    fn remote(error: &str) -> ClientError {
        ClientError::Remote {
            error: error.into(),
            parameters: serde_json::json!({}),
        }
    }

    #[test]
    fn describe_tells_what_went_wrong() {
        use std::io::{Error, ErrorKind};
        let path = Path::new("/run/user/1/tetsu/comp/wayland-0");
        let io = |kind| describe(&ClientError::Io(Error::from(kind)), path);
        for kind in [ErrorKind::NotFound, ErrorKind::ConnectionRefused] {
            assert_eq!(
                io(kind),
                "tetsu-comp is not running (no listener at /run/user/1/tetsu/comp/wayland-0)"
            );
        }
        assert_eq!(
            io(ErrorKind::PermissionDenied),
            "permission denied on /run/user/1/tetsu/comp/wayland-0: \
             the socket belongs to another user or session"
        );
        let other = describe(&ClientError::Io(Error::other("boom")), path);
        assert_eq!(
            other,
            "cannot talk to tetsu-comp at /run/user/1/tetsu/comp/wayland-0: boom"
        );
        assert_eq!(
            describe(&remote("org.tetsu.comp.config.v1.PermissionDenied"), path),
            "not allowed: tetsu-comp refused to show its config to this process"
        );
        for name in [
            "org.varlink.service.MethodNotFound",
            "org.varlink.service.InterfaceNotFound",
        ] {
            assert_eq!(
                describe(&remote(name), path),
                "this tetsu-comp does not serve org.tetsu.comp.config.v1 (too old?)"
            );
        }
        assert_eq!(describe(&remote("org.x.Odd"), path), "org.x.Odd");
    }

    #[test]
    fn config_parses() {
        assert_eq!(parse_args(&["config"]), Ok(Command::Config { json: false }));
        assert_eq!(
            parse_args(&["config", "--json"]),
            Ok(Command::Config { json: true })
        );
    }

    #[test]
    fn help_parses() {
        for args in [
            &["-h"][..],
            &["--help"],
            &["config", "--help"],
            &["config", "-h"],
        ] {
            assert_eq!(parse_args(args), Ok(Command::Help), "{args:?}");
        }
    }

    #[test]
    fn anything_else_is_a_usage_error() {
        for args in [
            &[][..],
            &["frob"],
            &["config", "extra"],
            &["config", "--json", "--json"],
            &["config", "--yaml"],
        ] {
            let error = parse_args(args).unwrap_err();
            assert!(error.contains(USAGE), "{args:?}: {error}");
        }
    }
}
