//! Starts the compositor: loop, backend, bus, first client.

use std::error::Error;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use smithay::reexports::calloop::EventLoop;
use smithay::reexports::calloop::channel::{self, Event};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::Display;

use crate::backend::{headless, headless_gpu, udev, winit};
use crate::bus::{Command as BusCommand, CommandSink, Publisher, Snapshot};
use crate::cli::{Backend, Cli};
use crate::config::glue::{RELOAD_POLL, config_reply};
use crate::config::store::Store;
use crate::settings::Settings;
use crate::socket::{COMP_SOCKET_ENV, bus_socket_path};
use crate::state::State;
use crate::trust::{Trust, spawn_trusted};
use crate::xwayland::{set_display, start_xwayland};

/// The bus publishes at most once per tick (`changed` is coalesced to it).
const PUBLISH_TICK: Duration = Duration::from_millis(16);

/// Runs until the loop stops.
pub fn run(cli: &Cli, store: Store) -> Result<(), Box<dyn Error>> {
    let settings = Settings::from_values(&store.view().values());
    let satellite = settings.xwayland.program().map(str::to_owned);
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from);
    let mut event_loop: EventLoop<'static, State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;
    let mut state = State::new(&mut event_loop, display, settings)?;

    // Before the backend: the outputs present at start get their config.
    state.config = Some(store);
    match cli.backend {
        Backend::Headless => {
            headless::init(&mut state, cli.headless_outputs, cli.headless_hotplug)?;
            headless_gpu::start(&mut state, headless_gpu::HEADLESS_DEVICE);
        }
        Backend::Winit => {
            winit::init(&mut state)?;
            headless_gpu::start(&mut state, headless_gpu::WINIT_DEVICE);
        }
        Backend::Tty => udev::init(&mut state)?,
    }
    // Before any client starts: they all get its DISPLAY.
    let x11 = start_xwayland(
        &mut state,
        satellite.as_deref(),
        cli.backend != Backend::Headless || cli.headless_xwayland,
    );

    // The outputs added so far are in the bus thread's first snapshot.
    let _ = state.engine.flush();
    let wayland = state.socket_name.to_string_lossy().into_owned();
    let bus_path = bus_socket_path(runtime_dir.as_deref(), &wayland)?;
    let (sender, commands) = channel::channel::<BusCommand>();
    state
        .loop_handle
        .insert_source(commands, |event, (), state| {
            if let Event::Msg(command) = event {
                match command {
                    BusCommand::SetManaged(call) => state.handle_config_command(call),
                    BusCommand::ConfigureOutputs(call) => state.handle_output_command(call),
                }
            }
        })
        .map_err(|e| e.error)?;
    let sink: CommandSink = Arc::new(move |command| sender.send(command).map_err(|e| e.0));
    state.publisher = Some(Publisher::spawn_with(
        &bus_path,
        Snapshot::of(state.engine.model()).with_config(config_reply(state.config.as_ref())),
        Some(sink),
    )?);
    state.comp_socket = Some(bus_path.clone());
    start_trusted_helpers(&mut state);
    state
        .loop_handle
        .insert_source(Timer::from_duration(PUBLISH_TICK), |_, (), state| {
            state.publish_tick();
            TimeoutAction::ToDuration(PUBLISH_TICK)
        })?;

    state
        .loop_handle
        .insert_source(Timer::from_duration(RELOAD_POLL), |_, (), state| {
            state.config_tick();
            TimeoutAction::ToDuration(RELOAD_POLL)
        })?;

    announce(&wayland, &bus_path, x11.as_deref())?;
    spawn_client(cli, &wayland, &bus_path, x11.as_deref());
    event_loop.run(None, &mut state, |state| {
        state.idle_loop_done();
        let _ = state.display_handle.flush_clients();
    })?;
    Ok(())
}

/// Starts the `[trust] input` helpers once, with the trust to bind the
/// virtual input protocols. One that cannot start is logged.
fn start_trusted_helpers(state: &mut State) {
    let helpers = state.trusted_input.clone();
    for argv in helpers {
        if let Err(e) = spawn_trusted(state, &argv, Trust::Input) {
            eprintln!("tetsu-comp: cannot start trusted helper {argv:?}: {e}");
        }
    }
}

/// The readiness line tests and scripts wait for; `DISPLAY` comes last,
/// and only when X11 is on.
fn announce(wayland: &str, bus_path: &Path, x11: Option<&str>) -> std::io::Result<()> {
    let mut out = std::io::stdout().lock();
    write!(
        out,
        "ready WAYLAND_DISPLAY={wayland} {COMP_SOCKET_ENV}={}",
        bus_path.display()
    )?;
    if let Some(display) = x11 {
        write!(out, " DISPLAY={display}")?;
    }
    writeln!(out)?;
    out.flush()
}

/// Starts the `-- <cmd>` client with the compositor's sockets and `DISPLAY`
/// (removed when X11 is off). The environment goes through
/// `Command::env`, never `set_var`.
fn spawn_client(cli: &Cli, wayland: &str, bus_path: &Path, x11: Option<&str>) {
    let Some((program, args)) = cli.command.split_first() else {
        return;
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .env("WAYLAND_DISPLAY", wayland)
        .env(COMP_SOCKET_ENV, bus_path);
    set_display(&mut command, x11);
    // The hotplug stream is the compositor's: the client must not read it.
    if cli.headless_hotplug {
        command.stdin(Stdio::null());
    }
    let spawned = command.spawn();
    if let Err(e) = spawned {
        eprintln!("tetsu-comp: cannot run {}: {e}", program.to_string_lossy());
    }
}
