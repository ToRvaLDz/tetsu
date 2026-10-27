//! The whole compositor, started headless: a real Wayland client maps a
//! window and the bus follows it. Every reply is checked against the
//! contract.

mod common;
#[path = "common/wayland.rs"]
mod wayland;

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use common::Client;
use serde_json::{Value, json};
use tetsu_comp::layout::dwindle::dwindle;
use tetsu_comp::layout::{Gaps, Rect};
use wayland::WlClient;

const WATCH_WINDOWS: &str = "org.tetsu.comp.window.v1.WatchWindows";
const LIST_WINDOWS: &str = "org.tetsu.comp.window.v1.ListWindows";
const LIST_WORKSPACES: &str = "org.tetsu.comp.workspace.v1.ListWorkspaces";
/// A hung compositor is killed after this.
const WATCHDOG: Duration = Duration::from_secs(60);

/// A running compositor, killed when dropped or when it hangs.
struct Comp {
    child: Child,
    runtime: tempfile::TempDir,
    display: String,
    bus: PathBuf,
    cancel: mpsc::Sender<()>,
}

impl Comp {
    fn start() -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_tetsu-comp"))
            .args(["--backend", "headless"])
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let (cancel, cancelled) = mpsc::channel();
        std::thread::spawn(move || {
            if cancelled.recv_timeout(WATCHDOG).is_err() {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
        });
        let mut line = String::new();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        stdout.read_line(&mut line).unwrap();
        let (display, bus) = parse_ready(&line);
        // The compositor writes nothing else; keep the pipe open.
        std::mem::forget(stdout);
        Self {
            child,
            runtime,
            display,
            bus,
            cancel,
        }
    }

    fn wayland_socket(&self) -> PathBuf {
        self.runtime.path().join(&self.display)
    }
}

impl Drop for Comp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.cancel.send(());
    }
}

/// `ready WAYLAND_DISPLAY=<n> TETSU_COMP_SOCKET=<p>`
fn parse_ready(line: &str) -> (String, PathBuf) {
    let mut words = line.split_whitespace();
    assert_eq!(words.next(), Some("ready"), "readiness line: {line:?}");
    let display = words
        .next()
        .and_then(|w| w.strip_prefix("WAYLAND_DISPLAY="))
        .unwrap_or_else(|| panic!("no WAYLAND_DISPLAY in {line:?}"));
    let socket = words
        .next()
        .and_then(|w| w.strip_prefix("TETSU_COMP_SOCKET="))
        .unwrap_or_else(|| panic!("no TETSU_COMP_SOCKET in {line:?}"));
    (display.to_owned(), PathBuf::from(socket))
}

#[test]
fn an_empty_compositor_lists_nothing_and_one_workspace() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut client = Client::connect(&comp.bus);

    let windows = client.call(&contract, LIST_WINDOWS);
    assert_eq!(windows, json!({ "windows": [], "focused_id": null }));

    let workspaces = client.call(&contract, LIST_WORKSPACES);
    let list = workspaces["workspaces"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    let id = list[0]["id"].as_str().unwrap();
    assert_eq!(workspaces["focused_id"], id);
    let output = list[0]["output_id"].as_str().unwrap();
    assert_eq!(workspaces["active"][output], id);
}

#[test]
fn a_mapped_window_is_opened_focused_changed_and_closed_on_the_bus() {
    let comp = Comp::start();
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let workspaces = bus.call(&contract, LIST_WORKSPACES);
    let workspace = workspaces["focused_id"].as_str().unwrap().to_owned();

    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_WINDOWS, true);
    let initial = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(initial["kind"], "initial");
    assert_eq!(initial["windows"], json!([]));
    assert_eq!(initial["focused_id"], Value::Null);

    // (b) a toplevel maps: opened, then focused; configured to the tile.
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("org.tetsu.test", "one");
    let output = Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    let tile = dwindle(output, Gaps::NONE, 1)[0];
    assert_eq!(wl.sizes(&top)[0], (tile.w as u32, tile.h as u32));

    let opened = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(opened["kind"], "opened");
    let window = &opened["window"];
    assert_eq!(window["app_id"], "org.tetsu.test");
    assert_eq!(window["title"], "one");
    assert_eq!(window["pid"], std::process::id());
    assert_eq!(window["workspace_id"], workspace);
    assert_eq!(window["placement"], "tiled");
    assert_eq!(window["fullscreen"], false);
    let id = window["id"].as_str().unwrap().to_owned();
    let focused = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(focused, json!({ "kind": "focused", "focused_id": id }));

    let listed = bus.call(&contract, LIST_WINDOWS);
    assert_eq!(listed["windows"].as_array().unwrap().len(), 1);
    assert_eq!(listed["windows"][0]["id"], id);
    assert_eq!(listed["focused_id"], id);

    // (c) one title change, one changed.
    wl.set_title(&top, "two");
    let changed = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(changed["kind"], "changed");
    assert_eq!(changed["window"]["title"], "two");
    assert_eq!(changed["window"]["id"], id);

    // (d) the focus goes before the window does.
    wl.destroy(top);
    let focused = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(focused, json!({ "kind": "focused", "focused_id": null }));
    let closed = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(closed, json!({ "kind": "closed", "id": id }));

    let listed = bus.call(&contract, LIST_WINDOWS);
    assert_eq!(listed, json!({ "windows": [], "focused_id": null }));
}

#[test]
fn a_second_window_is_tiled_beside_the_first() {
    let comp = Comp::start();
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let one = wl.map("a", "one");
    let two = wl.map("b", "two");
    // Both get half of the output.
    assert_eq!(wl.sizes(&two)[0], (960, 1080));
    wl.wait_for_size(&one, (960, 1080));
    // The first one gets the whole output back when the second goes.
    wl.destroy(two);
    wl.wait_for_size(&one, (1920, 1080));
}
