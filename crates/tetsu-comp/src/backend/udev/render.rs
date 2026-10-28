//! The render devices of the tty backend as `render.v1` reports them
//! (#107): one per card with a render node, its id the basename of that
//! node (`renderD128`), its driver and driver version. The marketing name
//! and the power draw are not read yet.
//!
//! The device renders until the next start: `gpu::control::tty_switch`
//! decides, and a switch to another device is persisted for then.

use std::collections::BTreeMap;

use smithay::backend::drm::DrmNode;
use smithay::reexports::drm::Device as BasicDevice;

use super::Tty;
use super::device::{NVIDIA_VERSION_FILE, Probed, Selected, candidate};
use crate::gpu::control::{DeviceFacts, RenderCtl};
use crate::gpu::select::{Candidate, Selection, select};
use crate::state::{OutputEntry, State};

/// What the probe tells about the render devices.
pub(super) struct Found {
    devices: Vec<DeviceFacts>,
    /// The heuristic's choice, whatever the config says.
    auto_id: Option<String>,
}

/// The id of a render node: the basename of its path.
pub(crate) fn device_id(node: &DrmNode) -> String {
    node.dev_path()
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| node.to_string())
}

fn driver_version(p: &Probed) -> Option<String> {
    if p.driver.contains("nvidia") {
        let text = std::fs::read_to_string(NVIDIA_VERSION_FILE).ok()?;
        return Some(text.trim().to_owned()).filter(|v| !v.is_empty());
    }
    let (major, minor, patch) = p.drm.get_driver().ok()?.version;
    Some(format!("{major}.{minor}.{patch}"))
}

/// Reads the facts of every card with a render node, before the cards are
/// opened for good.
pub(super) fn facts(probed: &[Probed]) -> Found {
    let mut devices: Vec<DeviceFacts> = Vec::new();
    for p in probed {
        let Some(render) = p.render_node else {
            continue;
        };
        let id = device_id(&render);
        if devices.iter().any(|d| d.id == id) {
            continue;
        }
        let node = render
            .dev_path()
            .map_or_else(|| format!("/dev/dri/{id}"), |p| p.display().to_string());
        devices.push(DeviceFacts {
            driver_version: driver_version(p),
            ..DeviceFacts::new(&id, &node, &p.driver)
        });
    }
    let candidates: Vec<Candidate> = probed.iter().filter_map(candidate).collect();
    let auto_id = match select(&candidates, None) {
        Selection::Device { id, .. } => DrmNode::from_dev_id(id).ok().map(|n| device_id(&n)),
        Selection::NoGpu => None,
    };
    Found { devices, auto_id }
}

/// Starts the render state with the chosen device.
pub(super) fn install(state: &mut State, found: Found, selected: &Selected) {
    let current = device_id(&selected.node);
    let mut ctl = RenderCtl::with_devices(found.devices, &current);
    if ctl.device(&current).is_none() {
        ctl.add(DeviceFacts::new(
            &current,
            &format!("/dev/dri/{current}"),
            &selected.driver,
        ));
    }
    ctl.automatic = selected.automatic;
    ctl.auto_id = found.auto_id.unwrap_or(current);
    state.render = ctl;
}

/// The outputs each device drives: a head counts for its card's render
/// node, or for the render device when the card only displays.
pub(crate) fn output_ids(tty: &Tty, outputs: &[OutputEntry]) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for head in &tty.heads {
        let render = tty
            .devices
            .iter()
            .find(|d| d.node == head.node)
            .and_then(|d| d.render_node)
            .unwrap_or(tty.render_node);
        if let Some(entry) = outputs.iter().find(|o| o.output == head.output) {
            map.entry(device_id(&render))
                .or_default()
                .push(entry.id.clone());
        }
    }
    map
}
