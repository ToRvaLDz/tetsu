//! The layout engine. It wraps the model and keeps, per workspace, the
//! layout, the window order, the master ratio and the floating rects.
//! Operations go in (open, close, `apply`); contract-ordered events come
//! out of `flush` and an `Arrangement` per output tells the glue where
//! every window goes. No Smithay in here.

mod action;
mod workspace;

use std::collections::BTreeMap;

use tetsu_bus::window::Placement;

pub use action::{Action, Effect};

use crate::layout::dwindle::dwindle;
use crate::layout::floating::{centered, clamp_into, from_tile};
use crate::layout::master::{clamp_ratio, master};
use crate::layout::{LayoutKind, Rect};
use crate::model::{Events, Model, WindowInfo, WindowUpdate};
use crate::rules::{Decision, resolve};
use crate::settings::Settings;
use workspace::WorkspaceState;

/// Master ratio change per `RatioDelta` step, in hundredths.
const RATIO_STEP: i32 = 5;
/// The viewport of an output the glue did not describe.
const FALLBACK_VIEW: Rect = Rect {
    x: 0,
    y: 0,
    w: 1920,
    h: 1080,
};

/// The output ids and the rect each one lays windows out in.
pub type Viewports<'a> = &'a [(String, Rect)];

/// Where a window opened now would go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The output (empty when there is none).
    pub output_id: String,
    /// The workspace index on that output.
    pub workspace_index: i64,
    /// Whether it opens floating.
    pub floating: bool,
    /// The size for the first configure: the tile it would get, or the
    /// rule size when floating; `None` lets the client choose.
    pub configure_size: Option<(i32, i32)>,
}

/// Where every window of one output goes. Rects are absolute.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// Tiled windows with their tiles.
    pub tiled: Vec<(String, Rect)>,
    /// Floating windows with their rects, in layout order.
    pub floating: Vec<(String, Rect)>,
    /// Windows of the output's inactive workspaces.
    pub hidden: Vec<String>,
}

/// The model plus the layout state.
#[derive(Debug)]
pub struct Engine {
    model: Model,
    settings: Settings,
    spaces: BTreeMap<String, WorkspaceState>,
}

impl Engine {
    /// An engine with no outputs.
    pub fn new(settings: Settings) -> Self {
        Self {
            model: Model::default(),
            settings,
            spaces: BTreeMap::new(),
        }
    }

    /// The model, for snapshots and queries.
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The settings it runs with.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Whether a window floats (by its own flag or its workspace's layout).
    pub fn is_floating(&self, id: &str) -> bool {
        self.model
            .workspace_of(id)
            .is_some_and(|ws| self.state(ws).is_floating(id))
    }

    /// Adds an output with its first workspace; returns the output id.
    pub fn add_output(&mut self, name: &str) -> String {
        self.model.add_output(name)
    }

    /// Changes a window's app_id or title.
    pub fn update(&mut self, id: &str, update: WindowUpdate) {
        self.model.update(id, update);
    }

    /// Focuses a window, or nothing.
    pub fn focus(&mut self, id: Option<&str>) {
        self.model.focus(id);
    }

    /// Takes the events since the last call.
    pub fn flush(&mut self) -> Events {
        self.model.flush()
    }

    fn state(&self, workspace: &str) -> WorkspaceState {
        self.spaces.get(workspace).cloned().unwrap_or_else(|| {
            WorkspaceState::new(self.settings.layout, self.settings.master_ratio)
        })
    }

    fn state_mut(&mut self, workspace: &str) -> &mut WorkspaceState {
        let (layout, ratio) = (self.settings.layout, self.settings.master_ratio);
        self.spaces
            .entry(workspace.to_owned())
            .or_insert_with(|| WorkspaceState::new(layout, ratio))
    }

    fn view_of(&self, workspace: &str, views: Viewports<'_>) -> Rect {
        let output = self.model.output_of(workspace);
        views
            .iter()
            .find(|(id, _)| Some(id.as_str()) == output)
            .map_or(FALLBACK_VIEW, |(_, rect)| *rect)
    }

    fn tile_rects(&self, state: &WorkspaceState, view: Rect, n: usize) -> Vec<Rect> {
        let gaps = self.settings.gaps;
        match state.layout {
            LayoutKind::Master => master(view, gaps, n, state.ratio),
            _ => dwindle(view, gaps, n),
        }
    }

    /// The tiles of a workspace, absolute.
    fn tiles_of(&self, workspace: &str, views: Viewports<'_>) -> Vec<(String, Rect)> {
        let state = self.state(workspace);
        let ids = state.tiled();
        let rects = self.tile_rects(&state, self.view_of(workspace, views), ids.len());
        ids.into_iter().zip(rects).collect()
    }

    /// Where the rules send a window: an output and a workspace index.
    fn target(&self, decision: &Decision) -> Option<(String, i64)> {
        let named = decision
            .output
            .as_deref()
            .and_then(|name| self.model.output_named(name));
        let output = named.or_else(|| self.model.focused_output().map(str::to_owned))?;
        let index = match decision.workspace {
            Some(n) => i64::from(n) - 1,
            None => {
                let active = self.model.active_workspace(&output)?;
                self.model.workspace_index(active)?
            }
        };
        Some((output, index))
    }

    /// Where a window with this app_id and title would go and what its
    /// first configure should say. Changes nothing.
    pub fn plan(&self, app_id: Option<&str>, title: Option<&str>, views: Viewports<'_>) -> Plan {
        let decision = resolve(&self.settings.rules, app_id, title);
        let Some((output_id, workspace_index)) = self.target(&decision) else {
            return Plan {
                output_id: String::new(),
                workspace_index: 0,
                floating: false,
                configure_size: None,
            };
        };
        let workspace = self.model.find_workspace(&output_id, workspace_index);
        let state = workspace
            .as_deref()
            .map_or_else(|| self.state("-"), |ws| self.state(ws));
        let floating = decision.floating == Some(true) || state.layout == LayoutKind::Floating;
        let configure_size = if floating {
            decision.size
        } else {
            let view = views
                .iter()
                .find(|(id, _)| *id == output_id)
                .map_or(FALLBACK_VIEW, |(_, rect)| *rect);
            let n = state.tiled().len() + 1;
            self.tile_rects(&state, view, n).last().map(|r| (r.w, r.h))
        };
        Plan {
            output_id,
            workspace_index,
            floating,
            configure_size,
        }
    }

    /// Maps a window where the rules say. It takes the focus only when its
    /// workspace is the focused one. `committed` is the size it came with.
    pub fn open(
        &mut self,
        info: WindowInfo,
        committed: Option<(i32, i32)>,
        views: Viewports<'_>,
    ) -> String {
        let decision = resolve(
            &self.settings.rules,
            info.app_id.as_deref(),
            info.title.as_deref(),
        );
        if let Some(name) = decision.output.as_deref()
            && self.model.output_named(name).is_none()
        {
            eprintln!(
                "tetsu-comp: window rule names unknown output {name:?}, using the focused one"
            );
        }
        let target = self.target(&decision);
        let workspace = target.and_then(|(output, index)| self.model.workspace_at(&output, index));
        let Some(workspace) = workspace else {
            return self.model.open(info);
        };
        let id = self.model.open_on(info, &workspace);
        let state = self.state_mut(&workspace);
        state.order.push(id.clone());
        if decision.floating == Some(true) {
            state.floated.insert(id.clone());
        }
        if state.is_floating(&id) {
            let view = self.view_of(&workspace, views);
            let size = committed
                .filter(|(w, h)| *w > 0 && *h > 0)
                .or(decision.size)
                .unwrap_or((view.w / 2, view.h / 2));
            self.remember(&workspace, &id, view, centered(view, size));
        }
        self.sync_placement(&workspace);
        if self.model.focused_workspace() == Some(workspace.as_str()) {
            self.model.focus(Some(&id));
        }
        id
    }

    /// Forgets a window that went away.
    pub fn close(&mut self, id: &str) {
        self.model.close(id);
        for state in self.spaces.values_mut() {
            state.remove(id);
        }
    }

    /// Stores an absolute rect relative to its viewport.
    fn remember(&mut self, workspace: &str, id: &str, view: Rect, rect: Rect) {
        let relative = Rect {
            x: rect.x - view.x,
            y: rect.y - view.y,
            ..rect
        };
        self.state_mut(workspace)
            .rects
            .insert(id.to_owned(), relative);
    }

    /// Publishes the effective placement of every window of a workspace.
    fn sync_placement(&mut self, workspace: &str) {
        let state = self.state(workspace);
        for id in &state.order {
            let placement = if state.is_floating(id) {
                Placement::Floating
            } else {
                Placement::Tiled
            };
            self.model.set_placement(id, placement);
        }
    }

    /// Gives every floating window without a remembered rect one, from the
    /// tile it had.
    fn remember_missing(
        &mut self,
        workspace: &str,
        tiles: &[(String, Rect)],
        views: Viewports<'_>,
    ) {
        let view = self.view_of(workspace, views);
        let state = self.state(workspace);
        for id in state.order.iter().filter(|id| state.is_floating(id)) {
            if state.rects.contains_key(id) {
                continue;
            }
            let rect = match tiles.iter().find(|(t, _)| t == id) {
                Some((_, tile)) => from_tile(view, *tile),
                None => centered(view, (view.w / 2, view.h / 2)),
            };
            self.remember(workspace, id, view, rect);
        }
    }

    /// Where every window of an output goes.
    pub fn arrange(&self, output_id: &str, viewport: Rect) -> Arrangement {
        let Some(active) = self.model.active_workspace(output_id) else {
            return Arrangement::default();
        };
        let state = self.state(active);
        let ids = state.tiled();
        let tiles = self.tile_rects(&state, viewport, ids.len());
        let floating = state
            .order
            .iter()
            .filter(|id| state.is_floating(id))
            .map(|id| {
                let rect = state.rects.get(id).map_or_else(
                    || centered(viewport, (viewport.w / 2, viewport.h / 2)),
                    |r| Rect {
                        x: r.x + viewport.x,
                        y: r.y + viewport.y,
                        ..*r
                    },
                );
                (id.clone(), rect)
            })
            .collect();
        let hidden = self
            .model
            .workspaces_of(output_id)
            .iter()
            .filter(|ws| ws.as_str() != active)
            .flat_map(|ws| self.model.windows_on(ws))
            .collect();
        Arrangement {
            tiled: ids.into_iter().zip(tiles).collect(),
            floating,
            hidden,
        }
    }

    /// Moves a floating window to `origin` (clamped onto the screen) and
    /// returns its rect; `None` when it does not float.
    pub fn move_floating(&mut self, id: &str, origin: (i32, i32), viewport: Rect) -> Option<Rect> {
        let workspace = self.model.workspace_of(id)?.to_owned();
        let state = self.state(&workspace);
        if !state.is_floating(id) {
            return None;
        }
        let known = state.rects.get(id)?;
        let moved = clamp_into(
            viewport,
            Rect {
                x: origin.0,
                y: origin.1,
                ..*known
            },
        );
        self.remember(&workspace, id, viewport, moved);
        Some(moved)
    }

    /// Runs a bind's action on the focused workspace.
    pub fn apply(&mut self, action: Action, views: Viewports<'_>) -> Effect {
        let Some(workspace) = self.model.focused_workspace().map(str::to_owned) else {
            return Effect::default();
        };
        let focused = self.model.focused_window().map(str::to_owned);
        let relayout = match action {
            Action::FocusNext => self.step_focus(&workspace, 1),
            Action::FocusPrev => self.step_focus(&workspace, -1),
            Action::SwapNext => self.swap(&workspace, focused.as_deref(), 1),
            Action::SwapPrev => self.swap(&workspace, focused.as_deref(), -1),
            Action::PromoteMaster => self.promote(&workspace, focused.as_deref()),
            Action::RatioDelta(steps) => self.change_ratio(&workspace, steps),
            Action::CycleLayout => self.cycle_layout(&workspace, views),
            Action::ToggleFloating => self.toggle_floating(&workspace, focused.as_deref(), views),
            Action::FocusWorkspace(n) => self.show_workspace(n),
            Action::MoveToWorkspace(n) => {
                self.send_to_workspace(&workspace, focused.as_deref(), n, views)
            }
            Action::Close => {
                return Effect {
                    relayout: false,
                    close: focused,
                };
            }
        };
        Effect {
            relayout,
            close: None,
        }
    }

    /// Focus next/prev in layout order. Focus changes need no relayout.
    fn step_focus(&mut self, workspace: &str, delta: isize) -> bool {
        let state = self.state(workspace);
        let len = state.order.len();
        if len == 0 {
            return false;
        }
        let at = self.model.focused_window().and_then(|f| state.position(f));
        let next = match at {
            Some(i) => (i as isize + delta).rem_euclid(len as isize) as usize,
            None if delta > 0 => 0,
            None => len - 1,
        };
        self.model.focus(Some(&state.order[next]));
        false
    }

    fn swap(&mut self, workspace: &str, focused: Option<&str>, delta: isize) -> bool {
        let state = self.state(workspace);
        let len = state.order.len();
        let Some(at) = focused.and_then(|f| state.position(f)) else {
            return false;
        };
        if len < 2 {
            return false;
        }
        let other = (at as isize + delta).rem_euclid(len as isize) as usize;
        self.state_mut(workspace).order.swap(at, other);
        true
    }

    fn promote(&mut self, workspace: &str, focused: Option<&str>) -> bool {
        let state = self.state(workspace);
        let Some(id) = focused else { return false };
        let Some(at) = state.position(id) else {
            return false;
        };
        if state.tiled().first().map(String::as_str) == Some(id) {
            return false;
        }
        let order = &mut self.state_mut(workspace).order;
        let moved = order.remove(at);
        order.insert(0, moved);
        true
    }

    fn change_ratio(&mut self, workspace: &str, steps: i8) -> bool {
        let state = self.state_mut(workspace);
        if state.layout != LayoutKind::Master {
            return false;
        }
        let hundredths = (state.ratio * 100.0).round() as i32 + i32::from(steps) * RATIO_STEP;
        let ratio = clamp_ratio(f64::from(hundredths) / 100.0);
        let changed = ratio != state.ratio;
        state.ratio = ratio;
        changed
    }

    fn cycle_layout(&mut self, workspace: &str, views: Viewports<'_>) -> bool {
        let tiles = self.tiles_of(workspace, views);
        let state = self.state_mut(workspace);
        state.layout = state.layout.next();
        self.remember_missing(workspace, &tiles, views);
        self.sync_placement(workspace);
        true
    }

    fn toggle_floating(
        &mut self,
        workspace: &str,
        focused: Option<&str>,
        views: Viewports<'_>,
    ) -> bool {
        let Some(id) = focused else { return false };
        if self.state(workspace).position(id).is_none() {
            return false;
        }
        let tiles = self.tiles_of(workspace, views);
        let state = self.state_mut(workspace);
        if !state.floated.remove(id) {
            state.floated.insert(id.to_owned());
        }
        self.remember_missing(workspace, &tiles, views);
        self.sync_placement(workspace);
        true
    }

    /// Mod+N: shows workspace n of the focused output, creating it.
    fn show_workspace(&mut self, n: u8) -> bool {
        let Some(index) = workspace_index(n) else {
            return false;
        };
        let Some(output) = self.model.focused_output().map(str::to_owned) else {
            return false;
        };
        let Some(workspace) = self.model.workspace_at(&output, index) else {
            return false;
        };
        self.state_mut(&workspace);
        self.model.focus_workspace(&workspace);
        true
    }

    /// Mod+Shift+N: sends the focused window to workspace n.
    fn send_to_workspace(
        &mut self,
        source: &str,
        focused: Option<&str>,
        n: u8,
        views: Viewports<'_>,
    ) -> bool {
        let (Some(id), Some(index)) = (focused, workspace_index(n)) else {
            return false;
        };
        let Some(output) = self.model.output_of(source).map(str::to_owned) else {
            return false;
        };
        if self.model.find_workspace(&output, index).as_deref() == Some(source) {
            return false;
        }
        let Some(target) = self.model.workspace_at(&output, index) else {
            return false;
        };
        let tiles = self.tiles_of(source, views);
        let from = self.state_mut(source);
        let was_floated = from.floated.contains(id);
        let rect = from.rects.get(id).copied();
        from.remove(id);
        let to = self.state_mut(&target);
        to.order.push(id.to_owned());
        if was_floated {
            to.floated.insert(id.to_owned());
        }
        if let Some(rect) = rect {
            to.rects.insert(id.to_owned(), rect);
        }
        self.remember_missing(&target, &tiles, views);
        self.model.move_to_workspace(id, &target);
        self.sync_placement(&target);
        true
    }
}

/// Workspace number 1..=9 to the index the model uses.
fn workspace_index(n: u8) -> Option<i64> {
    (1..=9).contains(&n).then(|| i64::from(n) - 1)
}
