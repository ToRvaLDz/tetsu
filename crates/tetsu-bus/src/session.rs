//! `org.tetsu.comp.session.v1`: the lock state, and a `WatchSession` client.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zlink::{Call, Connection};

use crate::into_io;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.comp.session.v1";

/// `SessionEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// The first event; carries `locked`.
    Initial,
    /// The session locked.
    Locked,
    /// The session unlocked.
    Unlocked,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchSession`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// Which event.
    pub kind: SessionKind,
    /// `initial`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked: Option<bool>,
}

#[derive(Debug, Serialize)]
struct WatchCall {
    method: &'static str,
}

#[derive(Debug, Deserialize)]
struct WatchReply {
    event: SessionEvent,
}

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
}

/// An open `WatchSession` stream.
#[derive(Debug)]
pub struct SessionWatch {
    connection: Connection<zlink::tokio::unix::Stream>,
    ended: bool,
}

/// Connects to the compositor socket and starts `WatchSession`.
pub async fn watch(path: &Path) -> io::Result<SessionWatch> {
    let mut connection = zlink::tokio::unix::connect(path).await.map_err(into_io)?;
    let call = Call::new(WatchCall {
        method: "org.tetsu.comp.session.v1.WatchSession",
    })
    .set_more(true);
    connection
        .send_call(&call, Vec::new())
        .await
        .map_err(into_io)?;
    Ok(SessionWatch {
        connection,
        ended: false,
    })
}

impl SessionWatch {
    /// The next event. After the final reply, or once the compositor went
    /// away, an `UnexpectedEof` error.
    pub async fn next(&mut self) -> io::Result<SessionEvent> {
        if self.ended {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "WatchSession ended",
            ));
        }
        let (reply, _fds) = self
            .connection
            .receive_reply::<WatchReply, RemoteError>()
            .await
            .map_err(into_io)?;
        match reply {
            Ok(reply) => {
                self.ended = reply.continues() != Some(true);
                reply
                    .into_parameters()
                    .map(|parameters| parameters.event)
                    .ok_or_else(|| io::Error::other("WatchSession reply without parameters"))
            }
            Err(error) => Err(io::Error::other(format!(
                "WatchSession failed: {}",
                error.error
            ))),
        }
    }
}
