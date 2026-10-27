//! The command line: `tetsu-comp [--backend winit|tty|headless] [-- cmd...]`.

use std::ffi::OsString;
use std::fmt;

/// Usage text.
pub const USAGE: &str = "usage: tetsu-comp [--backend winit|tty|headless] [-- <command>...]

  --backend   winit (nested in the host session), tty (a VT) or headless
              (no output device; for tests). Default: winit when
              WAYLAND_DISPLAY or DISPLAY is set, else tty.
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
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownBackend(name) => write!(f, "unknown backend {name:?}"),
            Self::MissingValue(option) => write!(f, "{option} needs a value"),
            Self::UnknownOption(option) => write!(f, "unknown argument {option:?}"),
        }
    }
}

impl std::error::Error for CliError {}

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
    let mut args = args.into_iter();
    let mut command = Vec::new();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();
        match text.as_str() {
            "--" => {
                command = args.by_ref().collect();
            }
            "-h" | "--help" => return Ok(Parsed::Help),
            "--backend" => {
                let value = args.next().ok_or(CliError::MissingValue("--backend"))?;
                backend = Some(backend_named(&value.to_string_lossy())?);
            }
            other => match other.strip_prefix("--backend=") {
                Some(name) => backend = Some(backend_named(name)?),
                None => return Err(CliError::UnknownOption(text)),
            },
        }
    }
    let backend = backend.unwrap_or(if hosted { Backend::Winit } else { Backend::Tty });
    Ok(Parsed::Run(Cli { backend, command }))
}
