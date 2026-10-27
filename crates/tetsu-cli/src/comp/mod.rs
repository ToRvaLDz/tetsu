//! `tetsu comp`: talk to the compositor's bus.

use std::process::ExitCode;

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
