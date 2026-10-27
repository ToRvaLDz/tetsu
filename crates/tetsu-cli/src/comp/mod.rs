//! `tetsu comp`: talk to the compositor's bus.

/// Usage of the `comp` group.
pub const USAGE: &str = "\
usage: tetsu comp config [--json]
";

/// A parsed `tetsu comp` command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `config [--json]`.
    Config { json: bool },
    /// `-h`, `--help`.
    Help,
}

/// Parse the arguments after `comp`; an error is the usage message.
pub fn parse(_args: &[String]) -> Result<Command, String> {
    todo!()
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
