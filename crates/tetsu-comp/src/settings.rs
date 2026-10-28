//! The settings: what the engine runs with, and the checks of each config
//! value. `--config <file>` is a TOML with `[layout]`, `[render]`, `[lock]`,
//! `[trust]`, `[decorations]` and `[[window_rule]]`; `parse` and `load` read one file whole
//! and stop at the first problem. The layers (defaults, managed, user), the
//! live reload and `config.v1` are in `crate::config`, which reuses these
//! checks key by key.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::decor::{Color, Decorations, MAX_BORDER_WIDTH, parse_color};
use crate::layout::master::{DEFAULT_RATIO, MAX_RATIO, MIN_RATIO};
use crate::layout::{Gaps, LayoutKind};
use crate::rules::{Pattern, Rule};

/// The idle timeout when the file does not set one, in seconds.
pub const DEFAULT_IDLE_TIMEOUT_S: u32 = 600;
/// The longest idle timeout, in seconds: a day.
pub const MAX_IDLE_TIMEOUT_S: u32 = 86_400;

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
    /// The session locker.
    pub lock: LockSettings,
    /// Helpers the compositor starts with trust.
    pub trust: TrustSettings,
    /// The window borders.
    pub decorations: Decorations,
}

/// The `[lock]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockSettings {
    /// The locker's argv, started on the lock bind and again when the
    /// locker dies. Empty: nothing is started.
    pub command: Vec<String>,
    /// Idle seconds before the locker starts, `0..=MAX_IDLE_TIMEOUT_S`;
    /// `0` turns it off.
    pub idle_timeout_s: u32,
}

impl LockSettings {
    /// The idle timeout; `None` when it is off.
    pub fn idle_timeout(&self) -> Option<std::time::Duration> {
        crate::idle::timeout_of(self.idle_timeout_s)
    }
}

impl Default for LockSettings {
    fn default() -> Self {
        Self {
            command: vec!["swaylock".to_owned()],
            idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
        }
    }
}

/// The `[trust]` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrustSettings {
    /// Commands started once at startup with the trust to bind the virtual
    /// input protocols.
    pub input: Vec<Vec<String>>,
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
            lock: LockSettings::default(),
            trust: TrustSettings::default(),
            decorations: Decorations::default(),
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
    /// `lock.command` starts with a blank program.
    BlankLockCommand,
    /// `lock.idle_timeout_s` outside `0..=MAX_IDLE_TIMEOUT_S`.
    BadIdleTimeout(i64),
    /// `trust.input[n]` is empty or starts with a blank program.
    BadTrustCommand(usize),
    /// `decorations.width` outside 0..=32.
    BadBorderWidth(i64),
    /// A colour that is not `#rrggbb` or `#rrggbbaa`; the key is
    /// `active_color` or `inactive_color`.
    BadColor(&'static str, String),
    /// A key no layer knows (the layers' registry, `config::keys`).
    UnknownKey(String),
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
            Self::BlankLockCommand => {
                write!(f, "lock.command: the program is blank (use [] for none)")
            }
            Self::BadIdleTimeout(value) => write!(
                f,
                "lock.idle_timeout_s: {value} is outside 0..={MAX_IDLE_TIMEOUT_S} (0 turns it off)"
            ),
            Self::BadTrustCommand(at) => {
                write!(f, "trust.input[{at}]: needs a command with a program name")
            }
            Self::BadBorderWidth(width) => write!(
                f,
                "decorations.width: {width} is outside 0..={MAX_BORDER_WIDTH}"
            ),
            Self::BadColor(key, text) => {
                write!(f, "decorations.{key}: {text:?} is not #rrggbb or #rrggbbaa")
            }
            Self::UnknownKey(key) => write!(f, "{key}: unknown key"),
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
    lock: RawLock,
    #[serde(default)]
    trust: RawTrust,
    #[serde(default)]
    decorations: RawDecorations,
    #[serde(default)]
    window_rule: Vec<RawRule>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawDecorations {
    width: Option<i64>,
    active_color: Option<String>,
    inactive_color: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLock {
    command: Option<Vec<String>>,
    idle_timeout_s: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawTrust {
    input: Option<Vec<Vec<String>>>,
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
pub(crate) struct RawRule {
    app_id: Option<String>,
    title: Option<String>,
    floating: Option<bool>,
    workspace: Option<i64>,
    output: Option<String>,
    size: Option<[i64; 2]>,
}

pub(crate) fn gap(
    key: &'static str,
    value: Option<i64>,
    default: i32,
) -> Result<i32, SettingsError> {
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

pub(crate) fn rule(index: usize, raw: RawRule) -> Result<Rule, SettingsError> {
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

pub(crate) fn device_choice(path: &str) -> Result<DeviceChoice, SettingsError> {
    if path == "auto" {
        return Ok(DeviceChoice::Auto);
    }
    let p = Path::new(path);
    let under_dri = p.strip_prefix("/dev/dri").is_ok_and(|rest| {
        let mut parts = rest.components();
        matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
    });
    if under_dri {
        Ok(DeviceChoice::Node(p.to_owned()))
    } else {
        Err(SettingsError::BadRenderDevice(path.to_owned()))
    }
}

pub(crate) fn sync_mode(name: &str) -> Result<SyncMode, SettingsError> {
    match name {
        "auto" => Ok(SyncMode::Auto),
        "on" => Ok(SyncMode::On),
        "off" => Ok(SyncMode::Off),
        other => Err(SettingsError::UnknownSyncMode(other.to_owned())),
    }
}

fn render(raw: RawRender) -> Result<RenderSettings, SettingsError> {
    let device = raw
        .device
        .as_deref()
        .map_or(Ok(DeviceChoice::Auto), device_choice)?;
    let explicit_sync = raw
        .explicit_sync
        .as_deref()
        .map_or(Ok(SyncMode::Auto), sync_mode)?;
    Ok(RenderSettings {
        device,
        explicit_sync,
        wait_for_frame_completion: raw.wait_for_frame_completion.unwrap_or(false),
    })
}

pub(crate) fn lock_command(command: &[String]) -> Result<(), SettingsError> {
    if command
        .first()
        .is_some_and(|program| program.trim().is_empty())
    {
        return Err(SettingsError::BlankLockCommand);
    }
    Ok(())
}

pub(crate) fn idle_timeout(secs: i64) -> Result<u32, SettingsError> {
    match u32::try_from(secs) {
        Ok(s) if s <= MAX_IDLE_TIMEOUT_S => Ok(s),
        _ => Err(SettingsError::BadIdleTimeout(secs)),
    }
}

pub(crate) fn trust_input(input: &[Vec<String>]) -> Result<(), SettingsError> {
    for (at, argv) in input.iter().enumerate() {
        if argv.first().is_none_or(|program| program.trim().is_empty()) {
            return Err(SettingsError::BadTrustCommand(at));
        }
    }
    Ok(())
}

pub(crate) fn border_width(width: i64) -> Result<i32, SettingsError> {
    match i32::try_from(width) {
        Ok(w) if (0..=MAX_BORDER_WIDTH).contains(&w) => Ok(w),
        _ => Err(SettingsError::BadBorderWidth(width)),
    }
}

pub(crate) fn border_color(key: &'static str, text: &str) -> Result<Color, SettingsError> {
    parse_color(text).ok_or_else(|| SettingsError::BadColor(key, text.to_owned()))
}

fn decorations(raw: RawDecorations) -> Result<Decorations, SettingsError> {
    let base = Decorations::default();
    Ok(Decorations {
        width: raw.width.map_or(Ok(base.width), border_width)?,
        active: raw
            .active_color
            .as_deref()
            .map_or(Ok(base.active), |c| border_color("active_color", c))?,
        inactive: raw
            .inactive_color
            .as_deref()
            .map_or(Ok(base.inactive), |c| border_color("inactive_color", c))?,
    })
}

fn lock(raw: RawLock) -> Result<LockSettings, SettingsError> {
    let command = raw
        .command
        .unwrap_or_else(|| LockSettings::default().command);
    lock_command(&command)?;
    let idle_timeout_s = raw
        .idle_timeout_s
        .map_or(Ok(DEFAULT_IDLE_TIMEOUT_S), idle_timeout)?;
    Ok(LockSettings {
        command,
        idle_timeout_s,
    })
}

fn trust(raw: RawTrust) -> Result<TrustSettings, SettingsError> {
    let input = raw.input.unwrap_or_default();
    trust_input(&input)?;
    Ok(TrustSettings { input })
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
        lock: lock(raw.lock)?,
        trust: trust(raw.trust)?,
        decorations: decorations(raw.decorations)?,
    })
}

/// Reads and parses a config file.
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    let text = std::fs::read_to_string(path).map_err(SettingsError::Io)?;
    parse(&text)
}
