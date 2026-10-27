//! Who a Wayland client is, decided when it connects and fixed for its
//! lifetime. Only clients the compositor starts itself are trusted: it makes
//! a socket pair, keeps one end as the client and hands the other to the
//! child as `WAYLAND_SOCKET`. Everything that comes through the listening
//! socket is untrusted, and so are the children of a trusted client (they
//! find `WAYLAND_DISPLAY`, not the socket pair).

use std::io;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::Arc;

use smithay::reexports::rustix;
use smithay::reexports::wayland_server::Client;

use crate::socket::COMP_SOCKET_ENV;
use crate::state::{ClientState, State};

/// How much a client is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trust {
    /// Every client that comes through the listening socket, and any client
    /// whose trust is unknown.
    #[default]
    Untrusted,
    /// The session locker the compositor started: it may take over a
    /// defunct lock.
    Locker,
    /// A helper from `[trust] input`: it may bind the virtual input
    /// protocols.
    Input,
}

/// The trust of a connected client; a client without our data is untrusted.
pub fn trust_of(client: &Client) -> Trust {
    client
        .get_data::<ClientState>()
        .map_or(Trust::Untrusted, |data| data.trust)
}

/// Starts `argv` as a client of the compositor with `trust`.
///
/// The child end of a socket pair is passed as `WAYLAND_SOCKET`; the child
/// is reaped on a thread. Fails when `argv` is empty or the program cannot
/// start, and then no client was added.
pub fn spawn_trusted(state: &mut State, argv: &[String], trust: Trust) -> io::Result<()> {
    let Some((program, args)) = argv.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an empty command",
        ));
    };
    let (ours, theirs) = UnixStream::pair()?;
    // `dup` leaves the copy without close-on-exec: that is the one the child
    // inherits. The original stays close-on-exec, so no other child gets it.
    let inherited = rustix::io::dup(&theirs).map_err(io::Error::from)?;
    drop(theirs);
    let mut command = Command::new(program);
    command
        .args(args)
        .env("WAYLAND_SOCKET", rustix_fd(&inherited).to_string())
        .env("WAYLAND_DISPLAY", &state.socket_name)
        .stdin(Stdio::null());
    if let Some(bus) = &state.comp_socket {
        command.env(COMP_SOCKET_ENV, bus);
    }
    let mut child = command.spawn()?;
    // The child has its copy; ours must go so the socket closes with it.
    drop(inherited);
    state
        .display_handle
        .insert_client(ours, Arc::new(ClientState::with_trust(trust)))
        .map_err(io::Error::other)?;
    std::thread::Builder::new()
        .name("tetsu-comp-reap".to_owned())
        .spawn(move || {
            let _ = child.wait();
        })?;
    Ok(())
}

fn rustix_fd(fd: &std::os::fd::OwnedFd) -> i32 {
    std::os::fd::AsRawFd::as_raw_fd(fd)
}
