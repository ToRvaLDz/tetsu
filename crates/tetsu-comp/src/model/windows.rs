//! The windows: their state, the focus stack and the events they emit.

use tetsu_bus::window::{ListWindowsReply, Placement, Window, WindowEvent, WindowKind};

/// What the compositor knows about a window when it maps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowInfo {
    /// xdg app_id.
    pub app_id: Option<String>,
    /// The title.
    pub title: Option<String>,
    /// The client's process id.
    pub pid: Option<i64>,
}

/// A change to the text fields of a window. `None` leaves a field alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowUpdate {
    /// The new app_id.
    pub app_id: Option<Option<String>>,
    /// The new title.
    pub title: Option<Option<String>>,
}

impl WindowUpdate {
    /// Sets the title.
    pub fn title(title: &str) -> Self {
        Self {
            title: Some(Some(title.to_owned())),
            ..Self::default()
        }
    }

    /// Sets the app_id.
    pub fn app_id(app_id: &str) -> Self {
        Self {
            app_id: Some(Some(app_id.to_owned())),
            ..Self::default()
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Windows {
    /// In the order they opened.
    list: Vec<Window>,
    next: u64,
    focused: Option<String>,
    /// Most recently focused last.
    stack: Vec<String>,
    /// Windows changed since the last flush, in the order they changed.
    dirty: Vec<String>,
    events: Vec<WindowEvent>,
}

impl Windows {
    pub(super) fn open(&mut self, info: WindowInfo, workspace_id: Option<String>) -> String {
        self.next += 1;
        let window = Window {
            id: format!("win-{}", self.next),
            app_id: info.app_id,
            title: info.title,
            pid: info.pid,
            workspace_id,
            placement: Placement::Tiled,
            fullscreen: false,
            xwayland: false,
        };
        let id = window.id.clone();
        // A window nobody focused yet is the least recently used.
        self.stack.insert(0, id.clone());
        self.events.push(WindowEvent::opened(window.clone()));
        self.list.push(window);
        id
    }

    fn mark_dirty(&mut self, id: &str) {
        if !self.dirty.iter().any(|d| d == id) {
            self.dirty.push(id.to_owned());
        }
    }

    /// The next window in the focus stack on `workspace_id`, skipping `except`.
    fn next_on(&self, workspace_id: Option<&str>, except: &str) -> Option<String> {
        self.stack
            .iter()
            .rev()
            .find(|s| {
                s.as_str() != except
                    && self
                        .list
                        .iter()
                        .any(|w| &w.id == *s && w.workspace_id.as_deref() == workspace_id)
            })
            .cloned()
    }

    /// The most recently focused window on a workspace.
    pub(super) fn top_on(&self, workspace_id: &str) -> Option<String> {
        self.next_on(Some(workspace_id), "")
    }

    /// Moves a window to a workspace. If it had the focus, the next
    /// window of the old workspace gets it. Returns whether it moved.
    pub(super) fn move_to(&mut self, id: &str, workspace_id: &str) -> bool {
        let Some(window) = self.list.iter_mut().find(|w| w.id == id) else {
            return false;
        };
        if window.workspace_id.as_deref() == Some(workspace_id) {
            return false;
        }
        let old = window.workspace_id.replace(workspace_id.to_owned());
        if self.focused.as_deref() == Some(id) {
            let next = self.next_on(old.as_deref(), id);
            self.set_focus(next);
        }
        self.mark_dirty(id);
        true
    }

    pub(super) fn set_placement(&mut self, id: &str, placement: Placement) {
        let Some(window) = self.list.iter_mut().find(|w| w.id == id) else {
            return;
        };
        if window.placement == placement {
            return;
        }
        window.placement = placement;
        let current = window.clone();
        // A window that has not been flushed yet opens with its placement.
        let pending = self.events.iter_mut().find(|e| {
            e.kind == WindowKind::Opened && e.window.as_ref().is_some_and(|w| w.id == id)
        });
        match pending {
            Some(event) => event.window = Some(current),
            None => self.mark_dirty(id),
        }
    }

    pub(super) fn update(&mut self, id: &str, update: WindowUpdate) {
        let Some(window) = self.list.iter_mut().find(|w| w.id == id) else {
            return;
        };
        let mut changed = false;
        if let Some(app_id) = update.app_id {
            changed |= window.app_id != app_id;
            window.app_id = app_id;
        }
        if let Some(title) = update.title {
            changed |= window.title != title;
            window.title = title;
        }
        if changed {
            self.mark_dirty(id);
        }
    }

    pub(super) fn close(&mut self, id: &str) {
        let Some(at) = self.list.iter().position(|w| w.id == id) else {
            return;
        };
        let window = self.list.remove(at);
        self.stack.retain(|s| s != id);
        self.dirty.retain(|d| d != id);
        if self.focused.as_deref() == Some(id) {
            // The next in the focus stack, on the same workspace.
            let next = self.next_on(window.workspace_id.as_deref(), id);
            self.set_focus(next);
        }
        self.events.push(WindowEvent::closed(id));
    }

    /// Focuses `id`; returns the window's workspace when the focus moved.
    pub(super) fn focus(&mut self, id: Option<&str>) -> Option<String> {
        let id = match id {
            Some(id) if self.list.iter().any(|w| w.id == id) => Some(id.to_owned()),
            Some(_) => return None,
            None => None,
        };
        if id == self.focused {
            return None;
        }
        if let Some(id) = &id {
            self.stack.retain(|s| s != id);
            self.stack.push(id.clone());
        }
        self.set_focus(id.clone());
        id.and_then(|id| self.workspace_of(&id).map(str::to_owned))
    }

    fn set_focus(&mut self, id: Option<String>) {
        self.focused = id.clone();
        self.events.push(WindowEvent::focused(id));
    }

    pub(super) fn flush(&mut self) -> Vec<WindowEvent> {
        let mut events = std::mem::take(&mut self.events);
        for id in std::mem::take(&mut self.dirty) {
            if let Some(window) = self.list.iter().find(|w| w.id == id) {
                events.push(WindowEvent::changed(window.clone()));
            }
        }
        events
    }

    pub(super) fn snapshot(&self) -> ListWindowsReply {
        ListWindowsReply {
            windows: self.list.clone(),
            focused_id: self.focused.clone(),
        }
    }

    pub(super) fn workspace_of(&self, id: &str) -> Option<&str> {
        let window = self.list.iter().find(|w| w.id == id)?;
        window.workspace_id.as_deref()
    }

    pub(super) fn on_workspace(&self, workspace_id: &str) -> Vec<String> {
        self.list
            .iter()
            .filter(|w| w.workspace_id.as_deref() == Some(workspace_id))
            .map(|w| w.id.clone())
            .collect()
    }

    pub(super) fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }
}
