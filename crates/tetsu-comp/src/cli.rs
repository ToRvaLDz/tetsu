//! The command line: `tetsu-comp [--backend winit|tty|headless]
//! [--config <file>] [--headless-outputs <n>] [--headless-hotplug] [-- cmd...]`.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

/// Usage text.
pub const USAGE: &str = "usage: tetsu-comp [--backend winit|tty|headless] [--config <file>]
                  [--headless-outputs <1..4>] [--headless-hotplug]
                  [-- <command>...]

  --backend   winit (nested in the host session), tty (a VT) or headless
              (no output device; for tests). Default: winit when
              WAYLAND_DISPLAY or DISPLAY is set, else tty.
  --config    a TOML file that replaces the user file
              ($XDG_CONFIG_HOME/tetsu/comp.toml). Any problem in it stops
              the start with exit code 2; problems in the XDG user file
              are logged, published on org.tetsu.comp.config.v1 and the
              compositor starts. Either way the file is watched and
              changes apply live. The layers below it: the managed state
              ($XDG_STATE_HOME/tetsu/comp.toml, written by SetManaged)
              and /usr/share/tetsu/comp.toml, over the built-in defaults
              (dwindle, no gaps, no rules).
  --headless-outputs
              headless only: the number of virtual outputs, 1 to 4.
              Default: 1.
  --headless-hotplug
              headless only: read hotplug commands from stdin, one per
              line: `add [<w>x<h>]`, `add-named <name> <w>x<h>`,
              `add-modes <name> <w>x<h>[,<w>x<h>...]` and
              `remove <name>`. For tests.
  -- <cmd>    run <cmd> as the first client, with WAYLAND_DISPLAY and
              TETSU_COMP_SOCKET set.";

/// Where the compositor draws and reads input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// A window of the host session.
    Winit,
    /// DRM/KMS and libinput on a VT.
    Tty,
    /// A virtual output, nothing drawn.
    Headless,
}

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// The backend.
    pub backend: Backend,
    /// The config file, if any.
    pub config: Option<PathBuf>,
    /// Virtual outputs of the headless backend, 1..=4.
    pub headless_outputs: u8,
    /// Whether the headless backend reads hotplug commands from stdin.
    pub headless_hotplug: bool,
    /// The first client and its arguments, empty for none.
    pub command: Vec<OsString>,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// Run the compositor.
    Run(Cli),
    /// Print the usage and exit.
    Help,
}

/// A bad command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// `--backend` with a name that is none of the three.
    UnknownBackend(String),
    /// An option that needs a value had none.
    MissingValue(&'static str),
    /// An option or argument nobody asked for.
    UnknownOption(String),
    /// A number that is not in range: the option and the text it got.
    BadNumber(&'static str, String),
    /// `--headless-outputs` or `--headless-hotplug` with a backend other
    /// than headless.
    HeadlessOnly,
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBackend(name) => write!(f, "unknown backend {name:?}"),
            Self::MissingValue(option) => write!(f, "{option} needs a value"),
            Self::UnknownOption(option) => write!(f, "unknown argument {option:?}"),
            Self::BadNumber(option, text) => {
                write!(
                    f,
                    "{option} wants a number from 1 to {MAX_OUTPUTS}, not {text:?}"
                )
            }
            Self::HeadlessOnly => write!(
                f,
                "--headless-outputs and --headless-hotplug need --backend headless"
            ),
        }
    }
}

impl std::error::Error for CliError {}

/// Most virtual outputs the headless backend adds.
pub const MAX_OUTPUTS: u8 = 4;

fn outputs_named(text: &str) -> Result<u8, CliError> {
    match text.parse::<u8>() {
        Ok(n) if (1..=MAX_OUTPUTS).contains(&n) => Ok(n),
        _ => Err(CliError::BadNumber("--headless-outputs", text.to_owned())),
    }
}

fn backend_named(name: &str) -> Result<Backend, CliError> {
    match name {
        "winit" => Ok(Backend::Winit),
        "tty" => Ok(Backend::Tty),
        "headless" => Ok(Backend::Headless),
        other => Err(CliError::UnknownBackend(other.to_owned())),
    }
}

/// Parses `args` (without the program name). `hosted` tells whether a host
/// session is around (`WAYLAND_DISPLAY` or `DISPLAY` is set).
pub fn parse(args: impl IntoIterator<Item = OsString>, hosted: bool) -> Result<Parsed, CliError> {
    let mut backend = None;
    let mut config = None;
    let mut outputs = None;
    let mut hotplug = false;
    let mut args = args.into_iter();
    let mut command = Vec::new();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();
        let (name, inline) = match text.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_owned())),
            _ => (text.as_str(), None),
        };
        let mut value = |option: &'static str| -> Result<String, CliError> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => args
                    .next()
                    .map(|v| v.to_string_lossy().into_owned())
                    .ok_or(CliError::MissingValue(option)),
            }
        };
        match name {
            "--" => command = args.by_ref().collect(),
            "-h" | "--help" => return Ok(Parsed::Help),
            "--backend" => backend = Some(backend_named(&value("--backend")?)?),
            "--config" => config = Some(PathBuf::from(value("--config")?)),
            "--headless-outputs" => {
                outputs = Some(outputs_named(&value("--headless-outputs")?)?);
            }
            "--headless-hotplug" if inline.is_none() => hotplug = true,
            _ => return Err(CliError::UnknownOption(text)),
        }
    }
    let backend = backend.unwrap_or(if hosted { Backend::Winit } else { Backend::Tty });
    if (outputs.is_some() || hotplug) && backend != Backend::Headless {
        return Err(CliError::HeadlessOnly);
    }
    Ok(Parsed::Run(Cli {
        backend,
        config,
        headless_outputs: outputs.unwrap_or(1),
        headless_hotplug: hotplug,
        command,
    }))
}
