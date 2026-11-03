//! The outputs' own state: modes, scale, transform, position, and the
//! `added`, `changed` and `removed` events. Outputs are laid out left to
//! right in the order they were added, by logical width, except those
//! with a pinned position.

use tetsu_bus::output::{ListOutputsReply, Mode, Output, OutputEvent, OutputKind, Transform};

use crate::layout::Rect;
use crate::outputs::configure::Target;
use crate::outputs::geometry::{PlaceIn, logical_size, place};

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
    /// Whether the output is lit. A dark one is listed but takes no
    /// workspaces; the model lights the only output whatever this says.
    pub enabled: bool,
    /// Where the output is pinned in the logical space; none is automatic.
    pub position: Option<(i64, i64)>,
}

impl OutputInfo {
    /// An enabled output with one 1920x1080@60 mode, scale 1, not rotated,
    /// placed automatically.
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
            enabled: true,
            position: None,
        }
    }
}

/// An output and what the wire does not show of it.
#[derive(Debug)]
struct Slot {
    out: Output,
    /// Where it is pinned; none is automatic.
    pin: Option<(i64, i64)>,
    /// The mode it had when last lit, for the next time.
    last_mode: Option<i64>,
}

/// Which outputs a `configure` lit and which it darkened.
#[derive(Debug, Default)]
pub(super) struct Switched {
    pub(super) lit: Vec<String>,
    pub(super) dark: Vec<String>,
}

#[derive(Debug, Default)]
pub(super) struct Outputs {
    /// In the order they were added.
    list: Vec<Slot>,
    next: u64,
    events: Vec<OutputEvent>,
}

impl Outputs {
    /// Adds an output; returns its id (`out-<n>`, never reused). A dark
    /// output keeps its mode for when it is lit.
    pub(super) fn add(&mut self, info: OutputInfo) -> String {
        self.next += 1;
        let current_mode = info
            .current_mode
            .filter(|i| *i < info.modes.len())
            .map(|i| i as i64);
        let output = Output {
            id: format!("out-{}", self.next),
            name: info.name,
            make: info.make,
            model: info.model,
            serial: info.serial,
            enabled: info.enabled,
            modes: info.modes,
            current_mode: current_mode.filter(|_| info.enabled),
            scale: info.scale,
            transform: info.transform,
            x: 0,
            y: 0,
        };
        let id = output.id.clone();
        self.events.push(OutputEvent::added(output.clone()));
        self.list.push(Slot {
            out: output,
            pin: info.position,
            last_mode: current_mode,
        });
        self.repack();
        id
    }

    /// Removes an output; false when it is unknown.
    pub(super) fn remove(&mut self, id: &str) -> bool {
        let Some(at) = self.list.iter().position(|s| s.out.id == id) else {
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
        let Some(slot) = self.list.iter_mut().find(|s| s.out.id == id) else {
            return;
        };
        let output = &mut slot.out;
        let current = current
            .filter(|i| *i < modes.len())
            .filter(|_| output.enabled)
            .map(|i| i as i64);
        if output.modes == modes && output.current_mode == current {
            return;
        }
        output.modes = modes;
        output.current_mode = current;
        slot.last_mode = slot
            .last_mode
            .filter(|i| (*i as usize) < output.modes.len());
        self.touch(id);
        self.repack();
    }

    /// Applies targets; unknown ids are ignored. Returns the outputs that
    /// were lit or darkened, which the caller moves workspaces for.
    pub(super) fn configure(&mut self, targets: &[Target]) -> Switched {
        let mut switched = Switched::default();
        for target in targets {
            let Some(slot) = self.list.iter_mut().find(|s| s.out.id == target.id) else {
                continue;
            };
            let before = slot.out.clone();
            let pinned = slot.pin;
            apply(slot, target);
            if before.enabled != slot.out.enabled {
                let list = if slot.out.enabled {
                    &mut switched.lit
                } else {
                    &mut switched.dark
                };
                list.push(target.id.clone());
            }
            if before != slot.out || pinned != slot.pin {
                self.touch(&target.id);
            }
        }
        self.repack();
        switched
    }

    /// Lights the first dark output; returns its id.
    pub(super) fn light_first(&mut self) -> Option<String> {
        let slot = self.list.iter_mut().find(|s| !s.out.enabled)?;
        let target = Target {
            id: slot.out.id.clone(),
            enabled: true,
            mode: None,
            scale: slot.out.scale,
            transform: slot.out.transform,
            pin: slot.pin,
        };
        apply(slot, &target);
        let id = slot.out.id.clone();
        self.touch(&id);
        self.repack();
        Some(id)
    }

    pub(super) fn any_enabled(&self) -> bool {
        self.list.iter().any(|s| s.out.enabled)
    }

    pub(super) fn contains(&self, id: &str) -> bool {
        self.slot(id).is_some()
    }

    pub(super) fn name_of(&self, id: &str) -> Option<String> {
        self.slot(id).map(|s| s.out.name.clone())
    }

    /// The first output added that is enabled and is not `except`.
    pub(super) fn first_except(&self, except: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|s| s.out.id != except && s.out.enabled)
            .map(|s| s.out.id.as_str())
    }

    /// Enabled outputs with a mode, as logical rects, in the order added.
    pub(super) fn layout(&self) -> Vec<(String, Rect)> {
        self.list
            .iter()
            .map(|s| &s.out)
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
            outputs: self.list.iter().map(|s| s.out.clone()).collect(),
        }
    }

    pub(super) fn flush(&mut self) -> Vec<OutputEvent> {
        std::mem::take(&mut self.events)
    }

    fn slot(&self, id: &str) -> Option<&Slot> {
        self.list.iter().find(|s| s.out.id == id)
    }

    /// Places the outputs (pins over the automatic layout); every output
    /// that moved is touched. A pin that overlaps an earlier one is
    /// ignored for the placement, and logged.
    fn repack(&mut self) {
        let sizes: Vec<(i64, i64)> = self
            .list
            .iter()
            .map(|s| logical(&s.out).unwrap_or((0, 0)))
            .collect();
        let input: Vec<PlaceIn<'_>> = self
            .list
            .iter()
            .zip(&sizes)
            .map(|(s, (w, h))| PlaceIn {
                id: &s.out.id,
                w: *w,
                h: *h,
                enabled: s.out.enabled,
                pin: s.pin,
            })
            .collect();
        let (placed, fell_back) = place(&input);
        for id in &fell_back {
            eprintln!(
                "tetsu-comp: the pinned position of {id} overlaps another output; placing it automatically"
            );
        }
        let mut moved = Vec::new();
        for (id, x, y) in placed {
            if let Some(slot) = self.list.iter_mut().find(|s| s.out.id == id)
                && (slot.out.x, slot.out.y) != (x, y)
            {
                slot.out.x = x;
                slot.out.y = y;
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
        let Some(current) = self.slot(id).map(|s| s.out.clone()) else {
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

/// Puts a target on a slot. A lit output takes the target's mode, else
/// the one it has, else the one it had, else the preferred one; a dark one
/// remembers its mode.
fn apply(slot: &mut Slot, target: &Target) {
    let output = &mut slot.out;
    let asked = target
        .mode
        .filter(|i| *i < output.modes.len())
        .map(|i| i as i64);
    let current = output.current_mode;
    if target.enabled {
        let preferred = output
            .modes
            .iter()
            .position(|m| m.preferred)
            .or_else(|| (!output.modes.is_empty()).then_some(0))
            .map(|i| i as i64);
        output.current_mode = asked.or(current).or(slot.last_mode).or(preferred);
        slot.last_mode = output.current_mode;
    } else {
        slot.last_mode = asked.or(current).or(slot.last_mode);
        output.current_mode = None;
    }
    output.enabled = target.enabled;
    output.scale = target.scale;
    output.transform = target.transform;
    slot.pin = target.pin;
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
