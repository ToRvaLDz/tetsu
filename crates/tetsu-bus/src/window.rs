//! `org.tetsu.comp.window.v1`: the wire types, shaped exactly as the IDL.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zlink::{Call, Connection};

use crate::into_io;
use crate::nullable::present;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.comp.window.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.comp.window.v1.varlink");
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.comp.window.v1.PermissionDenied";

/// `Window.placement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Placed by the layout.
    Tiled,
    /// Free-floating.
    Floating,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `Window`. The nullable fields are always written, as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Window {
    /// Opaque; compare for equality only.
    pub id: String,
    /// xdg app_id, or WM_CLASS for X11 windows.
    #[serde(default)]
    pub app_id: Option<String>,
    /// The window title.
    #[serde(default)]
    pub title: Option<String>,
    /// Null for Xwayland windows.
    #[serde(default)]
    pub pid: Option<i64>,
    /// Null while the window is not mapped on a workspace.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// How the window is placed.
    pub placement: Placement,
    /// Fullscreen.
    pub fullscreen: bool,
    /// An X11 window.
    pub xwayland: bool,
}

/// `WindowEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    /// The first event of a stream.
    Initial,
    /// A window mapped.
    Opened,
    /// A window's state changed.
    Changed,
    /// A window went away.
    Closed,
    /// The focus moved.
    Focused,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchWindows`. Absent fields are omitted, except
/// `focused_id`, which is `null` when nothing is focused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowEvent {
    /// Which event.
    pub kind: WindowKind,
    /// `initial`: every window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<Window>>,
    /// `initial`, `focused`: the focused window.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub focused_id: Option<Option<String>>,
    /// `opened`, `changed`: the full new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    /// `closed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl WindowEvent {
    fn of(kind: WindowKind) -> Self {
        Self {
            kind,
            windows: None,
            focused_id: None,
            window: None,
            id: None,
        }
    }

    /// `initial`.
    pub fn initial(windows: Vec<Window>, focused_id: Option<String>) -> Self {
        Self {
            windows: Some(windows),
            focused_id: Some(focused_id),
            ..Self::of(WindowKind::Initial)
        }
    }

    /// `opened`.
    pub fn opened(window: Window) -> Self {
        Self {
            window: Some(window),
            ..Self::of(WindowKind::Opened)
        }
    }

    /// `changed`.
    pub fn changed(window: Window) -> Self {
        Self {
            window: Some(window),
            ..Self::of(WindowKind::Changed)
        }
    }

    /// `closed`.
    pub fn closed(id: &str) -> Self {
        Self {
            id: Some(id.to_owned()),
            ..Self::of(WindowKind::Closed)
        }
    }

    /// `focused`.
    pub fn focused(focused_id: Option<String>) -> Self {
        Self {
            focused_id: Some(focused_id),
            ..Self::of(WindowKind::Focused)
        }
    }
}

/// The reply of `ListWindows`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWindowsReply {
    /// Every window.
    pub windows: Vec<Window>,
    /// The focused window, null when nothing is focused.
    #[serde(default)]
    pub focused_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct WatchCall {
    method: &'static str,
}

#[derive(Debug, Deserialize)]
struct WatchReply {
    event: WindowEvent,
}

/// A Varlink error reply; `error` is required, so a reply never matches it.
#[derive(Debug, Deserialize)]
struct RemoteError {
    error: String,
}

/// An open `WatchWindows` stream.
#[derive(Debug)]
pub struct WindowWatch {
    connection: Connection<zlink::tokio::unix::Stream>,
    ended: bool,
}

/// Connects to the compositor socket and starts `WatchWindows`.
pub async fn watch(path: &Path) -> io::Result<WindowWatch> {
    let mut connection = zlink::tokio::unix::connect(path).await.map_err(into_io)?;
    let call = Call::new(WatchCall {
        method: "org.tetsu.comp.window.v1.WatchWindows",
    })
    .set_more(true);
    connection
        .send_call(&call, Vec::new())
        .await
        .map_err(into_io)?;
    Ok(WindowWatch {
        connection,
        ended: false,
    })
}

impl WindowWatch {
    /// The next event. After the final reply, or once the compositor went
    /// away, an `UnexpectedEof` error.
    pub async fn next(&mut self) -> io::Result<WindowEvent> {
        if self.ended {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "WatchWindows ended",
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
                    .ok_or_else(|| io::Error::other("WatchWindows reply without parameters"))
            }
            Err(error) => Err(io::Error::other(format!(
                "WatchWindows failed: {}",
                error.error
            ))),
        }
    }
}
