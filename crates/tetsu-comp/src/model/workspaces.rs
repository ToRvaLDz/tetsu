//! The outputs' workspaces: which is active per output, which has focus.

use std::collections::BTreeMap;

use tetsu_bus::workspace::{ListWorkspacesReply, Workspace, WorkspaceEvent};

#[derive(Debug)]
struct Output {
    id: String,
    name: String,
}

/// Where a workspace lived before its output was unplugged.
#[derive(Debug, Clone)]
struct Home {
    /// The connector name.
    name: String,
    index: i64,
    was_active: bool,
}

#[derive(Debug, Default)]
pub(super) struct Workspaces {
    list: Vec<Workspace>,
    outputs: Vec<Output>,
    next_workspace: u64,
    /// Workspace id to the home it was moved away from.
    homes: BTreeMap<String, Home>,
    /// Output id to its active workspace id.
    active: BTreeMap<String, String>,
    focused: Option<String>,
    events: Vec<WorkspaceEvent>,
}

impl Workspaces {
    /// Adds output `id` named `name`: workspaces homed on that name come
    /// back at their old indices, workspaces without an output are
    /// adopted after them, and an output left empty gets a fresh one.
    pub(super) fn add_output(&mut self, id: &str, name: &str) {
        self.outputs.push(Output {
            id: id.to_owned(),
            name: name.to_owned(),
        });
        let mut sources = Vec::new();
        let returned = self.take_home(id, name, &mut sources);
        let adopted = self.adopt(id);
        let active = self
            .pick_active(&returned)
            .or_else(|| self.pick_active(&adopted));
        for source in sources {
            self.fix_active(&source);
        }
        let active = active.unwrap_or_else(|| self.create(id, 0));
        self.active.insert(id.to_owned(), active.clone());
        self.events
            .push(WorkspaceEvent::activated(Some(active.clone()), id));
        if self.focused.is_none() {
            self.focused = Some(active.clone());
            self.events.push(WorkspaceEvent::focused(Some(active)));
        }
    }

    /// Moves the workspaces homed on `name` to output `id` at their old
    /// indices; `sources` collects the outputs they left. Returns each
    /// with whether it was the active one at home.
    fn take_home(
        &mut self,
        id: &str,
        name: &str,
        sources: &mut Vec<String>,
    ) -> Vec<(String, bool)> {
        let mut homed: Vec<(i64, String, bool)> = self
            .homes
            .iter()
            .filter(|(_, home)| home.name == name)
            .map(|(ws, home)| (home.index, ws.clone(), home.was_active))
            .collect();
        homed.sort();
        let mut returned = Vec::new();
        for (index, ws, was_active) in homed {
            if let Some(source) = self.output_of(&ws).map(str::to_owned)
                && !sources.contains(&source)
            {
                sources.push(source);
            }
            self.place(&ws, Some(id), index);
            self.homes.remove(&ws);
            returned.push((ws, was_active));
        }
        returned
    }

    /// Moves the workspaces without an output to `id`, after its highest
    /// index, in index order. They keep their homes.
    fn adopt(&mut self, id: &str) -> Vec<(String, bool)> {
        let mut orphans: Vec<(i64, String)> = self
            .list
            .iter()
            .filter(|w| w.output_id.is_none())
            .map(|w| (w.index, w.id.clone()))
            .collect();
        orphans.sort();
        let mut adopted = Vec::new();
        for (_, ws) in orphans {
            let next = self.highest(id).map_or(0, |h| h + 1);
            self.place(&ws, Some(id), next);
            let was_active = self.homes.get(&ws).is_some_and(|h| h.was_active);
            adopted.push((ws, was_active));
        }
        adopted
    }

    /// The workspace to activate among those that just moved to an output:
    /// the focused one, else the one that was active at home, else the
    /// lowest.
    fn pick_active(&self, moved: &[(String, bool)]) -> Option<String> {
        let focused = moved
            .iter()
            .find(|(ws, _)| Some(ws) == self.focused.as_ref());
        let active = moved.iter().find(|(_, was_active)| *was_active);
        let lowest = moved.iter().min_by_key(|(ws, _)| self.index_of(ws));
        focused.or(active).or(lowest).map(|(ws, _)| ws.clone())
    }

    /// Sets a workspace's output and index and records a `changed`.
    fn place(&mut self, ws: &str, output: Option<&str>, index: i64) {
        let Some(workspace) = self.list.iter_mut().find(|w| w.id == ws) else {
            return;
        };
        workspace.output_id = output.map(str::to_owned);
        workspace.index = index;
        self.events.push(WorkspaceEvent::changed(workspace.clone()));
    }

    fn highest(&self, output_id: &str) -> Option<i64> {
        self.list
            .iter()
            .filter(|w| w.output_id.as_deref() == Some(output_id))
            .map(|w| w.index)
            .max()
    }

    fn create(&mut self, output_id: &str, index: i64) -> String {
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
        id
    }

    /// After workspaces left `output_id`: if its active one is gone, the
    /// lowest remaining one takes over, or the output has none.
    fn fix_active(&mut self, output_id: &str) {
        let Some(active) = self.active.get(output_id).cloned() else {
            return;
        };
        if self.output_of(&active) == Some(output_id) {
            return;
        }
        let next = self
            .list
            .iter()
            .filter(|w| w.output_id.as_deref() == Some(output_id))
            .min_by_key(|w| w.index)
            .map(|w| w.id.clone());
        match &next {
            Some(ws) => self.active.insert(output_id.to_owned(), ws.clone()),
            None => self.active.remove(output_id),
        };
        self.events.push(WorkspaceEvent::activated(next, output_id));
    }

    /// Removes output `id`. Its workspaces move to `target` after its
    /// highest index, in index order (or lose their output when there is
    /// none), each remembering its home.
    pub(super) fn remove_output(&mut self, id: &str, target: Option<&str>) {
        let Some(at) = self.outputs.iter().position(|o| o.id == id) else {
            return;
        };
        let output = self.outputs.remove(at);
        let was_active = self.active.remove(id);
        let mut moved: Vec<(i64, String)> = self
            .list
            .iter()
            .filter(|w| w.output_id.as_deref() == Some(id))
            .map(|w| (w.index, w.id.clone()))
            .collect();
        moved.sort();
        let mut next = target.and_then(|t| self.highest(t)).map_or(0, |h| h + 1);
        for (index, ws) in &moved {
            self.homes.entry(ws.clone()).or_insert_with(|| Home {
                name: output.name.clone(),
                index: *index,
                was_active: was_active.as_deref() == Some(ws.as_str()),
            });
            match target {
                Some(t) => {
                    self.place(ws, Some(t), next);
                    next += 1;
                }
                None => self.place(ws, None, *index),
            }
        }
        if was_active.is_some() {
            self.events.push(WorkspaceEvent::activated(None, id));
        }
        let focus_moved = moved
            .iter()
            .any(|(_, ws)| Some(ws) == self.focused.as_ref());
        if focus_moved {
            let heir = target.and_then(|t| self.active.get(t).cloned());
            self.focused = heir.clone();
            self.events.push(WorkspaceEvent::focused(heir));
        }
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
        Some(self.create(output_id, index))
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
