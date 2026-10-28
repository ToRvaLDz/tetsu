//! `org.tetsu.comp.render.v1` on the bus thread, over a real socket, and
//! behind the real gate. Every reply, event and error is checked against
//! the contract.

mod common;

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Client;
use common::capd::GatedComp;
use serde_json::{Value, json};
use tetsu_bus::render::{DropReason, GetRenderReply, Render, RenderEvent};
use tetsu_comp::bus::{Command, CommandSink, Publisher, Snapshot};
use tetsu_comp::gpu::control::{DeviceFacts, RenderCtl, RenderError};
use tetsu_comp::gpu::report;
use tetsu_comp::model::Model;

const GET_RENDER: &str = "org.tetsu.comp.render.v1.GetRender";
const WATCH_RENDER: &str = "org.tetsu.comp.render.v1.WatchRender";
const SET_DEVICE: &str = "org.tetsu.comp.render.v1.SetRenderDevice";
const SET_SYNC: &str = "org.tetsu.comp.render.v1.SetExplicitSync";
const LIST_STATS: &str = "org.tetsu.comp.render.v1.ListRenderStats";
const WATCH_STATS: &str = "org.tetsu.comp.render.v1.WatchRenderStats";

const TIMEOUT: Duration = Duration::from_secs(10);

fn socket(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("tetsu/comp/wayland-test")
}

/// The render state of a headless compositor with `extra` fake devices.
fn render(extra: &[&str]) -> Render {
    let mut ctl = RenderCtl::synthetic("headless-0", "headless");
    for id in extra {
        ctl.add(DeviceFacts::new(id, &format!("/dev/dri/{id}"), "headless"));
    }
    let outputs = BTreeMap::from([("headless-0".to_owned(), vec!["out-1".to_owned()])]);
    report::render(&ctl, &outputs)
}

fn snapshot(render: Render) -> Snapshot {
    Snapshot::of(&Model::default()).with_render(render)
}

fn spawn(dir: &tempfile::TempDir, commands: Option<CommandSink>) -> Publisher {
    Publisher::spawn_with(&socket(dir), snapshot(render(&[])), commands).unwrap()
}

fn get_render(client: &mut Client, contract: &tetsu_bus_contract::Contract) -> Render {
    let reply: GetRenderReply = serde_json::from_value(client.call(contract, GET_RENDER)).unwrap();
    reply.render
}

#[test]
fn get_render_matches_the_snapshot_and_follows_the_events() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let publisher = spawn(&dir, None);
    let mut client = Client::connect(&socket(&dir));
    assert_eq!(get_render(&mut client, &contract), render(&[]));

    let next = render(&["gpu-1"]);
    publisher.publish_render(vec![RenderEvent::changed(next.clone())]);
    let start = Instant::now();
    while get_render(&mut client, &contract) != next {
        assert!(
            start.elapsed() < TIMEOUT,
            "the event never reached GetRender"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_watch_sends_initial_then_the_events_in_order() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let publisher = spawn(&dir, None);
    let mut watch = Client::connect(&socket(&dir));
    watch.send(WATCH_RENDER, true);
    let initial: RenderEvent =
        serde_json::from_value(watch.event(&contract, WATCH_RENDER)).unwrap();
    assert_eq!(initial, RenderEvent::initial(render(&[])));

    let next = render(&["gpu-1"]);
    publisher.publish_render(vec![
        RenderEvent::pending_dropped(DropReason::DeviceGone),
        RenderEvent::changed(next.clone()),
    ]);
    let dropped: RenderEvent =
        serde_json::from_value(watch.event(&contract, WATCH_RENDER)).unwrap();
    assert_eq!(
        dropped,
        RenderEvent::pending_dropped(DropReason::DeviceGone)
    );
    common::check_render_event(&contract, &dropped);
    let changed: RenderEvent =
        serde_json::from_value(watch.event(&contract, WATCH_RENDER)).unwrap();
    assert_eq!(changed, RenderEvent::changed(next));
    common::check_render_event(&contract, &changed);
}

#[test]
fn a_watch_without_more_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _publisher = spawn(&dir, None);
    let mut client = Client::connect(&socket(&dir));
    client.send(WATCH_RENDER, false);
    assert_eq!(client.recv()["error"], "org.varlink.service.ExpectedMore");
}

/// One `GetRender` over a fresh connection; any failure is `None`.
fn try_get_render(path: &std::path::Path) -> Option<Value> {
    use std::io::Read;
    let mut stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    let mut call = serde_json::to_vec(&json!({ "method": GET_RENDER, "parameters": {} })).ok()?;
    call.push(0);
    stream.write_all(&call).ok()?;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 65536];
    while !reply.contains(&0) {
        match stream.read(&mut chunk).ok()? {
            0 => return None,
            n => reply.extend_from_slice(&chunk[..n]),
        }
    }
    let end = reply.iter().position(|b| *b == 0)?;
    serde_json::from_slice(&reply[..end]).ok()
}

#[test]
fn a_restarted_bus_thread_serves_the_render_state_of_its_snapshot() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let mut publisher = spawn(&dir, None);
    publisher.crash_for_test();
    let later = render(&["gpu-1"]);
    let start = Instant::now();
    let reply = loop {
        publisher.supervise(|| snapshot(later.clone()));
        if let Some(reply) = try_get_render(&socket(&dir)) {
            let got: Result<GetRenderReply, _> =
                serde_json::from_value(reply["parameters"].clone());
            if got.as_ref().is_ok_and(|got| got.render == later) {
                break reply;
            }
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "the bus thread was not restarted"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    common::check(&contract, GET_RENDER, &reply["parameters"]);
}

#[test]
fn the_service_lists_and_describes_render_v1() {
    let dir = tempfile::tempdir().unwrap();
    let _publisher = spawn(&dir, None);
    let mut client = Client::connect(&socket(&dir));
    client.send("org.varlink.service.GetInfo", false);
    let info = client.recv();
    assert!(
        info["parameters"]["interfaces"]
            .as_array()
            .unwrap()
            .contains(&json!("org.tetsu.comp.render.v1"))
    );
    client.send_with(
        "org.varlink.service.GetInterfaceDescription",
        &json!({ "interface": "org.tetsu.comp.render.v1" }),
        false,
    );
    assert_eq!(
        client.recv()["parameters"]["description"],
        tetsu_bus::render::IDL
    );
}

#[test]
fn the_stats_methods_are_not_implemented_yet() {
    let dir = tempfile::tempdir().unwrap();
    let _publisher = spawn(&dir, None);
    let mut client = Client::connect(&socket(&dir));
    for method in [LIST_STATS, WATCH_STATS] {
        client.send(method, method == WATCH_STATS);
        let reply = client.recv();
        assert_eq!(
            reply["error"], "org.varlink.service.MethodNotImplemented",
            "{method}"
        );
        assert_eq!(reply["parameters"]["method"], method);
    }
}

/// A sink that records each Set call and answers as `answer` says.
fn recording(
    answer: impl Fn(&str) -> Result<bool, tetsu_bus::server::ErrorReply> + Send + Sync + 'static,
) -> (CommandSink, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    let sink: CommandSink = Arc::new(move |command| {
        let (line, reply) = match command {
            Command::SetRenderDevice(call) => (format!("device:{:?}", call.target), call.reply),
            Command::SetExplicitSync(call) => (format!("sync:{:?}", call.mode), call.reply),
            _ => panic!("expected a render call"),
        };
        let _ = reply.send(answer(&line));
        log.lock().unwrap().push(line);
        Ok(())
    });
    (sink, seen)
}

#[test]
fn the_set_calls_forward_to_the_loop_and_map_its_answer() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let (sink, seen) = recording(|line| match line {
        "device:Some(\"nope\")" => Err(RenderError::DeviceNotFound("nope".into()).reply()),
        "sync:On" => Err(RenderError::UnsupportedByHardware("no syncobj_eventfd".into()).reply()),
        "sync:Off" => Ok(true),
        _ => Ok(false),
    });
    let _publisher = spawn(&dir, Some(sink));
    let mut client = Client::connect(&socket(&dir));

    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "gpu-1" }));
    assert_eq!(reply, json!({ "pending": false }));
    let reply = client.call_with(&contract, SET_DEVICE, &json!({}));
    assert_eq!(reply, json!({ "pending": false }));
    let reply = client.call_with(&contract, SET_SYNC, &json!({ "mode": "off" }));
    assert_eq!(reply, json!({ "pending": true }));
    let (name, params) = client.call_error(&contract, SET_DEVICE, &json!({ "device_id": "nope" }));
    assert_eq!(name, "org.tetsu.comp.render.v1.DeviceNotFound");
    assert_eq!(params, json!({ "id": "nope" }));
    let (name, _) = client.call_error(&contract, SET_SYNC, &json!({ "mode": "on" }));
    assert_eq!(name, "org.tetsu.comp.render.v1.UnsupportedByHardware");

    assert_eq!(
        *seen.lock().unwrap(),
        [
            "device:Some(\"gpu-1\")",
            "device:None",
            "sync:Off",
            "device:Some(\"nope\")",
            "sync:On",
        ]
    );
}

#[test]
fn bad_parameters_are_an_invalid_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(|_| panic!("nothing should be forwarded"));
    let _publisher = spawn(&dir, Some(commands));
    let mut client = Client::connect(&socket(&dir));
    for (method, params, name) in [
        (SET_DEVICE, json!({ "device_id": 3 }), "device_id"),
        (SET_SYNC, json!({ "mode": "maybe" }), "mode"),
        (SET_SYNC, json!({}), "mode"),
    ] {
        client.send_with(method, &params, false);
        let reply = client.recv();
        assert_eq!(reply["error"], "org.varlink.service.InvalidParameter");
        assert_eq!(reply["parameters"]["parameter"], name);
    }
}

#[test]
fn a_loop_that_is_gone_is_a_render_state_write_failure() {
    let contract = common::contract();
    for commands in [
        Arc::new(Err) as CommandSink,
        Arc::new(|_| Ok(())) as CommandSink,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let _publisher = spawn(&dir, Some(commands));
        let mut client = Client::connect(&socket(&dir));
        let (name, _) = client.call_error(&contract, SET_DEVICE, &json!({ "device_id": null }));
        assert_eq!(name, "org.tetsu.comp.render.v1.StateWriteFailed");
        let (name, _) = client.call_error(&contract, SET_SYNC, &json!({ "mode": "auto" }));
        assert_eq!(name, "org.tetsu.comp.render.v1.StateWriteFailed");
    }
}

#[test]
fn without_a_loop_the_set_calls_are_not_implemented() {
    let dir = tempfile::tempdir().unwrap();
    let _publisher = spawn(&dir, None);
    let mut client = Client::connect(&socket(&dir));
    client.send_with(SET_SYNC, &json!({ "mode": "auto" }), false);
    assert_eq!(
        client.recv()["error"],
        "org.varlink.service.MethodNotImplemented"
    );
}

// The gate.

fn gated(as_agent: bool) -> GatedComp {
    GatedComp::start(as_agent, snapshot(render(&[])))
}

fn checks(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().unwrap().clone()
}

#[test]
fn an_agents_set_calls_ask_capd_and_run_on_allow() {
    let contract = common::contract();
    let comp = gated(true);
    let mut client = Client::connect(&comp.socket);
    let reply = client.call_with(&contract, SET_DEVICE, &json!({ "device_id": "headless-0" }));
    assert_eq!(reply, json!({ "pending": false }));
    let reply = client.call_with(&contract, SET_SYNC, &json!({ "mode": "off" }));
    assert_eq!(reply, json!({ "pending": false }));
    assert_eq!(
        checks(&comp.capd),
        [format!("Check:{SET_DEVICE}"), format!("Check:{SET_SYNC}")]
    );
    assert_eq!(
        checks(&comp.sink),
        [
            "SetRenderDevice:Some(\"headless-0\")",
            "SetExplicitSync:Off"
        ]
    );
}

#[test]
fn a_denied_agent_set_call_is_permission_denied_and_nothing_is_forwarded() {
    let contract = common::contract();
    let comp = gated(true);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    for (method, params) in [
        (SET_DEVICE, json!({ "device_id": null })),
        (SET_SYNC, json!({ "mode": "auto" })),
    ] {
        let (name, params) = client.call_error(&contract, method, &params);
        assert_eq!(name, "org.tetsu.comp.render.v1.PermissionDenied");
        assert_eq!(params, json!({}));
    }
    assert_eq!(
        checks(&comp.capd),
        [format!("Check:{SET_DEVICE}"), format!("Check:{SET_SYNC}")]
    );
    assert!(checks(&comp.sink).is_empty());
}

#[test]
fn an_agents_reads_never_reach_capd() {
    let contract = common::contract();
    let comp = gated(true);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    assert_eq!(get_render(&mut client, &contract), render(&[]));
    client.send(WATCH_RENDER, true);
    let initial = client.event(&contract, WATCH_RENDER);
    assert_eq!(initial["kind"], "initial");
    assert!(checks(&comp.capd).is_empty());
}

#[test]
fn an_owners_set_calls_pass_without_capd() {
    let contract = common::contract();
    let comp = gated(false);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    let reply = client.call_with(&contract, SET_DEVICE, &json!({}));
    assert_eq!(reply, json!({ "pending": false }));
    let reply = client.call_with(&contract, SET_SYNC, &json!({ "mode": "auto" }));
    assert_eq!(reply, json!({ "pending": false }));
    assert!(checks(&comp.capd).is_empty());
    assert_eq!(checks(&comp.sink).len(), 2);
}
