//! The layout state of one workspace.

use std::collections::{BTreeMap, BTreeSet};

use crate::layout::{LayoutKind, Rect};

#[derive(Debug, Clone)]
pub(super) struct WorkspaceState {
    pub layout: LayoutKind,
    pub ratio: f64,
    /// The windows in layout order: new ones are appended.
    pub order: Vec<String>,
    /// Windows floated on their own, whatever the layout.
    pub floated: BTreeSet<String>,
    /// Remembered floating rects, relative to the viewport origin.
    pub rects: BTreeMap<String, Rect>,
}

impl WorkspaceState {
    pub(super) fn new(layout: LayoutKind, ratio: f64) -> Self {
        Self {
            layout,
            ratio,
            order: Vec::new(),
            floated: BTreeSet::new(),
            rects: BTreeMap::new(),
        }
    }

    /// The effective placement: floated, or on a floating workspace.
    pub(super) fn is_floating(&self, id: &str) -> bool {
        self.layout == LayoutKind::Floating || self.floated.contains(id)
    }

    pub(super) fn tiled(&self) -> Vec<String> {
        self.order
            .iter()
            .filter(|id| !self.is_floating(id))
            .cloned()
            .collect()
    }

    pub(super) fn position(&self, id: &str) -> Option<usize> {
        self.order.iter().position(|o| o == id)
    }

    pub(super) fn remove(&mut self, id: &str) {
        self.order.retain(|o| o != id);
        self.floated.remove(id);
        self.rects.remove(id);
    }
}
