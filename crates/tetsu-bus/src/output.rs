//! `org.tetsu.comp.output.v1`: the wire types, shaped exactly as the IDL.
//!
//! Only the read side: `ListOutputs` and `WatchOutputs`.

use serde::{Deserialize, Serialize};

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.comp.output.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.comp.output.v1.varlink");
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.comp.output.v1.PermissionDenied";

/// `Mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mode {
    /// Pixels.
    pub width: i64,
    /// Pixels.
    pub height: i64,
    /// Millihertz.
    pub refresh_mhz: i64,
    /// Whether the display prefers this mode.
    pub preferred: bool,
}

/// `Output.transform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    /// Not rotated.
    Normal,
    /// Rotated by 90 degrees.
    #[serde(rename = "rotate_90")]
    Rotate90,
    /// Rotated by 180 degrees.
    #[serde(rename = "rotate_180")]
    Rotate180,
    /// Rotated by 270 degrees.
    #[serde(rename = "rotate_270")]
    Rotate270,
    /// Mirrored.
    Flipped,
    /// Mirrored, then rotated by 90 degrees.
    #[serde(rename = "flipped_90")]
    Flipped90,
    /// Mirrored, then rotated by 180 degrees.
    #[serde(rename = "flipped_180")]
    Flipped180,
    /// Mirrored, then rotated by 270 degrees.
    #[serde(rename = "flipped_270")]
    Flipped270,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `Output`. The nullable fields are always written, as `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Output {
    /// Opaque; compare for equality only.
    pub id: String,
    /// Connector name, for example "DP-1".
    pub name: String,
    /// From the EDID. Untrusted.
    #[serde(default)]
    pub make: Option<String>,
    /// From the EDID. Untrusted.
    #[serde(default)]
    pub model: Option<String>,
    /// From the EDID. Untrusted.
    #[serde(default)]
    pub serial: Option<String>,
    /// Whether the output is lit.
    pub enabled: bool,
    /// Every mode the display offers.
    pub modes: Vec<Mode>,
    /// Index into `modes`, null when disabled.
    #[serde(default)]
    pub current_mode: Option<i64>,
    /// Scale factor.
    pub scale: f64,
    /// Rotation and mirroring.
    pub transform: Transform,
    /// Position in the global logical space.
    pub x: i64,
    /// Position in the global logical space.
    pub y: i64,
}

/// `OutputEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// The first event of a stream.
    Initial,
    /// An output appeared.
    Added,
    /// An output's state changed.
    Changed,
    /// An output went away.
    Removed,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchOutputs`. Absent fields are omitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputEvent {
    /// Which event.
    pub kind: OutputKind,
    /// `initial`: every output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<Output>>,
    /// `added`, `changed`: the full new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Output>,
    /// `removed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

impl OutputEvent {
    fn of(kind: OutputKind) -> Self {
        Self {
            kind,
            outputs: None,
            output: None,
            id: None,
        }
    }

    /// `initial`.
    pub fn initial(outputs: Vec<Output>) -> Self {
        Self {
            outputs: Some(outputs),
            ..Self::of(OutputKind::Initial)
        }
    }

    /// `added`.
    pub fn added(output: Output) -> Self {
        Self {
            output: Some(output),
            ..Self::of(OutputKind::Added)
        }
    }

    /// `changed`.
    pub fn changed(output: Output) -> Self {
        Self {
            output: Some(output),
            ..Self::of(OutputKind::Changed)
        }
    }

    /// `removed`.
    pub fn removed(id: &str) -> Self {
        Self {
            id: Some(id.to_owned()),
            ..Self::of(OutputKind::Removed)
        }
    }
}

/// The reply of `ListOutputs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListOutputsReply {
    /// Every output.
    pub outputs: Vec<Output>,
}
