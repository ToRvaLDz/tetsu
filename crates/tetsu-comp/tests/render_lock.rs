//! The render Set calls while the session is locked, on the whole
//! compositor with a real locker: kept pending, then applied or dropped at
//! unlock. Every reply, event and error goes through the contract checks.

mod common;
#[path = "common/locker.rs"]
mod locker;

use std::time::{Duration, Instant};

use common::Client;
use common::comp::Comp;
use locker::Locker;
use serde_json::json;
use tetsu_bus::render::{
    DropReason, GetRenderReply, PendingDevice, Render, RenderEvent, RenderKind, SyncMode,
};
use tetsu_bus_contract::Contract;

const GET_RENDER: &str = "org.tetsu.comp.render.v1.GetRender";
const WATCH_RENDER: &str = "org.tetsu.comp.render.v1.WatchRender";
const SET_DEVICE: &str = "org.tetsu.comp.render.v1.SetRenderDevice";
const SET_SYNC: &str = "org.tetsu.comp.render.v1.SetExplicitSync";
const WATCH_CONFIG: &str = "org.tetsu.comp.config.v1.WatchConfig";

const TIMEOUT: Duration = Duration::from_secs(10);

fn get_render(client: &mut Client, contract: &Contract) -> Render {
    let reply: GetRenderReply = serde_json::from_value(client.call(contract, GET_RENDER)).unwrap();
    reply.render
}

fn until(client: &mut Client, contract: &Contract, done: impl Fn(&Render) -> bool) -> Render {
    let start = Instant::now();
    loop {
        let render = get_render(client, contract);
        if done(&render) {
            return render;
        }
        assert!(start.elapsed() < TIMEOUT, "timed out; last {render:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn next(watch: &mut Client, contract: &Contract) -> RenderEvent {
    let event: RenderEvent = serde_json::from_value(watch.event(contract, WATCH_RENDER)).unwrap();
    common::check_render_event(contract, &event);
    event
}

/// A watch whose `initial` is already read.
fn watch(comp: &Comp, contract: &Contract) -> Client {
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_RENDER, true);
    assert_eq!(next(&mut watch, contract).kind, RenderKind::Initial);
    watch
}

/// A compositor with the fake devices of `lines`, all listed, and its
/// session locked.
fn locked_with(lines: &[&str]) -> (Comp, Locker, Client, Contract) {
    let mut comp = Comp::start_hotplug(&[], None);
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    for line in lines {
        comp.control(line);
    }
    until(&mut client, &contract, |r| {
        r.devices.len() == lines.len() + 1
    });
    let mut locker = Locker::connect(&comp.wayland_socket());
    locker.lock();
    locker.wait_locked();
    (comp, locker, client, contract)
}

fn managed_text(comp: &Comp) -> Option<String> {
    std::fs::read_to_string(comp.managed_file()).ok()
}

#[test]
fn locked_calls_wait_and_apply_at_unlock_in_one_changed() {
    let (comp, mut locker, mut client, contract) = locked_with(&["gpu add headless-1"]);
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "headless-1" }));
    assert_eq!(reply, json!({ "pending": true }));
    let reply = client.call_with(&contract, SET_SYNC, &json!({ "mode": "off" }));
    assert_eq!(reply, json!({ "pending": true }));
    let render = until(&mut client, &contract, |r| {
        r.pending_explicit_sync.is_some()
    });
    assert_eq!(
        render.pending_device,
        Some(PendingDevice {
            automatic: false,
            device_id: Some("headless-1".into()),
        })
    );
    assert_eq!(render.pending_explicit_sync, Some(SyncMode::Off));
    assert_eq!(render.device_id, "headless-0");
    assert_eq!(render.explicit_sync.mode, SyncMode::Auto);
    assert_eq!(managed_text(&comp), None);

    let mut watch = watch(&comp, &contract);
    locker.unlock();
    let event = next(&mut watch, &contract);
    assert_eq!(event.kind, RenderKind::Changed, "{event:?}");
    let render = event.render.unwrap();
    assert_eq!(render.device_id, "headless-1");
    assert!(!render.automatic);
    assert_eq!(render.explicit_sync.mode, SyncMode::Off);
    assert_eq!(render.pending_device, None);
    assert_eq!(render.pending_explicit_sync, None);
    let managed: toml::Table = toml::from_str(&managed_text(&comp).unwrap()).unwrap();
    assert_eq!(
        managed["render"]["device"].as_str(),
        Some("/dev/dri/headless-1")
    );
    assert_eq!(managed["render"]["explicit_sync"].as_str(), Some("off"));
}

#[test]
fn a_pending_device_that_went_away_is_dropped_at_unlock() {
    let (mut comp, mut locker, mut client, contract) = locked_with(&["gpu add headless-1"]);
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "headless-1" }));
    assert_eq!(reply, json!({ "pending": true }));
    comp.control("gpu remove headless-1");
    until(&mut client, &contract, |r| r.devices.len() == 1);

    let mut watch = watch(&comp, &contract);
    locker.unlock();
    let dropped = next(&mut watch, &contract);
    assert_eq!(
        dropped,
        RenderEvent::pending_dropped(DropReason::DeviceGone)
    );
    let event = next(&mut watch, &contract);
    assert_eq!(event.kind, RenderKind::Changed);
    let render = event.render.unwrap();
    assert_eq!(render.device_id, "headless-0");
    assert_eq!(render.pending_device, None);
    assert_eq!(managed_text(&comp), None);
}

#[test]
fn a_key_the_user_file_now_sets_is_dropped_at_unlock() {
    let (comp, mut locker, mut client, contract) = locked_with(&["gpu add headless-1"]);
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "headless-1" }));
    assert_eq!(reply, json!({ "pending": true }));

    let mut config = Client::connect(&comp.bus);
    config.send(WATCH_CONFIG, true);
    assert_eq!(config.event(&contract, WATCH_CONFIG)["kind"], "initial");
    std::fs::create_dir_all(comp.user_file().parent().unwrap()).unwrap();
    std::fs::write(comp.user_file(), "[render]\ndevice = \"auto\"\n").unwrap();
    loop {
        let event = config.event(&contract, WATCH_CONFIG);
        if event["kind"] == "changed"
            && event["entry"]["key"] == "render.device"
            && event["entry"]["origin"] == "user"
        {
            break;
        }
    }

    let mut watch = watch(&comp, &contract);
    locker.unlock();
    assert_eq!(
        next(&mut watch, &contract),
        RenderEvent::pending_dropped(DropReason::OverriddenByUser)
    );
    let event = next(&mut watch, &contract);
    assert_eq!(event.kind, RenderKind::Changed);
    let render = event.render.unwrap();
    assert_eq!(render.device_id, "headless-0");
    assert_eq!(render.pending_device, None);
    assert_eq!(managed_text(&comp), None);
}

#[test]
fn a_failed_switch_at_unlock_counts_and_drops_nothing() {
    let (comp, mut locker, mut client, contract) = locked_with(&["gpu add headless-2 fail"]);
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "headless-2" }));
    assert_eq!(reply, json!({ "pending": true }));
    until(&mut client, &contract, |r| r.pending_device.is_some());

    let mut watch = watch(&comp, &contract);
    locker.unlock();
    let event = next(&mut watch, &contract);
    assert_eq!(
        event.kind,
        RenderKind::Changed,
        "no pending_dropped: {event:?}"
    );
    let render = event.render.unwrap();
    assert_eq!(render.device_id, "headless-0");
    assert_eq!(render.pending_device, None);
    let failed = render
        .devices
        .iter()
        .find(|d| d.id == "headless-2")
        .unwrap();
    assert_eq!(failed.switch_failures, 1);
    assert_eq!(managed_text(&comp), None);
}
