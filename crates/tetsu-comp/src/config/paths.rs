//! Where the three layers live.

use std::ffi::OsString;
use std::path::PathBuf;

/// The image defaults, laid over the built-in values.
pub const DEFAULTS_FILE: &str = "/usr/share/tetsu/comp.toml";
/// Overrides [`DEFAULTS_FILE`], for tests.
pub const DEFAULTS_ENV: &str = "TETSU_COMP_DEFAULTS_FILE";
const FILE: &str = "tetsu/comp.toml";

/// The file of each layer. A layer whose home is unknown has no path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// The defaults layer.
    pub defaults: PathBuf,
    /// The managed layer: `$XDG_STATE_HOME/tetsu/comp.toml`.
    pub managed: Option<PathBuf>,
    /// The user layer: `$XDG_CONFIG_HOME/tetsu/comp.toml`.
    pub user: Option<PathBuf>,
}

/// An absolute path from the variable, else `$HOME/<fallback>`.
fn base(env: &impl Fn(&str) -> Option<OsString>, var: &str, fallback: &str) -> Option<PathBuf> {
    let absolute = |name: &str| {
        env(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute(var).or_else(|| absolute("HOME").map(|home| home.join(fallback)))
}

impl Paths {
    /// The paths for an environment, given as a lookup.
    pub fn from_env(env: impl Fn(&str) -> Option<OsString>) -> Self {
        let defaults = env(DEFAULTS_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULTS_FILE));
        Self {
            defaults,
            managed: base(&env, "XDG_STATE_HOME", ".local/state").map(|dir| dir.join(FILE)),
            user: base(&env, "XDG_CONFIG_HOME", ".config").map(|dir| dir.join(FILE)),
        }
    }

    /// The paths of the process environment.
    pub fn from_process_env() -> Self {
        Self::from_env(|name| std::env::var_os(name))
    }

    /// The same layers with the user layer at `path` (`--config`).
    #[must_use]
    pub fn with_user(self, path: PathBuf) -> Self {
        Self {
            user: Some(path),
            ..self
        }
    }
}
