//! Command line of `tetsu`.

use std::path::PathBuf;

pub const USAGE: &str = "\
usage: tetsu secureboot enroll [--without-microsoft]
       tetsu secureboot trust <cert>

  secureboot enroll   enroll the Secure Boot keys of /var/lib/tetsu/keys
                      together with the Microsoft certificates (the
                      firmware must be in Setup Mode); refused when the
                      certificates are missing, unless --without-microsoft
                      says Tetsu's keys alone are meant
  secureboot trust    trust one more certificate in the db: a .der, .crt or
                      .pem certificate, or a pre-signed .auth file

exit status: 0 done, 1 failed, 2 usage error";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Enroll { without_microsoft: bool },
    Trust(PathBuf),
    Help,
}

/// Parse the arguments after the program name; `None` is a usage error.
pub fn parse(args: &[String]) -> Option<Command> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["-h" | "--help"] | ["secureboot", "-h" | "--help"] => Some(Command::Help),
        ["secureboot", "enroll"] => Some(Command::Enroll {
            without_microsoft: false,
        }),
        ["secureboot", "enroll", "--without-microsoft"] => Some(Command::Enroll {
            without_microsoft: true,
        }),
        ["secureboot", "trust", cert] if !cert.is_empty() => {
            Some(Command::Trust(PathBuf::from(cert)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Option<Command> {
        parse(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn the_secureboot_group_parses() {
        assert_eq!(
            parse_args(&["secureboot", "enroll"]),
            Some(Command::Enroll {
                without_microsoft: false
            })
        );
        assert_eq!(
            parse_args(&["secureboot", "enroll", "--without-microsoft"]),
            Some(Command::Enroll {
                without_microsoft: true
            })
        );
        assert_eq!(
            parse_args(&["secureboot", "trust", "/c/a.der"]),
            Some(Command::Trust("/c/a.der".into()))
        );
        assert_eq!(parse_args(&["--help"]), Some(Command::Help));
        assert_eq!(parse_args(&["secureboot", "-h"]), Some(Command::Help));
    }

    #[test]
    fn anything_else_is_a_usage_error() {
        for args in [
            &[][..],
            &["update"],
            &["secureboot"],
            &["secureboot", "enroll", "extra"],
            &["secureboot", "trust"],
            &["secureboot", "trust", ""],
            &["secureboot", "trust", "a", "b"],
            &["secureboot", "frob"],
        ] {
            assert_eq!(parse_args(args), None, "{args:?}");
        }
    }
}
