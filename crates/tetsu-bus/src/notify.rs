//! `org.tetsu.notify.v1`: the wire types, shaped exactly as the IDL.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zlink::{Call, Connection};

use crate::into_io;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.notify.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.notify.v1.varlink");
/// `NotificationNotFound (id: string)`.
pub const NOTIFICATION_NOT_FOUND: &str = "org.tetsu.notify.v1.NotificationNotFound";
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.notify.v1.PermissionDenied";

/// A button; its id comes back in `invoked`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// Returned in `invoked`.
    pub id: String,
    /// The button text.
    pub label: String,
}

impl Action {
    /// An action.
    pub fn new(id: &str, label: &str) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
        }
    }
}

/// `urgency`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    /// Low.
    Low,
    /// Normal.
    Normal,
    /// Critical: never expires.
    Critical,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// The parameters of `Show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShowParams {
    /// One line.
    pub title: String,
    /// Plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Null for normal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub urgency: Option<Urgency>,
    /// The buttons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<Action>>,
    /// Null for the server default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
}

/// The parameters of `Close`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CloseParams {
    /// The id from the `shown` reply.
    pub id: String,
}

/// `NotifyReply.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    /// The notification is on screen; carries `id`.
    Shown,
    /// An action was invoked; carries `action_id`.
    Invoked,
    /// It went away unanswered; carries `reason`.
    Dismissed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `NotifyReply.reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DismissReason {
    /// The user dismissed it.
    User,
    /// Its timeout ran out.
    Expired,
    /// Its caller closed it.
    Closed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One reply of `Show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifyReply {
    /// What happened.
    pub kind: NotifyKind,
    /// `shown`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `invoked`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_id: Option<String>,
    /// `dismissed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<DismissReason>,
}

impl NotifyReply {
    fn of(kind: NotifyKind) -> Self {
        Self {
            kind,
            id: None,
            action_id: None,
            reason: None,
        }
    }

    /// `shown`.
    pub fn shown(id: &str) -> Self {
        Self {
            id: Some(id.to_owned()),
            ..Self::of(NotifyKind::Shown)
        }
    }

    /// `invoked`.
    pub fn invoked(action_id: &str) -> Self {
        Self {
            action_id: Some(action_id.to_owned()),
            ..Self::of(NotifyKind::Invoked)
        }
    }

    /// `dismissed`.
    pub fn dismissed(reason: DismissReason) -> Self {
        Self {
            reason: Some(reason),
            ..Self::of(NotifyKind::Dismissed)
        }
    }
}

/// `PendingEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingKind {
    /// The first event of the stream.
    Initial,
    /// The count changed.
    Changed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchPending`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingEvent {
    /// Which event.
    pub kind: PendingKind,
    /// Notifications with actions waiting for an answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
}

impl PendingEvent {
    /// `initial`.
    pub fn initial(count: i64) -> Self {
        Self {
            kind: PendingKind::Initial,
            count: Some(count),
        }
    }

    /// `changed`.
    pub fn changed(count: i64) -> Self {
        Self {
            kind: PendingKind::Changed,
            count: Some(count),
        }
    }
}

#[derive(Debug, Serialize)]
struct ShowCall<'a> {
    method: &'static str,
    parameters: &'a ShowParams,
}

#[derive(Debug, Deserialize)]
struct ShowReply {
    reply: NotifyReply,
}

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
}

/// An open `Show` stream. Dropping it closes the connection, which
/// withdraws the notification.
#[derive(Debug)]
pub struct ShowStream {
    connection: Connection<zlink::tokio::unix::Stream>,
    ended: bool,
}

async fn open(
    path: &Path,
    params: &ShowParams,
    call: fn(Call<ShowCall<'_>>) -> Call<ShowCall<'_>>,
) -> io::Result<Connection<zlink::tokio::unix::Stream>> {
    let mut connection = zlink::tokio::unix::connect(path).await.map_err(into_io)?;
    let call = call(Call::new(ShowCall {
        method: "org.tetsu.notify.v1.Show",
        parameters: params,
    }));
    connection
        .send_call(&call, Vec::new())
        .await
        .map_err(into_io)?;
    Ok(connection)
}

/// Connects to the notification server and calls `Show` with `more`.
pub async fn show(path: &Path, params: &ShowParams) -> io::Result<ShowStream> {
    let connection = open(path, params, |call| call.set_more(true)).await?;
    Ok(ShowStream {
        connection,
        ended: false,
    })
}

/// Calls `Show` oneway: no reply is read, so the notification cannot be
/// answered and only the server's own surfaces can dismiss it.
pub async fn show_oneway(path: &Path, params: &ShowParams) -> io::Result<()> {
    open(path, params, |call| call.set_oneway(true))
        .await
        .map(drop)
}

impl ShowStream {
    /// The next reply: `shown`, then `invoked` or `dismissed`. After the
    /// final reply, or once the server went away, an `UnexpectedEof` error.
    pub async fn next(&mut self) -> io::Result<NotifyReply> {
        if self.ended {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Show ended"));
        }
        let (reply, _fds) = self
            .connection
            .receive_reply::<ShowReply, RemoteError>()
            .await
            .map_err(into_io)?;
        match reply {
            Ok(reply) => {
                self.ended = reply.continues() != Some(true);
                reply
                    .into_parameters()
                    .map(|parameters| parameters.reply)
                    .ok_or_else(|| io::Error::other("Show reply without parameters"))
            }
            Err(error) => Err(io::Error::other(format!("Show failed: {}", error.error))),
        }
    }
}
