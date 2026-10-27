//! The window and workspace model.
//!
//! Operations go in, contract-ordered events come out at `flush`: `opened`
//! before any `focused` that names the window, `focused` for the next
//! window before `closed`, at most one `changed` per window per flush, and
//! `added` before `activated` and `focused` for a workspace.

mod outputs;
mod windows;
mod workspaces;

use tetsu_bus::output::{ListOutputsReply, Mode, OutputEvent};
use tetsu_bus::window::{ListWindowsReply, Placement, WindowEvent};
use tetsu_bus::workspace::{ListWorkspacesReply, WorkspaceEvent};

use crate::layout::Rect;
pub use outputs::OutputInfo;
use outputs::Outputs;
use windows::Windows;
pub use windows::{WindowInfo, WindowUpdate};
use workspaces::Workspaces;

/// What changed since the last flush, per interface.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Events {
    /// For `WatchWindows`.
    pub window: Vec<WindowEvent>,
    /// For `WatchWorkspaces`.
    pub workspace: Vec<WorkspaceEvent>,
    /// For `WatchOutputs`.
    pub output: Vec<OutputEvent>,
}

impl Events {
    /// Nothing changed.
    pub fn is_empty(&self) -> bool {
        self.window.is_empty() && self.workspace.is_empty() && self.output.is_empty()
    }
}

/// Windows, workspaces and outputs.
#[derive(Debug, Default)]
pub struct Model {
    windows: Windows,
    workspaces: Workspaces,
    outputs: Outputs,
}

impl Model {
    /// Adds an output with one 1920x1080 mode; see `add_output_info`.
    pub fn add_output(&mut self, name: &str) -> String {
        self.add_output_info(OutputInfo::named(name))
    }

    /// Adds an output; returns its id. Workspaces that were on an output
    /// of the same name come back, workspaces left without an output are
    /// adopted, and an output with none gets a fresh one.
    pub fn add_output_info(&mut self, info: OutputInfo) -> String {
        let name = info.name.clone();
        let before = self.workspaces.focused().map(str::to_owned);
        let id = self.outputs.add(info);
        self.workspaces.add_output(&id, &name);
        // Windows opened while no output existed sit on no workspace.
        if let Some(focused) = self.workspaces.focused().map(str::to_owned) {
            self.windows.adopt_orphans(&focused);
        }
        self.follow_focus(before);
        id
    }

    /// Unplugs an output. Its workspaces move to the first remaining
    /// output (after its highest index) and return when an output of the
    /// same name does; with none left they lose their output. Unknown
    /// ids are ignored.
    pub fn remove_output(&mut self, id: &str) {
        if !self.outputs.contains(id) {
            return;
        }
        let target = self.outputs.first_except(id).map(str::to_owned);
        let before = self.workspaces.focused().map(str::to_owned);
        self.outputs.remove(id);
        self.workspaces.remove_output(id, target.as_deref());
        self.follow_focus(before);
    }

    /// Replaces an output's modes and current mode. Unknown ids are
    /// ignored.
    pub fn set_output_mode(&mut self, id: &str, modes: Vec<Mode>, current: Option<usize>) {
        self.outputs.set_mode(id, modes, current);
    }

    /// The window focus follows the workspace focus when that moved.
    fn follow_focus(&mut self, before: Option<String>) {
        let now = self.workspaces.focused().map(str::to_owned);
        if now == before {
            return;
        }
        let top = now.and_then(|ws| self.windows.top_on(&ws));
        self.windows.focus(top.as_deref());
    }

    /// The enabled outputs and their rects in the global logical space,
    /// in the order they were added.
    pub fn output_layout(&self) -> Vec<(String, Rect)> {
        self.outputs.layout()
    }

    /// For `ListOutputs`.
    pub fn outputs_snapshot(&self) -> ListOutputsReply {
        self.outputs.snapshot()
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
            output: self.outputs.flush(),
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

    /// The workspace of an output at an index, if it exists (nothing is
    /// created).
    pub fn find_workspace(&self, output_id: &str, index: i64) -> Option<String> {
        self.workspaces.find(output_id, index)
    }

    /// A workspace's index on its output.
    pub fn workspace_index(&self, workspace_id: &str) -> Option<i64> {
        self.workspaces.index_of(workspace_id)
    }

    /// The output a workspace is on.
    pub fn output_of(&self, workspace_id: &str) -> Option<&str> {
        self.workspaces.output_of(workspace_id)
    }

    /// The output of the focused workspace.
    pub fn focused_output(&self) -> Option<&str> {
        self.output_of(self.focused_workspace()?)
    }

    /// The workspaces of an output, in the order they were created.
    pub fn workspaces_of(&self, output_id: &str) -> Vec<String> {
        self.workspaces.of_output(output_id)
    }

    /// The id of the output added with `name`.
    pub fn output_named(&self, name: &str) -> Option<String> {
        self.workspaces.output_named(name)
    }

    /// The name an output was added with.
    pub fn output_name(&self, output_id: &str) -> Option<&str> {
        self.workspaces.output_name(output_id)
    }
}
