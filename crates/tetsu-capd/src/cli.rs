//! `tetsu cap`: list and change grants through `org.tetsu.cap.v1`.

use std::io::{self, Write};
use std::process::ExitCode;

use tetsu_bus::cap::{self, Capability, ClientError, Grant, Layer, SetParams, State};

use crate::catalog;
use crate::paths;
use crate::store::Store;

/// The usage text.
pub const USAGE: &str = "\
usage: tetsu cap list
       tetsu cap set <class> <automatic|notify|ask|off> [--scope <scope>]
       tetsu cap revoke <class> [--scope <scope>]";

/// A parsed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `-h`.
    Help,
    /// `list`.
    List,
    /// `set`.
    Set {
        /// The class.
        class: String,
        /// The scope.
        scope: Option<String>,
        /// The new state.
        state: Grant,
    },
    /// `revoke`: removes the managed value.
    Revoke {
        /// The class.
        class: String,
        /// The scope.
        scope: Option<String>,
    },
}

/// Parses the arguments after `cap`. The error is the usage message.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let usage = |why: &str| Err(format!("tetsu: {why}\n{USAGE}"));
    let strs: Vec<&str> = args.iter().map(String::as_str).collect();
    match strs.as_slice() {
        ["-h" | "--help"] => Ok(Command::Help),
        ["list"] => Ok(Command::List),
        ["set", class, state, rest @ ..] => {
            let Some(state) = grant(state) else {
                return usage(&format!("`{state}` is not automatic, notify, ask or off"));
            };
            match scope(rest) {
                Ok(scope) => Ok(Command::Set {
                    class: (*class).to_owned(),
                    scope,
                    state,
                }),
                Err(why) => usage(why),
            }
        }
        ["revoke", class, rest @ ..] => match scope(rest) {
            Ok(scope) => Ok(Command::Revoke {
                class: (*class).to_owned(),
                scope,
            }),
            Err(why) => usage(why),
        },
        [] => usage("no command"),
        [other, ..] => usage(&format!("unknown command `{other}`")),
    }
}

fn grant(text: &str) -> Option<Grant> {
    [Grant::Automatic, Grant::Notify, Grant::Ask, Grant::Off]
        .into_iter()
        .find(|g| g.as_str() == text)
}

/// The optional trailing `--scope <scope>`.
fn scope(rest: &[&str]) -> Result<Option<String>, &'static str> {
    match rest {
        [] => Ok(None),
        ["--scope", scope] => Ok(Some((*scope).to_owned())),
        ["--scope"] => Err("--scope needs a value"),
        _ => Err("unexpected arguments"),
    }
}

/// What the command needs from the service.
pub trait CapApi {
    /// `ListCapabilities`.
    fn list(&mut self) -> impl Future<Output = Result<Vec<Capability>, ClientError>>;

    /// `Set`.
    fn set(&mut self, params: &SetParams) -> impl Future<Output = Result<(), ClientError>>;
}

impl CapApi for cap::Client {
    async fn list(&mut self) -> Result<Vec<Capability>, ClientError> {
        cap::Client::list(self).await
    }

    async fn set(&mut self, params: &SetParams) -> Result<(), ClientError> {
        cap::Client::set(self, params).await
    }
}

/// Runs a command; returns the exit status (0 done, 1 failed).
pub async fn run<A: CapApi>(
    command: Command,
    api: &mut A,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    match command {
        Command::Help => {
            let _ = writeln!(out, "{USAGE}");
            0
        }
        Command::List => match api.list().await {
            Ok(caps) => {
                let _ = out.write_all(table(&caps).as_bytes());
                0
            }
            Err(e) => fail(err, &e, None),
        },
        Command::Set {
            class,
            scope,
            state,
        } => {
            let params = SetParams {
                class: class.clone(),
                scope: scope.clone(),
                state: Some(state),
            };
            if let Err(e) = api.set(&params).await {
                return fail(err, &e, Some(&class));
            }
            let destructive = catalog::class(&class).is_some_and(|c| c.destructive);
            let mut line = format!(
                "{}: {} (managed)",
                name(&class, scope.as_deref()),
                state.as_str()
            );
            if destructive && matches!(state, Grant::Automatic | Grant::Notify) {
                line.push_str(&format!(
                    ". Note: {class} is destructive, so it still asks before every call."
                ));
            }
            let _ = writeln!(out, "{line}");
            0
        }
        Command::Revoke { class, scope } => {
            let params = SetParams {
                class: class.clone(),
                scope: scope.clone(),
                state: None,
            };
            if let Err(e) = api.set(&params).await {
                return fail(err, &e, Some(&class));
            }
            let _ = writeln!(
                out,
                "{}: managed value removed",
                name(&class, scope.as_deref())
            );
            0
        }
    }
}

fn name(class: &str, scope: Option<&str>) -> String {
    match scope {
        Some(scope) => format!("{class} [{scope}]"),
        None => class.to_owned(),
    }
}

/// Prints the error as one line and returns the exit status.
fn fail(err: &mut dyn Write, error: &ClientError, class: Option<&str>) -> u8 {
    let _ = writeln!(err, "tetsu: {}", describe(error, class));
    1
}

/// One line for an error of the service or of the socket.
pub fn describe(error: &ClientError, class: Option<&str>) -> String {
    let class = class.unwrap_or("this class");
    match error {
        ClientError::Io(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            "tetsu-capd is not running".to_owned()
        }
        ClientError::Io(e) => format!("cannot talk to tetsu-capd: {e}"),
        ClientError::Remote { error, parameters } => match error.as_str() {
            cap::OVERRIDDEN_BY_USER => format!(
                "{class} is set in ~/.config/tetsu/capabilities.toml, which overrides it; edit that file instead"
            ),
            cap::UNKNOWN_CLASS => format!("unknown class {class}"),
            cap::PERMISSION_DENIED => {
                "not allowed: the agent cannot change capabilities".to_owned()
            }
            "org.varlink.service.InvalidParameter" => {
                let parameter = parameters["parameter"].as_str().unwrap_or("a parameter");
                format!(
                    "invalid {parameter} (is a capabilities file malformed? see the log of tetsu-capd)"
                )
            }
            other => other.to_owned(),
        },
    }
}

fn state_name(state: State) -> &'static str {
    match state {
        State::Unset => "unset",
        State::Automatic => "automatic",
        State::Notify => "notify",
        State::Ask => "ask",
        State::Off => "off",
        State::Unknown => "?",
    }
}

fn layer_name(layer: Option<Layer>) -> &'static str {
    match layer {
        None => "-",
        Some(Layer::Image) => "image",
        Some(Layer::Managed) => "managed",
        Some(Layer::User) => "user",
        Some(Layer::Unknown) => "?",
    }
}

/// The list as an aligned table, one row per capability.
fn table(caps: &[Capability]) -> String {
    let mut rows = vec![
        [
            "CLASS",
            "SCOPE",
            "STATE",
            "LAYER",
            "D",
            "ASKS",
            "APPR",
            "DENY",
            "SUPP",
            "LAST USED",
            "SUPPRESSED UNTIL",
        ]
        .map(str::to_owned),
    ];
    for c in caps {
        rows.push([
            c.class.clone(),
            c.scope.clone().unwrap_or_else(|| "-".into()),
            state_name(c.state).to_owned(),
            layer_name(c.layer).to_owned(),
            if c.destructive { "!" } else { "" }.to_owned(),
            c.asks.to_string(),
            c.approvals.to_string(),
            c.denials.to_string(),
            c.suppressions.to_string(),
            c.last_used_at.clone().unwrap_or_else(|| "-".into()),
            c.suppressed_until_at.clone().unwrap_or_else(|| "-".into()),
        ]);
    }
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    let mut text = String::new();
    for row in &rows {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        text.push_str(line.join("  ").trim_end());
        text.push('\n');
    }
    text
}

/// `tetsu cap …`: connects to the daemon of this session and runs.
pub fn main(args: impl IntoIterator<Item = String>) -> ExitCode {
    let args: Vec<String> = args.into_iter().collect();
    let command = match parse(&args) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
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
    let listing = command == Command::List;
    let code = runtime.block_on(async {
        let socket = match paths::socket() {
            Ok(socket) => socket,
            Err(why) => {
                eprintln!("tetsu: {why}");
                return 1;
            }
        };
        let mut client = match cap::Client::connect(&socket).await {
            Ok(client) => client,
            Err(e) => {
                eprintln!("tetsu: {}", describe(&e, None));
                return 1;
            }
        };
        let (mut out, mut err) = (io::stdout(), io::stderr());
        run(command, &mut client, &mut out, &mut err).await
    });
    if listing {
        warn_if_malformed();
    }
    ExitCode::from(code)
}

/// A layer that does not parse makes the daemon fail closed: say so.
fn warn_if_malformed() {
    let Ok(layers) = paths::layers() else { return };
    if let Err(e) = Store::new(layers).refresh() {
        eprintln!(
            "tetsu: warning: {e}\ntetsu: tetsu-capd denies state-changing calls until that is fixed"
        );
    }
}
