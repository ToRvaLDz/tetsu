//! The outputs' workspaces: which is active per output, which has focus.

use std::collections::BTreeMap;

use tetsu_bus::workspace::{ListWorkspacesReply, Workspace, WorkspaceEvent};

#[derive(Debug)]
struct Output {
    id: String,
    name: String,
}

#[derive(Debug, Default)]
pub(super) struct Workspaces {
    list: Vec<Workspace>,
    outputs: Vec<Output>,
    next_workspace: u64,
    next_output: u64,
    /// Output id to its active workspace id.
    active: BTreeMap<String, String>,
    focused: Option<String>,
    events: Vec<WorkspaceEvent>,
}

impl Workspaces {
    pub(super) fn add_output(&mut self, name: &str) -> String {
        self.next_output += 1;
        self.next_workspace += 1;
        let output = Output {
            id: format!("out-{}", self.next_output),
            name: name.to_owned(),
        };
        let workspace = Workspace {
            id: format!("ws-{}", self.next_workspace),
            name: None,
            output_id: Some(output.id.clone()),
            index: 0,
        };
        let out_id = output.id.clone();
        self.outputs.push(output);
        self.events.push(WorkspaceEvent::added(workspace.clone()));
        self.active.insert(out_id.clone(), workspace.id.clone());
        self.events.push(WorkspaceEvent::activated(
            Some(workspace.id.clone()),
            &out_id,
        ));
        if self.focused.is_none() {
            self.focused = Some(workspace.id.clone());
            self.events
                .push(WorkspaceEvent::focused(Some(workspace.id.clone())));
        }
        self.list.push(workspace);
        out_id
    }

    /// The workspace of `output_id` at `index`, created (`added`, never
    /// `activated`) when it does not exist. `None` for an unknown output.
    pub(super) fn at(&mut self, output_id: &str, index: i64) -> Option<String> {
        if !self.outputs.iter().any(|o| o.id == output_id) {
            return None;
        }
        if let Some(found) = self.find(output_id, index) {
            return Some(found);
        }
        self.next_workspace += 1;
        let workspace = Workspace {
            id: format!("ws-{}", self.next_workspace),
            name: None,
            output_id: Some(output_id.to_owned()),
            index,
        };
        let id = workspace.id.clone();
        self.events.push(WorkspaceEvent::added(workspace.clone()));
        self.list.push(workspace);
        Some(id)
    }

    /// The workspace of an output at an index, if it exists.
    pub(super) fn find(&self, output_id: &str, index: i64) -> Option<String> {
        self.list
            .iter()
            .find(|w| w.output_id.as_deref() == Some(output_id) && w.index == index)
            .map(|w| w.id.clone())
    }

    pub(super) fn index_of(&self, id: &str) -> Option<i64> {
        self.list.iter().find(|w| w.id == id).map(|w| w.index)
    }

    pub(super) fn output_of(&self, id: &str) -> Option<&str> {
        let workspace = self.list.iter().find(|w| w.id == id)?;
        workspace.output_id.as_deref()
    }

    pub(super) fn of_output(&self, output_id: &str) -> Vec<String> {
        self.list
            .iter()
            .filter(|w| w.output_id.as_deref() == Some(output_id))
            .map(|w| w.id.clone())
            .collect()
    }

    pub(super) fn output_named(&self, name: &str) -> Option<String> {
        self.outputs
            .iter()
            .find(|o| o.name == name)
            .map(|o| o.id.clone())
    }

    pub(super) fn exists(&self, id: &str) -> bool {
        self.list.iter().any(|w| w.id == id)
    }

    /// Makes `id` the active workspace of its output and the focused one.
    pub(super) fn focus(&mut self, id: &str) {
        let Some(output) = self
            .list
            .iter()
            .find(|w| w.id == id)
            .and_then(|w| w.output_id.clone())
        else {
            return;
        };
        if self.active.get(&output).map(String::as_str) != Some(id) {
            self.active.insert(output.clone(), id.to_owned());
            self.events
                .push(WorkspaceEvent::activated(Some(id.to_owned()), &output));
        }
        if self.focused.as_deref() != Some(id) {
            self.focused = Some(id.to_owned());
            self.events
                .push(WorkspaceEvent::focused(Some(id.to_owned())));
        }
    }

    pub(super) fn flush(&mut self) -> Vec<WorkspaceEvent> {
        std::mem::take(&mut self.events)
    }

    pub(super) fn snapshot(&self) -> ListWorkspacesReply {
        ListWorkspacesReply {
            workspaces: self.list.clone(),
            active: self.active.clone(),
            focused_id: self.focused.clone(),
        }
    }

    pub(super) fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    pub(super) fn active(&self, output_id: &str) -> Option<&str> {
        self.active.get(output_id).map(String::as_str)
    }

    pub(super) fn output_name(&self, output_id: &str) -> Option<&str> {
        let output = self.outputs.iter().find(|o| o.id == output_id)?;
        Some(&output.name)
    }
}
