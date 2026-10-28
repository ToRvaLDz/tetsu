//! The X11 side without a compositor: the satellite policy (who is
//! Xwayland, which argv) and the X11 display's lock and sockets.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::process::Command;

use tetsu_comp::trust::Trust;
use tetsu_comp::xwayland::satellite::{
    peer_trust, probe_command, satellite_command, window_origin,
};

fn args(command: &Command) -> Vec<&OsStr> {
    command.get_args().collect()
}

fn envs(command: &Command) -> HashMap<OsString, Option<OsString>> {
    command
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(OsStr::to_owned)))
        .collect()
}

#[test]
fn only_the_live_satellite_is_xwayland() {
    assert_eq!(peer_trust(Some(41), Some(41)), Trust::Xwayland);
    assert_eq!(peer_trust(Some(42), Some(41)), Trust::Untrusted);
    assert_eq!(peer_trust(None, Some(41)), Trust::Untrusted);
    assert_eq!(peer_trust(Some(41), None), Trust::Untrusted);
    assert_eq!(peer_trust(None, None), Trust::Untrusted);
}

#[test]
fn an_xwayland_window_hides_the_satellite_pid() {
    assert_eq!(window_origin(Trust::Xwayland, Some(42)), (None, true));
    for trust in [Trust::Untrusted, Trust::Locker, Trust::Input] {
        assert_eq!(window_origin(trust, Some(42)), (Some(42), false));
        assert_eq!(window_origin(trust, None), (None, false));
    }
}

#[test]
fn satellite_gets_the_display_and_both_listening_fds() {
    let command = satellite_command("/x/sat", ":3", &[7, 8], OsStr::new("wayland-9"));
    assert_eq!(command.get_program(), "/x/sat");
    assert_eq!(args(&command), [":3", "-listenfd", "7", "-listenfd", "8"]);
    let envs = envs(&command);
    assert_eq!(envs.get(OsStr::new("DISPLAY")), Some(&None));
    assert_eq!(envs.get(OsStr::new("WAYLAND_SOCKET")), Some(&None));
    assert_eq!(
        envs.get(OsStr::new("WAYLAND_DISPLAY")),
        Some(&Some(OsString::from("wayland-9")))
    );
}

#[test]
fn satellite_without_an_abstract_socket_gets_one_fd() {
    let command = satellite_command("/x/sat", ":0", &[5], OsStr::new("wayland-1"));
    assert_eq!(args(&command), [":0", "-listenfd", "5"]);
}

#[test]
fn the_probe_asks_for_listenfd_support() {
    let command = probe_command("/x/sat");
    assert_eq!(command.get_program(), "/x/sat");
    assert_eq!(args(&command), [":0", "--test-listenfd-support"]);
    assert_eq!(envs(&command).get(OsStr::new("DISPLAY")), Some(&None));
}
