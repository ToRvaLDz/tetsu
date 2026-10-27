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
use tetsu_comp::layout::master::master;
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
        Self::start_with(&[], None)
    }

    /// Starts with extra arguments and, if given, a config file.
    fn start_with(args: &[&str], config: Option<&str>) -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_tetsu-comp"));
        command.args(["--backend", "headless"]).args(args);
        if let Some(text) = config {
            let path = runtime.path().join("config.toml");
            std::fs::write(&path, text).unwrap();
            command.arg("--config").arg(path);
        }
        let mut child = command
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

const VIEW: Rect = Rect {
    x: 0,
    y: 0,
    w: 1920,
    h: 1080,
};

fn size(rect: Rect) -> (u32, u32) {
    (rect.w as u32, rect.h as u32)
}

/// Maps `n` windows and waits until each has its tile of `rects`.
fn map_and_check(comp: &Comp, rects: impl Fn(usize) -> Vec<Rect>, n: usize) {
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let tops: Vec<_> = (0..n).map(|i| wl.map(&format!("app{i}"), "t")).collect();
    let expected = rects(n);
    for (top, rect) in tops.iter().zip(&expected) {
        wl.wait_for_size(top, size(*rect));
        assert!(wl.states(top), "tiled states");
    }
}

#[test]
fn three_windows_dwindle_by_default() {
    let comp = Comp::start();
    map_and_check(&comp, |n| dwindle(VIEW, Gaps::NONE, n), 3);
}

#[test]
fn the_master_layout_can_be_the_default() {
    let comp = Comp::start_with(&[], Some("[layout]\ndefault = \"master\"\n"));
    map_and_check(&comp, |n| master(VIEW, Gaps::NONE, n, 0.55), 3);
}

#[test]
fn the_floating_layout_leaves_the_size_to_the_client() {
    let comp = Comp::start_with(&[], Some("[layout]\ndefault = \"floating\"\n"));
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map_floating("a", "one", (400, 300));
    assert_eq!(wl.sizes(&top), []);
    assert!(!wl.states(&top));
    let listed = bus.call(&contract, LIST_WINDOWS);
    assert_eq!(listed["windows"][0]["placement"], "floating");
}

#[test]
fn gaps_inset_the_window() {
    let comp = Comp::start_with(&[], Some("[layout]\ngaps_outer = 10\ngaps_inner = 20\n"));
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("a", "one");
    assert_eq!(wl.sizes(&top)[0], (1900, 1060));
}

#[test]
fn a_floating_rule_with_a_size_floats_the_window_and_leaves_the_tiles_alone() {
    let comp = Comp::start_with(
        &[],
        Some("[[window_rule]]\napp_id = \"^pavucontrol$\"\nfloating = true\nsize = [640, 480]\n"),
    );
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let tiled = wl.map("foot", "shell");
    wl.wait_for_size(&tiled, (1920, 1080));
    let floating = wl.map_floating("pavucontrol", "Volume", (100, 100));
    assert_eq!(wl.sizes(&floating)[0], (640, 480));
    assert!(!wl.states(&floating));
    wl.sync();
    assert_eq!(wl.sizes(&tiled).last(), Some(&(1920, 1080)));
    assert!(wl.states(&tiled));

    let listed = bus.call(&contract, LIST_WINDOWS);
    let placements: Vec<_> = listed["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| {
            (
                w["app_id"].as_str().unwrap().to_owned(),
                w["placement"].clone(),
            )
        })
        .collect();
    assert_eq!(
        placements,
        [
            ("foot".to_owned(), json!("tiled")),
            ("pavucontrol".to_owned(), json!("floating"))
        ]
    );
}

#[test]
fn a_workspace_rule_opens_the_window_on_that_workspace_without_the_focus() {
    let comp = Comp::start_with(
        &[],
        Some("[[window_rule]]\napp_id = \"^music$\"\nworkspace = 2\n"),
    );
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let before = bus.call(&contract, LIST_WORKSPACES);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let first = wl.map("foot", "shell");
    let focused = bus.call(&contract, LIST_WINDOWS)["focused_id"].clone();

    let mut watch = Client::connect(&comp.bus);
    watch.send(WATCH_WINDOWS, true);
    watch.event(&contract, WATCH_WINDOWS);
    let _music = wl.map("music", "player");
    let opened = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(opened["kind"], "opened");
    let window = opened["window"].clone();

    let after = bus.call(&contract, LIST_WORKSPACES);
    assert_eq!(after["focused_id"], before["focused_id"]);
    assert_eq!(after["active"], before["active"]);
    let list = after["workspaces"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let second = list.iter().find(|w| w["index"] == 1).unwrap();
    assert_eq!(window["workspace_id"], second["id"]);

    // No focused event follows: the next one is the title change.
    wl.set_title(&first, "other");
    let next = watch.event(&contract, WATCH_WINDOWS);
    assert_eq!(next["kind"], "changed");
    assert_eq!(bus.call(&contract, LIST_WINDOWS)["focused_id"], focused);
}

#[test]
fn an_output_rule_opens_the_window_on_the_other_output() {
    let comp = Comp::start_with(
        &["--headless-outputs", "2"],
        Some("[[window_rule]]\napp_id = \"^side$\"\noutput = \"HEADLESS-2\"\n"),
    );
    let contract = common::contract();
    let mut bus = Client::connect(&comp.bus);
    let mut wl = WlClient::connect(&comp.wayland_socket());
    let top = wl.map("side", "one");
    wl.wait_for_size(&top, (1920, 1080));
    let windows = bus.call(&contract, LIST_WINDOWS);
    let workspaces = bus.call(&contract, LIST_WORKSPACES);
    assert_eq!(workspaces["active"].as_object().unwrap().len(), 2);
    let second = workspaces["active"]["out-2"].as_str().unwrap();
    assert_eq!(windows["windows"][0]["workspace_id"], second);
    assert_ne!(workspaces["active"]["out-1"], workspaces["active"]["out-2"]);
}

#[test]
fn a_bad_config_stops_the_start_and_names_the_key() {
    let runtime = tempfile::tempdir().unwrap();
    let path = runtime.path().join("config.toml");
    std::fs::write(&path, "[layout]\nmaster_ratio = 5.0\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_tetsu-comp"))
        .args(["--backend", "headless", "--config"])
        .arg(&path)
        .env("XDG_RUNTIME_DIR", runtime.path())
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A compositor that starts anyway is killed after a few seconds.
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "no readiness line");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("layout.master_ratio"), "{stderr}");
    assert!(stderr.contains("config.toml"), "{stderr}");
}
