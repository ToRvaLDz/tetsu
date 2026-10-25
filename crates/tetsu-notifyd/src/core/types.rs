//! The public types of the state machine.

use std::time::{Duration, Instant};

use tetsu_bus::notify::{Action, NotifyReply, Urgency};

/// Actions arm this long after their buttons appear (#23).
pub const ARM_DELAY: Duration = Duration::from_millis(500);
/// A popup that never expires moves into the panel after this long.
pub const COLLAPSE_AFTER: Duration = Duration::from_secs(30);
/// Popups on screen at once; the oldest beyond it moves into the panel.
pub const POPUP_LIMIT: usize = 3;
/// Entries kept in the in-memory history.
pub const HISTORY_LIMIT: usize = 50;
/// Default lifetime of a low-urgency notification.
pub const LOW_TIMEOUT: Duration = Duration::from_secs(5);
/// Default lifetime of a normal notification.
pub const NORMAL_TIMEOUT: Duration = Duration::from_secs(8);

/// A notification, the same number as its freedesktop id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(pub u32);

/// A Varlink caller, from its socket credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    /// The owning process.
    pub pid: i32,
    /// The name shown in the Tetsu frame.
    pub sender: String,
    /// The caller is the agent or one of its children.
    pub agent: bool,
}

/// A freedesktop `Notify` call, already decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FdoShow {
    /// `app_name`, chosen by the client.
    pub app_name: String,
    /// The D-Bus unique name of the client, for signals.
    pub client: String,
    /// `replaces_id`, 0 for none.
    pub replaces_id: u32,
    /// `summary`, plain text.
    pub summary: String,
    /// `body`, plain text.
    pub body: String,
    /// `actions`: key, label, key, label...
    pub actions: Vec<String>,
    /// From the `urgency` hint.
    pub urgency: Urgency,
    /// From the `category` hint.
    pub category: Option<String>,
    /// `expire_timeout`: -1 default, 0 never, else milliseconds.
    pub expire_timeout: i32,
}

/// Where a notification is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// A popup on screen.
    Popup,
    /// Inside the panel only.
    Panel,
}

/// Who sent a notification, as the user is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A bus caller, named from its credentials: drawn in the Tetsu frame.
    Tetsu(String),
    /// A freedesktop client, by the `app_name` it chose.
    App(String),
}

/// What the state machine asks the outside to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// A reply on the `Show` stream of a note; `last` ends the stream.
    Reply {
        /// The note.
        note: NoteId,
        /// The reply.
        reply: NotifyReply,
        /// The stream ends with it.
        last: bool,
    },
    /// The pending count changed.
    PendingChanged(i64),
    /// freedesktop `ActionInvoked`.
    FdoActionInvoked {
        /// The notification id.
        id: u32,
        /// The action key.
        action: String,
        /// The client to signal.
        client: String,
    },
    /// freedesktop `NotificationClosed`.
    FdoClosed {
        /// The notification id.
        id: u32,
        /// 1 expired, 2 dismissed, 3 closed by a call.
        reason: u32,
        /// The client to signal.
        client: String,
    },
}

/// Why `Show` or `Close` failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowError {
    /// The agent sent actions.
    PermissionDenied,
    /// A parameter is not acceptable; names it.
    InvalidParameter(&'static str),
    /// No notification of this caller has this id.
    NotFound,
}

/// Why a click or dismissal did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The session is locked.
    Locked,
    /// The buttons are not armed yet.
    NotArmed,
    /// The note has no such action.
    NoSuchAction,
    /// The note is gone.
    Gone,
}

/// How a notification ended, for the history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// An action was chosen; its label.
    Invoked(String),
    /// The user dismissed it.
    Dismissed,
    /// It expired.
    Expired,
    /// Its sender closed it.
    Closed,
    /// Its caller went away.
    Withdrawn,
}

/// One live notification as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteView {
    /// The note.
    pub id: NoteId,
    /// Who sent it.
    pub source: Source,
    /// One line.
    pub title: String,
    /// Plain text.
    pub body: String,
    /// Its urgency.
    pub urgency: Urgency,
    /// The buttons, without a freedesktop `default` action.
    pub buttons: Vec<Action>,
    /// Clicking the body invokes `default`.
    pub default_action: bool,
    /// When the buttons accept clicks.
    pub armed_at: Instant,
}

/// One past notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryView {
    /// Who sent it.
    pub source: Source,
    /// One line.
    pub title: String,
    /// Plain text.
    pub body: String,
    /// How it ended.
    pub outcome: Outcome,
}

/// Everything the surfaces draw.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct View {
    /// The session is locked: nothing is drawn.
    pub locked: bool,
    /// Popups, oldest first.
    pub popups: Vec<NoteView>,
    /// Pending approvals, for the top of the panel.
    pub pending: Vec<NoteView>,
    /// Other live notifications in the panel.
    pub open: Vec<NoteView>,
    /// Past notifications, newest first.
    pub history: Vec<HistoryView>,
}
