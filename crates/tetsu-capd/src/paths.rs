//! Where the daemon and the command find their files.

use std::path::PathBuf;

use tetsu_bus::cap;

use crate::audit::JOURNAL_SOCKET;
use crate::store::Layers;

/// Where the image layer is installed.
pub const IMAGE_FILE: &str = "/usr/share/tetsu/capabilities.toml";

/// An environment path, else `fallback()`.
pub fn env_path(
    name: &str,
    fallback: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    match std::env::var_os(name).filter(|v| !v.is_empty()) {
        Some(value) => Ok(PathBuf::from(value)),
        None => fallback(),
    }
}

fn home_dir(under: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(under))
}

/// `$XDG_RUNTIME_DIR`.
pub fn runtime_dir() -> Result<PathBuf, String> {
    env_path("XDG_RUNTIME_DIR", || {
        Err("XDG_RUNTIME_DIR is not set".to_owned())
    })
}

/// The socket of the capability service.
pub fn socket() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join("tetsu").join(cap::INTERFACE))
}

/// The journal socket, overridable for tests.
pub fn journal() -> Result<PathBuf, String> {
    env_path("TETSU_CAPD_JOURNAL_SOCKET", || {
        Ok(PathBuf::from(JOURNAL_SOCKET))
    })
}

/// The three layer files.
pub fn layers() -> Result<Layers, String> {
    let state = env_path("XDG_STATE_HOME", || home_dir(".local/state"))?;
    let config = env_path("XDG_CONFIG_HOME", || home_dir(".config"))?;
    Ok(Layers {
        image: env_path("TETSU_CAPD_IMAGE_FILE", || Ok(PathBuf::from(IMAGE_FILE)))?,
        managed: state.join("tetsu/capabilities.toml"),
        user: config.join("tetsu/capabilities.toml"),
    })
}
