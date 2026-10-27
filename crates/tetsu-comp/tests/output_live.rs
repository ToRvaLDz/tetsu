//! `ConfigureOutputs` against the whole compositor, started headless: real
//! files, a real Wayland client, the bus over its socket. Every reply,
//! error and event goes through the contract checks. Nothing here touches
//! the developer's own config or state (the `Comp` isolation).

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use std::fs;
use std::io::Read;
use std::time::{Duration, Instant};

use common::Client;
use common::comp::{Comp, Layers};
use serde_json::{Value, json};
use wayland::WlClient;

const CONFIGURE: &str = "org.tetsu.comp.output.v1.ConfigureOutputs";
const LIST_OUTPUTS: &str = "org.tetsu.comp.output.v1.ListOutputs";
const WATCH_OUTPUTS: &str = "org.tetsu.comp.output.v1.WatchOutputs";
const WATCH_WORKSPACES: &str = "org.tetsu.comp.workspace.v1.WatchWorkspaces";
const GET_CONFIG: &str = "org.tetsu.comp.config.v1.GetConfig";
const WATCH_CONFIG: &str = "org.tetsu.comp.config.v1.WatchConfig";
const METHOD: &str = "org.tetsu.comp.output.v1.ConfigureOutputs";

const TWO: [&str; 2] = ["--headless-outputs", "2"];
const RULE_ON_2: &str = "[[window_rule]]\napp_id = \"^org.tetsu.test$\"\noutput = \"HEADLESS-2\"\n";

fn watch(comp: &Comp, method: &str) -> Client {
    let mut watch = Client::connect(&comp.bus);
    watch.send(method, true);
    let initial = watch.event(&common::contract(), method);
    assert_eq!(initial["kind"], "initial");
    watch
}

/// Reads events of `method` until one satisfies `want`.
fn until(watch: &mut Client, method: &str, want: impl Fn(&Value) -> bool) -> Value {
    let contract = common::contract();
    loop {
        let event = watch.event(&contract, method);
        if want(&event) {
            return event;
        }
    }
}

/// Whether nothing arrives on the stream for 500 ms.
fn quiet(watch: &mut Client) -> bool {
    watch
        .stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let mut byte = [0u8; 1];
    let silent = watch.buffer.is_empty()
        && matches!(
            watch.stream.read(&mut byte),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)
        );
    watch
        .stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    silent
}

fn list(client: &mut Client) -> Value {
    client.call(&common::contract(), LIST_OUTPUTS)["outputs"].clone()
}

fn find(outputs: &Value, name: &str) -> Value {
    outputs
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["name"] == name)
        .unwrap_or_else(|| panic!("no output {name} in {outputs}"))
        .clone()
}

fn configure(client: &mut Client, configs: &Value) -> Value {
    client.call_with(
        &common::contract(),
        CONFIGURE,
        &json!({ "configs": configs }),
    )
}

fn refused(client: &mut Client, configs: &Value) -> (String, Value) {
    client.call_error(
        &common::contract(),
        CONFIGURE,
        &json!({ "configs": configs }),
    )
}

fn entry(config: &Value, key: &str) -> Value {
    config["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == key)
        .unwrap_or_else(|| panic!("no entry {key}"))
        .clone()
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn managed_text(comp: &Comp) -> Option<String> {
    fs::read_to_string(comp.managed_file()).ok()
}

#[test]
fn a_valid_call_applies_persists_and_is_published() {
    let comp = Comp::start_with(&TWO, Some(RULE_ON_2));
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);
    let mut outputs = watch(&comp, WATCH_OUTPUTS);
    let mut config = watch(&comp, WATCH_CONFIG);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&top, (1920, 1080));

    let reply = configure(
        &mut client,
        &json!({ "out-2": { "scale": 2.0, "x": 0, "y": 1080 }, "out-1": { "x": 0, "y": 0 } }),
    );
    assert_eq!(reply, json!({}));

    let event = until(&mut outputs, WATCH_OUTPUTS, |e| {
        e["kind"] == "changed" && e["output"]["id"] == "out-2" && e["output"]["scale"] == 2.0
    });
    assert_eq!(event["output"]["x"], 0);
    assert_eq!(event["output"]["y"], 1080);
    wl.wait_for_size(&top, (960, 540));

    let text = managed_text(&comp).expect("the managed file is written");
    assert!(text.contains("[output.HEADLESS-1]"), "{text}");
    assert!(text.contains("[output.HEADLESS-2]"), "{text}");
    assert!(text.contains("scale = 2.0"), "{text}");

    let now = client.call(&contract, GET_CONFIG);
    let scale = entry(&now, "output.HEADLESS-2.scale");
    assert_eq!(scale["origin"], "managed");
    assert_eq!(scale["writable"], false);
    assert_eq!(scale["method"], METHOD);
    let changed = until(&mut config, WATCH_CONFIG, |e| {
        e["kind"] == "changed" && e["entry"]["key"] == "output.HEADLESS-2.scale"
    });
    assert_eq!(changed["entry"]["value"], 2.0);
}

#[test]
fn an_invalid_call_changes_nothing() {
    let comp = Comp::start_with(&TWO, None);
    let mut client = Client::connect(&comp.bus);
    let before = list(&mut client);
    let mut outputs = watch(&comp, WATCH_OUTPUTS);
    let bad_mode = json!({ "width": 1024, "height": 768, "refresh_mhz": 60000 });
    let cases = [
        (
            json!({ "out-1": { "x": 0, "y": 0 }, "out-2": { "x": 100, "y": 100 } }),
            "org.tetsu.comp.output.v1.InvalidConfiguration",
        ),
        (
            json!({ "out-9": { "scale": 2.0 } }),
            "org.tetsu.comp.output.v1.OutputNotFound",
        ),
        (
            json!({ "out-1": { "scale": 0 } }),
            "org.tetsu.comp.output.v1.InvalidConfiguration",
        ),
        (
            json!({ "out-1": { "mode": bad_mode } }),
            "org.tetsu.comp.output.v1.InvalidConfiguration",
        ),
        (
            json!({ "out-1": { "enabled": false }, "out-2": { "enabled": false } }),
            "org.tetsu.comp.output.v1.InvalidConfiguration",
        ),
    ];
    for (configs, expected) in cases {
        let (name, _) = refused(&mut client, &configs);
        assert_eq!(name, expected, "{configs}");
        assert_eq!(managed_text(&comp), None, "{configs}");
        assert_eq!(list(&mut client), before, "{configs}");
        assert!(quiet(&mut outputs), "an event arrived for {configs}");
    }
}

#[test]
fn the_user_file_wins_field_by_field() {
    let comp = Comp::start_layers(
        &TWO,
        Layers {
            user: Some("[output.HEADLESS-2]\nscale = 1.5\n"),
            ..Layers::default()
        },
    );
    let mut client = Client::connect(&comp.bus);
    let before = list(&mut client);
    assert_eq!(find(&before, "HEADLESS-2")["scale"], 1.5);

    let (name, params) = refused(&mut client, &json!({ "out-2": { "scale": 2.0 } }));
    assert_eq!(name, "org.tetsu.comp.output.v1.OverriddenByUser");
    assert_eq!(params, json!({ "keys": ["output.HEADLESS-2.scale"] }));
    assert_eq!(managed_text(&comp), None);
    assert_eq!(list(&mut client), before);

    let reply = configure(&mut client, &json!({ "out-2": { "x": 0, "y": 1080 } }));
    assert_eq!(reply, json!({}));
    eventually("out-2 moved", || {
        let now = find(&list(&mut client), "HEADLESS-2");
        now["y"] == 1080 && now["scale"] == 1.5
    });
}

#[test]
fn disabling_an_output_unplugs_it_and_enabling_brings_it_back() {
    let comp = Comp::start_with(&TWO, None);
    let mut client = Client::connect(&comp.bus);
    let mut outputs = watch(&comp, WATCH_OUTPUTS);
    let mut workspaces = watch(&comp, WATCH_WORKSPACES);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    wl.wait_for_outputs(&["HEADLESS-1", "HEADLESS-2"]);

    configure(&mut client, &json!({ "out-1": { "enabled": false } }));
    let event = until(&mut outputs, WATCH_OUTPUTS, |e| {
        e["kind"] == "changed" && e["output"]["id"] == "out-1" && e["output"]["enabled"] == false
    });
    assert_eq!(event["output"]["current_mode"], Value::Null);
    until(&mut workspaces, WATCH_WORKSPACES, |e| {
        e["kind"] == "activated" && e["output_id"] == "out-1" && e["id"].is_null()
    });
    wl.wait_for_outputs(&["HEADLESS-2"]);
    let listed = list(&mut client);
    assert_eq!(find(&listed, "HEADLESS-1")["enabled"], false);

    configure(&mut client, &json!({ "out-1": { "enabled": true } }));
    let event = until(&mut outputs, WATCH_OUTPUTS, |e| {
        e["kind"] == "changed" && e["output"]["id"] == "out-1" && e["output"]["enabled"] == true
    });
    assert_eq!(event["output"]["current_mode"], 0);
    until(&mut workspaces, WATCH_WORKSPACES, |e| {
        e["kind"] == "activated" && e["output_id"] == "out-1" && !e["id"].is_null()
    });
    wl.wait_for_outputs(&["HEADLESS-1", "HEADLESS-2"]);
}

const PERSISTED: &str = "[output.HEADLESS-2]\nscale = 2.0\nposition = { x = 0, y = 1080 }\n";

#[test]
fn managed_values_apply_at_start_and_again_to_a_replugged_output() {
    let mut comp = Comp::start_hotplug_layers(
        &TWO,
        Layers {
            managed: Some(PERSISTED),
            ..Layers::default()
        },
    );
    let mut client = Client::connect(&comp.bus);
    let at_start = find(&list(&mut client), "HEADLESS-2");
    assert_eq!(at_start["scale"], 2.0);
    assert_eq!(
        (at_start["x"].clone(), at_start["y"].clone()),
        (json!(0), json!(1080))
    );

    comp.control("remove HEADLESS-2");
    eventually("HEADLESS-2 gone", || {
        list(&mut client).as_array().unwrap().len() == 1
    });
    comp.control("add-named HEADLESS-2 1920x1080");
    eventually("HEADLESS-2 is back with its config", || {
        let outputs = list(&mut client);
        outputs.as_array().unwrap().iter().any(|o| {
            o["name"] == "HEADLESS-2" && o["id"] == "out-3" && o["scale"] == 2.0 && o["y"] == 1080
        })
    });
}

#[test]
fn a_mode_change_resizes_the_tile() {
    let mut comp = Comp::start_hotplug(
        &[],
        Some("[[window_rule]]\napp_id = \"^org.tetsu.test$\"\noutput = \"HEADLESS-9\"\n"),
    );
    let mut client = Client::connect(&comp.bus);
    comp.control("add-modes HEADLESS-9 1920x1080,1280x720");
    eventually("HEADLESS-9", || {
        list(&mut client).as_array().unwrap().len() == 2
    });
    let id = find(&list(&mut client), "HEADLESS-9")["id"].clone();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&top, (1920, 1080));

    let mode = json!({ "width": 1280, "height": 720, "refresh_mhz": 60000 });
    let reply = configure(
        &mut client,
        &json!({ id.as_str().unwrap(): { "mode": mode } }),
    );
    assert_eq!(reply, json!({}));
    eventually("mode 1 current", || {
        find(&list(&mut client), "HEADLESS-9")["current_mode"] == 1
    });
    wl.wait_for_size(&top, (1280, 720));
}

#[test]
fn deleting_a_user_table_removes_its_keys_and_the_output_goes_back_to_automatic() {
    let comp = Comp::start_layers(
        &TWO,
        Layers {
            user: Some("[output.HEADLESS-2]\nscale = 1.5\n"),
            ..Layers::default()
        },
    );
    let mut client = Client::connect(&comp.bus);
    let mut config = watch(&comp, WATCH_CONFIG);
    assert_eq!(find(&list(&mut client), "HEADLESS-2")["scale"], 1.5);

    fs::write(comp.user_file(), "").unwrap();
    let removed = until(&mut config, WATCH_CONFIG, |e| e["kind"] == "removed");
    assert_eq!(removed["key"], "output.HEADLESS-2.scale");
    eventually("scale 1 at the automatic position", || {
        let now = find(&list(&mut client), "HEADLESS-2");
        now["scale"] == 1.0 && now["x"] == 1920 && now["y"] == 0
    });
}
