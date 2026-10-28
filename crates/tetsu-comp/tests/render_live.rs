//! `org.tetsu.comp.render.v1` on the whole compositor, started headless:
//! the synthetic device, fake devices from the hotplug stream, hot
//! switches written to the managed file, and the errors. Every reply,
//! event and error goes through the contract checks.

mod common;

use std::time::{Duration, Instant};

use common::Client;
use common::comp::{Comp, Layers};
use serde_json::{Value, json};
use tetsu_bus::render::{GetRenderReply, Render, RenderEvent, RenderKind, SyncMode};
use tetsu_bus_contract::Contract;

const GET_RENDER: &str = "org.tetsu.comp.render.v1.GetRender";
const WATCH_RENDER: &str = "org.tetsu.comp.render.v1.WatchRender";
const SET_DEVICE: &str = "org.tetsu.comp.render.v1.SetRenderDevice";
const SET_SYNC: &str = "org.tetsu.comp.render.v1.SetExplicitSync";
const GET_CONFIG: &str = "org.tetsu.comp.config.v1.GetConfig";
const LIST_OUTPUTS: &str = "org.tetsu.comp.output.v1.ListOutputs";

const TIMEOUT: Duration = Duration::from_secs(10);

fn get_render(client: &mut Client, contract: &Contract) -> Render {
    let reply: GetRenderReply = serde_json::from_value(client.call(contract, GET_RENDER)).unwrap();
    reply.render
}

/// Asks until `done` holds; returns the state that matched.
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

/// The next `changed` the watch gets that satisfies `done`.
fn changed(watch: &mut Client, contract: &Contract, done: impl Fn(&Render) -> bool) -> Render {
    loop {
        let event: RenderEvent =
            serde_json::from_value(watch.event(contract, WATCH_RENDER)).unwrap();
        common::check_render_event(contract, &event);
        assert_eq!(event.kind, RenderKind::Changed, "{event:?}");
        let render = event.render.expect("a changed carries the state");
        if done(&render) {
            return render;
        }
    }
}

fn watch(comp: &Comp, contract: &Contract) -> Client {
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_RENDER, true);
    let initial = watch.event(contract, WATCH_RENDER);
    assert_eq!(initial["kind"], "initial");
    watch
}

fn output_id(client: &mut Client, contract: &Contract, name: &str) -> String {
    let list = client.call(contract, LIST_OUTPUTS);
    list["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == name)
        .unwrap_or_else(|| panic!("no output {name} in {list}"))["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn managed(comp: &Comp) -> Option<toml::Table> {
    let text = std::fs::read_to_string(comp.managed_file()).ok()?;
    Some(toml::from_str(&text).unwrap())
}

fn managed_render(comp: &Comp, key: &str) -> Option<String> {
    managed(comp)?
        .get("render")?
        .get(key)?
        .as_str()
        .map(str::to_owned)
}

fn config_entry(client: &mut Client, contract: &Contract, key: &str) -> Value {
    let config = client.call(contract, GET_CONFIG);
    config["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == key)
        .unwrap_or_else(|| panic!("no entry {key}"))
        .clone()
}

fn failures(render: &Render, id: &str) -> i64 {
    render
        .devices
        .iter()
        .find(|d| d.id == id)
        .unwrap_or_else(|| panic!("no device {id}"))
        .switch_failures
}

#[test]
fn headless_reports_its_synthetic_device() {
    let comp = Comp::start_hotplug(&[], None);
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let out = output_id(&mut client, &contract, "HEADLESS-1");
    let render = get_render(&mut client, &contract);
    assert_eq!(render.device_id, "headless-0");
    assert!(render.automatic);
    assert_eq!(render.devices.len(), 1);
    let device = &render.devices[0];
    assert_eq!(device.id, "headless-0");
    assert_eq!(device.node, "/dev/dri/headless-0");
    assert_eq!(device.driver, "headless");
    assert_eq!(device.output_ids, [out]);
    assert_eq!(device.switch_failures, 0);
    let sync = &render.explicit_sync;
    assert_eq!(sync.mode, SyncMode::Auto);
    assert!(!sync.supported);
    assert!(!sync.enabled);
    assert_eq!(sync.reason.as_deref(), Some("no syncobj_eventfd"));
    assert_eq!(render.pending_device, None);
    assert_eq!(render.pending_explicit_sync, None);
}

#[test]
fn a_switch_is_hot_written_and_reported() {
    let mut comp = Comp::start_hotplug(&[], None);
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let mut watch = watch(&comp, &contract);

    // (b) A fake device comes.
    comp.control("gpu add headless-1");
    changed(&mut watch, &contract, |r| r.devices.len() == 2);

    // (c) A switch to it is hot and persisted.
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "headless-1" }));
    assert_eq!(reply, json!({ "pending": false }));
    let now = changed(&mut watch, &contract, |r| r.device_id == "headless-1");
    assert!(!now.automatic);
    assert_eq!(
        managed_render(&comp, "device").as_deref(),
        Some("/dev/dri/headless-1")
    );
    let entry = config_entry(&mut client, &contract, "render.device");
    assert_eq!(entry["origin"], "managed");
    assert_eq!(entry["value"], "/dev/dri/headless-1");

    // (d) An unknown id.
    let (name, params) = client.call_error(&contract, SET_DEVICE, &json!({ "device_id": "nope" }));
    assert_eq!(name, "org.tetsu.comp.render.v1.DeviceNotFound");
    assert_eq!(params, json!({ "id": "nope" }));

    // (e) A device whose switch fails: nothing moves, nothing is written.
    comp.control("gpu add headless-2 fail");
    changed(&mut watch, &contract, |r| r.devices.len() == 3);
    let before = std::fs::read_to_string(comp.managed_file()).unwrap();
    let (name, params) =
        client.call_error(&contract, SET_DEVICE, &json!({ "device_id": "headless-2" }));
    assert_eq!(name, "org.tetsu.comp.render.v1.SwitchFailed");
    assert!(params["reason"].as_str().is_some());
    let now = changed(&mut watch, &contract, |r| failures(r, "headless-2") == 1);
    assert_eq!(now.device_id, "headless-1");
    assert_eq!(get_render(&mut client, &contract).device_id, "headless-1");
    assert_eq!(
        std::fs::read_to_string(comp.managed_file()).unwrap(),
        before
    );

    // (h) Null goes back to the automatic choice.
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": null }));
    assert_eq!(reply, json!({ "pending": false }));
    let now = changed(&mut watch, &contract, |r| r.device_id == "headless-0");
    assert!(now.automatic);
    assert_eq!(managed_render(&comp, "device").as_deref(), Some("auto"));
}

#[test]
fn explicit_sync_on_is_refused_and_off_is_written() {
    let comp = Comp::start_hotplug(&[], None);
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let (name, params) = client.call_error(&contract, SET_SYNC, &json!({ "mode": "on" }));
    assert_eq!(name, "org.tetsu.comp.render.v1.UnsupportedByHardware");
    assert_eq!(params, json!({ "reason": "no syncobj_eventfd" }));
    assert_eq!(managed(&comp), None);

    let reply = client.call_with(&contract, SET_SYNC, &json!({ "mode": "off" }));
    assert_eq!(reply, json!({ "pending": false }));
    let render = until(&mut client, &contract, |r| {
        r.explicit_sync.mode == SyncMode::Off
    });
    assert!(!render.explicit_sync.enabled);
    assert_eq!(
        render.explicit_sync.reason.as_deref(),
        Some("off in config")
    );
    assert_eq!(
        managed_render(&comp, "explicit_sync").as_deref(),
        Some("off")
    );
}

#[test]
fn a_key_the_user_file_sets_is_overridden_by_user() {
    let comp = Comp::start_hotplug_layers(
        &[],
        Layers {
            user: Some("[render]\ndevice = \"auto\"\nexplicit_sync = \"auto\"\n"),
            ..Layers::default()
        },
    );
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let (name, params) = client.call_error(&contract, SET_DEVICE, &json!({ "device_id": null }));
    assert_eq!(name, "org.tetsu.comp.render.v1.OverriddenByUser");
    assert_eq!(params, json!({ "keys": ["render.device"] }));
    let (name, params) = client.call_error(&contract, SET_SYNC, &json!({ "mode": "off" }));
    assert_eq!(name, "org.tetsu.comp.render.v1.OverriddenByUser");
    assert_eq!(params, json!({ "keys": ["render.explicit_sync"] }));
    assert_eq!(managed(&comp), None);
}

#[test]
fn a_new_output_joins_the_synthetic_device() {
    let mut comp = Comp::start_hotplug(&[], None);
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    comp.control("add");
    let start = Instant::now();
    let added = loop {
        let list = client.call(&contract, LIST_OUTPUTS);
        if list["outputs"].as_array().unwrap().len() == 2 {
            break output_id(&mut client, &contract, "HEADLESS-2");
        }
        assert!(start.elapsed() < TIMEOUT, "the output never came");
        std::thread::sleep(Duration::from_millis(20));
    };
    let render = until(&mut client, &contract, |r| {
        r.devices[0].output_ids.contains(&added)
    });
    assert_eq!(render.devices[0].output_ids.len(), 2);
}
