//! `tetsu comp`: talk to the compositor's bus.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use tetsu_bus::cap::ClientError;

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
pub fn describe(_error: &ClientError, _socket: &Path) -> String {
    todo!()
}

/// Runs a parsed command against the compositor at `socket`. Returns the
/// exit status: 0 shown, 1 failed.
pub async fn run(
    _command: Command,
    _socket: &Path,
    _timeout: Duration,
    _out: &mut dyn Write,
    _err: &mut dyn Write,
) -> u8 {
    todo!()
}

/// Entry point of `tetsu comp ...`: exit status 0 shown, 1 failed, 2 usage.
pub fn main(args: Vec<String>) -> ExitCode {
    match parse(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Config { .. }) => {
            eprintln!("tetsu: comp config is not implemented yet");
            ExitCode::FAILURE
        }
        Err(message) => {
            eprintln!("{message}");
            ExitCode::from(2)
        }
    }
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
