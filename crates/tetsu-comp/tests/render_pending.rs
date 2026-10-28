//! The changes kept while locked, settled at unlock: dropped with a reason
//! when their device is gone or the user file now sets their key.

mod common;

use tetsu_bus::render::{DropReason, RenderEvent};
use tetsu_comp::gpu::control::{DeviceFacts, RenderCtl, Target};
use tetsu_comp::gpu::pending::{Resolution, resolve};
use tetsu_comp::settings::SyncMode;

fn ctl() -> RenderCtl {
    let mut ctl = RenderCtl::synthetic("headless-0", "headless");
    ctl.add(DeviceFacts::new("gpu-1", "/dev/dri/gpu-1", "headless"));
    ctl
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|k| (*k).to_owned()).collect()
}

fn present() -> Vec<String> {
    strings(&["headless-0", "gpu-1"])
}

#[test]
fn a_pending_device_that_is_gone_is_dropped() {
    let mut ctl = ctl();
    ctl.pending_device = Some(Target::Device("gpu-1".into()));
    let resolution = resolve(&ctl, &strings(&["headless-0"]), &[]);
    assert_eq!(
        resolution,
        Resolution {
            dropped: vec![DropReason::DeviceGone],
            device: None,
            sync: None,
        }
    );
}

#[test]
fn a_key_the_user_file_now_sets_is_dropped() {
    let mut ctl = ctl();
    ctl.pending_device = Some(Target::Auto);
    let resolution = resolve(&ctl, &present(), &strings(&["render.device"]));
    assert_eq!(resolution.dropped, [DropReason::OverriddenByUser]);
    assert_eq!(resolution.device, None);

    let mut ctl = self::ctl();
    ctl.pending_sync = Some(SyncMode::Off);
    let resolution = resolve(&ctl, &present(), &strings(&["render.explicit_sync"]));
    assert_eq!(resolution.dropped, [DropReason::OverriddenByUser]);
    assert_eq!(resolution.sync, None);
}

#[test]
fn both_dropped_give_two_reasons_the_device_first() {
    let mut ctl = ctl();
    ctl.pending_device = Some(Target::Device("gpu-9".into()));
    ctl.pending_sync = Some(SyncMode::Off);
    let resolution = resolve(&ctl, &present(), &strings(&["render.explicit_sync"]));
    assert_eq!(
        resolution.dropped,
        [DropReason::DeviceGone, DropReason::OverriddenByUser]
    );
}

#[test]
fn nothing_pending_gives_nothing() {
    let resolution = resolve(&ctl(), &present(), &strings(&["render.device"]));
    assert_eq!(resolution, Resolution::default());
}

#[test]
fn a_surviving_change_is_returned_for_apply() {
    let mut ctl = ctl();
    ctl.pending_device = Some(Target::Device("gpu-1".into()));
    ctl.pending_sync = Some(SyncMode::Off);
    let resolution = resolve(&ctl, &present(), &strings(&["layout.default"]));
    assert_eq!(
        resolution,
        Resolution {
            dropped: vec![],
            device: Some(Target::Device("gpu-1".into())),
            sync: Some(SyncMode::Off),
        }
    );
    // The automatic choice is never gone.
    ctl.pending_device = Some(Target::Auto);
    let resolution = resolve(&ctl, &[], &[]);
    assert_eq!(resolution.device, Some(Target::Auto));
}

#[test]
fn every_pending_dropped_passes_the_contract() {
    let contract = common::contract();
    for reason in [DropReason::DeviceGone, DropReason::OverriddenByUser] {
        common::check_render_event(&contract, &RenderEvent::pending_dropped(reason));
    }
}
