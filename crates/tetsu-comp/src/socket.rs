//! Where the bus socket lives.

use std::io;
use std::path::{Path, PathBuf};

/// The environment variable naming the compositor's bus socket.
pub const COMP_SOCKET_ENV: &str = "TETSU_COMP_SOCKET";

/// `$XDG_RUNTIME_DIR/tetsu/comp/<wayland socket name>`: one socket per
/// compositor instance, so a nested and a real session do not collide.
pub fn bus_socket_path(runtime_dir: Option<&Path>, wayland_socket: &str) -> io::Result<PathBuf> {
    let invalid = |what: &str| io::Error::new(io::ErrorKind::InvalidInput, what.to_owned());
    let runtime_dir = match runtime_dir {
        Some(dir) if dir.is_absolute() => dir,
        Some(_) => return Err(invalid("XDG_RUNTIME_DIR is not an absolute path")),
        None => return Err(invalid("XDG_RUNTIME_DIR is not set")),
    };
    if wayland_socket.is_empty() || wayland_socket.contains('/') {
        return Err(invalid("the Wayland socket name is not a file name"));
    }
    Ok(runtime_dir.join("tetsu/comp").join(wayland_socket))
}
