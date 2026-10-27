//! tetsu-capd: serves `org.tetsu.cap.v1` at
//! `$XDG_RUNTIME_DIR/tetsu/org.tetsu.cap.v1`.

use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use tetsu_bus::server;
use tetsu_capd::ask::BACKOFF;
use tetsu_capd::audit::JournalSink;
use tetsu_capd::clock::SystemClock;
use tetsu_capd::engine::Engine;
use tetsu_capd::notify::BusNotify;
use tetsu_capd::paths;
use tetsu_capd::service::CapService;
use tetsu_capd::store::{Layers, Store};
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::LocalSet;

const USAGE: &str = "usage: tetsu-capd

Serves org.tetsu.cap.v1 for this user session.";

/// How often grouped `notify` notices are due-checked.
const NOTICE_TICK: Duration = Duration::from_secs(1);

fn main() -> ExitCode {
    if let Some(arg) = std::env::args().nth(1) {
        return match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                ExitCode::SUCCESS
            }
            other => {
                eprintln!("tetsu-capd: unknown argument {other}\n{USAGE}");
                ExitCode::from(2)
            }
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tetsu-capd: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let runtime_dir = paths::runtime_dir()?;
    let socket = paths::socket()?;
    let journal = paths::journal()?;
    let layers = paths::layers()?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start the runtime: {e}"))?;
    LocalSet::new().block_on(&runtime, serve(&socket, &runtime_dir, &journal, layers))
}

async fn serve(
    socket: &std::path::Path,
    runtime_dir: &std::path::Path,
    journal: &std::path::Path,
    layers: Layers,
) -> Result<(), String> {
    let sink = JournalSink::new(journal).map_err(|e| format!("cannot open the journal: {e}"))?;
    // The listener removes its own socket when dropped, never another's.
    let listener =
        server::bind(socket).map_err(|e| format!("cannot bind {}: {e}", socket.display()))?;
    let engine = Rc::new(Engine::new(
        BusNotify::in_runtime_dir(runtime_dir),
        SystemClock,
        sink,
        Store::new(layers),
        BACKOFF.to_vec(),
    ));
    let own_uid = rustix::process::geteuid().as_raw();
    let service = Rc::new(CapService::new(engine.clone(), own_uid));

    let ticking = engine.clone();
    tokio::task::spawn_local(async move {
        loop {
            tokio::time::sleep(NOTICE_TICK).await;
            ticking.flush_notices().await;
        }
    });

    let mut terminate =
        signal(SignalKind::terminate()).map_err(|e| format!("cannot watch SIGTERM: {e}"))?;
    let mut interrupt =
        signal(SignalKind::interrupt()).map_err(|e| format!("cannot watch SIGINT: {e}"))?;
    tokio::select! {
        result = server::serve(listener, service) => {
            result.map_err(|e| format!("the cap.v1 socket failed: {e}"))
        }
        _ = terminate.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
    }
}
