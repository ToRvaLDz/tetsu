//! What the config says about an output, and the target state that makes
//! of it. This is the one source the model applies: at start, on hotplug,
//! on a live reload and after a `ConfigureOutputs` write.
//!
//! A field the config leaves out is automatic: enabled, the preferred
//! mode, scale 1, not rotated, placed by the automatic layout. A value
//! that cannot apply (a mode the display lacks) is ignored here, never an
//! error: the planner is the strict side.

use tetsu_bus::output::{ModeSpec, Output, Transform};
use toml::Value;

use super::configure::Target;
use super::geometry::{PlaceIn, logical_size, place};
use super::limits;
use crate::config::merge::Values;
use crate::config::output_keys::{key, number, transform};
use crate::model::OutputInfo;

/// The config of one output by connector name, as far as it is usable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputPrefs {
    /// `enabled`.
    pub enabled: Option<bool>,
    /// `mode`.
    pub mode: Option<ModeSpec>,
    /// `scale`.
    pub scale: Option<f64>,
    /// `transform`.
    pub transform: Option<Transform>,
    /// `position`.
    pub pin: Option<(i64, i64)>,
}

impl OutputPrefs {
    /// The prefs of connector `name` in the effective `values`.
    pub fn of(name: &str, values: &Values) -> Self {
        if !limits::valid_connector(name) {
            return Self::default();
        }
        let get = |field: &str| values.get(&key(name, field));
        let pin = get("position").and_then(|v| {
            let table = v.as_table()?;
            Some((table.get("x")?.as_integer()?, table.get("y")?.as_integer()?))
        });
        Self {
            enabled: get("enabled").and_then(Value::as_bool),
            mode: get("mode").and_then(|v| v.clone().try_into().ok()),
            scale: get("scale")
                .and_then(number)
                .filter(|s| limits::scale_ok(*s)),
            transform: get("transform").and_then(transform),
            pin,
        }
    }
}

fn mode_index(output: &Output, prefs: &OutputPrefs) -> Option<usize> {
    let current = output.current_mode.and_then(|i| usize::try_from(i).ok());
    match &prefs.mode {
        Some(spec) => output
            .modes
            .iter()
            .position(|m| {
                (m.width, m.height, m.refresh_mhz) == (spec.width, spec.height, spec.refresh_mhz)
            })
            .or(current),
        None => output
            .modes
            .iter()
            .position(|m| m.preferred)
            .or(current)
            .or_else(|| (!output.modes.is_empty()).then_some(0)),
    }
}

/// The targets the config asks for, before the overlap fallback: the
/// first output stays enabled when the config would disable all.
pub fn resolve(outputs: &[Output], values: &Values) -> Vec<Target> {
    let mut targets: Vec<Target> = outputs
        .iter()
        .map(|output| {
            let prefs = OutputPrefs::of(&output.name, values);
            Target {
                id: output.id.clone(),
                enabled: prefs.enabled.unwrap_or(true),
                mode: mode_index(output, &prefs),
                scale: prefs.scale.unwrap_or(1.0),
                transform: prefs.transform.unwrap_or(Transform::Normal),
                pin: prefs.pin,
            }
        })
        .collect();
    if !targets.iter().any(|t| t.enabled)
        && let Some(first) = targets.first_mut()
    {
        first.enabled = true;
    }
    targets
}

/// The size of a target in the logical space; none without a mode.
pub fn size_of(output: &Output, target: &Target) -> (i64, i64) {
    target
        .mode
        .and_then(|i| output.modes.get(i))
        .map_or((0, 0), |m| logical_size(m, target.scale, target.transform))
}

/// The target state of every output, leniently: a pin that overlaps an
/// earlier pin is dropped (the output is placed automatically).
pub fn targets(outputs: &[Output], values: &Values) -> Vec<Target> {
    let mut targets = resolve(outputs, values);
    let sizes: Vec<(i64, i64)> = outputs
        .iter()
        .zip(&targets)
        .map(|(output, target)| size_of(output, target))
        .collect();
    let input: Vec<PlaceIn<'_>> = targets
        .iter()
        .zip(&sizes)
        .map(|(t, (w, h))| PlaceIn {
            id: &t.id,
            w: *w,
            h: *h,
            enabled: t.enabled,
            pin: t.pin,
        })
        .collect();
    let (_, fell_back) = place(&input);
    for target in &mut targets {
        if fell_back.contains(&target.id) {
            target.pin = None;
        }
    }
    targets
}

/// Puts the config's say on an output that is about to be added: lit or
/// dark, mode (when the display offers it), scale, transform and pin. What
/// the config leaves out stays as the backend described it.
pub fn apply_to_info(info: &mut OutputInfo, values: &Values) {
    let prefs = OutputPrefs::of(&info.name, values);
    if let Some(enabled) = prefs.enabled {
        info.enabled = enabled;
    }
    let spec = prefs.mode;
    let at = spec.and_then(|s| {
        info.modes
            .iter()
            .position(|m| (m.width, m.height, m.refresh_mhz) == (s.width, s.height, s.refresh_mhz))
    });
    if at.is_some() {
        info.current_mode = at;
    }
    if let Some(scale) = prefs.scale {
        info.scale = scale;
    }
    if let Some(transform) = prefs.transform {
        info.transform = transform;
    }
    info.position = prefs.pin;
}
