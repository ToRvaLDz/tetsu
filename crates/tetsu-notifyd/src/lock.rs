//! The session lock state, from `org.tetsu.comp.session.v1`.
//!
//! `TETSU_COMP_SOCKET` names the compositor socket. Unset, notifyd runs
//! under a foreign compositor (development): the session counts as
//! unlocked, with a warning. Set but unreachable, or a stream that fails or
//! ends, counts as locked until the compositor answers again.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use tetsu_bus::session::{self, SessionKind};
use tokio::sync::mpsc;

/// The environment variable naming the compositor socket.
pub const COMP_SOCKET_ENV: &str = "TETSU_COMP_SOCKET";
/// Wait between attempts to reach the compositor.
const RETRY: Duration = Duration::from_secs(1);

/// Where the lock state comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockSource {
    /// No Tetsu compositor: never locked.
    Unmanaged,
    /// `tetsu-comp` at this socket.
    Compositor(PathBuf),
}

impl LockSource {
    /// From the value of `TETSU_COMP_SOCKET`.
    pub fn from_env(value: Option<OsString>) -> Self {
        match value {
            Some(path) if !path.is_empty() => Self::Compositor(PathBuf::from(path)),
            _ => Self::Unmanaged,
        }
    }
}

/// Sends every lock state (`true` = locked) until `tx` closes.
pub async fn watch(source: LockSource, tx: mpsc::UnboundedSender<bool>) {
    let path = match source {
        LockSource::Unmanaged => {
            eprintln!(
                "tetsu-notifyd: {COMP_SOCKET_ENV} is unset, no Tetsu compositor: \
                 the session counts as unlocked"
            );
            let _ = tx.send(false);
            return;
        }
        LockSource::Compositor(path) => path,
    };
    let mut reported_failure = false;
    while !tx.is_closed() {
        match follow(&path, &tx).await {
            Ok(()) => reported_failure = false,
            Err(e) if !reported_failure => {
                eprintln!(
                    "tetsu-notifyd: lock state unavailable from {}: {e}; locked until it answers",
                    path.display()
                );
                reported_failure = true;
            }
            Err(_) => {}
        }
        // Whatever ended the stream, fail closed until it answers again.
        if tx.send(true).is_err() {
            return;
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// Follows one `WatchSession` stream until it ends or fails.
async fn follow(path: &std::path::Path, tx: &mpsc::UnboundedSender<bool>) -> std::io::Result<()> {
    let mut stream = session::watch(path).await?;
    loop {
        let event = match stream.next().await {
            Ok(event) => event,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let locked = match event.kind {
            SessionKind::Initial => event.locked.unwrap_or(true),
            SessionKind::Locked => true,
            SessionKind::Unlocked => false,
            SessionKind::Unknown => continue,
        };
        if tx.send(locked).is_err() {
            return Ok(());
        }
    }
}
