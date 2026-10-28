//! The flat key registry: every config key with its validator, whether
//! `SetManaged` may write it and, if not, the method that does.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use toml::Value;

use super::output_keys;
use crate::decor::{Color, Decorations};
use crate::layout::Gaps;
use crate::layout::LayoutKind;
use crate::layout::master::{MAX_RATIO, MIN_RATIO};
use crate::settings::{
    self, DeviceChoice, LockSettings, RawRule, RenderSettings, Settings, SettingsError,
    TrustSettings,
};

/// One config key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDef {
    /// The dotted name.
    pub name: &'static str,
    /// Whether `SetManaged` accepts the key.
    pub writable: bool,
    /// For a key that is not writable, the method that writes it.
    pub method: Option<&'static str>,
    /// Whether the value is a command the compositor would run. Such a key
    /// is never read from the managed layer (#31.9).
    pub runs_commands: bool,
}

const fn writable(name: &'static str) -> KeyDef {
    KeyDef {
        name,
        writable: true,
        method: None,
        runs_commands: false,
    }
}

const fn via(name: &'static str, method: &'static str) -> KeyDef {
    KeyDef {
        name,
        writable: false,
        method: Some(method),
        runs_commands: false,
    }
}

const fn command(name: &'static str) -> KeyDef {
    KeyDef {
        name,
        writable: false,
        method: None,
        runs_commands: true,
    }
}

/// Every key, in the order `GetConfig` lists them.
pub const KEYS: [KeyDef; 14] = [
    writable("layout.default"),
    writable("layout.gaps_inner"),
    writable("layout.gaps_outer"),
    writable("layout.master_ratio"),
    writable("render.wait_for_frame_completion"),
    writable("window_rule"),
    writable("decorations.width"),
    writable("decorations.active_color"),
    writable("decorations.inactive_color"),
    writable("lock.idle_timeout_s"),
    via("render.device", "org.tetsu.comp.render.v1.SetRenderDevice"),
    via(
        "render.explicit_sync",
        "org.tetsu.comp.render.v1.SetExplicitSync",
    ),
    command("lock.command"),
    command("trust.input"),
];

/// The key called `name`.
pub fn lookup(name: &str) -> Option<&'static KeyDef> {
    if name.starts_with("output.") {
        return output_keys::lookup(name);
    }
    KEYS.iter().find(|key| key.name == name)
}

/// The table names of the user file; `window_rule` is a key by itself.
pub const TABLES: [&str; 6] = ["layout", "render", "lock", "trust", "decorations", "output"];

/// Why a value is refused: the rule's position for `window_rule`.
pub type Invalid = (Option<usize>, SettingsError);

fn typed<T: DeserializeOwned>(key: &str, value: &Value) -> Result<T, SettingsError> {
    value
        .clone()
        .try_into()
        .map_err(|e: toml::de::Error| SettingsError::Toml(format!("{key}: {}", e.message())))
}

fn ratio(value: &Value) -> Result<f64, SettingsError> {
    let ratio: f64 = typed("layout.master_ratio", value)?;
    if (MIN_RATIO..=MAX_RATIO).contains(&ratio) {
        Ok(ratio)
    } else {
        Err(SettingsError::BadRatio(ratio))
    }
}

fn gap_of(name: &'static str, value: &Value) -> Result<i32, SettingsError> {
    let raw: i64 = typed(&format!("layout.{name}"), value)?;
    settings::gap(name, Some(raw), 0)
}

fn layout_of(value: &Value) -> Result<LayoutKind, SettingsError> {
    let name: String = typed("layout.default", value)?;
    name.parse().map_err(|_| SettingsError::UnknownLayout(name))
}

fn rules_of(value: &Value) -> Result<Vec<crate::rules::Rule>, Invalid> {
    let items: Vec<Value> = typed("window_rule", value).map_err(|e| (None, e))?;
    let mut rules = Vec::new();
    for (at, item) in items.iter().enumerate() {
        let raw: RawRule = typed(&format!("window_rule[{at}]"), item).map_err(|e| (Some(at), e))?;
        rules.push(settings::rule(at, raw).map_err(|e| (Some(at), e))?);
    }
    Ok(rules)
}

fn width_of(value: &Value) -> Result<i32, SettingsError> {
    let raw: i64 = typed("decorations.width", value)?;
    settings::border_width(raw)
}

fn color_of(name: &'static str, value: &Value) -> Result<Color, SettingsError> {
    let text: String = typed(&format!("decorations.{name}"), value)?;
    settings::border_color(name, &text)
}

fn idle_timeout_of(value: &Value) -> Result<u32, SettingsError> {
    let raw: i64 = typed("lock.idle_timeout_s", value)?;
    settings::idle_timeout(raw)
}

/// Whether `value` is valid for the known key `name`.
pub fn validate(name: &str, value: &Value) -> Result<(), Invalid> {
    if name.starts_with("output.") {
        return output_keys::validate(name, value).map_err(|e| (None, e));
    }
    let result = match name {
        "layout.default" => layout_of(value).map(drop),
        "layout.gaps_inner" => gap_of("gaps_inner", value).map(drop),
        "layout.gaps_outer" => gap_of("gaps_outer", value).map(drop),
        "layout.master_ratio" => ratio(value).map(drop),
        "render.device" => {
            typed::<String>(name, value).and_then(|s| settings::device_choice(&s).map(drop))
        }
        "render.explicit_sync" => {
            typed::<String>(name, value).and_then(|s| settings::sync_mode(&s).map(drop))
        }
        "render.wait_for_frame_completion" => typed::<bool>(name, value).map(drop),
        "lock.command" => {
            typed::<Vec<String>>(name, value).and_then(|c| settings::lock_command(&c))
        }
        "lock.idle_timeout_s" => idle_timeout_of(value).map(drop),
        "trust.input" => {
            typed::<Vec<Vec<String>>>(name, value).and_then(|i| settings::trust_input(&i))
        }
        "decorations.width" => width_of(value).map(drop),
        "decorations.active_color" => color_of("active_color", value).map(drop),
        "decorations.inactive_color" => color_of("inactive_color", value).map(drop),
        "window_rule" => return rules_of(value).map(drop),
        other => Err(SettingsError::UnknownKey(other.to_owned())),
    };
    result.map_err(|e| (None, e))
}

fn s(text: &str) -> Value {
    Value::String(text.to_owned())
}

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
}

/// The built-in value of every key: the bottom of the stack.
pub fn builtin() -> BTreeMap<String, Value> {
    let defaults = Settings::default();
    let entries = [
        ("layout.default", s("dwindle")),
        (
            "layout.gaps_inner",
            Value::Integer(i64::from(defaults.gaps.inner)),
        ),
        (
            "layout.gaps_outer",
            Value::Integer(i64::from(defaults.gaps.outer)),
        ),
        ("layout.master_ratio", Value::Float(defaults.master_ratio)),
        ("render.device", s("auto")),
        ("render.explicit_sync", s("auto")),
        ("render.wait_for_frame_completion", Value::Boolean(false)),
        (
            "lock.command",
            Value::Array(defaults.lock.command.iter().map(|a| s(a)).collect()),
        ),
        (
            "lock.idle_timeout_s",
            Value::Integer(i64::from(defaults.lock.idle_timeout_s)),
        ),
        ("trust.input", Value::Array(Vec::new())),
        ("window_rule", Value::Array(Vec::new())),
        (
            "decorations.width",
            Value::Integer(i64::from(defaults.decorations.width)),
        ),
        (
            "decorations.active_color",
            s(&hex(defaults.decorations.active)),
        ),
        (
            "decorations.inactive_color",
            s(&hex(defaults.decorations.inactive)),
        ),
    ];
    entries
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
}

impl Settings {
    /// The settings of effective values, as `merge` makes them. A value
    /// that does not validate (the layers check every value first) falls
    /// back to the built-in one.
    pub fn from_values(values: &BTreeMap<String, Value>) -> Self {
        let base = Self::default();
        let get = |key: &str| values.get(key);
        let layout = get("layout.default")
            .and_then(|v| layout_of(v).ok())
            .unwrap_or(base.layout);
        let gap = |name: &'static str, key: &str, fallback: i32| {
            get(key)
                .and_then(|v| gap_of(name, v).ok())
                .unwrap_or(fallback)
        };
        let render = RenderSettings {
            device: get("render.device")
                .and_then(|v| v.as_str())
                .and_then(|p| settings::device_choice(p).ok())
                .unwrap_or(DeviceChoice::Auto),
            explicit_sync: get("render.explicit_sync")
                .and_then(|v| v.as_str())
                .and_then(|m| settings::sync_mode(m).ok())
                .unwrap_or_default(),
            wait_for_frame_completion: get("render.wait_for_frame_completion")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let command = get("lock.command")
            .and_then(|v| typed::<Vec<String>>("lock.command", v).ok())
            .unwrap_or(base.lock.command);
        let idle_timeout_s = get("lock.idle_timeout_s")
            .and_then(|v| idle_timeout_of(v).ok())
            .unwrap_or(base.lock.idle_timeout_s);
        let input = get("trust.input")
            .and_then(|v| typed::<Vec<Vec<String>>>("trust.input", v).ok())
            .unwrap_or_default();
        let decorations = Decorations {
            width: get("decorations.width")
                .and_then(|v| width_of(v).ok())
                .unwrap_or(base.decorations.width),
            active: get("decorations.active_color")
                .and_then(|v| color_of("active_color", v).ok())
                .unwrap_or(base.decorations.active),
            inactive: get("decorations.inactive_color")
                .and_then(|v| color_of("inactive_color", v).ok())
                .unwrap_or(base.decorations.inactive),
        };
        Self {
            layout,
            gaps: Gaps {
                inner: gap("gaps_inner", "layout.gaps_inner", base.gaps.inner),
                outer: gap("gaps_outer", "layout.gaps_outer", base.gaps.outer),
            },
            master_ratio: get("layout.master_ratio")
                .and_then(|v| ratio(v).ok())
                .unwrap_or(base.master_ratio),
            rules: get("window_rule")
                .and_then(|v| rules_of(v).ok())
                .unwrap_or_default(),
            render,
            lock: LockSettings {
                command,
                idle_timeout_s,
            },
            trust: TrustSettings { input },
            decorations,
        }
    }
}
