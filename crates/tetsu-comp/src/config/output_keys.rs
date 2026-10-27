//! The `output.<connector>.<field>` keys: one per field of an output's
//! configuration. The set of keys is open (any connector), the fields are
//! fixed. `ConfigureOutputs` writes them; `SetManaged` does not.

use serde::Deserialize;
use tetsu_bus::output::Transform;
use toml::Value;

use super::keys::KeyDef;
use crate::outputs::limits;
use crate::settings::SettingsError;

/// The method that writes these keys.
pub const METHOD: &str = "org.tetsu.comp.output.v1.ConfigureOutputs";

/// The field names, in the order a connector's keys are listed.
pub const FIELDS: [&str; 5] = ["enabled", "mode", "position", "scale", "transform"];

const fn field(name: &'static str) -> KeyDef {
    KeyDef {
        name,
        writable: false,
        method: Some(METHOD),
        runs_commands: false,
    }
}

/// One definition per field; the name is a pattern, not a real key.
pub const OUTPUT_FIELDS: [KeyDef; 5] = [
    field("output.*.enabled"),
    field("output.*.mode"),
    field("output.*.position"),
    field("output.*.scale"),
    field("output.*.transform"),
];

/// `output.<connector>.<field>` as its two parts, when both are valid.
pub fn split(key: &str) -> Option<(&str, &str)> {
    let (connector, field) = key.strip_prefix("output.")?.split_once('.')?;
    (limits::valid_connector(connector) && FIELDS.contains(&field)).then_some((connector, field))
}

/// The key `output.<connector>.<field>`.
pub fn key(connector: &str, field: &str) -> String {
    format!("output.{connector}.{field}")
}

/// The definition of an output key.
pub fn lookup(key: &str) -> Option<&'static KeyDef> {
    let (_, field) = split(key)?;
    FIELDS
        .iter()
        .position(|f| *f == field)
        .map(|at| &OUTPUT_FIELDS[at])
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeValue {
    width: i64,
    height: i64,
    refresh_mhz: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PositionValue {
    x: i64,
    y: i64,
}

fn bad(key: &str, reason: impl std::fmt::Display) -> SettingsError {
    SettingsError::Toml(format!("{key}: {reason}"))
}

fn typed<T: serde::de::DeserializeOwned>(key: &str, value: &Value) -> Result<T, SettingsError> {
    value
        .clone()
        .try_into()
        .map_err(|e: toml::de::Error| bad(key, e.message()))
}

/// The scale as a number: TOML writes `2` and `2.0` differently.
pub fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(f) => Some(*f),
        Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// The transform named by a TOML string; `unknown` is not a name.
pub fn transform(value: &Value) -> Option<Transform> {
    let name = value.as_str()?;
    let parsed: Transform =
        serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()?;
    (parsed != Transform::Unknown).then_some(parsed)
}

/// Whether `value` is valid for the output key `key`.
pub fn validate(key: &str, value: &Value) -> Result<(), SettingsError> {
    let (_, field) = split(key).ok_or_else(|| SettingsError::UnknownKey(key.to_owned()))?;
    match field {
        "enabled" => typed::<bool>(key, value).map(drop),
        "scale" => match number(value) {
            Some(scale) if limits::scale_ok(scale) => Ok(()),
            Some(scale) => Err(bad(
                key,
                format!(
                    "{scale} is outside {}..={}",
                    limits::MIN_SCALE,
                    limits::MAX_SCALE
                ),
            )),
            None => Err(bad(key, "expected a number")),
        },
        "transform" => transform(value)
            .map(drop)
            .ok_or_else(|| bad(key, "expected normal, rotate_90, rotate_180, rotate_270, flipped, flipped_90, flipped_180 or flipped_270")),
        "mode" => {
            let mode: ModeValue = typed(key, value)?;
            if mode.width > 0 && mode.height > 0 && mode.refresh_mhz > 0 {
                Ok(())
            } else {
                Err(bad(key, "width, height and refresh_mhz must be positive"))
            }
        }
        _ => {
            let at: PositionValue = typed(key, value)?;
            if limits::coord_ok(at.x) && limits::coord_ok(at.y) {
                Ok(())
            } else {
                Err(bad(
                    key,
                    format!("x and y must be within +-{}", limits::MAX_COORD),
                ))
            }
        }
    }
}
