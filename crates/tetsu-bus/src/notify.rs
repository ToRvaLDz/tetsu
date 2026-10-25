//! `org.tetsu.notify.v1`: the wire types, shaped exactly as the IDL.

use serde::{Deserialize, Serialize};

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.notify.v1";
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
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ShowParams {
    /// One line.
    pub title: String,
    /// Plain text.
    #[serde(default)]
    pub body: Option<String>,
    /// Null for normal.
    #[serde(default)]
    pub urgency: Option<Urgency>,
    /// The buttons.
    #[serde(default)]
    pub actions: Option<Vec<Action>>,
    /// Null for the server default.
    #[serde(default)]
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
