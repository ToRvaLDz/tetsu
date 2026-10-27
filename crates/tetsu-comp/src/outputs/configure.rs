//! `ConfigureOutputs`, the pure part: validates a call against the
//! outputs and the config layers, and gives the managed values to persist
//! and the target state of every output. Nothing is written or applied
//! here.

use std::collections::BTreeMap;

use serde_json::json;
use tetsu_bus::output::{
    INVALID_CONFIGURATION, ModeSpec, OUTPUT_NOT_FOUND, OVERRIDDEN_BY_USER, Output, OutputConfig,
    Transform,
};
use tetsu_bus::server::ErrorReply;
use toml::Value;

use super::limits;
use super::prefs::{self, size_of};
use crate::config::merge::{Values, View};
use crate::config::output_keys::key;

/// What one output should look like.
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    /// The output's id.
    pub id: String,
    /// Lit or dark.
    pub enabled: bool,
    /// Index into the output's modes; none keeps what the model has (or
    /// the remembered mode when it is enabled again).
    pub mode: Option<usize>,
    /// Scale factor.
    pub scale: f64,
    /// Rotation and mirroring.
    pub transform: Transform,
    /// Where it is pinned in the logical space; none is automatic.
    pub pin: Option<(i64, i64)>,
}

/// What a valid call gives.
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    /// The managed values the call sets, to merge into the managed layer.
    pub managed: Values,
    /// The target state of every output after the write.
    pub targets: Vec<Target>,
}

/// Why a call is rejected as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigureError {
    /// No output has this id.
    NotFound(String),
    /// The configuration cannot be applied.
    Invalid {
        /// The output at fault.
        id: String,
        /// What is wrong.
        reason: String,
    },
    /// The user file sets these keys.
    OverriddenByUser(Vec<String>),
}

impl ConfigureError {
    /// The Varlink error reply, under the `output.v1` names.
    pub fn reply(&self) -> ErrorReply {
        match self {
            Self::NotFound(id) => ErrorReply::new(OUTPUT_NOT_FOUND, json!({ "id": id })),
            Self::Invalid { id, reason } => {
                ErrorReply::new(INVALID_CONFIGURATION, json!({ "id": id, "reason": reason }))
            }
            Self::OverriddenByUser(keys) => {
                ErrorReply::new(OVERRIDDEN_BY_USER, json!({ "keys": keys }))
            }
        }
    }
}

fn invalid(id: &str, reason: impl Into<String>) -> ConfigureError {
    ConfigureError::Invalid {
        id: id.to_owned(),
        reason: reason.into(),
    }
}

fn set_mode(output: &Output, spec: &ModeSpec) -> bool {
    output
        .modes
        .iter()
        .any(|m| (m.width, m.height, m.refresh_mhz) == (spec.width, spec.height, spec.refresh_mhz))
}

fn check_output(output: &Output, config: &OutputConfig) -> Result<(), ConfigureError> {
    let id = output.id.as_str();
    if config != &OutputConfig::default() && !limits::valid_connector(&output.name) {
        return Err(invalid(
            id,
            format!(
                "the connector name {} cannot be stored in the config",
                output.name
            ),
        ));
    }
    if config.enabled == Some(true) && output.modes.is_empty() {
        return Err(invalid(id, "the output has no mode to enable"));
    }
    if let Some(spec) = &config.mode
        && !set_mode(output, spec)
    {
        return Err(invalid(
            id,
            format!(
                "the output has no mode {}x{}@{}",
                spec.width, spec.height, spec.refresh_mhz
            ),
        ));
    }
    if let Some(scale) = config.scale
        && !limits::scale_ok(scale)
    {
        return Err(invalid(
            id,
            format!(
                "scale {scale} is outside {}..={}",
                limits::MIN_SCALE,
                limits::MAX_SCALE
            ),
        ));
    }
    if config.transform == Some(Transform::Unknown) {
        return Err(invalid(id, "the transform is unknown"));
    }
    for (name, value) in [("x", config.x), ("y", config.y)] {
        if let Some(value) = value
            && !limits::coord_ok(value)
        {
            return Err(invalid(
                id,
                format!("position {name} {value} is outside +-{}", limits::MAX_COORD),
            ));
        }
    }
    Ok(())
}

fn table(pairs: &[(&str, i64)]) -> Value {
    Value::Table(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Value::Integer(*v)))
            .collect(),
    )
}

/// The managed values a valid config sets: its non-null fields.
fn managed_of(output: &Output, config: &OutputConfig) -> Values {
    let mut values = Values::new();
    let name = output.name.as_str();
    if let Some(enabled) = config.enabled {
        values.insert(key(name, "enabled"), Value::Boolean(enabled));
    }
    if let Some(spec) = &config.mode {
        let mode = table(&[
            ("width", spec.width),
            ("height", spec.height),
            ("refresh_mhz", spec.refresh_mhz),
        ]);
        values.insert(key(name, "mode"), mode);
    }
    if let Some(scale) = config.scale {
        values.insert(key(name, "scale"), Value::Float(scale));
    }
    if let Some(transform) = config.transform
        && let Ok(Value::String(text)) = Value::try_from(transform)
    {
        values.insert(key(name, "transform"), Value::String(text));
    }
    if config.x.is_some() || config.y.is_some() {
        let x = config.x.unwrap_or(output.x);
        let y = config.y.unwrap_or(output.y);
        values.insert(key(name, "position"), table(&[("x", x), ("y", y)]));
    }
    values
}

fn disables_first(configs: &BTreeMap<String, OutputConfig>, outputs: &[Output]) -> String {
    configs
        .iter()
        .find(|(_, c)| c.enabled == Some(false))
        .map(|(id, _)| id.clone())
        .or_else(|| configs.keys().next().cloned())
        .or_else(|| outputs.first().map(|o| o.id.clone()))
        .unwrap_or_default()
}

/// Validates a call; the first failure rejects it whole. The order: an
/// unknown id, an invalid configuration (per output, then the global
/// checks), a key the user file sets.
pub fn plan(
    outputs: &[Output],
    configs: &BTreeMap<String, OutputConfig>,
    view: &View,
) -> Result<Planned, ConfigureError> {
    if let Some(id) = configs
        .keys()
        .find(|id| !outputs.iter().any(|o| &o.id == *id))
    {
        return Err(ConfigureError::NotFound(id.clone()));
    }
    let find = |id: &str| outputs.iter().find(|o| o.id == id);
    let mut managed = Values::new();
    for (id, config) in configs {
        if let Some(output) = find(id) {
            check_output(output, config)?;
            managed.extend(managed_of(output, config));
        }
    }
    let mut values = view.values();
    values.extend(managed.clone());
    let resolved = prefs::resolve(outputs, &values);
    let raw_enabled = outputs.iter().any(|o| {
        prefs::OutputPrefs::of(&o.name, &values)
            .enabled
            .unwrap_or(true)
    });
    if !outputs.is_empty() && !raw_enabled {
        return Err(invalid(
            &disables_first(configs, outputs),
            "no output would stay enabled",
        ));
    }
    for (later, (output, target)) in outputs.iter().zip(&resolved).enumerate() {
        let Some((x, y)) = target.pin.filter(|_| target.enabled) else {
            continue;
        };
        let (w, h) = size_of(output, target);
        for (other, other_target) in outputs.iter().zip(&resolved).take(later) {
            let Some((ox, oy)) = other_target.pin.filter(|_| other_target.enabled) else {
                continue;
            };
            let (ow, oh) = size_of(other, other_target);
            let touched = configs.contains_key(&output.id) || configs.contains_key(&other.id);
            if touched && x < ox + ow && ox < x + w && y < oy + oh && oy < y + h {
                return Err(invalid(
                    &output.id,
                    format!("it would overlap {} ({})", other.id, other.name),
                ));
            }
        }
    }
    let user = view.user_keys();
    let mut overridden: Vec<String> = managed
        .keys()
        .filter(|k| user.contains(k))
        .cloned()
        .collect();
    if !overridden.is_empty() {
        overridden.sort();
        return Err(ConfigureError::OverriddenByUser(overridden));
    }
    Ok(Planned {
        managed,
        targets: prefs::targets(outputs, &values),
    })
}
