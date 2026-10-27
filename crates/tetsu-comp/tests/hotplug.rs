//! Output hotplug on the whole compositor, started headless with the stdin
//! hook: the bus follows, the Wayland client sees the `wl_output`s come and
//! go, and the workspaces move with them. Every reply is contract-checked.

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use std::time::{Duration, Instant};

use common::Client;
use common::comp::Comp;
use serde_json::{Value, json};
use wayland::WlClient;

const WATCH_OUTPUTS: &str = "org.tetsu.comp.output.v1.WatchOutputs";
const LIST_OUTPUTS: &str = "org.tetsu.comp.output.v1.ListOutputs";
const LIST_WORKSPACES: &str = "org.tetsu.comp.workspace.v1.ListWorkspaces";
const LIST_WINDOWS: &str = "org.tetsu.comp.window.v1.ListWindows";

/// Polls `check` until it holds (the bus follows at the publish tick).
fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn watch_outputs(comp: &Comp) -> Client {
    let contract = common::contract();
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_OUTPUTS, true);
    let initial = watch.event(&contract, WATCH_OUTPUTS);
    assert_eq!(initial["kind"], "initial");
    watch
}

fn names(list: &Value) -> Vec<String> {
    list["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn an_added_output_is_announced_on_the_bus_and_to_wayland_clients() {
    let mut comp = Comp::start_hotplug(
        &["--headless-outputs", "2"],
        Some("[[window_rule]]\napp_id = \"^side$\"\noutput = \"HEADLESS-3\"\n"),
    );
    let contract = common::contract();
    let mut watch = watch_outputs(&comp);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    assert_eq!(wl.output_names(), ["HEADLESS-1", "HEADLESS-2"]);

    comp.control("add 1280x720");
    let added = watch.event(&contract, WATCH_OUTPUTS);
    assert_eq!(added["kind"], "added");
    let output = &added["output"];
    assert_eq!(output["name"], "HEADLESS-3");
    assert_eq!(output["x"], 3840);
    assert_eq!(output["y"], 0);
    assert_eq!(output["scale"], 1.0);
    assert_eq!(output["transform"], "normal");
    assert_eq!(output["current_mode"], 0);
    assert_eq!(output["serial"], Value::Null);
    assert_eq!(output["enabled"], true);
    assert_eq!(
        output["modes"],
        json!([{ "width": 1280, "height": 720, "refresh_mhz": 60000, "preferred": true }])
    );

    wl.wait_for_outputs(&["HEADLESS-1", "HEADLESS-2", "HEADLESS-3"]);
    // A window the rules send to the new output is tiled to its size.
    let top = wl.map("side", "one");
    assert_eq!(wl.sizes(&top)[0], (1280, 720));

    let mut bus = Client::connect(&comp.bus);
    let list = bus.call(&contract, LIST_OUTPUTS);
    assert_eq!(names(&list), ["HEADLESS-1", "HEADLESS-2", "HEADLESS-3"]);
}

#[test]
fn a_removed_output_hands_its_windows_to_the_next_one() {
    let mut comp = Comp::start_hotplug(&["--headless-outputs", "2"], None);
    let contract = common::contract();
    let mut watch = watch_outputs(&comp);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let _top = wl.map("a", "one");
    let mut bus = Client::connect(&comp.bus);
    let window = eventually("the window", || {
        let listed = bus.call(&contract, LIST_WINDOWS);
        listed["windows"].as_array().unwrap().first().cloned()
    });
    assert_eq!(window["workspace_id"], "ws-1");

    comp.control("remove HEADLESS-1");
    let removed = watch.event(&contract, WATCH_OUTPUTS);
    assert_eq!(removed, json!({ "kind": "removed", "id": "out-1" }));
    let slid = watch.event(&contract, WATCH_OUTPUTS);
    assert_eq!(slid["kind"], "changed");
    assert_eq!(slid["output"]["id"], "out-2");
    assert_eq!(slid["output"]["x"], 0);

    wl.wait_for_outputs(&["HEADLESS-2"]);
    // The window is still there, its workspace now on the other output.
    let workspaces = eventually("the workspace moved", || {
        let list = bus.call(&contract, LIST_WORKSPACES);
        let moved = list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == "ws-1" && w["output_id"] == "out-2");
        moved.then_some(list)
    });
    assert_eq!(workspaces["active"].as_object().unwrap().len(), 1);
    assert_eq!(workspaces["focused_id"], "ws-2");
    let listed = bus.call(&contract, LIST_WINDOWS);
    assert_eq!(listed["windows"].as_array().unwrap().len(), 1);
    assert_eq!(listed["windows"][0]["workspace_id"], "ws-1");
}

#[test]
fn an_output_that_comes_back_gets_its_workspace_back() {
    let mut comp = Comp::start_hotplug(&["--headless-outputs", "2"], None);
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let _top = wl.map("a", "one");

    comp.control("remove HEADLESS-1");
    wl.wait_for_outputs(&["HEADLESS-2"]);
    comp.control("add-named HEADLESS-1 1920x1080");
    wl.wait_for_outputs(&["HEADLESS-1", "HEADLESS-2"]);

    let list = eventually("the workspace back home", || {
        let list = bus.call(&contract, LIST_WORKSPACES);
        let home = list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["id"] == "ws-1" && w["output_id"] == "out-3");
        home.then_some(list)
    });
    // No workspace was created for the new output; ws-1 is active there.
    assert_eq!(list["workspaces"].as_array().unwrap().len(), 2);
    assert_eq!(list["active"]["out-3"], "ws-1");
    assert_eq!(list["active"]["out-2"], "ws-2");
}

#[test]
fn losing_every_output_does_not_end_the_compositor() {
    let mut comp = Comp::start_hotplug(&["--headless-outputs", "2"], None);
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("a", "one");

    comp.control("remove HEADLESS-2");
    comp.control("remove HEADLESS-1");
    wl.wait_for_outputs(&[]);
    let list = eventually("every workspace orphaned", || {
        let list = bus.call(&contract, LIST_WORKSPACES);
        let orphaned = list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["output_id"].is_null());
        orphaned.then_some(list)
    });
    assert_eq!(list["focused_id"], Value::Null);
    assert_eq!(list["active"], json!({}));
    // The client's window survives and the bus keeps answering.
    wl.sync();
    assert_eq!(bus.call(&contract, LIST_OUTPUTS)["outputs"], json!([]));

    comp.control("add 1024x768");
    wl.wait_for_outputs(&["HEADLESS-3"]);
    let list = eventually("the workspaces adopted", || {
        let list = bus.call(&contract, LIST_WORKSPACES);
        let adopted = list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["output_id"] == "out-3");
        adopted.then_some(list)
    });
    assert_eq!(list["active"]["out-3"], "ws-1");
    assert_eq!(list["focused_id"], "ws-1");
    // The window is tiled again, to the new output.
    wl.wait_for_size(&top, (1024, 768));
}

#[test]
fn malformed_hotplug_lines_are_ignored() {
    let mut comp = Comp::start_hotplug(&[], None);
    let contract = common::contract();
    let mut watch = watch_outputs(&comp);
    for line in [
        "add 0x0",
        "remove NOPE",
        "bogus",
        "add-named",
        "add 99999x1",
        "",
    ] {
        comp.control(line);
    }
    comp.control("add 640x480");
    let added = watch.event(&contract, WATCH_OUTPUTS);
    assert_eq!(added["kind"], "added", "nothing but the valid line acted");
    assert_eq!(added["output"]["name"], "HEADLESS-2");
    assert_eq!(added["output"]["x"], 1920);
}
