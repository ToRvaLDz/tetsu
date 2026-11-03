//! tetsu-notifyd: serves `org.tetsu.notify.v1` at
//! `$XDG_RUNTIME_DIR/tetsu/org.tetsu.notify.v1` and
//! `org.freedesktop.Notifications` on the session bus, and draws the
//! popups and the panel.

use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;

use tetsu_bus::server;
use tetsu_notifyd::daemon::{Command, Daemon};
use tetsu_notifyd::lock::{self, COMP_SOCKET_ENV, LockSource};
use tetsu_notifyd::varlink::NotifyService;
use tetsu_notifyd::{fdo, ui};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;

const USAGE: &str = "usage: tetsu-notifyd [--replace]

  --replace  take org.freedesktop.Notifications from a running server";

fn main() -> ExitCode {
    let mut replace = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--replace" => replace = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tetsu-notifyd: unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    match run(replace) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tetsu-notifyd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn socket_path() -> Result<PathBuf, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .ok_or("XDG_RUNTIME_DIR is not set")?;
    Ok(PathBuf::from(runtime)
        .join("tetsu")
        .join(tetsu_bus::notify::INTERFACE))
}

fn run(replace: bool) -> Result<(), String> {
    let path = socket_path()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    // The listener removes its own socket when dropped, never another's.
    LocalSet::new().block_on(&runtime, serve(&path, replace))
}

async fn serve(path: &std::path::Path, replace: bool) -> Result<(), String> {
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (signals_tx, signals_rx) = mpsc::unbounded_channel();
    let (daemon, commands) = Daemon::new(ui_tx);
    tokio::task::spawn_local(daemon.with_fdo_signals(signals_tx).run());

    // The screen runs on its own thread with its own event loop.
    let (to_screen, screen_rx) = ui::channel();
    let (screen_gone_tx, screen_gone) = oneshot::channel();
    let screen_commands = commands.clone();
    std::thread::Builder::new()
        .name("tetsu-notifyd-ui".into())
        .spawn(move || {
            let _ = screen_gone_tx.send(ui::run(screen_rx, screen_commands));
        })
        .map_err(|e| format!("cannot start the UI thread: {e}"))?;
    tokio::task::spawn_local(async move {
        while let Some(msg) = ui_rx.recv().await {
            if to_screen.send(msg).is_err() {
                return;
            }
        }
    });

    let (lock_tx, mut lock_rx) = mpsc::unbounded_channel();
    let source = LockSource::from_env(std::env::var_os(COMP_SOCKET_ENV));
    tokio::task::spawn_local(lock::watch(source, lock_tx));
    let lock_commands = commands.clone();
    tokio::task::spawn_local(async move {
        while let Some(locked) = lock_rx.recv().await {
            if lock_commands.send(Command::Lock(locked)).is_err() {
                return;
            }
        }
    });

    // The socket exists before the bus name, which is what readiness means
    // for `Type=dbus`.
    let listener =
        server::bind(path).map_err(|e| format!("cannot bind {}: {e}", path.display()))?;
    let _bus = fdo::serve(
        zbus::connection::Builder::session(),
        commands.clone(),
        signals_rx,
        replace,
    )
    .await
    .map_err(|e| format!("cannot own {} on the session bus: {e}", fdo::BUS_NAME))?;

    let service = Rc::new(NotifyService::new(commands));
    let mut terminate =
        signal(SignalKind::terminate()).map_err(|e| format!("cannot watch SIGTERM: {e}"))?;
    let mut interrupt =
        signal(SignalKind::interrupt()).map_err(|e| format!("cannot watch SIGINT: {e}"))?;
    tokio::select! {
        result = server::serve(listener, service) => {
            result.map_err(|e| format!("the notify.v1 socket failed: {e}"))
        }
        result = screen_gone => match result {
            Ok(Ok(())) | Err(_) => Err("the UI stopped".into()),
            Ok(Err(e)) => Err(format!("the UI failed: {e}")),
        },
        _ = terminate.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
    }
}
