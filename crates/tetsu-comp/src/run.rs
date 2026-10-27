//! Starts the compositor: loop, backend, bus, first client.

use std::error::Error;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use smithay::reexports::calloop::EventLoop;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::Display;

use crate::backend::{headless, udev, winit};
use crate::bus::{Publisher, Snapshot};
use crate::cli::{Backend, Cli};
use crate::socket::{COMP_SOCKET_ENV, bus_socket_path};
use crate::state::State;

/// The bus publishes at most once per tick (`changed` is coalesced to it).
const PUBLISH_TICK: Duration = Duration::from_millis(16);

/// Runs until the loop stops.
pub fn run(cli: &Cli) -> Result<(), Box<dyn Error>> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    let mut event_loop: EventLoop<'static, State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;
    let mut state = State::new(&mut event_loop, display)?;

    match cli.backend {
        Backend::Headless => headless::init(&mut state)?,
        Backend::Winit => winit::init(&mut state)?,
        Backend::Tty => udev::init(&mut state)?,
    }

    // The outputs added so far are in the bus thread's first snapshot.
    let _ = state.model.flush();
    let wayland = state.socket_name.to_string_lossy().into_owned();
    let bus_path = bus_socket_path(runtime_dir.as_deref(), &wayland)?;
    state.publisher = Some(Publisher::spawn(&bus_path, Snapshot::of(&state.model))?);
    state
        .loop_handle
        .insert_source(Timer::from_duration(PUBLISH_TICK), |_, (), state| {
            state.publish_tick();
            TimeoutAction::ToDuration(PUBLISH_TICK)
        })?;

    announce(&wayland, &bus_path)?;
    spawn_client(cli, &wayland, &bus_path);
    event_loop.run(None, &mut state, |state| {
        let _ = state.display_handle.flush_clients();
    })?;
    Ok(())
}

/// The readiness line tests and scripts wait for.
fn announce(wayland: &str, bus_path: &Path) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    writeln!(
        out,
        "ready WAYLAND_DISPLAY={wayland} {COMP_SOCKET_ENV}={}",
        bus_path.display()
    )?;
    out.flush()
}

/// Starts the `-- <cmd>` client with the compositor's sockets. The
/// environment goes through `Command::env`, never `set_var`.
fn spawn_client(cli: &Cli, wayland: &str, bus_path: &Path) {
    let Some((program, args)) = cli.command.split_first() else {
        return;
    };
    let spawned = Command::new(program)
        .args(args)
        .env("WAYLAND_DISPLAY", wayland)
        .env(COMP_SOCKET_ENV, bus_path)
        .spawn();
    if let Err(e) = spawned {
        eprintln!("tetsu-comp: cannot run {}: {e}", program.to_string_lossy());
    }
}
