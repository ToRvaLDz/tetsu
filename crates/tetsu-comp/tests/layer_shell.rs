//! `zwlr_layer_shell_v1` on the whole compositor, started headless: a client
//! that behaves like tetsu-bar gets a bar on every output, its exclusive
//! zone shrinks the tiles, and layers follow hotplug.

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use common::comp::Comp;
use wayland::WlClient;

const OUTPUT_2_RULE: &str = "[[window_rule]]\napp_id = \"^second$\"\noutput = \"HEADLESS-2\"\n";

#[test]
fn headless_advertises_the_layer_shell() {
    let comp = Comp::start();
    let globals = wayland::globals(&comp.wayland_socket());
    assert!(
        globals.iter().any(|g| g == "zwlr_layer_shell_v1"),
        "{globals:?}"
    );
}

#[test]
fn a_bar_on_every_output_is_configured_and_gets_frame_callbacks() {
    let comp = Comp::start_with(&["--headless-outputs", "2"], None);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    assert!(wl.has_layer_shell());
    wl.bar_on_every_output();
    wl.wait_for_layers(2);
    assert_eq!(
        wl.layer_sizes(),
        [
            (Some("HEADLESS-1".to_owned()), (1920, 28)),
            (Some("HEADLESS-2".to_owned()), (1920, 28)),
        ]
    );
    wl.wait_for_frame_on("HEADLESS-1");
    wl.wait_for_frame_on("HEADLESS-2");
}

#[test]
fn a_toplevel_mapped_after_the_bar_is_tiled_below_it() {
    let comp = Comp::start();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    wl.bar_on_every_output();
    wl.wait_for_layers(1);
    let top = wl.map("a", "one");
    assert_eq!(wl.sizes(&top)[0], (1920, 1052));
}

#[test]
fn a_bar_on_one_output_leaves_the_other_alone() {
    let comp = Comp::start_with(&["--headless-outputs", "2"], Some(OUTPUT_2_RULE));
    let mut wl = WlClient::connect(&comp.wayland_socket());
    wl.bar_on("HEADLESS-1");
    wl.wait_for_layers(1);
    let first = wl.map("first", "one");
    assert_eq!(wl.sizes(&first)[0], (1920, 1052));
    let second = wl.map("second", "two");
    assert_eq!(wl.sizes(&second)[0], (1920, 1080));
}

#[test]
fn a_toplevel_already_tiled_shrinks_when_the_bar_appears_and_grows_when_it_goes() {
    let comp = Comp::start();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("a", "one");
    assert_eq!(wl.sizes(&top)[0], (1920, 1080));
    wl.bar_on_every_output();
    wl.wait_for_layers(1);
    wl.wait_for_size(&top, (1920, 1052));
    wl.destroy_bars();
    wl.wait_for_size(&top, (1920, 1080));
}

#[test]
fn a_hotplugged_output_gets_a_bar_and_a_removed_one_closes_its_bar() {
    let mut comp = Comp::start_hotplug(&["--headless-outputs", "1"], None);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    wl.bar_on_every_output();
    wl.wait_for_layers(1);
    wl.wait_for_frame_on("HEADLESS-1");

    comp.control("add");
    wl.wait_for_layers(2);
    wl.wait_for_frame_on("HEADLESS-2");
    assert_eq!(wl.closed_count(), 0);

    comp.control("remove HEADLESS-2");
    wl.wait_for_closed(1);
    wl.wait_for_outputs(&["HEADLESS-1"]);
    // The bar of the output that stayed is not touched.
    assert_eq!(wl.closed_count(), 1);
}

#[test]
fn an_exclusive_overlay_takes_the_keyboard_and_gives_it_back() {
    let comp = Comp::start();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("a", "one");
    wl.wait_for_keyboard_events(&top, 1);
    assert_eq!(wl.keyboard_events(&top), [true]);

    let overlay = wl.overlay_exclusive();
    wl.wait_for_keyboard_events(&top, 2);
    assert_eq!(wl.keyboard_events(&top), [true, false]);

    wl.destroy_overlay(overlay);
    wl.wait_for_keyboard_events(&top, 3);
    assert_eq!(wl.keyboard_events(&top), [true, false, true]);
}
