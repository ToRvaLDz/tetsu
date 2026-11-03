//! Command line of `tetsu`.

use std::path::PathBuf;

pub const USAGE: &str = "\
usage: tetsu secureboot enroll [--without-microsoft]
       tetsu secureboot trust <cert>
       tetsu cap list
       tetsu cap set <class> <automatic|notify|ask|off> [--scope <scope>]
       tetsu cap revoke <class> [--scope <scope>]
       tetsu comp config [--json]
       tetsu secrets add|rotate|rm|list|log ...

  secureboot enroll   enroll the Secure Boot keys of /var/lib/tetsu/keys
                      together with the Microsoft certificates (the
                      firmware must be in Setup Mode); refused when the
                      certificates are missing, unless --without-microsoft
                      says Tetsu's keys alone are meant
  secureboot trust    trust one more certificate in the db: a .der, .crt or
                      .pem certificate, or a pre-signed .auth file
  comp config         show the effective compositor config: every key with
                      its value, origin and writability, then the problems
                      of the user file; --json prints the wire reply

  secrets             the secrets vault: add, rotate and rm read the value
                      from a hidden prompt or from stdin; list shows names
                      and bindings, never values; log shows the audit
                      trail (`tetsu secrets` alone prints its usage)

exit status: 0 done, 1 failed, 2 usage error";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Enroll {
        without_microsoft: bool,
    },
    Trust(PathBuf),
    Cap(Vec<String>),
    Comp(Vec<String>),
    /// The arguments after `secrets`, run by `tetsu-secrets`.
    Secrets(Vec<String>),
    Help,
}

/// Parse the arguments after the program name; `None` is a usage error.
pub fn parse(args: &[String]) -> Option<Command> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["-h" | "--help"] | ["secureboot", "-h" | "--help"] => Some(Command::Help),
        ["secrets", rest @ ..] => Some(Command::Secrets(
            rest.iter().map(|arg| (*arg).to_owned()).collect(),
        )),
        ["secureboot", "enroll"] => Some(Command::Enroll {
            without_microsoft: false,
        }),
        ["secureboot", "enroll", "--without-microsoft"] => Some(Command::Enroll {
            without_microsoft: true,
        }),
        ["secureboot", "trust", cert] if !cert.is_empty() => {
            Some(Command::Trust(PathBuf::from(cert)))
        }
        ["cap", rest @ ..] => Some(Command::Cap(rest.iter().map(|s| (*s).to_owned()).collect())),
        ["comp", rest @ ..] => Some(Command::Comp(
            rest.iter().map(|s| (*s).to_owned()).collect(),
        )),
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
    fn cap_forwards_its_arguments() {
        assert_eq!(
            parse_args(&["cap", "set", "a.b", "ask"]),
            Some(Command::Cap(vec!["set".into(), "a.b".into(), "ask".into()]))
        );
        assert_eq!(parse_args(&["cap"]), Some(Command::Cap(vec![])));
    }

    #[test]
    fn comp_forwards_its_arguments() {
        assert_eq!(
            parse_args(&["comp", "config", "--json"]),
            Some(Command::Comp(vec!["config".into(), "--json".into()]))
        );
        assert_eq!(parse_args(&["comp"]), Some(Command::Comp(vec![])));
    }

    #[test]
    fn the_secrets_group_is_forwarded_whole() {
        assert_eq!(
            parse_args(&["secrets", "list"]),
            Some(Command::Secrets(vec!["list".into()]))
        );
        assert_eq!(
            parse_args(&["secrets", "add", "demo", "--kind", "bearer"]),
            Some(Command::Secrets(
                ["add", "demo", "--kind", "bearer"].map(String::from).into()
            ))
        );
        assert_eq!(parse_args(&["secrets"]), Some(Command::Secrets(vec![])));
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
