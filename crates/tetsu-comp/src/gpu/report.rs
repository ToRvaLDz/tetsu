//! The `render.v1` state as the bus reports it (#107), built from the
//! loop's `RenderCtl`, and the `changed` owed when it moved. Pure.

use std::collections::BTreeMap;

use tetsu_bus::render::{
    self as wire, ExplicitSync, PendingDevice, Render, RenderDevice, RenderEvent,
};

use super::control::{RenderCtl, Target};
use super::sync::decide;
use crate::settings::SyncMode;

/// The reason of a mode that only a restart can bring in.
pub const NEXT_START: &str = "applies at the next start";

/// A mode on the wire.
pub fn wire_mode(mode: SyncMode) -> wire::SyncMode {
    match mode {
        SyncMode::Auto => wire::SyncMode::Auto,
        SyncMode::On => wire::SyncMode::On,
        SyncMode::Off => wire::SyncMode::Off,
    }
}

/// The explicit sync state: the requested mode at once; what runs is the
/// decision made at start unless the backend follows the mode live.
pub fn explicit_sync(ctl: &RenderCtl) -> ExplicitSync {
    let wanted = decide(ctl.mode, ctl.support.clone());
    let enabled = if ctl.sync_live {
        wanted.enabled
    } else {
        ctl.sync_running
    };
    let reason = if enabled == wanted.enabled {
        wanted.reason
    } else {
        Some(NEXT_START.to_owned())
    };
    ExplicitSync {
        mode: wire_mode(ctl.mode),
        supported: wanted.supported,
        enabled,
        reason,
    }
}

fn pending_device(target: &Target) -> PendingDevice {
    match target {
        Target::Auto => PendingDevice {
            automatic: true,
            device_id: None,
        },
        Target::Device(id) => PendingDevice {
            automatic: false,
            device_id: Some(id.clone()),
        },
    }
}

/// The state of `ctl`; `outputs` maps a device id to the outputs wired to
/// it.
pub fn render(ctl: &RenderCtl, outputs: &BTreeMap<String, Vec<String>>) -> Render {
    let devices = ctl
        .devices
        .iter()
        .map(|d| RenderDevice {
            id: d.id.clone(),
            node: d.node.clone(),
            driver: d.driver.clone(),
            driver_version: d.driver_version.clone(),
            name: d.name.clone(),
            output_ids: outputs.get(&d.id).cloned().unwrap_or_default(),
            power_mw: None,
            switch_failures: ctl.failures.get(&d.id).copied().unwrap_or(0),
        })
        .collect();
    Render {
        device_id: ctl.current.clone(),
        automatic: ctl.automatic,
        devices,
        explicit_sync: explicit_sync(ctl),
        pending_device: ctl.pending_device.as_ref().map(pending_device),
        pending_explicit_sync: ctl.pending_sync.map(wire_mode),
    }
}

/// The `changed` owed to the watchers, if anything moved.
pub fn diff(last: &Render, now: &Render) -> Option<RenderEvent> {
    (last != now).then(|| RenderEvent::changed(now.clone()))
}
