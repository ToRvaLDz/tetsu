//! tetsu-capd: serves `org.tetsu.cap.v1` at
//! `$XDG_RUNTIME_DIR/tetsu/org.tetsu.cap.v1`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Duration;

use tetsu_bus::{cap, server};
use tetsu_capd::ask::BACKOFF;
use tetsu_capd::audit::{JOURNAL_SOCKET, JournalSink};
use tetsu_capd::clock::SystemClock;
use tetsu_capd::engine::Engine;
use tetsu_capd::notify::BusNotify;
use tetsu_capd::service::CapService;
use tetsu_capd::store::{Layers, Store};
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::LocalSet;

const USAGE: &str = "usage: tetsu-capd

Serves org.tetsu.cap.v1 for this user session.";

/// How often grouped `notify` notices are due-checked.
const NOTICE_TICK: Duration = Duration::from_secs(1);
/// Where the image layer is installed.
const IMAGE_FILE: &str = "/usr/share/tetsu/capabilities.toml";

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

/// An absolute path from the environment, else `fallback()`.
fn env_path(
    name: &str,
    fallback: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    match std::env::var_os(name).filter(|v| !v.is_empty()) {
        Some(value) => Ok(PathBuf::from(value)),
        None => fallback(),
    }
}

fn home_dir(under: &str) -> Result<PathBuf, String> {
    let home: OsString = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(under))
}

fn layers() -> Result<Layers, String> {
    let state = env_path("XDG_STATE_HOME", || home_dir(".local/state"))?;
    let config = env_path("XDG_CONFIG_HOME", || home_dir(".config"))?;
    Ok(Layers {
        image: env_path("TETSU_CAPD_IMAGE_FILE", || Ok(PathBuf::from(IMAGE_FILE)))?,
        managed: state.join("tetsu/capabilities.toml"),
        user: config.join("tetsu/capabilities.toml"),
    })
}

fn run() -> Result<(), String> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .ok_or("XDG_RUNTIME_DIR is not set")?;
    let socket = runtime_dir.join("tetsu").join(cap::INTERFACE);
    let journal = env_path("TETSU_CAPD_JOURNAL_SOCKET", || {
        Ok(PathBuf::from(JOURNAL_SOCKET))
    })?;
    let layers = layers()?;
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
