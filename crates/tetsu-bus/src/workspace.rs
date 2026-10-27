//! `org.tetsu.comp.workspace.v1`: the wire types, shaped exactly as the IDL.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::nullable::present;

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.comp.workspace.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.comp.workspace.v1.varlink");
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.comp.workspace.v1.PermissionDenied";

/// `Workspace`. The nullable fields are always written, as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// Opaque; compare for equality only.
    pub id: String,
    /// From the user configuration, null when unnamed.
    #[serde(default)]
    pub name: Option<String>,
    /// The output it lives on.
    #[serde(default)]
    pub output_id: Option<String>,
    /// Position on its output, 0-based.
    pub index: i64,
}

/// `WorkspaceEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceKind {
    /// The first event of a stream.
    Initial,
    /// A workspace appeared.
    Added,
    /// A workspace's state changed.
    Changed,
    /// A workspace went away.
    Removed,
    /// The active workspace of an output changed.
    Activated,
    /// The focused workspace changed.
    Focused,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchWorkspaces`. Absent fields are omitted, except
/// `focused_id` on `initial` and `focused`, and `id` on `activated`, which
/// are `null` when empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEvent {
    /// Which event.
    pub kind: WorkspaceKind,
    /// `initial`: every workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Vec<Workspace>>,
    /// `initial`: output id to its active workspace id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<BTreeMap<String, String>>,
    /// `initial`, `focused`: the focused workspace.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub focused_id: Option<Option<String>>,
    /// `added`, `changed`: the full new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<Workspace>,
    /// `removed`; `activated` (null: the output has no active workspace).
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub id: Option<Option<String>>,
    /// `activated`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_id: Option<String>,
}

impl WorkspaceEvent {
    fn of(kind: WorkspaceKind) -> Self {
        Self {
            kind,
            workspaces: None,
            active: None,
            focused_id: None,
            workspace: None,
            id: None,
            output_id: None,
        }
    }

    /// `initial`.
    pub fn initial(
        workspaces: Vec<Workspace>,
        active: BTreeMap<String, String>,
        focused_id: Option<String>,
    ) -> Self {
        Self {
            workspaces: Some(workspaces),
            active: Some(active),
            focused_id: Some(focused_id),
            ..Self::of(WorkspaceKind::Initial)
        }
    }

    /// `added`.
    pub fn added(workspace: Workspace) -> Self {
        Self {
            workspace: Some(workspace),
            ..Self::of(WorkspaceKind::Added)
        }
    }

    /// `changed`.
    pub fn changed(workspace: Workspace) -> Self {
        Self {
            workspace: Some(workspace),
            ..Self::of(WorkspaceKind::Changed)
        }
    }

    /// `removed`.
    pub fn removed(id: &str) -> Self {
        Self {
            id: Some(Some(id.to_owned())),
            ..Self::of(WorkspaceKind::Removed)
        }
    }

    /// `activated`; `id` is `None` when the output has no active workspace.
    pub fn activated(id: Option<String>, output_id: &str) -> Self {
        Self {
            id: Some(id),
            output_id: Some(output_id.to_owned()),
            ..Self::of(WorkspaceKind::Activated)
        }
    }

    /// `focused`.
    pub fn focused(focused_id: Option<String>) -> Self {
        Self {
            focused_id: Some(focused_id),
            ..Self::of(WorkspaceKind::Focused)
        }
    }
}

/// The reply of `ListWorkspaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListWorkspacesReply {
    /// Every workspace.
    pub workspaces: Vec<Workspace>,
    /// Output id to its active workspace id.
    pub active: BTreeMap<String, String>,
    /// The focused workspace, null when none.
    #[serde(default)]
    pub focused_id: Option<String>,
}
