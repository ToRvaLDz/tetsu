//! The bus thread over a real socket, spoken to in raw NUL-framed Varlink.
//! Every reply is checked against the contract.

mod common;

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tetsu_bus::window::{ListWindowsReply, WindowEvent};
use tetsu_bus::workspace::ListWorkspacesReply;
use tetsu_bus_contract::{Contract, check_error};
use tetsu_comp::bus::{Mirror, Publisher, Snapshot, WATCH_QUEUE};
use tetsu_comp::model::{Model, WindowInfo, WindowUpdate};
use tetsu_comp::socket::{COMP_SOCKET_ENV, bus_socket_path};

const TIMEOUT: Duration = Duration::from_secs(10);
const WATCH_WINDOWS: &str = "org.tetsu.comp.window.v1.WatchWindows";
const LIST_WINDOWS: &str = "org.tetsu.comp.window.v1.ListWindows";
const WATCH_WORKSPACES: &str = "org.tetsu.comp.workspace.v1.WatchWorkspaces";
const LIST_WORKSPACES: &str = "org.tetsu.comp.workspace.v1.ListWorkspaces";

struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
}

impl Client {
    fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        Self {
            stream,
            buffer: Vec::new(),
        }
    }

    fn send(&mut self, method: &str, more: bool) {
        let mut call = json!({ "method": method, "parameters": {} });
        if more {
            call["more"] = json!(true);
        }
        let mut bytes = serde_json::to_vec(&call).unwrap();
        bytes.push(0);
        self.stream.write_all(&bytes).unwrap();
    }

    /// The next message, or `None` at end of file.
    fn try_recv(&mut self) -> Option<Value> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == 0) {
                let message: Vec<u8> = self.buffer.drain(..=end).collect();
                return Some(serde_json::from_slice(&message[..end]).unwrap());
            }
            let mut chunk = [0u8; 65536];
            match self.stream.read(&mut chunk) {
                Ok(0) => return None,
                Ok(n) => self.buffer.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return None,
                Err(e) => panic!("no reply in time: {e}"),
            }
        }
    }

    fn recv(&mut self) -> Value {
        self.try_recv().expect("the server closed the connection")
    }

    /// One reply, checked against `method`'s outputs.
    fn call(&mut self, contract: &Contract, method: &str) -> Value {
        self.send(method, false);
        let reply = self.recv();
        let parameters = reply["parameters"].clone();
        common::check(contract, method, &parameters);
        parameters
    }

    /// The next streamed reply of `method`, checked; returns its event.
    fn event(&mut self, contract: &Contract, method: &str) -> Value {
        let reply = self.recv();
        assert_eq!(reply["continues"], true, "{reply}");
        common::check(contract, method, &reply["parameters"]);
        reply["parameters"]["event"].clone()
    }
}

fn socket(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("tetsu/comp/wayland-test")
}

fn info(title: &str) -> WindowInfo {
    WindowInfo {
        app_id: Some("foot".into()),
        title: Some(title.into()),
        pid: Some(7),
    }
}

/// A model with one output, one focused window, flushed.
fn model_with_window() -> (Model, String) {
    let mut model = Model::default();
    model.add_output("HEADLESS-1");
    let id = model.open(info("one"));
    model.focus(Some(&id));
    model.flush();
    (model, id)
}

/// Polls `check` until it holds.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn list_windows(client: &mut Client, contract: &Contract) -> ListWindowsReply {
    serde_json::from_value(client.call(contract, LIST_WINDOWS)).unwrap()
}

fn list_workspaces(client: &mut Client, contract: &Contract) -> ListWorkspacesReply {
    serde_json::from_value(client.call(contract, LIST_WORKSPACES)).unwrap()
}

#[test]
fn lists_match_the_snapshot_and_follow_the_deltas() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let (mut model, first) = model_with_window();
    let publisher = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();
    let mut client = Client::connect(&socket(&dir));

    assert_eq!(
        list_windows(&mut client, &contract),
        model.windows_snapshot()
    );
    assert_eq!(
        list_workspaces(&mut client, &contract),
        model.workspaces_snapshot()
    );

    let second = model.open(info("two"));
    model.focus(Some(&second));
    model.update(&first, WindowUpdate::title("renamed"));
    publisher.publish(model.flush());
    eventually("the deltas in the lists", || {
        list_windows(&mut client, &contract) == model.windows_snapshot()
    });
    assert_eq!(
        list_workspaces(&mut client, &contract),
        model.workspaces_snapshot()
    );
}

#[test]
fn a_watch_sends_initial_then_the_deltas_in_order() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let (mut model, first) = model_with_window();
    let publisher = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();

    let mut windows = Client::connect(&socket(&dir));
    windows.send(WATCH_WINDOWS, true);
    let initial = windows.event(&contract, WATCH_WINDOWS);
    let expected = model.windows_snapshot();
    assert_eq!(
        serde_json::from_value::<WindowEvent>(initial).unwrap(),
        WindowEvent::initial(expected.windows, expected.focused_id)
    );

    let mut workspaces = Client::connect(&socket(&dir));
    workspaces.send(WATCH_WORKSPACES, true);
    let initial = workspaces.event(&contract, WATCH_WORKSPACES);
    assert_eq!(initial["kind"], "initial");
    assert_eq!(initial["focused_id"], "ws-1");

    let second = model.open(info("two"));
    model.focus(Some(&second));
    model.add_output("HEADLESS-2");
    publisher.publish(model.flush());
    model.close(&second);
    model.update(&first, WindowUpdate::title("late"));
    publisher.publish(model.flush());

    let kinds: Vec<_> = (0..5)
        .map(|_| windows.event(&contract, WATCH_WINDOWS)["kind"].clone())
        .collect();
    assert_eq!(kinds, ["opened", "focused", "focused", "closed", "changed"]);
    let kinds: Vec<_> = (0..2)
        .map(|_| workspaces.event(&contract, WATCH_WORKSPACES)["kind"].clone())
        .collect();
    assert_eq!(kinds, ["added", "activated"]);
}

#[test]
fn a_watch_without_more_is_refused() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let (model, _) = model_with_window();
    let _publisher = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();
    let mut client = Client::connect(&socket(&dir));
    client.send(WATCH_WINDOWS, false);
    assert_eq!(client.recv()["error"], "org.varlink.service.ExpectedMore");
    drop(contract);
}

#[test]
fn the_service_describes_itself_and_refuses_other_methods() {
    let dir = tempfile::tempdir().unwrap();
    let (model, _) = model_with_window();
    let _publisher = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();
    let mut client = Client::connect(&socket(&dir));

    client.send("org.varlink.service.GetInfo", false);
    let info = client.recv();
    let interfaces = &info["parameters"]["interfaces"];
    assert!(
        interfaces
            .as_array()
            .unwrap()
            .contains(&json!("org.tetsu.comp.window.v1"))
    );
    assert!(
        interfaces
            .as_array()
            .unwrap()
            .contains(&json!("org.tetsu.comp.workspace.v1"))
    );

    let mut call = json!({
        "method": "org.varlink.service.GetInterfaceDescription",
        "parameters": { "interface": "org.tetsu.comp.window.v1" },
    })
    .to_string()
    .into_bytes();
    call.push(0);
    client.stream.write_all(&call).unwrap();
    let described = client.recv();
    assert_eq!(
        described["parameters"]["description"],
        tetsu_bus::window::IDL
    );

    client.send("org.tetsu.comp.window.v1.Nope", false);
    let error = client.recv();
    assert_eq!(error["error"], "org.varlink.service.MethodNotFound");
    let contract = common::contract();
    assert_eq!(
        check_error(
            &contract,
            error["error"].as_str().unwrap(),
            &error["parameters"]
        ),
        vec![]
    );
}

#[test]
fn a_watcher_that_never_reads_is_cut_while_another_keeps_receiving() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let (mut model, id) = model_with_window();
    let publisher = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();

    let mut slow = Client::connect(&socket(&dir));
    slow.send(WATCH_WINDOWS, true);
    let mut fast = Client::connect(&socket(&dir));
    fast.send(WATCH_WINDOWS, true);
    fast.event(&contract, WATCH_WINDOWS);
    // Both watchers are registered once the fast one has its initial; the
    // slow one is registered before it, on the same connection order.
    let (seen_tx, seen_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        while let Some(reply) = fast.try_recv() {
            let title = reply["parameters"]["event"]["window"]["title"].clone();
            if seen_tx.send(title).is_err() {
                break;
            }
        }
    });

    let big = "x".repeat(8 * 1024);
    let total = 4 * WATCH_QUEUE;
    for n in 0..total {
        model.update(&id, WindowUpdate::title(&format!("{n:06}{big}")));
        publisher.publish(model.flush());
        if n % 16 == 15 {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    // The fast watcher saw the last title.
    let last = format!("{:06}", total - 1);
    let start = Instant::now();
    loop {
        let left = TIMEOUT.saturating_sub(start.elapsed());
        let title = seen_rx
            .recv_timeout(left)
            .expect("the fast watcher keeps receiving");
        if title.as_str().is_some_and(|t| t.starts_with(&last)) {
            break;
        }
    }
    drop(seen_rx);

    // The slow one is read at last: what was buffered, then end of file.
    let mut received = 0;
    while slow.try_recv().is_some() {
        received += 1;
    }
    assert!(
        received < total,
        "the slow watcher was not cut ({received})"
    );
    drop(reader);
}

#[test]
fn a_new_watch_after_a_crash_gets_the_current_snapshot() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let (mut model, _) = model_with_window();
    let mut publisher = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();

    let mut before = Client::connect(&socket(&dir));
    before.send(WATCH_WINDOWS, true);
    before.event(&contract, WATCH_WINDOWS);

    publisher.crash_for_test();
    let second = model.open(info("after the crash"));
    model.focus(Some(&second));
    model.flush();

    // The old watcher's connection ends with the thread.
    assert!(before.try_recv().is_none());

    let expected = model.windows_snapshot();
    let start = Instant::now();
    let initial = loop {
        publisher.supervise(|| Snapshot::of(&model));
        if let Ok(stream) = UnixStream::connect(socket(&dir)) {
            let mut client = Client {
                stream,
                buffer: Vec::new(),
            };
            client.stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            client.send(WATCH_WINDOWS, true);
            if let Some(reply) = client.try_recv() {
                common::check(&contract, WATCH_WINDOWS, &reply["parameters"]);
                break reply["parameters"]["event"].clone();
            }
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "the bus thread was not restarted"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        serde_json::from_value::<WindowEvent>(initial).unwrap(),
        WindowEvent::initial(expected.windows, expected.focused_id)
    );
}

#[test]
fn the_mirror_equals_the_model_after_any_sequence() {
    let mut model = Model::default();
    let mut mirror = Mirror::from_snapshot(&Snapshot::of(&model));
    let mut check = |model: &mut Model, mirror: &mut Mirror| {
        let events = model.flush();
        for event in &events.window {
            mirror.apply_window(event);
        }
        for event in &events.workspace {
            mirror.apply_workspace(event);
        }
        assert_eq!(mirror.windows(), model.windows_snapshot());
        assert_eq!(mirror.workspaces(), model.workspaces_snapshot());
    };
    check(&mut model, &mut mirror);
    model.add_output("A");
    check(&mut model, &mut mirror);
    let one = model.open(info("one"));
    let two = model.open(info("two"));
    model.focus(Some(&two));
    check(&mut model, &mut mirror);
    model.update(&one, WindowUpdate::title("1"));
    model.add_output("B");
    model.focus_workspace("ws-2");
    let three = model.open(info("three"));
    model.focus(Some(&three));
    check(&mut model, &mut mirror);
    model.close(&three);
    model.close(&one);
    model.focus(Some(&two));
    check(&mut model, &mut mirror);
    model.close(&two);
    check(&mut model, &mut mirror);
}

#[test]
fn the_socket_path_is_per_wayland_instance() {
    let path = bus_socket_path(Some(Path::new("/run/user/1000")), "wayland-1").unwrap();
    assert_eq!(path, Path::new("/run/user/1000/tetsu/comp/wayland-1"));
    let other = bus_socket_path(Some(Path::new("/run/user/1000")), "wayland-2").unwrap();
    assert_ne!(path, other);
    assert_eq!(COMP_SOCKET_ENV, "TETSU_COMP_SOCKET");
}

#[test]
fn a_missing_runtime_dir_is_an_error() {
    assert!(bus_socket_path(None, "wayland-1").is_err());
    assert!(bus_socket_path(Some(Path::new("")), "wayland-1").is_err());
    assert!(bus_socket_path(Some(Path::new("relative")), "wayland-1").is_err());
}

#[test]
fn binding_twice_on_one_socket_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (model, _) = model_with_window();
    let _first = Publisher::spawn(&socket(&dir), Snapshot::of(&model)).unwrap();
    let second = Publisher::spawn(&socket(&dir), Snapshot::of(&model));
    assert!(second.is_err());
}
