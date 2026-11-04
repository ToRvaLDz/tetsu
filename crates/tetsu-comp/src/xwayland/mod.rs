//! X11 clients through xwayland-satellite: the compositor owns an X11
//! display (`display`) and starts satellite on its first connection;
//! satellite is an ordinary Wayland client, told apart by its pid
//! (`satellite`). When satellite exits, the next X11 client starts it
//! again.

pub mod display;
pub mod satellite;

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::sync::mpsc;

use smithay::reexports::calloop::channel::{self, Event, Sender};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode as PollMode, PostAction, RegistrationToken};
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use crate::state::State;
use crate::trust::{Trust, trust_of};
use display::{X11Display, pid_alive};

/// Where the X11 lock and sockets live.
const X11_ROOT: &str = "/tmp";

/// The X11 display and the satellite serving it.
pub struct Xwayland {
    display: X11Display,
    /// The satellite program.
    path: String,
    /// The pid of the running satellite.
    satellite: Option<u32>,
    /// The watches on the listening sockets (by position), while nobody
    /// serves them.
    watches: Vec<(usize, RegistrationToken)>,
    /// The reaper tells the loop that satellite exited.
    exited: Sender<ExitStatus>,
}

impl Xwayland {
    /// The pid of the running satellite.
    pub fn satellite_pid(&self) -> Option<u32> {
        self.satellite
    }
}

/// Sets `DISPLAY` for a client the compositor starts, or removes it when
/// X11 is off (a nested compositor must not send it to the host's X).
pub fn set_display(command: &mut Command, display: Option<&str>) {
    match display {
        Some(display) => command.env("DISPLAY", display),
        None => command.env_remove("DISPLAY"),
    };
}

/// Opens the X11 display when `enabled` and satellite supports listening
/// sockets; returns `DISPLAY`. Any failure is logged and leaves X11 off.
pub fn start_xwayland(state: &mut State, path: Option<&str>, enabled: bool) -> Option<String> {
    let path = path.filter(|_| enabled)?;
    match satellite::probe_command(path).status() {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("tetsu-comp: {path} cannot take listening sockets ({status}); no X11");
            return None;
        }
        Err(e) => {
            eprintln!("tetsu-comp: cannot run {path}: {e}; no X11");
            return None;
        }
    }
    let display = match X11Display::open(Path::new(X11_ROOT), 0, pid_alive) {
        Ok(display) => display,
        Err(e) => {
            eprintln!("tetsu-comp: cannot open an X11 display: {e}; no X11");
            return None;
        }
    };
    let (exited, receiver) = channel::channel::<ExitStatus>();
    let inserted = state
        .loop_handle
        .insert_source(receiver, |event, (), state| {
            if let Event::Msg(status) = event {
                state.on_satellite_exit(status);
            }
        });
    if let Err(e) = inserted {
        eprintln!(
            "tetsu-comp: cannot watch xwayland-satellite: {}; no X11",
            e.error
        );
        return None;
    }
    let name = display.display_name().to_owned();
    state.xwayland = Some(Xwayland {
        display,
        path: path.to_owned(),
        satellite: None,
        watches: Vec::new(),
        exited,
    });
    state.arm_watch();
    eprintln!("tetsu-comp: X11 on DISPLAY={name}");
    Some(name)
}

/// Accepts and drops what waits on `fd`: connections satellite failed to
/// take would wake the watch forever.
fn drain(fd: BorrowedFd<'_>) -> std::io::Result<OwnedFd> {
    let listener = UnixListener::from(fd.try_clone_to_owned()?);
    // The flag is shared with every copy: back to blocking for satellite.
    listener.set_nonblocking(true)?;
    while listener.accept().is_ok() {}
    listener.set_nonblocking(false)?;
    Ok(OwnedFd::from(listener))
}

impl State {
    /// `DISPLAY` of the X11 display, when X11 is on.
    pub fn x11_display(&self) -> Option<&str> {
        self.xwayland.as_ref().map(|x| x.display.display_name())
    }

    /// The pid and the `xwayland` flag of the window of `surface`.
    pub fn window_origin(&self, surface: &WlSurface) -> (Option<i64>, bool) {
        let trust = self
            .display_handle
            .get_client(surface.id())
            .map_or(Trust::Untrusted, |client| trust_of(&client));
        satellite::window_origin(trust, self.client_pid(surface))
    }

    /// Watches the X11 sockets for the next client, after dropping what
    /// waits there.
    fn arm_watch(&mut self) {
        let Some(xwayland) = self.xwayland.as_ref() else {
            return;
        };
        let mut copies = Vec::new();
        for fd in xwayland.display.listeners() {
            match drain(fd) {
                Ok(copy) => copies.push(copy),
                Err(e) => eprintln!("tetsu-comp: cannot watch an X11 socket: {e}"),
            }
        }
        let mut watches = Vec::new();
        for (at, copy) in copies.into_iter().enumerate() {
            let source = Generic::new(copy, Interest::READ, PollMode::Level);
            let inserted = self.loop_handle.insert_source(source, move |_, _, state| {
                state.on_x11_connection(at);
                Ok(PostAction::Remove)
            });
            match inserted {
                Ok(token) => watches.push((at, token)),
                Err(e) => eprintln!("tetsu-comp: cannot watch an X11 socket: {}", e.error),
            }
        }
        if let Some(xwayland) = self.xwayland.as_mut() {
            xwayland.watches = watches;
        }
    }

    /// The first X11 client: the watches go (the one that fired removes
    /// itself, `fired` is its position) and satellite starts.
    fn on_x11_connection(&mut self, fired: usize) {
        let Some(xwayland) = self.xwayland.as_mut() else {
            return;
        };
        let watches = std::mem::take(&mut xwayland.watches);
        for (at, token) in watches {
            if at != fired {
                self.loop_handle.remove(token);
            }
        }
        if let Err(e) = self.spawn_satellite() {
            eprintln!("tetsu-comp: cannot start xwayland-satellite: {e}");
            self.arm_watch();
        }
    }

    /// Starts satellite with copies of the listening sockets and a reaper.
    /// The reaper starts first and waits for the child: when it cannot
    /// start, nothing is spawned and the caller watches the sockets again.
    fn spawn_satellite(&mut self) -> std::io::Result<()> {
        let Some(xwayland) = self.xwayland.as_mut() else {
            return Ok(());
        };
        let exited = xwayland.exited.clone();
        let (hand_over, child_in) = mpsc::channel::<Child>();
        std::thread::Builder::new()
            .name("tetsu-comp-satellite".to_owned())
            .spawn(move || {
                // No child comes when the spawn failed: the sender is gone.
                let Ok(mut child) = child_in.recv() else {
                    return;
                };
                if let Ok(status) = child.wait() {
                    let _ = exited.send(status);
                }
            })?;
        // `dup` leaves the copies without close-on-exec: satellite inherits
        // them, and they close here once it has.
        let inherited = xwayland
            .display
            .listeners()
            .into_iter()
            .map(|fd| rustix::io::dup(fd.as_fd()).map_err(std::io::Error::from))
            .collect::<std::io::Result<Vec<OwnedFd>>>()?;
        let raw: Vec<i32> = inherited.iter().map(AsRawFd::as_raw_fd).collect();
        let mut command = satellite::satellite_command(
            &xwayland.path,
            xwayland.display.display_name(),
            &raw,
            &self.socket_name,
        );
        let child = command.spawn()?;
        drop(inherited);
        let pid = child.id();
        xwayland.satellite = Some(pid);
        if hand_over.send(child).is_err() {
            eprintln!("tetsu-comp: the reaper of xwayland-satellite {pid} is gone");
        }
        eprintln!("tetsu-comp: xwayland-satellite {pid} started");
        Ok(())
    }

    /// Satellite exited: the next X11 client starts it again.
    fn on_satellite_exit(&mut self, status: ExitStatus) {
        let Some(xwayland) = self.xwayland.as_mut() else {
            return;
        };
        let pid = xwayland.satellite.take();
        eprintln!("tetsu-comp: xwayland-satellite {pid:?} exited: {status}");
        self.arm_watch();
    }
}
