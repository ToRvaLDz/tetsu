//! The satellite policy, free of any state: which client is Xwayland, what
//! its windows report, and the commands that probe and start it.

use std::ffi::OsStr;
use std::os::fd::RawFd;
use std::process::{Command, Stdio};

use crate::trust::Trust;

/// The trust of a client that came through the listening socket: Xwayland
/// when its pid is the live satellite's, else untrusted.
pub fn peer_trust(peer: Option<u32>, satellite: Option<u32>) -> Trust {
    match (peer, satellite) {
        (Some(peer), Some(satellite)) if peer == satellite => Trust::Xwayland,
        _ => Trust::Untrusted,
    }
}

/// The pid and the `xwayland` flag a window of a client of `trust` reports:
/// an X11 window has no pid (satellite's would mislead).
pub fn window_origin(trust: Trust, pid: Option<i64>) -> (Option<i64>, bool) {
    match trust {
        Trust::Xwayland => (None, true),
        _ => (pid, false),
    }
}

/// Starts satellite on `display` with its listening sockets: it connects to
/// `wayland` and must not find an X server or a Wayland socket pair.
pub fn satellite_command(
    path: impl AsRef<OsStr>,
    display: &str,
    fds: &[RawFd],
    wayland: &OsStr,
) -> Command {
    let mut command = quiet(path);
    command.arg(display);
    for fd in fds {
        command.arg("-listenfd").arg(fd.to_string());
    }
    command
        .env("WAYLAND_DISPLAY", wayland)
        .env_remove("WAYLAND_SOCKET");
    command
}

/// Asks satellite whether it can take listening sockets (`-listenfd`).
pub fn probe_command(path: impl AsRef<OsStr>) -> Command {
    let mut command = quiet(path);
    command.args([":0", "--test-listenfd-support"]);
    command
}

/// `path` without stdio, an X server or backtraces.
fn quiet(path: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(path);
    command
        .env_remove("DISPLAY")
        .env_remove("RUST_BACKTRACE")
        .env_remove("RUST_LIB_BACKTRACE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}
