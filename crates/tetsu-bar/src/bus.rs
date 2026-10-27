//! The bus task: follows `WatchWorkspaces` and sends each new workspace
//! strip. It runs on the bus thread's current-thread runtime, so the UI loop
//! never waits on the bus.
//!
//! `TETSU_COMP_SOCKET` names the compositor socket. Unset (a foreign
//! compositor), the workspace segment is hidden, with one warning. Set but
//! unreachable, or a stream that fails or ends, clears the strip (`None`)
//! until the compositor answers again; the task retries meanwhile.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tetsu_bus::workspace::{self, WorkspaceKind};
use tokio::sync::mpsc;

use crate::model::workspaces::{Mirror, WsCell};

/// The environment variable naming the compositor socket.
pub const COMP_SOCKET_ENV: &str = "TETSU_COMP_SOCKET";
/// Wait between attempts to reach the compositor.
pub const RETRY: Duration = Duration::from_secs(1);

/// Where the workspaces come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompSource {
    /// No Tetsu compositor: no workspace segment.
    Unmanaged,
    /// `tetsu-comp` at this socket.
    Compositor(PathBuf),
}

impl CompSource {
    /// From the value of `TETSU_COMP_SOCKET`.
    pub fn from_env(value: Option<OsString>) -> Self {
        match value {
            Some(path) if !path.is_empty() => Self::Compositor(PathBuf::from(path)),
            _ => Self::Unmanaged,
        }
    }
}

/// What the bar shows of the workspaces: `None` hides the segment.
pub type Strip = Option<Vec<WsCell>>;

/// Sends `strip` unless it is what was sent last. `false` when the
/// receiver is gone.
fn send(tx: &mpsc::UnboundedSender<Strip>, last: &mut Option<Strip>, strip: Strip) -> bool {
    if last.as_ref() == Some(&strip) {
        return true;
    }
    *last = Some(strip.clone());
    tx.send(strip).is_ok()
}

/// Sends every new strip until `tx` closes, reconnecting after `retry`.
pub async fn run(source: CompSource, tx: mpsc::UnboundedSender<Strip>, retry: Duration) {
    let path = match source {
        CompSource::Unmanaged => {
            eprintln!(
                "tetsu-bar: {COMP_SOCKET_ENV} is unset, no Tetsu compositor: \
                 no workspaces shown"
            );
            let _ = tx.send(None);
            return;
        }
        CompSource::Compositor(path) => path,
    };
    let mut last = None;
    let mut reported_failure = false;
    loop {
        let ended = tokio::select! {
            () = tx.closed() => return,
            ended = follow(&path, &tx, &mut last) => ended,
        };
        match ended {
            Ok(()) => reported_failure = false,
            Err(e) if !reported_failure => {
                eprintln!(
                    "tetsu-bar: workspaces unavailable from {}: {e}; retrying",
                    path.display()
                );
                reported_failure = true;
            }
            Err(_) => {}
        }
        // Whatever ended the stream, the strip is stale.
        if !send(&tx, &mut last, None) {
            return;
        }
        tokio::select! {
            () = tx.closed() => return,
            () = tokio::time::sleep(retry) => {}
        }
    }
}

/// Follows one `WatchWorkspaces` stream until it ends, fails, or the
/// receiver goes away.
async fn follow(
    path: &Path,
    tx: &mpsc::UnboundedSender<Strip>,
    last: &mut Option<Strip>,
) -> io::Result<()> {
    let mut stream = workspace::watch(path).await?;
    let mut mirror = Mirror::default();
    let mut seen_initial = false;
    loop {
        let event = match stream.next().await {
            Ok(event) => event,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        seen_initial |= event.kind == WorkspaceKind::Initial;
        mirror = mirror.apply(&event);
        if seen_initial && !send(tx, last, Some(mirror.strip())) {
            return Ok(());
        }
    }
}
