//! The outputs' own state: modes, scale, transform, position, and the
//! `added`, `changed` and `removed` events. Outputs are laid out left to
//! right in the order they were added, by logical width.

use tetsu_bus::output::{ListOutputsReply, Mode, Output, OutputEvent, OutputKind, Transform};

use crate::layout::Rect;
use crate::outputs::geometry::{logical_size, pack};

/// What the backend knows about an output when it appears.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputInfo {
    /// Connector name, for example `DP-1`.
    pub name: String,
    /// EDID make (untrusted, already cleaned).
    pub make: Option<String>,
    /// EDID model.
    pub model: Option<String>,
    /// EDID serial.
    pub serial: Option<String>,
    /// Every mode of the display.
    pub modes: Vec<Mode>,
    /// Index into `modes` of the mode in use.
    pub current_mode: Option<usize>,
    /// Scale factor.
    pub scale: f64,
    /// Rotation and mirroring.
    pub transform: Transform,
}

impl OutputInfo {
    /// An output with one 1920x1080@60 mode, scale 1, not rotated.
    pub fn named(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            make: None,
            model: None,
            serial: None,
            modes: vec![Mode {
                width: 1920,
                height: 1080,
                refresh_mhz: 60000,
                preferred: true,
            }],
            current_mode: Some(0),
            scale: 1.0,
            transform: Transform::Normal,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Outputs {
    /// In the order they were added.
    list: Vec<Output>,
    next: u64,
    events: Vec<OutputEvent>,
}

impl Outputs {
    /// Adds an output; returns its id (`out-<n>`, never reused).
    pub(super) fn add(&mut self, info: OutputInfo) -> String {
        self.next += 1;
        let current_mode = info.current_mode.filter(|i| *i < info.modes.len());
        let output = Output {
            id: format!("out-{}", self.next),
            name: info.name,
            make: info.make,
            model: info.model,
            serial: info.serial,
            enabled: true,
            modes: info.modes,
            current_mode: current_mode.map(|i| i as i64),
            scale: info.scale,
            transform: info.transform,
            x: 0,
            y: 0,
        };
        let id = output.id.clone();
        self.events.push(OutputEvent::added(output.clone()));
        self.list.push(output);
        self.repack();
        id
    }

    /// Removes an output; false when it is unknown.
    pub(super) fn remove(&mut self, id: &str) -> bool {
        let Some(at) = self.list.iter().position(|o| o.id == id) else {
            return false;
        };
        self.list.remove(at);
        self.events
            .retain(|e| !(e.kind == OutputKind::Changed && event_id(e) == Some(id)));
        self.events.push(OutputEvent::removed(id));
        self.repack();
        true
    }

    pub(super) fn set_mode(&mut self, id: &str, modes: Vec<Mode>, current: Option<usize>) {
        let Some(output) = self.list.iter_mut().find(|o| o.id == id) else {
            return;
        };
        let current = current.filter(|i| *i < modes.len()).map(|i| i as i64);
        if output.modes == modes && output.current_mode == current {
            return;
        }
        output.modes = modes;
        output.current_mode = current;
        self.touch(id);
        self.repack();
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.list.iter().any(|o| o.id == id)
    }

    /// The first output added that is not `except`.
    pub(super) fn first_except(&self, except: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|o| o.id != except)
            .map(|o| o.id.as_str())
    }

    /// Enabled outputs with a mode, as logical rects, in the order added.
    pub(super) fn layout(&self) -> Vec<(String, Rect)> {
        self.list
            .iter()
            .filter_map(|o| {
                let (w, h) = logical(o)?;
                let rect = Rect {
                    x: o.x as i32,
                    y: o.y as i32,
                    w: w as i32,
                    h: h as i32,
                };
                o.enabled.then(|| (o.id.clone(), rect))
            })
            .collect()
    }

    pub(super) fn snapshot(&self) -> ListOutputsReply {
        ListOutputsReply {
            outputs: self.list.clone(),
        }
    }

    pub(super) fn flush(&mut self) -> Vec<OutputEvent> {
        std::mem::take(&mut self.events)
    }

    /// Packs left to right; every output that moved is touched.
    fn repack(&mut self) {
        let sizes: Vec<(&str, i64, bool)> = self
            .list
            .iter()
            .map(|o| (o.id.as_str(), logical(o).map_or(0, |s| s.0), o.enabled))
            .collect();
        let packed = pack(&sizes);
        let mut moved = Vec::new();
        for (id, x, y) in packed {
            if let Some(o) = self.list.iter_mut().find(|o| o.id == id)
                && (o.x, o.y) != (x, y)
            {
                o.x = x;
                o.y = y;
                moved.push(id);
            }
        }
        for id in moved {
            self.touch(&id);
        }
    }

    /// Records that an output changed: into its pending `added` or
    /// `changed` event when there is one, else as a new `changed`.
    fn touch(&mut self, id: &str) {
        let Some(current) = self.list.iter().find(|o| o.id == id).cloned() else {
            return;
        };
        let pending = self.events.iter_mut().find(|e| {
            matches!(e.kind, OutputKind::Added | OutputKind::Changed) && event_id(e) == Some(id)
        });
        match pending {
            Some(event) => event.output = Some(current),
            None => self.events.push(OutputEvent::changed(current)),
        }
    }
}

fn logical(output: &Output) -> Option<(i64, i64)> {
    let mode = output
        .modes
        .get(usize::try_from(output.current_mode?).ok()?)?;
    Some(logical_size(mode, output.scale, output.transform))
}

fn event_id(event: &OutputEvent) -> Option<&str> {
    match (&event.output, &event.id) {
        (Some(o), _) => Some(o.id.as_str()),
        (None, id) => id.as_deref(),
    }
}
