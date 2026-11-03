//! A mirror of the compositor's workspaces, built from `WatchWorkspaces`.
//!
//! Every event gives a new [`Mirror`]; the one it was applied to is left as
//! it was. Before the `initial` event nothing is known, so other events are
//! ignored.

use std::collections::BTreeMap;

use tetsu_bus::workspace::{Workspace, WorkspaceEvent, WorkspaceKind};

/// One workspace as the bar draws it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsCell {
    /// The configured name, else the 1-based position on its output.
    pub label: String,
    /// The active workspace of some output.
    pub active: bool,
    /// The focused workspace.
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Known {
    workspaces: BTreeMap<String, Workspace>,
    /// Output id to its active workspace id.
    active: BTreeMap<String, String>,
    focused: Option<String>,
}

/// The workspaces known so far.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Mirror {
    known: Option<Known>,
}

impl Mirror {
    /// The mirror after `event`.
    pub fn apply(&self, event: &WorkspaceEvent) -> Mirror {
        if event.kind == WorkspaceKind::Initial {
            return Mirror {
                known: Some(initial(event)),
            };
        }
        let Some(known) = &self.known else {
            return self.clone();
        };
        Mirror {
            known: Some(delta(known, event)),
        }
    }

    /// The cells in drawing order: by output, then position.
    pub fn strip(&self) -> Vec<WsCell> {
        let Some(known) = &self.known else {
            return Vec::new();
        };
        let mut sorted: Vec<&Workspace> = known.workspaces.values().collect();
        sorted.sort_by(|a, b| (&a.output_id, a.index, &a.id).cmp(&(&b.output_id, b.index, &b.id)));
        sorted
            .into_iter()
            .map(|w| WsCell {
                label: w.name.clone().unwrap_or_else(|| (w.index + 1).to_string()),
                active: known.active.values().any(|id| *id == w.id),
                focused: known.focused.as_deref() == Some(w.id.as_str()),
            })
            .collect()
    }
}

fn initial(event: &WorkspaceEvent) -> Known {
    Known {
        workspaces: event
            .workspaces
            .iter()
            .flatten()
            .map(|w| (w.id.clone(), w.clone()))
            .collect(),
        active: event.active.clone().unwrap_or_default(),
        focused: event.focused_id.clone().flatten(),
    }
}

fn delta(known: &Known, event: &WorkspaceEvent) -> Known {
    let mut next = known.clone();
    match event.kind {
        WorkspaceKind::Added | WorkspaceKind::Changed => {
            if let Some(w) = &event.workspace {
                next.workspaces.insert(w.id.clone(), w.clone());
            }
        }
        WorkspaceKind::Removed => {
            if let Some(Some(id)) = &event.id {
                next.workspaces.remove(id);
            }
        }
        WorkspaceKind::Activated => {
            if let (Some(id), Some(output)) = (&event.id, &event.output_id) {
                match id {
                    Some(id) => next.active.insert(output.clone(), id.clone()),
                    None => next.active.remove(output),
                };
            }
        }
        WorkspaceKind::Focused => next.focused = event.focused_id.clone().flatten(),
        WorkspaceKind::Initial | WorkspaceKind::Unknown => {}
    }
    next
}
