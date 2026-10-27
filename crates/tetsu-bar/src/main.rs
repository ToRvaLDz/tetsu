//! tetsu-bar: the status bar.
//!
//! The main thread runs the Wayland surfaces and the timers. A second
//! thread runs the bus client on a current-thread runtime and forwards each
//! workspace strip to the main thread.

use std::process::ExitCode;

use tetsu_bar::bus::{self, COMP_SOCKET_ENV, CompSource};
use tetsu_bar::ui::{self, Msg};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const USAGE: &str = "usage: tetsu-bar

Shows workspaces, clock, network and battery on a layer-shell surface.
Environment:
  TETSU_COMP_SOCKET  the tetsu-comp socket (workspaces are hidden when unset)
  TETSU_BAR_TRACE=1  print one `draw <reason>` line per redraw";

fn main() -> ExitCode {
    // No arguments are taken besides help; the first one decides.
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "-h" || arg == "--help" {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        eprintln!("tetsu-bar: unknown argument {arg}\n{USAGE}");
        return ExitCode::from(2);
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tetsu-bar: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let trace = std::env::var_os("TETSU_BAR_TRACE").is_some_and(|v| v == "1");
    let source = CompSource::from_env(std::env::var_os(COMP_SOCKET_ENV));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    let (to_ui, ui_rx) = ui::channel();
    std::thread::Builder::new()
        .name("tetsu-bar-bus".into())
        .spawn(move || LocalSet::new().block_on(&runtime, bus_thread(source, to_ui)))
        .map_err(|e| format!("cannot start the bus thread: {e}"))?;
    ui::run(ui_rx, trace).map_err(|e| e.to_string())
}

/// Follows the workspaces and watches for termination, until the UI is gone.
async fn bus_thread(
    source: CompSource,
    to_ui: smithay_client_toolkit::reexports::calloop::channel::Sender<Msg>,
) {
    let (strips_tx, mut strips) = mpsc::unbounded_channel();
    tokio::task::spawn_local(bus::run(source, strips_tx, bus::RETRY));
    let (Ok(mut terminate), Ok(mut interrupt)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        eprintln!("tetsu-bar: cannot watch SIGTERM and SIGINT");
        return;
    };
    let mut strips_open = true;
    loop {
        tokio::select! {
            // `None`: the task returned (no compositor); only signals remain.
            strip = strips.recv(), if strips_open => match strip {
                Some(strip) => {
                    if to_ui.send(Msg::Strip(strip)).is_err() {
                        return;
                    }
                }
                None => strips_open = false,
            },
            _ = terminate.recv() => break,
            _ = interrupt.recv() => break,
        }
    }
    let _ = to_ui.send(Msg::Quit);
}
