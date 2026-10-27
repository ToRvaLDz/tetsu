//! The window and workspace model.
//!
//! Operations go in, contract-ordered events come out at `flush`: `opened`
//! before any `focused` that names the window, `focused` for the next
//! window before `closed`, at most one `changed` per window per flush, and
//! `added` before `activated` and `focused` for a workspace.

mod windows;
mod workspaces;

use tetsu_bus::window::{ListWindowsReply, Placement, WindowEvent};
use tetsu_bus::workspace::{ListWorkspacesReply, WorkspaceEvent};

use windows::Windows;
pub use windows::{WindowInfo, WindowUpdate};
use workspaces::Workspaces;

/// What changed since the last flush, per interface.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Events {
    /// For `WatchWindows`.
    pub window: Vec<WindowEvent>,
    /// For `WatchWorkspaces`.
    pub workspace: Vec<WorkspaceEvent>,
}

impl Events {
    /// Nothing changed.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty() && self.workspace.is_empty()
    }
}

/// Windows, workspaces and outputs.
#[derive(Debug, Default)]
pub struct Model {
    windows: Windows,
    workspaces: Workspaces,
}

impl Model {
    /// Adds an output with its first workspace; returns the output id.
    pub fn add_output(&mut self, name: &str) -> String {
        self.workspaces.add_output(name)
    }

    /// Maps a window on the focused workspace; returns its id. It is not
    /// focused: call `focus`.
    pub fn open(&mut self, info: WindowInfo) -> String {
        let workspace = self.workspaces.focused().map(str::to_owned);
        self.windows.open(info, workspace)
    }

    /// Maps a window on `workspace_id` (the focused workspace when it is
    /// unknown). It is not focused: call `focus`.
    pub fn open_on(&mut self, info: WindowInfo, workspace_id: &str) -> String {
        if !self.workspaces.exists(workspace_id) {
            return self.open(info);
        }
        self.windows.open(info, Some(workspace_id.to_owned()))
    }

    /// Moves a window to another workspace, which stays as it is. If the
    /// window was focused, the next one on the old workspace is. Unknown
    /// ids and a no-op move are ignored.
    pub fn move_to_workspace(&mut self, id: &str, workspace_id: &str) {
        if self.workspaces.exists(workspace_id) {
            self.windows.move_to(id, workspace_id);
        }
    }

    /// Sets a window's placement; nothing is emitted when it is the same.
    pub fn set_placement(&mut self, id: &str, placement: Placement) {
        self.windows.set_placement(id, placement);
    }

    /// The workspace of an output at an index, created on demand with an
    /// `added` event. `None` for an unknown output.
    pub fn workspace_at(&mut self, output_id: &str, index: i64) -> Option<String> {
        self.workspaces.at(output_id, index)
    }

    /// Changes a window's app_id or title. Unknown ids are ignored.
    pub fn update(&mut self, id: &str, update: WindowUpdate) {
        self.windows.update(id, update);
    }

    /// Unmaps a window. Unknown ids are ignored.
    pub fn close(&mut self, id: &str) {
        self.windows.close(id);
    }

    /// Focuses a window, or nothing. Focusing a window on another
    /// workspace focuses that workspace too.
    pub fn focus(&mut self, id: Option<&str>) {
        if let Some(workspace) = self.windows.focus(id) {
            self.workspaces.focus(&workspace);
        }
    }

    /// Makes a workspace the active one of its output and the focused one,
    /// and focuses the most recently focused window on it, or nothing.
    pub fn focus_workspace(&mut self, id: &str) {
        if !self.workspaces.exists(id) {
            return;
        }
        self.workspaces.focus(id);
        let top = self.windows.top_on(id);
        self.windows.focus(top.as_deref());
    }

    /// Takes the events since the last call.
    pub fn flush(&mut self) -> Events {
        Events {
            window: self.windows.flush(),
            workspace: self.workspaces.flush(),
        }
    }

    /// For `ListWindows`.
    pub fn windows_snapshot(&self) -> ListWindowsReply {
        self.windows.snapshot()
    }

    /// For `ListWorkspaces`.
    pub fn workspaces_snapshot(&self) -> ListWorkspacesReply {
        self.workspaces.snapshot()
    }

    /// The window ids on a workspace, in the order they opened.
    pub fn windows_on(&self, workspace_id: &str) -> Vec<String> {
        self.windows.on_workspace(workspace_id)
    }

    /// The workspace a window is on.
    pub fn workspace_of(&self, window_id: &str) -> Option<&str> {
        self.windows.workspace_of(window_id)
    }

    /// The active workspace of an output.
    pub fn active_workspace(&self, output_id: &str) -> Option<&str> {
        self.workspaces.active(output_id)
    }

    /// The focused window.
    pub fn focused_window(&self) -> Option<&str> {
        self.windows.focused()
    }

    /// The focused workspace.
    pub fn focused_workspace(&self) -> Option<&str> {
        self.workspaces.focused()
    }

    /// The name an output was added with.
    pub fn output_name(&self, output_id: &str) -> Option<&str> {
        self.workspaces.output_name(output_id)
    }
}
