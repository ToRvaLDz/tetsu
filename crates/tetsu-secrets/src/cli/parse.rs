//! The command line of `tetsu secrets`.

use crate::model::{Host, KindSpec, SecretId, ToolId};

/// The usage text.
pub const USAGE: &str = "usage:
  tetsu secrets add <id> --kind bearer|header:<name>|basic --host <host>[:port] [--allow <tool>]...
  tetsu secrets rotate <id>
  tetsu secrets rm <id>
  tetsu secrets list
  tetsu secrets log [-f] [-n <lines>]

The value is read from a hidden prompt, or from stdin when it is piped.";

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Add a secret.
    Add {
        /// The name.
        id: SecretId,
        /// How the value is presented.
        kind: KindSpec,
        /// The only host it may go to.
        host: Host,
        /// Tool ids that may receive it.
        allow: Vec<ToolId>,
    },
    /// Replace a value.
    Rotate {
        /// The name.
        id: SecretId,
    },
    /// Delete a secret.
    Rm {
        /// The name.
        id: SecretId,
    },
    /// List the secrets.
    List,
    /// Show the audit trail.
    Log {
        /// Keep following.
        follow: bool,
        /// How many lines.
        lines: Option<u32>,
    },
}

/// Parses the arguments after `secrets`; an error is a usage message.
///
/// A message never repeats an argument: the user may have typed a value on
/// the command line by mistake, and it must not be echoed.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let (name, rest) = args.split_first().ok_or("missing command")?;
    match name.as_str() {
        "add" => parse_add(rest),
        "rotate" => Ok(Command::Rotate { id: one_id(rest)? }),
        "rm" | "remove" => Ok(Command::Rm { id: one_id(rest)? }),
        "list" if rest.is_empty() => Ok(Command::List),
        "list" => Err("list takes no arguments".to_owned()),
        "log" => parse_log(rest),
        _ => Err("unknown command".to_owned()),
    }
}

fn one_id(rest: &[String]) -> Result<SecretId, String> {
    match rest {
        [id] if !id.starts_with('-') => SecretId::parse(id).map_err(|e| e.to_string()),
        [] => Err("missing the secret id".to_owned()),
        _ => Err("expected one secret id and nothing else".to_owned()),
    }
}

/// Splits `--name=value` into its name and value.
fn split_option(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((name, value)) if name.starts_with("--") => (name, Some(value)),
        _ => (arg, None),
    }
}

/// The value of an option: after `=`, or the next argument.
fn option_value<'a>(
    name: &str,
    inline: Option<&'a str>,
    rest: &mut std::slice::Iter<'a, String>,
) -> Result<&'a str, String> {
    match inline {
        Some(value) => Ok(value),
        None => rest
            .next()
            .map(String::as_str)
            .ok_or_else(|| format!("{name} needs a value")),
    }
}

fn parse_add(rest: &[String]) -> Result<Command, String> {
    let mut id = None;
    let (mut kind, mut host) = (None, None);
    let mut allow = Vec::new();
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        let (name, inline) = split_option(arg);
        match name {
            "--kind" => kind = Some(KindSpec::parse(option_value(name, inline, &mut iter)?)),
            "--host" => host = Some(Host::parse(option_value(name, inline, &mut iter)?)),
            "--allow" => {
                for tool in option_value(name, inline, &mut iter)?.split(',') {
                    allow.push(ToolId::parse(tool).map_err(|e| e.to_string())?);
                }
            }
            _ if name.starts_with('-') => return Err(format!("unknown option {name}")),
            _ if id.is_none() => id = Some(SecretId::parse(arg).map_err(|e| e.to_string())?),
            _ => return Err("unexpected argument".to_owned()),
        }
    }
    Ok(Command::Add {
        id: id.ok_or("missing the secret id")?,
        kind: kind.ok_or("missing --kind")?.map_err(|e| e.to_string())?,
        host: host.ok_or("missing --host")?.map_err(|e| e.to_string())?,
        allow,
    })
}

fn parse_log(rest: &[String]) -> Result<Command, String> {
    let (mut follow, mut lines) = (false, None);
    let mut iter = rest.iter();
    while let Some(arg) = iter.next() {
        let (name, inline) = split_option(arg);
        match (name, inline) {
            ("-f" | "--follow", None) => follow = true,
            ("-n" | "--lines", _) => {
                let count = option_value(name, inline, &mut iter)?;
                lines = Some(count.parse().map_err(|_| "-n needs a number".to_owned())?);
            }
            _ if name.starts_with('-') => return Err(format!("unknown option {name}")),
            _ => return Err("unexpected argument".to_owned()),
        }
    }
    Ok(Command::Log { follow, lines })
}

/// The `journalctl` arguments that show the audit trail.
pub fn log_argv(follow: bool, lines: Option<u32>) -> Vec<String> {
    let mut argv: Vec<String> = ["--user", "-t", "tetsu-secrets", "-o", "short-iso"]
        .map(str::to_owned)
        .into();
    if follow {
        argv.push("-f".to_owned());
    }
    if let Some(lines) = lines {
        argv.push("-n".to_owned());
        argv.push(lines.to_string());
    }
    argv
}
