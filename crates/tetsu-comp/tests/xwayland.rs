//! X11 clients, end to end and headless: what satellite needs from the
//! compositor, the X11 display and `DISPLAY`, and real X11 windows through
//! xwayland-satellite and Xwayland (skipped with a note when either is not
//! on `PATH`; `TETSU_REQUIRE_XWAYLAND=1` makes that a failure).

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use common::comp::Comp;

#[test]
fn satellite_finds_the_viewporter_and_no_xwayland_shell() {
    let comp = Comp::start();
    let globals = wayland::globals(&comp.wayland_socket());
    assert!(globals.iter().any(|g| g == "wp_viewporter"), "{globals:?}");
    assert!(
        !globals.iter().any(|g| g == "xwayland_shell_v1"),
        "{globals:?}"
    );
}
