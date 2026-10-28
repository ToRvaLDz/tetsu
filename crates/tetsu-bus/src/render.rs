//! `org.tetsu.comp.render.v1`: the wire types of the device and explicit
//! sync half, shaped exactly as the IDL. The stats types come with the
//! stats methods.

use serde::{Deserialize, Deserializer, Serialize};

/// The interface name.
pub const INTERFACE: &str = "org.tetsu.comp.render.v1";
/// The interface definition, for `GetInterfaceDescription`.
pub const IDL: &str = include_str!("../idl/org.tetsu.comp.render.v1.varlink");

/// The fully qualified `GetRender`.
pub const GET_RENDER: &str = "org.tetsu.comp.render.v1.GetRender";
/// The fully qualified `WatchRender`.
pub const WATCH_RENDER: &str = "org.tetsu.comp.render.v1.WatchRender";
/// The fully qualified `ListRenderStats`.
pub const LIST_RENDER_STATS: &str = "org.tetsu.comp.render.v1.ListRenderStats";
/// The fully qualified `WatchRenderStats`.
pub const WATCH_RENDER_STATS: &str = "org.tetsu.comp.render.v1.WatchRenderStats";
/// The fully qualified `SetRenderDevice`.
pub const SET_RENDER_DEVICE: &str = "org.tetsu.comp.render.v1.SetRenderDevice";
/// The fully qualified `SetExplicitSync`.
pub const SET_EXPLICIT_SYNC: &str = "org.tetsu.comp.render.v1.SetExplicitSync";

/// `DeviceNotFound (id)`.
pub const DEVICE_NOT_FOUND: &str = "org.tetsu.comp.render.v1.DeviceNotFound";
/// `UnsupportedByHardware (reason)`.
pub const UNSUPPORTED_BY_HARDWARE: &str = "org.tetsu.comp.render.v1.UnsupportedByHardware";
/// `SwitchFailed (reason)`.
pub const SWITCH_FAILED: &str = "org.tetsu.comp.render.v1.SwitchFailed";
/// `OverriddenByUser (keys)`.
pub const OVERRIDDEN_BY_USER: &str = "org.tetsu.comp.render.v1.OverriddenByUser";
/// `StateWriteFailed (reason)`.
pub const STATE_WRITE_FAILED: &str = "org.tetsu.comp.render.v1.StateWriteFailed";
/// `PermissionDenied ()`.
pub const PERMISSION_DENIED: &str = "org.tetsu.comp.render.v1.PermissionDenied";

/// `RenderDevice`. The nullable fields are always written, as `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderDevice {
    /// Opaque; compare for equality only.
    pub id: String,
    /// The DRM render node.
    pub node: String,
    /// The kernel driver.
    pub driver: String,
    /// Null where not readable.
    #[serde(default)]
    pub driver_version: Option<String>,
    /// From the PCI ids database; null when unknown.
    #[serde(default)]
    pub name: Option<String>,
    /// The outputs wired to this device.
    pub output_ids: Vec<String>,
    /// Power draw; null where not readable.
    #[serde(default)]
    pub power_mw: Option<i64>,
    /// Hot switches to this device that failed this session.
    pub switch_failures: i64,
}

/// The explicit sync mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Enabled where supported.
    #[default]
    Auto,
    /// Asked for; refused where unsupported.
    On,
    /// Never.
    Off,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `ExplicitSync`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExplicitSync {
    /// The requested mode.
    pub mode: SyncMode,
    /// Whether the driver and the render device support it.
    pub supported: bool,
    /// Whether it is in use now.
    pub enabled: bool,
    /// Why it is unsupported or disabled.
    #[serde(default)]
    pub reason: Option<String>,
}

/// `PendingDevice`: a switch that waits for unlock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingDevice {
    /// True to return to the automatic choice; `device_id` is then null.
    pub automatic: bool,
    /// The device asked for.
    #[serde(default)]
    pub device_id: Option<String>,
}

/// `Render`. The nullable fields are always written, as `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Render {
    /// The device that renders now.
    pub device_id: String,
    /// True when the heuristic chose the device.
    pub automatic: bool,
    /// Every render device.
    pub devices: Vec<RenderDevice>,
    /// The explicit sync state.
    pub explicit_sync: ExplicitSync,
    /// A device switch waiting for unlock.
    #[serde(default)]
    pub pending_device: Option<PendingDevice>,
    /// An explicit sync mode waiting for unlock.
    #[serde(default)]
    pub pending_explicit_sync: Option<SyncMode>,
}

/// `RenderEvent.kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderKind {
    /// The first event of a stream.
    Initial,
    /// The full new state.
    Changed,
    /// A pending change was discarded at unlock; a `changed` follows.
    PendingDropped,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// `RenderEvent.reason`: why a pending change was discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// The device is no longer there.
    DeviceGone,
    /// The user file now sets the key.
    OverriddenByUser,
    /// A value this build does not know.
    #[serde(other)]
    Unknown,
}

/// One event of `WatchRender`. Absent fields are omitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderEvent {
    /// Which event.
    pub kind: RenderKind,
    /// `initial`, `changed`: the full new state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<Render>,
    /// `pending_dropped`: why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<DropReason>,
}

impl RenderEvent {
    /// `initial`.
    pub fn initial(render: Render) -> Self {
        Self {
            kind: RenderKind::Initial,
            render: Some(render),
            reason: None,
        }
    }

    /// `changed`.
    pub fn changed(render: Render) -> Self {
        Self {
            kind: RenderKind::Changed,
            render: Some(render),
            reason: None,
        }
    }

    /// `pending_dropped`.
    pub fn pending_dropped(reason: DropReason) -> Self {
        Self {
            kind: RenderKind::PendingDropped,
            render: None,
            reason: Some(reason),
        }
    }
}

/// The reply of `GetRender`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GetRenderReply {
    /// The render state.
    pub render: Render,
}

/// The parameters of `SetRenderDevice`. An absent `device_id` is null.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRenderDeviceParams {
    /// Null returns to the automatic choice.
    #[serde(default)]
    pub device_id: Option<String>,
}

/// The parameters of `SetExplicitSync`. Only the three modes are read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetExplicitSyncParams {
    /// `auto`, `on` or `off`.
    #[serde(deserialize_with = "known_mode")]
    pub mode: SyncMode,
}

/// The reply of both Set calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetRenderReply {
    /// The change waits for unlock.
    pub pending: bool,
}

/// A mode a caller sends: an unknown one is refused, not mapped.
fn known_mode<'de, D: Deserializer<'de>>(d: D) -> Result<SyncMode, D::Error> {
    match SyncMode::deserialize(d)? {
        SyncMode::Unknown => Err(serde::de::Error::custom("mode is none of auto, on, off")),
        mode => Ok(mode),
    }
}
