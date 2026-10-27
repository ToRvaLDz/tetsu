//! The bus thread's view of the compositor: a mirror of the model, fed by
//! deltas, and the watchers that follow it.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use tetsu_bus::server::StreamItem;
use tetsu_bus::window::{ListWindowsReply, Window, WindowEvent, WindowKind};
use tetsu_bus::workspace::{ListWorkspacesReply, Workspace, WorkspaceEvent, WorkspaceKind};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

use crate::model::{Events, Model};

/// Events a watcher may have queued before it is cut.
pub const WATCH_QUEUE: usize = 256;

/// The full state, to start a mirror from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// For `ListWindows`.
    pub windows: ListWindowsReply,
    /// For `ListWorkspaces`.
    pub workspaces: ListWorkspacesReply,
}

impl Snapshot {
    /// The state of a model.
    pub fn of(model: &Model) -> Self {
        Self {
            windows: model.windows_snapshot(),
            workspaces: model.workspaces_snapshot(),
        }
    }
}

/// The state rebuilt from the events the model emitted.
#[derive(Debug, Clone)]
pub struct Mirror {
    windows: Vec<Window>,
    focused_window: Option<String>,
    workspaces: Vec<Workspace>,
    active: BTreeMap<String, String>,
    focused_workspace: Option<String>,
}

impl Mirror {
    /// Starts from a snapshot.
    pub fn from_snapshot(snapshot: &Snapshot) -> Self {
        Self {
            windows: snapshot.windows.windows.clone(),
            focused_window: snapshot.windows.focused_id.clone(),
            workspaces: snapshot.workspaces.workspaces.clone(),
            active: snapshot.workspaces.active.clone(),
            focused_workspace: snapshot.workspaces.focused_id.clone(),
        }
    }

    /// Applies one window event.
    pub fn apply_window(&mut self, event: &WindowEvent) {
        match event.kind {
            WindowKind::Initial => {
                self.windows = event.windows.clone().unwrap_or_default();
                self.focused_window = event.focused_id.clone().flatten();
            }
            WindowKind::Opened => {
                if let Some(window) = &event.window {
                    self.windows.push(window.clone());
                }
            }
            WindowKind::Changed => {
                if let Some(window) = &event.window
                    && let Some(slot) = self.windows.iter_mut().find(|w| w.id == window.id)
                {
                    *slot = window.clone();
                }
            }
            WindowKind::Closed => {
                if let Some(id) = &event.id {
                    self.windows.retain(|w| &w.id != id);
                }
            }
            WindowKind::Focused => self.focused_window = event.focused_id.clone().flatten(),
            WindowKind::Unknown => {}
        }
    }

    /// Applies one workspace event.
    pub fn apply_workspace(&mut self, event: &WorkspaceEvent) {
        match event.kind {
            WorkspaceKind::Initial => {
                self.workspaces = event.workspaces.clone().unwrap_or_default();
                self.active = event.active.clone().unwrap_or_default();
                self.focused_workspace = event.focused_id.clone().flatten();
            }
            WorkspaceKind::Added => {
                if let Some(workspace) = &event.workspace {
                    self.workspaces.push(workspace.clone());
                }
            }
            WorkspaceKind::Changed => {
                if let Some(workspace) = &event.workspace
                    && let Some(slot) = self.workspaces.iter_mut().find(|w| w.id == workspace.id)
                {
                    *slot = workspace.clone();
                }
            }
            WorkspaceKind::Removed => {
                if let Some(Some(id)) = &event.id {
                    self.workspaces.retain(|w| &w.id != id);
                }
            }
            WorkspaceKind::Activated => {
                if let Some(output) = &event.output_id {
                    match event.id.clone().flatten() {
                        Some(id) => {
                            self.active.insert(output.clone(), id);
                        }
                        None => {
                            self.active.remove(output);
                        }
                    }
                }
            }
            WorkspaceKind::Focused => {
                self.focused_workspace = event.focused_id.clone().flatten();
            }
            WorkspaceKind::Unknown => {}
        }
    }

    /// For `ListWindows`.
    pub fn windows(&self) -> ListWindowsReply {
        ListWindowsReply {
            windows: self.windows.clone(),
            focused_id: self.focused_window.clone(),
        }
    }

    /// For `ListWorkspaces`.
    pub fn workspaces(&self) -> ListWorkspacesReply {
        ListWorkspacesReply {
            workspaces: self.workspaces.clone(),
            active: self.active.clone(),
            focused_id: self.focused_workspace.clone(),
        }
    }

    fn windows_initial(&self) -> WindowEvent {
        WindowEvent::initial(self.windows.clone(), self.focused_window.clone())
    }

    fn workspaces_initial(&self) -> WorkspaceEvent {
        WorkspaceEvent::initial(
            self.workspaces.clone(),
            self.active.clone(),
            self.focused_workspace.clone(),
        )
    }
}

type Watchers = Vec<mpsc::Sender<StreamItem>>;

/// The mirror and the watchers of both interfaces.
#[derive(Debug)]
pub struct Hub {
    mirror: Mirror,
    window_watchers: Watchers,
    workspace_watchers: Watchers,
}

impl Hub {
    /// A hub whose mirror starts from `snapshot`.
    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            mirror: Mirror::from_snapshot(snapshot),
            window_watchers: Vec::new(),
            workspace_watchers: Vec::new(),
        }
    }

    /// The mirror, for the `List*` calls.
    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// Applies the deltas to the mirror and sends them to the watchers. A
    /// watcher whose queue is full or gone is dropped; dropping its sender
    /// cuts its connection.
    pub fn publish(&mut self, events: &Events) {
        for event in &events.window {
            self.mirror.apply_window(event);
            fan_out(&mut self.window_watchers, event);
        }
        for event in &events.workspace {
            self.mirror.apply_workspace(event);
            fan_out(&mut self.workspace_watchers, event);
        }
    }

    /// A new `WatchWindows` stream, `initial` already queued.
    pub fn watch_windows(&mut self) -> mpsc::Receiver<StreamItem> {
        watch(&mut self.window_watchers, &self.mirror.windows_initial())
    }

    /// A new `WatchWorkspaces` stream, `initial` already queued.
    pub fn watch_workspaces(&mut self) -> mpsc::Receiver<StreamItem> {
        watch(
            &mut self.workspace_watchers,
            &self.mirror.workspaces_initial(),
        )
    }
}

fn reply<T: serde::Serialize>(event: &T) -> StreamItem {
    let event: Value = serde_json::to_value(event).unwrap_or(Value::Null);
    StreamItem::Reply(json!({ "event": event }))
}

fn fan_out<T: serde::Serialize>(watchers: &mut Watchers, event: &T) {
    if watchers.is_empty() {
        return;
    }
    let item = reply(event);
    watchers.retain(|watcher| match watcher.try_send(item.clone()) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            eprintln!("tetsu-comp: dropping a bus watcher that does not keep up");
            false
        }
        Err(TrySendError::Closed(_)) => false,
    });
}

fn watch<T: serde::Serialize>(watchers: &mut Watchers, initial: &T) -> mpsc::Receiver<StreamItem> {
    let (tx, rx) = mpsc::channel(WATCH_QUEUE);
    // A fresh queue has room for the first event.
    let _ = tx.try_send(reply(initial));
    watchers.push(tx);
    rx
}
