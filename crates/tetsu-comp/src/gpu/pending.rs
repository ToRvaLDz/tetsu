//! What a change kept for unlock becomes at unlock (#107): dropped when its
//! device is gone or the user file now sets its key, else applied. Pure:
//! `gpu::glue` clears the pending slots and applies what is left.

use tetsu_bus::render::DropReason;

use super::control::{DEVICE_KEY, RenderCtl, SYNC_KEY, Target};
use crate::settings::SyncMode;

/// The outcome at unlock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolution {
    /// Why each discarded change went, the device first.
    pub dropped: Vec<DropReason>,
    /// The device switch to apply.
    pub device: Option<Target>,
    /// The mode to apply.
    pub sync: Option<SyncMode>,
}

/// Settles the pending changes of `ctl` against the devices present and
/// the keys the user file sets now.
pub fn resolve(ctl: &RenderCtl, device_ids: &[String], user_keys: &[String]) -> Resolution {
    let user_sets = |key: &str| user_keys.iter().any(|k| k == key);
    let mut resolution = Resolution::default();
    if let Some(target) = &ctl.pending_device {
        let gone = matches!(target, Target::Device(id) if !device_ids.contains(id));
        if gone {
            resolution.dropped.push(DropReason::DeviceGone);
        } else if user_sets(DEVICE_KEY) {
            resolution.dropped.push(DropReason::OverriddenByUser);
        } else {
            resolution.device = Some(target.clone());
        }
    }
    if let Some(mode) = ctl.pending_sync {
        if user_sets(SYNC_KEY) {
            resolution.dropped.push(DropReason::OverriddenByUser);
        } else {
            resolution.sync = Some(mode);
        }
    }
    resolution
}
