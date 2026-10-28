//! The render device and explicit sync control of `render.v1` (#107): what
//! the loop knows of the devices, and the pure planning of
//! `SetRenderDevice` and `SetExplicitSync`. Nothing is switched or written
//! here; `gpu::glue` acts on the plans.
//!
//! The checks run in a fixed order and the first failure rejects the call:
//! the device must exist (`on` must be supported), the user file must not
//! set the key, and a locked session keeps the change pending for unlock.

use std::collections::BTreeMap;

use serde_json::json;
use tetsu_bus::render::{self as wire, Render};
use tetsu_bus::server::ErrorReply;

use super::sync::{Support, decide};
use crate::settings::SyncMode;

/// The config key `SetRenderDevice` writes.
pub const DEVICE_KEY: &str = "render.device";
/// The config key `SetExplicitSync` writes.
pub const SYNC_KEY: &str = "render.explicit_sync";

/// One render device, as the backend found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceFacts {
    /// The id callers name it by: the basename of its node.
    pub id: String,
    /// Its render node path, the value persisted in `render.device`.
    pub node: String,
    /// The kernel driver.
    pub driver: String,
    /// The driver version, where readable.
    pub driver_version: Option<String>,
    /// The marketing name, where known.
    pub name: Option<String>,
    /// A fake device whose every hot switch fails (headless tests only).
    pub fails: bool,
}

impl DeviceFacts {
    /// A device with only its id, node and driver known.
    pub fn new(id: &str, node: &str, driver: &str) -> Self {
        Self {
            id: id.to_owned(),
            node: node.to_owned(),
            driver: driver.to_owned(),
            driver_version: None,
            name: None,
            fails: false,
        }
    }
}

/// Where a `SetRenderDevice` points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Back to the automatic choice (`device_id` null).
    Auto,
    /// The device with this id.
    Device(String),
}

impl Target {
    /// The target of a `device_id`.
    pub fn of(device_id: Option<&str>) -> Self {
        device_id.map_or(Self::Auto, |id| Self::Device(id.to_owned()))
    }
}

/// One accepted change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A render device switch.
    Device(Target),
    /// A new explicit sync mode.
    Sync(SyncMode),
}

/// What to do with an accepted call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// The session is locked: keep it for unlock, write nothing.
    Pending(Change),
    /// Apply it now and write `value` to the managed `key`.
    Apply {
        /// The change.
        change: Change,
        /// The config key.
        key: &'static str,
        /// The managed value, as TOML text of a string.
        value: String,
    },
}

/// How a backend took a device switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Switch {
    /// The device renders now.
    Applied,
    /// The device renders from the next start; the current one stays.
    NextStart,
    /// The hot switch failed; the previous device renders.
    Failed(String),
}

/// Why a call is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// No render device has this id.
    DeviceNotFound(String),
    /// The hardware or driver cannot do it.
    UnsupportedByHardware(String),
    /// The hot switch failed.
    SwitchFailed(String),
    /// The user file sets these keys.
    OverriddenByUser(Vec<String>),
    /// The managed file could not be written.
    StateWriteFailed(String),
}

impl RenderError {
    /// The `render.v1` error reply.
    pub fn reply(&self) -> ErrorReply {
        match self {
            Self::DeviceNotFound(id) => {
                ErrorReply::new(wire::DEVICE_NOT_FOUND, json!({ "id": id }))
            }
            Self::UnsupportedByHardware(reason) => {
                ErrorReply::new(wire::UNSUPPORTED_BY_HARDWARE, json!({ "reason": reason }))
            }
            Self::SwitchFailed(reason) => {
                ErrorReply::new(wire::SWITCH_FAILED, json!({ "reason": reason }))
            }
            Self::OverriddenByUser(keys) => {
                ErrorReply::new(wire::OVERRIDDEN_BY_USER, json!({ "keys": keys }))
            }
            Self::StateWriteFailed(reason) => {
                ErrorReply::new(wire::STATE_WRITE_FAILED, json!({ "reason": reason }))
            }
        }
    }
}

/// The support of a backend without a renderer of its own (headless,
/// winit): never explicit sync.
pub fn no_syncobj() -> Support {
    Support {
        eventfd: false,
        driver: "headless".to_owned(),
        version: None,
    }
}

/// The loop's render state: the devices, which renders, explicit sync,
/// what waits for unlock and the hot switches that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderCtl {
    /// Every render device, in the order found.
    pub devices: Vec<DeviceFacts>,
    /// The id of the device that renders now.
    pub current: String,
    /// The heuristic chose `current`.
    pub automatic: bool,
    /// The id of the heuristic's choice.
    pub auto_id: String,
    /// What the render device offers for explicit sync.
    pub support: Support,
    /// Explicit sync follows the mode at once (no syncobj global to keep).
    pub sync_live: bool,
    /// Explicit sync runs now (decided at start when not live).
    pub sync_running: bool,
    /// The requested mode: the effective `render.explicit_sync`.
    pub mode: SyncMode,
    /// A device switch waiting for unlock.
    pub pending_device: Option<Target>,
    /// A mode waiting for unlock.
    pub pending_sync: Option<SyncMode>,
    /// Failed hot switches per device id.
    pub failures: BTreeMap<String, i64>,
    /// The state last sent to the bus.
    pub published: Option<Render>,
    /// While set, nothing is published (a change is half done).
    pub batch: bool,
}

impl Default for RenderCtl {
    fn default() -> Self {
        Self {
            devices: Vec::new(),
            current: String::new(),
            automatic: true,
            auto_id: String::new(),
            support: no_syncobj(),
            sync_live: true,
            sync_running: false,
            mode: SyncMode::Auto,
            pending_device: None,
            pending_sync: None,
            failures: BTreeMap::new(),
            published: None,
            batch: false,
        }
    }
}

impl RenderCtl {
    /// One synthetic device `id` (node `/dev/dri/<id>`) that renders, chosen
    /// automatically; explicit sync is never supported.
    pub fn synthetic(id: &str, driver: &str) -> Self {
        Self::with_devices(
            vec![DeviceFacts::new(id, &format!("/dev/dri/{id}"), driver)],
            id,
        )
    }

    /// `devices` with `current` rendering, chosen automatically.
    pub fn with_devices(devices: Vec<DeviceFacts>, current: &str) -> Self {
        Self {
            devices,
            current: current.to_owned(),
            auto_id: current.to_owned(),
            ..Self::default()
        }
    }

    /// The device with this id.
    pub fn device(&self, id: &str) -> Option<&DeviceFacts> {
        self.devices.iter().find(|d| d.id == id)
    }

    /// Every device id, in order.
    pub fn ids(&self) -> Vec<String> {
        self.devices.iter().map(|d| d.id.clone()).collect()
    }

    /// Adds a device; false when the id is taken.
    pub fn add(&mut self, facts: DeviceFacts) -> bool {
        if self.device(&facts.id).is_some() {
            return false;
        }
        self.devices.push(facts);
        true
    }

    /// Removes a device. The rendering one and the automatic choice stay,
    /// so `device_id` and the automatic target always name a listed device.
    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        if id == self.current || id == self.auto_id {
            return Err(format!("{id} renders or is the automatic choice"));
        }
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        if self.devices.len() == before {
            return Err(format!("no device {id}"));
        }
        Ok(())
    }

    /// Keeps a change for unlock; a later one of the same kind replaces it.
    pub fn hold(&mut self, change: Change) {
        match change {
            Change::Device(target) => self.pending_device = Some(target),
            Change::Sync(mode) => self.pending_sync = Some(mode),
        }
    }

    /// Counts a failed hot switch to `id`.
    pub fn failed(&mut self, id: &str) {
        *self.failures.entry(id.to_owned()).or_insert(0) += 1;
    }

    /// The device id a target resolves to.
    pub fn resolve(&self, target: &Target) -> String {
        match target {
            Target::Auto => self.auto_id.clone(),
            Target::Device(id) => id.clone(),
        }
    }
}

/// The text of a mode, as the config and the wire spell it.
pub fn mode_name(mode: SyncMode) -> &'static str {
    match mode {
        SyncMode::Auto => "auto",
        SyncMode::On => "on",
        SyncMode::Off => "off",
    }
}

/// The value a change writes: the node path, `auto`, or the mode.
pub fn config_value(ctl: &RenderCtl, change: &Change) -> String {
    match change {
        Change::Device(Target::Auto) => "auto".to_owned(),
        Change::Device(Target::Device(id)) => ctl
            .device(id)
            .map_or_else(|| format!("/dev/dri/{id}"), |d| d.node.clone()),
        Change::Sync(mode) => mode_name(*mode).to_owned(),
    }
}

fn settle(
    ctl: &RenderCtl,
    change: Change,
    key: &'static str,
    user_keys: &[String],
    locked: bool,
) -> Result<Plan, RenderError> {
    if user_keys.iter().any(|k| k == key) {
        return Err(RenderError::OverriddenByUser(vec![key.to_owned()]));
    }
    if locked {
        return Ok(Plan::Pending(change));
    }
    let value = config_value(ctl, &change);
    Ok(Plan::Apply { change, key, value })
}

/// Plans a `SetRenderDevice`.
pub fn plan_device(
    ctl: &RenderCtl,
    target: &Target,
    user_keys: &[String],
    locked: bool,
) -> Result<Plan, RenderError> {
    if let Target::Device(id) = target
        && ctl.device(id).is_none()
    {
        return Err(RenderError::DeviceNotFound(id.clone()));
    }
    settle(
        ctl,
        Change::Device(target.clone()),
        DEVICE_KEY,
        user_keys,
        locked,
    )
}

/// Plans a `SetExplicitSync`.
pub fn plan_sync(
    ctl: &RenderCtl,
    mode: SyncMode,
    user_keys: &[String],
    locked: bool,
) -> Result<Plan, RenderError> {
    if mode == SyncMode::On {
        let can = decide(SyncMode::On, ctl.support.clone());
        if !can.supported {
            let reason = can.reason.unwrap_or_else(|| "unsupported".to_owned());
            return Err(RenderError::UnsupportedByHardware(reason));
        }
    }
    settle(ctl, Change::Sync(mode), SYNC_KEY, user_keys, locked)
}

/// The tty backend keeps its device until the next start: only a switch
/// to the device already rendering (the automatic choice included) is
/// applied now.
pub fn tty_switch(current: &str, target: &Target, auto_id: &str) -> Switch {
    let wanted = match target {
        Target::Auto => auto_id,
        Target::Device(id) => id.as_str(),
    };
    if wanted == current {
        Switch::Applied
    } else {
        Switch::NextStart
    }
}
