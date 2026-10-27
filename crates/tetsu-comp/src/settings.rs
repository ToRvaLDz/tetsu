//! The settings: an optional `--config <file>` TOML with `[layout]` and
//! `[render]` and `[[window_rule]]`. Without a file the defaults apply. The layering of
//! default, managed and user files, live reload and config.v1 are #73's;
//! this schema is the user-file schema it builds on.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::layout::master::{DEFAULT_RATIO, MAX_RATIO, MIN_RATIO};
use crate::layout::{Gaps, LayoutKind};
use crate::rules::{Pattern, Rule};

/// What the engine runs with.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// The layout of a new workspace.
    pub layout: LayoutKind,
    /// Space around and between windows.
    pub gaps: Gaps,
    /// The master's share of the width, `MIN_RATIO..=MAX_RATIO`.
    pub master_ratio: f64,
    /// The window rules, in file order.
    pub rules: Vec<Rule>,
    /// The GPU and explicit sync choices of the tty backend.
    pub render: RenderSettings,
}

/// Which GPU renders.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeviceChoice {
    /// The compositor picks (`gpu::select`).
    #[default]
    Auto,
    /// A node under `/dev/dri/`.
    Node(PathBuf),
}

/// The `render.explicit_sync` switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// On when the render device supports it.
    #[default]
    Auto,
    /// On where possible, loud when not.
    On,
    /// Never.
    Off,
}

/// The `[render]` table; mirrors `org.tetsu.comp.render.v1` (#73).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderSettings {
    /// The render device.
    pub device: DeviceChoice,
    /// Explicit sync.
    pub explicit_sync: SyncMode,
    /// Wait for each frame to finish before queueing it.
    pub wait_for_frame_completion: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            layout: LayoutKind::Dwindle,
            gaps: Gaps::NONE,
            master_ratio: DEFAULT_RATIO,
            rules: Vec::new(),
            render: RenderSettings::default(),
        }
    }
}

/// Why a config file was refused. Every message names the key.
#[derive(Debug)]
pub enum SettingsError {
    /// The file could not be read.
    Io(std::io::Error),
    /// Not TOML, an unknown key or a value of the wrong type; the message
    /// has the line.
    Toml(String),
    /// `layout.default` is none of the three layouts.
    UnknownLayout(String),
    /// A gap below zero; the key is `layout.gaps_inner` or `gaps_outer`.
    NegativeGap(&'static str, i64),
    /// `layout.master_ratio` outside 0.1..=0.9.
    BadRatio(f64),
    /// A pattern that is not a regex.
    BadRegex {
        /// The rule's position in the file.
        rule: usize,
        /// `app_id` or `title`.
        key: &'static str,
        /// The regex error.
        message: String,
    },
    /// A rule with neither `app_id` nor `title`.
    NoMatcher(usize),
    /// `workspace` outside 1..=9.
    BadWorkspace(usize, i64),
    /// `size` with a side that is not positive.
    BadSize(usize, i64, i64),
    /// `render.device` is neither `auto` nor an absolute path under `/dev/dri/`.
    BadRenderDevice(String),
    /// `render.explicit_sync` is none of `auto`, `on`, `off`.
    UnknownSyncMode(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "{e}"),
            Self::Toml(message) => write!(f, "{}", message.trim_end()),
            Self::UnknownLayout(name) => write!(
                f,
                "layout.default: unknown layout {name:?} (dwindle, master or floating)"
            ),
            Self::NegativeGap(key, value) => {
                write!(f, "layout.{key}: {value} is negative")
            }
            Self::BadRatio(ratio) => write!(
                f,
                "layout.master_ratio: {ratio} is outside {MIN_RATIO}..={MAX_RATIO}"
            ),
            Self::BadRegex { rule, key, message } => {
                write!(f, "window_rule[{rule}].{key}: bad regex: {message}")
            }
            Self::NoMatcher(rule) => {
                write!(f, "window_rule[{rule}]: needs app_id or title")
            }
            Self::BadWorkspace(rule, value) => {
                write!(f, "window_rule[{rule}].workspace: {value} is outside 1..=9")
            }
            Self::BadSize(rule, w, h) => write!(
                f,
                "window_rule[{rule}].size: [{w}, {h}] needs two positive numbers"
            ),
            Self::BadRenderDevice(value) => write!(
                f,
                "render.device: {value:?} is neither \"auto\" nor a path under /dev/dri/"
            ),
            Self::UnknownSyncMode(name) => write!(
                f,
                "render.explicit_sync: unknown mode {name:?} (auto, on or off)"
            ),
        }
    }
}

impl std::error::Error for SettingsError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    layout: RawLayout,
    #[serde(default)]
    render: RawRender,
    #[serde(default)]
    window_rule: Vec<RawRule>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawRender {
    device: Option<String>,
    explicit_sync: Option<String>,
    wait_for_frame_completion: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLayout {
    default: Option<String>,
    gaps_inner: Option<i64>,
    gaps_outer: Option<i64>,
    master_ratio: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    app_id: Option<String>,
    title: Option<String>,
    floating: Option<bool>,
    workspace: Option<i64>,
    output: Option<String>,
    size: Option<[i64; 2]>,
}

fn gap(key: &'static str, value: Option<i64>, default: i32) -> Result<i32, SettingsError> {
    match value {
        None => Ok(default),
        Some(v) if v < 0 => Err(SettingsError::NegativeGap(key, v)),
        Some(v) => Ok(i32::try_from(v).unwrap_or(i32::MAX / 4)),
    }
}

fn pattern(
    rule: usize,
    key: &'static str,
    source: Option<String>,
) -> Result<Option<Pattern>, SettingsError> {
    source
        .map(|s| {
            Pattern::new(&s).map_err(|e| SettingsError::BadRegex {
                rule,
                key,
                message: e.to_string(),
            })
        })
        .transpose()
}

fn rule(index: usize, raw: RawRule) -> Result<Rule, SettingsError> {
    if raw.app_id.is_none() && raw.title.is_none() {
        return Err(SettingsError::NoMatcher(index));
    }
    let workspace = match raw.workspace {
        None => None,
        Some(n @ 1..=9) => Some(n as u8),
        Some(n) => return Err(SettingsError::BadWorkspace(index, n)),
    };
    let size = match raw.size {
        None => None,
        Some([w, h]) => match (i32::try_from(w), i32::try_from(h)) {
            (Ok(w), Ok(h)) if w > 0 && h > 0 => Some((w, h)),
            _ => return Err(SettingsError::BadSize(index, w, h)),
        },
    };
    Ok(Rule {
        app_id: pattern(index, "app_id", raw.app_id)?,
        title: pattern(index, "title", raw.title)?,
        floating: raw.floating,
        workspace,
        output: raw.output,
        size,
    })
}

fn render(raw: RawRender) -> Result<RenderSettings, SettingsError> {
    let device = match raw.device.as_deref() {
        None | Some("auto") => DeviceChoice::Auto,
        Some(path) => {
            let p = Path::new(path);
            let under_dri = p.strip_prefix("/dev/dri").is_ok_and(|rest| {
                let mut parts = rest.components();
                matches!(parts.next(), Some(std::path::Component::Normal(_)))
                    && parts.next().is_none()
            });
            if !under_dri {
                return Err(SettingsError::BadRenderDevice(path.to_owned()));
            }
            DeviceChoice::Node(p.to_owned())
        }
    };
    let explicit_sync = match raw.explicit_sync.as_deref() {
        None | Some("auto") => SyncMode::Auto,
        Some("on") => SyncMode::On,
        Some("off") => SyncMode::Off,
        Some(other) => return Err(SettingsError::UnknownSyncMode(other.to_owned())),
    };
    Ok(RenderSettings {
        device,
        explicit_sync,
        wait_for_frame_completion: raw.wait_for_frame_completion.unwrap_or(false),
    })
}

/// Parses the text of a config file.
pub fn parse(text: &str) -> Result<Settings, SettingsError> {
    let raw: RawFile = toml::from_str(text).map_err(|e| SettingsError::Toml(e.to_string()))?;
    let defaults = Settings::default();
    let layout = match raw.layout.default {
        None => defaults.layout,
        Some(name) => name
            .parse()
            .map_err(|_| SettingsError::UnknownLayout(name))?,
    };
    let master_ratio = match raw.layout.master_ratio {
        None => defaults.master_ratio,
        Some(r) if (MIN_RATIO..=MAX_RATIO).contains(&r) => r,
        Some(r) => return Err(SettingsError::BadRatio(r)),
    };
    let rules = raw
        .window_rule
        .into_iter()
        .enumerate()
        .map(|(i, r)| rule(i, r))
        .collect::<Result<_, _>>()?;
    Ok(Settings {
        layout,
        gaps: Gaps {
            outer: gap("gaps_outer", raw.layout.gaps_outer, defaults.gaps.outer)?,
            inner: gap("gaps_inner", raw.layout.gaps_inner, defaults.gaps.inner)?,
        },
        master_ratio,
        rules,
        render: render(raw.render)?,
    })
}

/// Reads and parses a config file.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let text = std::fs::read_to_string(path).map_err(SettingsError::Io)?;
    parse(&text)
}
