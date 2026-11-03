//! The whole compositor, started headless with config layers: the three
//! files load, a change of the user file applies live, and `SetManaged`
//! writes the managed file. Every reply goes through the contract checks.

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use std::fs;

use common::Client;
use common::comp::{Comp, Layers};
use serde_json::{Value, json};
use wayland::WlClient;

const GET_CONFIG: &str = "org.tetsu.comp.config.v1.GetConfig";
const WATCH_CONFIG: &str = "org.tetsu.comp.config.v1.WatchConfig";
const SET_MANAGED: &str = "org.tetsu.comp.config.v1.SetManaged";

const FULL: (u32, u32) = (1920, 1080);
const GAPPED: (u32, u32) = (1900, 1060);

fn entry(config: &Value, key: &str) -> Value {
    config["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == key)
        .unwrap_or_else(|| panic!("no entry {key}"))
        .clone()
}

#[test]
fn startup_loads_the_three_layers_with_their_origins() {
    let comp = Comp::start_layers(
        &[],
        Layers {
            defaults: Some("[layout]\ngaps_inner = 1\ngaps_outer = 9\n"),
            managed: Some("[layout]\ngaps_inner = 2\ngaps_outer = 3\n"),
            user: Some("[layout]\ngaps_inner = 4\n"),
        },
    );
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let config = client.call(&contract, GET_CONFIG);
    assert_eq!(config["diagnostics"], json!([]));

    let inner = entry(&config, "layout.gaps_inner");
    assert_eq!(inner["origin"], "user");
    assert_eq!(inner["value"], 4);
    assert_eq!(inner["default_value"], 1);
    assert_eq!(inner["managed_value"], 2);
    assert_eq!(inner["user_value"], 4);
    let outer = entry(&config, "layout.gaps_outer");
    assert_eq!(outer["origin"], "managed");
    assert_eq!(outer["value"], 3);
    assert_eq!(outer["default_value"], 9);
    let ratio = entry(&config, "layout.master_ratio");
    assert_eq!(ratio["origin"], "default");
    let device = entry(&config, "render.device");
    assert_eq!(device["writable"], false);
    assert_eq!(device["method"], "org.tetsu.comp.render.v1.SetRenderDevice");
}

#[test]
fn editing_the_user_file_applies_live_and_is_published() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_CONFIG, true);
    assert_eq!(watch.event(&contract, WATCH_CONFIG)["kind"], "initial");

    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&top, FULL);

    fs::create_dir_all(comp.user_file().parent().unwrap()).unwrap();
    fs::write(comp.user_file(), "[layout]\ngaps_outer = 10\n").unwrap();

    let changed = watch.event(&contract, WATCH_CONFIG);
    assert_eq!(changed["kind"], "changed");
    assert_eq!(changed["entry"]["key"], "layout.gaps_outer");
    assert_eq!(changed["entry"]["origin"], "user");
    assert_eq!(changed["entry"]["value"], 10);
    let diagnostics = watch.event(&contract, WATCH_CONFIG);
    assert_eq!(
        diagnostics,
        json!({ "kind": "diagnostics", "diagnostics": [] })
    );
    wl.wait_for_size(&top, GAPPED);

    // A bad line: the session keeps its values and publishes the problem.
    fs::write(comp.user_file(), "[layout]\ngaps_outer = 10\nbogus = 1\n").unwrap();
    let diagnostics = watch.event(&contract, WATCH_CONFIG);
    assert_eq!(diagnostics["kind"], "diagnostics");
    assert_eq!(diagnostics["diagnostics"][0]["line"], 3);
    assert_eq!(diagnostics["diagnostics"][0]["key"], "layout.bogus");
    let mut client = Client::connect(&comp.bus);
    let config = client.call(&contract, GET_CONFIG);
    assert_eq!(entry(&config, "layout.gaps_outer")["value"], 10);
}

#[test]
fn a_user_file_with_a_bad_value_still_starts_and_names_line_and_key() {
    let comp = Comp::start_layers(
        &[],
        Layers {
            user: Some("[layout]\ngaps_outer = 4\nmaster_ratio = 5.0\n"),
            ..Layers::default()
        },
    );
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let config = client.call(&contract, GET_CONFIG);
    let found = &config["diagnostics"];
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    assert_eq!(found[0]["line"], 3);
    assert_eq!(found[0]["key"], "layout.master_ratio");
    assert_eq!(entry(&config, "layout.gaps_outer")["origin"], "user");
    assert_eq!(entry(&config, "layout.master_ratio")["origin"], "default");
}

#[test]
fn set_managed_applies_persists_and_a_remove_brings_the_default_back() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&top, FULL);

    let reply = client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.gaps_outer": 10 } }),
    );
    assert_eq!(reply, json!({}));
    wl.wait_for_size(&top, GAPPED);
    let saved = fs::read_to_string(comp.managed_file()).unwrap();
    assert!(saved.contains("gaps_outer = 10"), "{saved}");
    let config = client.call(&contract, GET_CONFIG);
    let outer = entry(&config, "layout.gaps_outer");
    assert_eq!(outer["origin"], "managed");
    assert_eq!(outer["managed_value"], 10);

    client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": {}, "remove": ["layout.gaps_outer"] }),
    );
    wl.wait_for_size(&top, FULL);
    let saved = fs::read_to_string(comp.managed_file()).unwrap();
    assert!(!saved.contains("gaps_outer"), "{saved}");
    let config = client.call(&contract, GET_CONFIG);
    assert_eq!(entry(&config, "layout.gaps_outer")["origin"], "default");
}

#[test]
fn set_managed_on_a_key_the_user_sets_is_refused_and_writes_nothing() {
    let comp = Comp::start_layers(
        &[],
        Layers {
            user: Some("[layout]\ngaps_outer = 4\n"),
            ..Layers::default()
        },
    );
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let (name, params) = client.call_error(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.gaps_outer": 10, "layout.gaps_inner": 2 } }),
    );
    assert_eq!(name, "org.tetsu.comp.config.v1.OverriddenByUser");
    assert_eq!(params, json!({ "keys": ["layout.gaps_outer"] }));
    assert!(!comp.managed_file().exists());
}

#[test]
fn set_managed_publishes_changed_to_the_watchers() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_CONFIG, true);
    watch.event(&contract, WATCH_CONFIG);
    let mut client = Client::connect(&comp.bus);
    client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.master_ratio": 0.7 } }),
    );
    let changed = watch.event(&contract, WATCH_CONFIG);
    assert_eq!(changed["kind"], "changed");
    assert_eq!(changed["entry"]["key"], "layout.master_ratio");
    assert_eq!(changed["entry"]["origin"], "managed");
    assert_eq!(changed["entry"]["value"], 0.7);
}
