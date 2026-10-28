//! X11 clients, end to end and headless: what satellite needs from the
//! compositor, the X11 display and `DISPLAY`, and real X11 windows through
//! xwayland-satellite and Xwayland (skipped with a note when either is not
//! on `PATH`; `TETSU_REQUIRE_XWAYLAND=1` makes that a failure).

mod common;
#[path = "common/wayland.rs"]
mod wayland;
#[path = "common/x11.rs"]
mod x11;

use std::path::Path;
use std::time::{Duration, Instant};

use common::Client;
use common::comp::Comp;
use serde_json::Value;
use tetsu_bus_contract::Contract;
use wayland::WlClient;

const WATCH_WINDOWS: &str = "org.tetsu.comp.window.v1.WatchWindows";
const LIST_WINDOWS: &str = "org.tetsu.comp.window.v1.ListWindows";
const XWAYLAND: &str = "--headless-xwayland";
const CLASS: &str = "tetsu-x11-probe";

/// Starting satellite and Xwayland takes a while on the first window.
const X11_TIMEOUT: Duration = Duration::from_secs(20);

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

/// Waits for the client command to write `out`, and reads it.
fn read_out(out: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(out)
            && !text.is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "{} never written", out.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Starts with `args` and `-- sh -c 'printf ... > $OUT'`; returns the
/// compositor and what the client saw as `DISPLAY`.
fn client_display(args: &[&str], env: &[(&str, &str)]) -> (Comp, String) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("display");
    let out_text = out.to_str().unwrap();
    let mut all = args.to_vec();
    all.extend([
        "--",
        "sh",
        "-c",
        "printf '%s.' \"${DISPLAY-unset}\" > \"$OUT\"",
    ]);
    let mut env = env.to_vec();
    env.push(("OUT", out_text));
    let comp = Comp::start_with_env(&all, None, &env);
    let seen = read_out(&out);
    (comp, seen.trim_end_matches('.').to_owned())
}

#[test]
fn without_x11_there_is_no_display_and_the_host_one_is_removed() {
    // A host DISPLAY (nested) must not leak to the compositor's clients.
    let (comp, seen) = client_display(&[], &[("DISPLAY", ":99")]);
    assert_eq!(comp.x11_display(), None);
    assert_eq!(seen, "unset");
}

#[test]
fn a_satellite_that_cannot_run_leaves_x11_off() {
    let config = "[xwayland]\nsatellite = \"/nonexistent/xwayland-satellite\"\n";
    let comp = Comp::start_with(&[XWAYLAND], Some(config));
    assert_eq!(comp.x11_display(), None);
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let windows = bus.call(&contract, LIST_WINDOWS);
    assert_eq!(windows["windows"], serde_json::json!([]));
}

/// A `WatchWindows` stream, past its initial event.
fn watch(comp: &Comp, contract: &Contract) -> Client {
    let mut watch = Client::connect(&comp.bus);
    watch.stream.set_read_timeout(Some(X11_TIMEOUT)).unwrap();
    watch.send(WATCH_WINDOWS, true);
    let initial = watch.event(contract, WATCH_WINDOWS);
    assert_eq!(initial["kind"], "initial");
    watch
}

/// The next event of `kind`, every one before it checked and skipped.
fn next_of(watch: &mut Client, contract: &Contract, kind: &str) -> Value {
    loop {
        let event = watch.event(contract, WATCH_WINDOWS);
        if event["kind"] == kind {
            return event;
        }
    }
}

/// The pids of the compositor's children called xwayland-satellite.
fn satellite_pids(comp: &Comp) -> Vec<u32> {
    let tasks = format!("/proc/{}/task", comp.pid());
    let mut pids = Vec::new();
    for task in std::fs::read_dir(tasks).unwrap().flatten() {
        let children = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
        for pid in children.split_whitespace().filter_map(|p| p.parse().ok()) {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            if comm.starts_with("xwayland-satel") {
                pids.push(pid);
            }
        }
    }
    pids
}

/// Asserts an `opened` event is the X11 probe window titled `title`.
fn assert_x11_window(opened: &Value, title: &str) {
    let window = &opened["window"];
    assert_eq!(window["app_id"], CLASS, "{opened}");
    assert_eq!(window["title"], title, "{opened}");
    assert_eq!(window["xwayland"], true, "{opened}");
    assert_eq!(window["pid"], Value::Null, "{opened}");
}

#[test]
fn x11_window_maps() {
    if !x11::require() {
        return;
    }
    let comp = Comp::start_with(&[XWAYLAND], None);
    let display = comp.x11_display().expect("DISPLAY on the ready line");
    assert!(display.starts_with(':'), "{display}");
    let contract = common::contract();
    let mut watch = watch(&comp, &contract);

    let probe = x11::probe_window(display, CLASS, "x11 probe");
    let opened = next_of(&mut watch, &contract, "opened");
    assert_x11_window(&opened, "x11 probe");
    let id = opened["window"]["id"].clone();

    probe.destroy();
    let closed = next_of(&mut watch, &contract, "closed");
    assert_eq!(closed["id"], id);
}

#[test]
fn the_client_command_gets_the_x11_display() {
    if !x11::require() {
        return;
    }
    let (comp, seen) = client_display(&[XWAYLAND], &[("DISPLAY", ":99")]);
    let display = comp.x11_display().expect("DISPLAY on the ready line");
    assert_eq!(seen, display);
}

/// Waits until `pid` is no child of the compositor any more.
fn wait_reaped(comp: &Comp, pid: u32) {
    let deadline = Instant::now() + X11_TIMEOUT;
    while satellite_pids(comp).contains(&pid) {
        assert!(Instant::now() < deadline, "satellite {pid} never went");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A connection to `display`, retried while the compositor re-arms its
/// sockets after a crash.
fn reconnect(display: &str) -> x11rb::rust_connection::RustConnection {
    let mut last = String::new();
    for _ in 0..5 {
        match x11::connect(display) {
            Ok(conn) => return conn,
            Err(e) => last = e,
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("no X11 connection after the crash: {last}");
}

#[test]
fn satellite_restarts_after_crash() {
    if !x11::require() {
        return;
    }
    let comp = Comp::start_with(&[XWAYLAND], None);
    let display = comp.x11_display().expect("DISPLAY").to_owned();
    let contract = common::contract();
    let mut watch = watch(&comp, &contract);

    let _first = x11::probe_window(&display, CLASS, "first");
    assert_x11_window(&next_of(&mut watch, &contract, "opened"), "first");
    let before = satellite_pids(&comp);
    assert_eq!(before.len(), 1, "{before:?}");
    let status = std::process::Command::new("kill")
        .args(["-KILL", &before[0].to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    wait_reaped(&comp, before[0]);

    let _second = x11::probe_window_on(reconnect(&display), CLASS, "second");
    let opened = loop {
        let opened = next_of(&mut watch, &contract, "opened");
        if opened["window"]["title"] == "second" {
            break opened;
        }
    };
    assert_x11_window(&opened, "second");
    let after = satellite_pids(&comp);
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after[0], before[0]);
}

#[test]
fn a_wayland_window_beside_satellite_keeps_its_pid() {
    if !x11::require() {
        return;
    }
    let comp = Comp::start_with(&[XWAYLAND], None);
    let display = comp.x11_display().expect("DISPLAY").to_owned();
    let contract = common::contract();
    let mut watch = watch(&comp, &contract);
    let _probe = x11::probe_window(&display, CLASS, "x11");
    assert_x11_window(&next_of(&mut watch, &contract, "opened"), "x11");

    let mut wl = WlClient::connect(&comp.wayland_socket());
    let _top = wl.map("org.tetsu.test", "wayland");
    let opened = next_of(&mut watch, &contract, "opened");
    let window = &opened["window"];
    assert_eq!(window["app_id"], "org.tetsu.test", "{opened}");
    assert_eq!(window["xwayland"], false, "{opened}");
    assert_eq!(window["pid"], std::process::id(), "{opened}");
}
