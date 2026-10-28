//! The `render.v1` state built from the loop's render control, and the
//! `changed` owed when it moved. Everything built passes the contract.

mod common;

use std::collections::BTreeMap;

use serde_json::json;
use tetsu_bus::render::{self as wire, PendingDevice, Render, RenderEvent, RenderKind};
use tetsu_bus_contract::Contract;
use tetsu_comp::gpu::control::{DeviceFacts, RenderCtl, Target};
use tetsu_comp::gpu::report::{diff, render};
use tetsu_comp::gpu::sync::Support;
use tetsu_comp::settings::SyncMode;

fn headless() -> RenderCtl {
    let mut ctl = RenderCtl::synthetic("headless-0", "headless");
    ctl.add(DeviceFacts::new("gpu-1", "/dev/dri/gpu-1", "headless"));
    ctl
}

/// A tty that started with explicit sync on (`running`) a device that
/// supports it (`eventfd`).
fn tty(eventfd: bool, running: bool) -> RenderCtl {
    let mut ctl = RenderCtl::with_devices(
        vec![DeviceFacts {
            driver_version: Some("1.6.0".into()),
            ..DeviceFacts::new("renderD128", "/dev/dri/renderD128", "i915")
        }],
        "renderD128",
    );
    ctl.support = Support {
        eventfd,
        driver: "i915".into(),
        version: None,
    };
    ctl.sync_live = false;
    ctl.sync_running = running;
    ctl
}

fn outputs(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(device, ids)| {
            (
                (*device).to_owned(),
                ids.iter().map(|id| (*id).to_owned()).collect(),
            )
        })
        .collect()
}

fn checked(contract: &Contract, render: &Render) {
    common::check(
        contract,
        "org.tetsu.comp.render.v1.GetRender",
        &json!({ "render": serde_json::to_value(render).unwrap() }),
    );
}

fn sync(ctl: &RenderCtl) -> (wire::SyncMode, bool, bool, Option<String>) {
    let s = render(ctl, &BTreeMap::new()).explicit_sync;
    (s.mode, s.supported, s.enabled, s.reason)
}

#[test]
fn the_device_id_is_always_listed() {
    let contract = common::contract();
    let mut ctl = headless();
    let _ = ctl.remove("headless-0");
    let built = render(&ctl, &BTreeMap::new());
    assert_eq!(built.device_id, "headless-0");
    assert!(built.automatic);
    assert!(built.devices.iter().any(|d| d.id == built.device_id));
    assert_eq!(built.devices.len(), 2);
    assert_eq!(built.devices[0].node, "/dev/dri/headless-0");
    assert_eq!(built.devices[0].driver, "headless");
    assert_eq!(built.devices[0].power_mw, None);
    checked(&contract, &built);
}

#[test]
fn output_ids_and_switch_failures_are_per_device() {
    let contract = common::contract();
    let mut ctl = headless();
    ctl.failed("gpu-1");
    let built = render(&ctl, &outputs(&[("headless-0", &["out-1", "out-2"])]));
    assert_eq!(built.devices[0].output_ids, ["out-1", "out-2"]);
    assert!(built.devices[1].output_ids.is_empty());
    assert_eq!(built.devices[0].switch_failures, 0);
    assert_eq!(built.devices[1].switch_failures, 1);
    checked(&contract, &built);
}

#[test]
fn headless_never_supports_explicit_sync_and_shows_the_mode_at_once() {
    let mut ctl = headless();
    let reason = |r: &str| Some(r.to_owned());
    assert_eq!(
        sync(&ctl),
        (
            wire::SyncMode::Auto,
            false,
            false,
            reason("no syncobj_eventfd")
        )
    );
    ctl.mode = SyncMode::Off;
    assert_eq!(
        sync(&ctl),
        (wire::SyncMode::Off, false, false, reason("off in config"))
    );
}

#[test]
fn the_tty_reports_a_mode_that_needs_a_restart() {
    let next = Some("applies at the next start".to_owned());
    // Started on: auto keeps it, off waits for the next start.
    let mut on = tty(true, true);
    assert_eq!(sync(&on), (wire::SyncMode::Auto, true, true, None));
    on.mode = SyncMode::Off;
    assert_eq!(sync(&on), (wire::SyncMode::Off, true, true, next.clone()));
    // Started off on a device that supports it: auto waits.
    let mut off = tty(true, false);
    off.mode = SyncMode::Off;
    assert_eq!(
        sync(&off),
        (
            wire::SyncMode::Off,
            true,
            false,
            Some("off in config".into())
        )
    );
    off.mode = SyncMode::Auto;
    assert_eq!(sync(&off), (wire::SyncMode::Auto, true, false, next));
    // Unsupported: nothing changes with a restart.
    let unsupported = tty(false, false);
    assert_eq!(
        sync(&unsupported),
        (
            wire::SyncMode::Auto,
            false,
            false,
            Some("no syncobj_eventfd".into())
        )
    );
}

#[test]
fn the_pending_changes_are_reported() {
    let contract = common::contract();
    let mut ctl = headless();
    ctl.pending_device = Some(Target::Device("gpu-1".into()));
    ctl.pending_sync = Some(SyncMode::Off);
    let built = render(&ctl, &BTreeMap::new());
    assert_eq!(
        built.pending_device,
        Some(PendingDevice {
            automatic: false,
            device_id: Some("gpu-1".into()),
        })
    );
    assert_eq!(built.pending_explicit_sync, Some(wire::SyncMode::Off));
    checked(&contract, &built);
    ctl.pending_device = Some(Target::Auto);
    let built = render(&ctl, &BTreeMap::new());
    assert_eq!(
        built.pending_device,
        Some(PendingDevice {
            automatic: true,
            device_id: None,
        })
    );
    checked(&contract, &built);
}

#[test]
fn diff_is_none_when_nothing_moved_and_changed_otherwise() {
    let contract = common::contract();
    let mut ctl = tty(true, true);
    let last = render(&ctl, &BTreeMap::new());
    assert_eq!(diff(&last, &render(&ctl, &BTreeMap::new())), None);
    ctl.automatic = false;
    let now = render(&ctl, &BTreeMap::new());
    let event = diff(&last, &now).expect("a changed");
    assert_eq!(event.kind, RenderKind::Changed);
    assert_eq!(event, RenderEvent::changed(now.clone()));
    common::check_render_event(&contract, &event);
    common::check_render_event(&contract, &RenderEvent::initial(now));
}
