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
    // Not written yet.
    let _ = (ctl, device_ids, user_keys, DEVICE_KEY, SYNC_KEY);
    Resolution::default()
}
