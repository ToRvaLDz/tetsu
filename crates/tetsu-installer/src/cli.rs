//! Command line of `tetsu-installer`.

use std::path::PathBuf;

/// Where the installation medium is mounted unless `--medium` says so.
pub const DEFAULT_MEDIUM: &str = "/run/tetsu/medium";

pub const USAGE: &str = "\
usage: tetsu-installer [--medium <dir>] [--answers <file>]

  --medium <dir>    where the installation medium is mounted, holding
                    tetsu-payload.tar (default /run/tetsu/medium)
  --answers <file>  read answers from this file instead of looking for a
                    TETSU-ANSWERS volume or tetsu.answers= on the kernel
                    command line
  -h, --help        show this text

exit status: 0 installed, 1 failed or aborted, 2 usage error";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub medium: PathBuf,
    pub answers: Option<PathBuf>,
}

/// A command line that cannot be run; the message says why.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(String);

fn usage(msg: impl Into<String>) -> UsageError {
    UsageError(msg.into())
}

/// Parse the arguments after the program name.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    let mut medium = None;
    let mut answers = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (arg, None),
        };
        let slot = match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--medium" => &mut medium,
            "--answers" => &mut answers,
            other => return Err(usage(format!("unknown argument: {other}"))),
        };
        let value = inline
            .or_else(|| args.next())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| usage(format!("{flag} needs a value")))?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(usage(format!("{flag} given twice")));
        }
    }
    Ok(Command::Run(Options {
        medium: medium.unwrap_or_else(|| PathBuf::from(DEFAULT_MEDIUM)),
        answers,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, UsageError> {
        parse(args.iter().map(|s| (*s).to_owned()))
    }

    fn run(medium: &str, answers: Option<&str>) -> Command {
        Command::Run(Options {
            medium: medium.into(),
            answers: answers.map(PathBuf::from),
        })
    }

    #[test]
    fn defaults_to_the_standard_medium() {
        assert_eq!(parse_args(&[]), Ok(run("/run/tetsu/medium", None)));
    }

    #[test]
    fn flags_take_a_value_in_both_forms() {
        assert_eq!(
            parse_args(&["--medium", "/m", "--answers=/a.toml"]),
            Ok(run("/m", Some("/a.toml")))
        );
    }

    #[test]
    fn help_wins() {
        assert_eq!(parse_args(&["--medium", "/m", "-h"]), Ok(Command::Help));
    }

    #[test]
    fn bad_command_lines_are_usage_errors() {
        for args in [
            &["--medium"][..],
            &["--medium="],
            &["--medium", "/a", "--medium", "/b"],
            &["--frobnicate"],
            &["stray"],
        ] {
            assert!(parse_args(args).is_err(), "{args:?}");
        }
    }
}
