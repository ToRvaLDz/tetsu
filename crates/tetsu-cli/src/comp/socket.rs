//! Finding the compositor's bus socket.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};

/// The variable that names the socket outright.
pub const ENV_SOCKET: &str = "TETSU_COMP_SOCKET";

/// Where the compositor keeps its sockets, below `XDG_RUNTIME_DIR`. The
/// layout is the compositor's; it is repeated here because this crate does
/// not depend on it.
const COMP_DIR: [&str; 2] = ["tetsu", "comp"];

/// The environment discovery reads, made explicit so tests can set it.
#[derive(Debug, Default, Clone)]
pub struct SocketEnv {
    /// `TETSU_COMP_SOCKET`.
    pub comp_socket: Option<OsString>,
    /// `XDG_RUNTIME_DIR`.
    pub runtime_dir: Option<OsString>,
    /// `WAYLAND_DISPLAY`.
    pub wayland_display: Option<OsString>,
}

impl SocketEnv {
    /// The environment of this process.
    pub fn from_process() -> Self {
        Self {
            comp_socket: std::env::var_os(ENV_SOCKET),
            runtime_dir: std::env::var_os("XDG_RUNTIME_DIR"),
            wayland_display: std::env::var_os("WAYLAND_DISPLAY"),
        }
    }
}

/// Why no socket was found.
#[derive(Debug, PartialEq, Eq)]
pub enum Discovery {
    /// There is no usable `XDG_RUNTIME_DIR`.
    NoRuntimeDir,
    /// The comp directory holds no socket.
    NotRunning(PathBuf),
    /// It holds several, none of them picked out; the names, sorted.
    Ambiguous(Vec<String>),
}

impl fmt::Display for Discovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoRuntimeDir => write!(f, "XDG_RUNTIME_DIR is not set; set {ENV_SOCKET}"),
            Self::NotRunning(dir) => write!(
                f,
                "tetsu-comp is not running (no socket in {}); \
                 set {ENV_SOCKET} if it listens elsewhere",
                dir.display()
            ),
            Self::Ambiguous(names) => write!(
                f,
                "several compositors are running ({}): \
                 set {ENV_SOCKET} to the one to inspect",
                names.join(", ")
            ),
        }
    }
}

/// Whether `path` is a Unix socket (not followed through a symlink).
fn is_socket(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// A display name that is a single path component.
fn plain_name(name: &OsStr) -> bool {
    !name.is_empty() && !name.as_encoded_bytes().contains(&b'/')
}

/// The sockets of the comp directory, by sorted name.
fn scan(dir: &Path) -> Vec<String> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = read
        .filter_map(Result::ok)
        .filter(|e| is_socket(&e.path()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The socket to talk to: `TETSU_COMP_SOCKET`; else the socket named like
/// `WAYLAND_DISPLAY` in the comp directory; else the only socket there.
pub fn discover(env: &SocketEnv) -> Result<PathBuf, Discovery> {
    if let Some(path) = env.comp_socket.as_ref().filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let runtime = env
        .runtime_dir
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or(Discovery::NoRuntimeDir)?;
    let dir = COMP_DIR.iter().fold(runtime, |dir, part| dir.join(part));
    if let Some(display) = env.wayland_display.as_deref().filter(|d| plain_name(d)) {
        let path = dir.join(display);
        if is_socket(&path) {
            return Ok(path);
        }
    }
    let mut names = scan(&dir);
    match names.len() {
        0 => Err(Discovery::NotRunning(dir)),
        1 => Ok(dir.join(names.remove(0))),
        _ => Err(Discovery::Ambiguous(names)),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;
    use std::path::Path;

    use super::*;

    struct Rig {
        dir: tempfile::TempDir,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
            }
        }

        fn comp_dir(&self) -> PathBuf {
            self.dir.path().join("tetsu").join("comp")
        }

        /// Binds a real socket in the comp directory.
        fn socket(&self, name: &str) -> (PathBuf, UnixListener) {
            std::fs::create_dir_all(self.comp_dir()).unwrap();
            let path = self.comp_dir().join(name);
            let listener = UnixListener::bind(&path).unwrap();
            (path, listener)
        }

        fn env(&self, display: Option<&str>) -> SocketEnv {
            SocketEnv {
                comp_socket: None,
                runtime_dir: Some(self.dir.path().into()),
                wayland_display: display.map(OsString::from),
            }
        }
    }

    #[test]
    fn the_env_var_wins_over_the_comp_dir() {
        let rig = Rig::new();
        let _one = rig.socket("wayland-1");
        let mut env = rig.env(Some("wayland-1"));
        env.comp_socket = Some("/elsewhere/comp".into());
        assert_eq!(discover(&env), Ok(PathBuf::from("/elsewhere/comp")));
    }

    #[test]
    fn an_empty_env_var_counts_as_unset() {
        let rig = Rig::new();
        let (path, _l) = rig.socket("wayland-1");
        let mut env = rig.env(None);
        env.comp_socket = Some("".into());
        assert_eq!(discover(&env), Ok(path));
    }

    #[test]
    fn the_runtime_dir_must_be_set_and_absolute() {
        let mut env = SocketEnv::default();
        assert_eq!(discover(&env), Err(Discovery::NoRuntimeDir));
        env.runtime_dir = Some("run/user/1000".into());
        assert_eq!(discover(&env), Err(Discovery::NoRuntimeDir));
        env.runtime_dir = Some("".into());
        assert_eq!(discover(&env), Err(Discovery::NoRuntimeDir));
    }

    #[test]
    fn the_env_var_needs_no_runtime_dir() {
        let env = SocketEnv {
            comp_socket: Some("/s".into()),
            ..SocketEnv::default()
        };
        assert_eq!(discover(&env), Ok(PathBuf::from("/s")));
    }

    #[test]
    fn the_wayland_display_picks_among_several() {
        let rig = Rig::new();
        let _a = rig.socket("wayland-0");
        let (b, _lb) = rig.socket("wayland-1");
        assert_eq!(discover(&rig.env(Some("wayland-1"))), Ok(b));
    }

    #[test]
    fn a_display_with_a_slash_or_without_a_socket_falls_back_to_the_scan() {
        let rig = Rig::new();
        let (only, _l) = rig.socket("wayland-0");
        for display in ["../wayland-0", "a/b", "wayland-9", ""] {
            assert_eq!(
                discover(&rig.env(Some(display))),
                Ok(only.clone()),
                "{display}"
            );
        }
    }

    #[test]
    fn a_single_socket_is_found_without_a_display() {
        let rig = Rig::new();
        let (only, _l) = rig.socket("wayland-0");
        assert_eq!(discover(&rig.env(None)), Ok(only));
    }

    #[test]
    fn a_regular_file_is_ignored() {
        let rig = Rig::new();
        let (only, _l) = rig.socket("wayland-0");
        std::fs::write(rig.comp_dir().join("notes"), "x").unwrap();
        assert_eq!(discover(&rig.env(None)), Ok(only.clone()));
        // A regular file named by the display is no socket either.
        std::fs::write(rig.comp_dir().join("wayland-5"), "x").unwrap();
        assert_eq!(discover(&rig.env(Some("wayland-5"))), Ok(only));
    }

    #[test]
    fn no_comp_dir_or_no_socket_is_not_running() {
        let rig = Rig::new();
        let expected = Err(Discovery::NotRunning(rig.comp_dir()));
        assert_eq!(discover(&rig.env(None)), expected);
        std::fs::create_dir_all(rig.comp_dir()).unwrap();
        std::fs::write(rig.comp_dir().join("file"), "x").unwrap();
        assert_eq!(discover(&rig.env(None)), expected);
    }

    #[test]
    fn two_sockets_are_ambiguous_with_sorted_names() {
        let rig = Rig::new();
        let _b = rig.socket("wayland-1");
        let _a = rig.socket("wayland-0");
        assert_eq!(
            discover(&rig.env(None)),
            Err(Discovery::Ambiguous(vec![
                "wayland-0".into(),
                "wayland-1".into()
            ]))
        );
    }

    #[test]
    fn the_messages_tell_what_to_do() {
        let dir = Path::new("/run/user/1/tetsu/comp");
        let not_running = Discovery::NotRunning(dir.into()).to_string();
        assert_eq!(
            not_running,
            "tetsu-comp is not running (no socket in /run/user/1/tetsu/comp); \
             set TETSU_COMP_SOCKET if it listens elsewhere"
        );
        let several = Discovery::Ambiguous(vec!["a".into(), "b".into()]).to_string();
        assert_eq!(
            several,
            "several compositors are running (a, b): \
             set TETSU_COMP_SOCKET to the one to inspect"
        );
        assert_eq!(
            Discovery::NoRuntimeDir.to_string(),
            "XDG_RUNTIME_DIR is not set; set TETSU_COMP_SOCKET"
        );
    }
}
