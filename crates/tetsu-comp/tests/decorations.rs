//! Server-side decorations on the whole compositor, headless: the tile is
//! inset by the border, the border follows the config live, and a real
//! client is told the decoration mode is server-side.

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
fn a_tile_is_inset_by_the_configured_border() {
    let comp = Comp::start_with(
        &[],
        Some(
            "[decorations]\nwidth = 5\n\n[[window_rule]]\napp_id = \"^floaty$\"\nfloating = true\nsize = [640, 480]\n",
        ),
    );
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let one = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&one, (1910, 1070));

    let two = wl.map("org.tetsu.test", "two");
    wl.wait_for_size(&one, (950, 1070));
    wl.wait_for_size(&two, (950, 1070));

    let floating = wl.map_floating("floaty", "float", (640, 480));
    wl.wait_for_size(&floating, (640, 480));
}

#[test]
fn the_builtin_border_is_two_pixels() {
    let comp = Comp::start_layers(
        &[],
        Layers {
            defaults: Some(""),
            ..Layers::default()
        },
    );
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&top, (1916, 1076));
}

#[test]
fn decorations_reload_live_and_bad_values_are_diagnosed() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_CONFIG, true);
    assert_eq!(watch.event(&contract, WATCH_CONFIG)["kind"], "initial");

    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    wl.wait_for_size(&top, (1920, 1080));

    fs::create_dir_all(comp.user_file().parent().unwrap()).unwrap();
    fs::write(
        comp.user_file(),
        "[decorations]\nwidth = 4\nactive_color = \"#ff0000\"\n",
    )
    .unwrap();
    let mut changed = Vec::new();
    while changed.len() < 2 {
        let event = watch.event(&contract, WATCH_CONFIG);
        if event["kind"] == "changed" {
            changed.push(event["entry"]["key"].as_str().unwrap().to_owned());
        }
    }
    changed.sort();
    assert_eq!(changed, ["decorations.active_color", "decorations.width"]);
    wl.wait_for_size(&top, (1912, 1072));

    // A bad width is dropped with its line: the value falls back a layer.
    fs::write(
        comp.user_file(),
        "[decorations]\nactive_color = \"#ff0000\"\nwidth = -3\n",
    )
    .unwrap();
    let diagnostics = loop {
        let event = watch.event(&contract, WATCH_CONFIG);
        if event["kind"] == "diagnostics" && event["diagnostics"] != json!([]) {
            break event;
        }
    };
    assert_eq!(diagnostics["diagnostics"][0]["line"], 3);
    assert_eq!(diagnostics["diagnostics"][0]["key"], "decorations.width");
    let mut client = Client::connect(&comp.bus);
    let config = client.call(&contract, GET_CONFIG);
    assert_eq!(entry(&config, "decorations.width")["origin"], "default");
    assert_eq!(
        entry(&config, "decorations.active_color")["value"],
        "#ff0000"
    );
    wl.wait_for_size(&top, (1920, 1080));

    let reply = client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "decorations.inactive_color": "#101010" } }),
    );
    assert_eq!(reply, json!({}));
    let (name, params) = client.call_error(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "decorations.width": 99 } }),
    );
    assert_eq!(name, "org.tetsu.comp.config.v1.InvalidValue");
    assert_eq!(params["key"], "decorations.width");
}
