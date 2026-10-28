//! The compositor's sockets: the Wayland listening socket, where every
//! client that is not started by the compositor comes in, and where the bus
//! socket lives.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as PollMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::wayland::socket::ListeningSocketSource;

use crate::state::{ClientState, State};

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

/// Listens on a new `wayland-<n>` socket and dispatches the clients of
/// `display` on the loop. Returns the socket name.
pub fn listen(
    display: Display<State>,
    event_loop: &mut EventLoop<'static, State>,
) -> io::Result<OsString> {
    let listening = ListeningSocketSource::new_auto().map_err(io::Error::other)?;
    let socket_name = listening.socket_name().to_os_string();
    let handle = event_loop.handle();
    handle
        .insert_source(listening, |stream, (), state| {
            let data = Arc::new(ClientState::default());
            if let Err(e) = state.display_handle.insert_client(stream, data) {
                eprintln!("tetsu-comp: cannot accept a client: {e}");
            }
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
    handle
        .insert_source(
            Generic::new(display, Interest::READ, PollMode::Level),
            |_, display, state| {
                // SAFETY: the display is never dropped while the source lives.
                #[expect(unsafe_code)]
                let result = unsafe { display.get_mut().dispatch_clients(state) };
                match result {
                    Ok(_) => Ok(PostAction::Continue),
                    Err(e) => {
                        eprintln!("tetsu-comp: wayland dispatch failed: {e}");
                        Ok(PostAction::Remove)
                    }
                }
            },
        )
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(socket_name)
}
