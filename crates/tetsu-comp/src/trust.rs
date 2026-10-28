//! Who a Wayland client is, decided when it connects and fixed for its
//! lifetime. Only clients the compositor starts itself are trusted: it makes
//! a socket pair, keeps one end as the client and hands the other to the
//! child as `WAYLAND_SOCKET`. Everything that comes through the listening
//! socket is untrusted, and so are the children of a trusted client (they
//! find `WAYLAND_DISPLAY`, not the socket pair).

use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use smithay::reexports::rustix;
use smithay::reexports::rustix::process::{Pid, PidfdFlags, Signal};
use smithay::reexports::wayland_server::backend::DisconnectReason;
use smithay::reexports::wayland_server::{Client, DisplayHandle};

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
    /// The running xwayland-satellite, known by its pid when it connects:
    /// its windows are X11 windows. It gets no other privilege.
    Xwayland,
}

/// The trust of a connected client; a client without our data is untrusted.
pub fn trust_of(client: &Client) -> Trust {
    client
        .get_data::<ClientState>()
        .map_or(Trust::Untrusted, |data| data.trust)
}

/// A child `spawn_trusted` started: its process and its client.
pub struct Spawned {
    pid: u32,
    /// The process, to signal it without a reused pid; `None` when the
    /// kernel has no pidfd.
    pidfd: Option<OwnedFd>,
    client: Client,
    /// Set by the reaper once the process exited.
    exited: Arc<AtomicBool>,
}

impl Spawned {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the process still runs: the reaper has not seen it exit.
    pub fn runs(&self) -> bool {
        !self.exited.load(Ordering::Acquire)
    }

    /// Kills the process and drops its client, whose socket a child of the
    /// process could hold open.
    pub fn kill(&self, dh: &DisplayHandle) {
        if let Some(pidfd) = &self.pidfd
            && let Err(e) = rustix::process::pidfd_send_signal(pidfd, Signal::KILL)
            && e != rustix::io::Errno::SRCH
        {
            eprintln!("tetsu-comp: cannot kill {}: {e}", self.pid);
        }
        dh.backend_handle()
            .kill_client(self.client.id(), DisconnectReason::ConnectionClosed);
    }
}

/// Starts `argv` as a client of the compositor with `trust`.
///
/// The child end of a socket pair is passed as `WAYLAND_SOCKET`; the child
/// is reaped on a thread. Fails when `argv` is empty or the program cannot
/// start, and then no client was added.
pub fn spawn_trusted(state: &mut State, argv: &[String], trust: Trust) -> io::Result<Spawned> {
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
    let pid = child.id();
    // Not reaped yet, so the pid is still this child's.
    let pidfd = rustix::process::pidfd_open(Pid::from_child(&child), PidfdFlags::empty())
        .inspect_err(|e| eprintln!("tetsu-comp: no pidfd for {pid}: {e}"))
        .ok();
    let client = state
        .display_handle
        .insert_client(ours, Arc::new(ClientState::with_trust(trust)))
        .map_err(io::Error::other)?;
    let exited = Arc::new(AtomicBool::new(false));
    let reaped = Arc::clone(&exited);
    std::thread::Builder::new()
        .name("tetsu-comp-reap".to_owned())
        .spawn(move || {
            let _ = child.wait();
            reaped.store(true, Ordering::Release);
        })?;
    Ok(Spawned {
        pid,
        pidfd,
        client,
        exited,
    })
}

fn rustix_fd(fd: &std::os::fd::OwnedFd) -> i32 {
    std::os::fd::AsRawFd::as_raw_fd(fd)
}
