//! `org.tetsu.comp.config.v1` on the bus thread, over a real socket, and
//! behind the real gate. Every reply, event and error is checked against
//! the contract.

mod common;

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Client;
use common::capd::GatedComp;
use serde_json::{Value, json};
use tetsu_bus::config::{ConfigEvent, GetConfigReply, Origin};
use tetsu_comp::bus::{Command, CommandSink, Publisher, Snapshot};
use tetsu_comp::config::layer::parse;
use tetsu_comp::config::merge::{Values, merge};
use tetsu_comp::model::Model;

const GET_CONFIG: &str = "org.tetsu.comp.config.v1.GetConfig";
const WATCH_CONFIG: &str = "org.tetsu.comp.config.v1.WatchConfig";
const SET_MANAGED: &str = "org.tetsu.comp.config.v1.SetManaged";
const CONFIGURE_OUTPUTS: &str = "org.tetsu.comp.output.v1.ConfigureOutputs";
/// The values and removals of each forwarded call.
type Calls = Vec<(Value, Vec<String>)>;

const TIMEOUT: Duration = Duration::from_secs(10);

fn socket(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("tetsu/comp/wayland-test")
}

fn config(user: &str) -> GetConfigReply {
    let view = merge(&Values::new(), &Values::new(), &parse(user).values);
    GetConfigReply {
        entries: view.entries().to_vec(),
        diagnostics: vec![],
    }
}

fn snapshot(config: GetConfigReply) -> Snapshot {
    Snapshot::of(&Model::default()).with_config(config)
}

fn get_config(client: &mut Client, contract: &tetsu_bus_contract::Contract) -> GetConfigReply {
    serde_json::from_value(client.call(contract, GET_CONFIG)).unwrap()
}

#[test]
fn get_config_matches_the_snapshot_and_follows_the_deltas() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let first = config("[layout]\ngaps_outer = 5\n");
    let publisher = Publisher::spawn(&socket(&dir), snapshot(first.clone())).unwrap();
    let mut client = Client::connect(&socket(&dir));
    assert_eq!(get_config(&mut client, &contract), first);

    let next = config("[layout]\ngaps_outer = 6\n");
    let entry = next.entries[2].clone();
    assert_eq!(entry.key, "layout.gaps_outer");
    let diagnostic = tetsu_bus::config::Diagnostic {
        line: Some(3),
        key: Some("layout.nope".into()),
        message: "layout.nope: unknown key".into(),
    };
    publisher.publish_config(vec![
        ConfigEvent::changed(entry),
        ConfigEvent::diagnostics(vec![diagnostic.clone()]),
    ]);
    let start = Instant::now();
    loop {
        let now = get_config(&mut client, &contract);
        if now.entries == next.entries && now.diagnostics == [diagnostic.clone()] {
            break;
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "the deltas never reached GetConfig"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_watch_sends_initial_then_the_deltas_in_order() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let first = config("");
    let publisher = Publisher::spawn(&socket(&dir), snapshot(first.clone())).unwrap();
    let mut watch = Client::connect(&socket(&dir));
    watch.send(WATCH_CONFIG, true);
    let initial: ConfigEvent =
        serde_json::from_value(watch.event(&contract, WATCH_CONFIG)).unwrap();
    assert_eq!(initial, ConfigEvent::initial(first.entries.clone(), vec![]));

    let mut changed = first.entries[1].clone();
    changed.value = json!(9);
    changed.origin = Origin::Managed;
    changed.managed_value = Some(json!(9));
    publisher.publish_config(vec![
        ConfigEvent::changed(changed.clone()),
        ConfigEvent::diagnostics(vec![]),
    ]);
    let event: ConfigEvent = serde_json::from_value(watch.event(&contract, WATCH_CONFIG)).unwrap();
    assert_eq!(event, ConfigEvent::changed(changed));
    common::check_config_event(&contract, &event);
    let event: ConfigEvent = serde_json::from_value(watch.event(&contract, WATCH_CONFIG)).unwrap();
    assert_eq!(event, ConfigEvent::diagnostics(vec![]));
}

#[test]
fn a_watch_without_more_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _publisher = Publisher::spawn(&socket(&dir), snapshot(config(""))).unwrap();
    let mut client = Client::connect(&socket(&dir));
    client.send(WATCH_CONFIG, false);
    assert_eq!(client.recv()["error"], "org.varlink.service.ExpectedMore");
}

#[test]
fn a_restarted_bus_thread_serves_the_config_of_its_snapshot() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let mut publisher = Publisher::spawn(&socket(&dir), snapshot(config(""))).unwrap();
    publisher.crash_for_test();
    let later = config("[layout]\ngaps_outer = 8\n");
    let start = Instant::now();
    loop {
        publisher.supervise(|| snapshot(later.clone()));
        if let Ok(mut client) =
            std::os::unix::net::UnixStream::connect(socket(&dir)).map(|stream| {
                stream.set_read_timeout(Some(TIMEOUT)).unwrap();
                Client {
                    stream,
                    buffer: Vec::new(),
                }
            })
        {
            client.send(GET_CONFIG, false);
            if let Some(reply) = client.try_recv() {
                common::check(&contract, GET_CONFIG, &reply["parameters"]);
                let got: GetConfigReply =
                    serde_json::from_value(reply["parameters"].clone()).unwrap();
                assert_eq!(got, later);
                break;
            }
        }
        assert!(
            start.elapsed() < TIMEOUT,
            "the bus thread was not restarted"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn the_service_lists_and_describes_config_v1() {
    let dir = tempfile::tempdir().unwrap();
    let _publisher = Publisher::spawn(&socket(&dir), snapshot(config(""))).unwrap();
    let mut client = Client::connect(&socket(&dir));
    client.send("org.varlink.service.GetInfo", false);
    let info = client.recv();
    assert!(
        info["parameters"]["interfaces"]
            .as_array()
            .unwrap()
            .contains(&json!("org.tetsu.comp.config.v1"))
    );
    let mut call = json!({
        "method": "org.varlink.service.GetInterfaceDescription",
        "parameters": { "interface": "org.tetsu.comp.config.v1" },
    })
    .to_string()
    .into_bytes();
    call.push(0);
    client.stream.write_all(&call).unwrap();
    assert_eq!(
        client.recv()["parameters"]["description"],
        tetsu_bus::config::IDL
    );
}

#[test]
fn set_managed_forwards_to_the_loop_and_maps_its_answer() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let seen: Arc<Mutex<Calls>> = Arc::default();
    let log = seen.clone();
    let commands: CommandSink = Arc::new(move |command| {
        let Command::SetManaged(command) = command else {
            panic!("expected SetManaged");
        };
        let bad = command.values.contains_key("layout.nope");
        log.lock()
            .unwrap()
            .push((json!(command.values), command.remove.clone()));
        let answer = if bad {
            Err(tetsu_comp::config::set::SetError::UnknownKey("layout.nope".into()).reply())
        } else {
            Ok(())
        };
        let _ = command.reply.send(answer);
        Ok(())
    });
    let _publisher =
        Publisher::spawn_with(&socket(&dir), snapshot(config("")), Some(commands)).unwrap();
    let mut client = Client::connect(&socket(&dir));

    let reply = client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.gaps_outer": 10 }, "remove": ["layout.gaps_inner"] }),
    );
    assert_eq!(reply, json!({}));
    let (name, params) = client.call_error(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.nope": 1 } }),
    );
    assert_eq!(name, "org.tetsu.comp.config.v1.UnknownKey");
    assert_eq!(params, json!({ "key": "layout.nope" }));

    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].0, json!({ "layout.gaps_outer": 10 }));
    assert_eq!(seen[0].1, ["layout.gaps_inner"]);
    assert!(seen[1].1.is_empty(), "an absent remove is empty");
}

#[test]
fn set_managed_with_bad_parameters_is_an_invalid_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(|command| {
        if let Command::SetManaged(call) = command {
            let _ = call.reply.send(Ok(()));
        }
        Ok(())
    });
    let _publisher =
        Publisher::spawn_with(&socket(&dir), snapshot(config("")), Some(commands)).unwrap();
    let mut client = Client::connect(&socket(&dir));
    client.send_with(SET_MANAGED, &json!({ "values": 3 }), false);
    assert_eq!(
        client.recv()["error"],
        "org.varlink.service.InvalidParameter"
    );
}

#[test]
fn set_managed_when_the_loop_is_gone_is_a_state_write_failure() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(Err);
    let _publisher =
        Publisher::spawn_with(&socket(&dir), snapshot(config("")), Some(commands)).unwrap();
    let mut client = Client::connect(&socket(&dir));
    let (name, _) = client.call_error(&contract, SET_MANAGED, &json!({ "values": {} }));
    assert_eq!(name, "org.tetsu.comp.config.v1.StateWriteFailed");
}

// The gate.

fn gated(as_agent: bool) -> GatedComp {
    GatedComp::start(as_agent, snapshot(config("")))
}

fn checks(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().unwrap().clone()
}

#[test]
fn an_agents_set_managed_asks_capd_and_runs_on_allow() {
    let contract = common::contract();
    let comp = gated(true);
    let mut client = Client::connect(&comp.socket);
    let reply = client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.gaps_outer": 3 } }),
    );
    assert_eq!(reply, json!({}));
    assert_eq!(checks(&comp.capd), [format!("Check:{SET_MANAGED}")]);
    assert_eq!(checks(&comp.sink).len(), 1);
}

#[test]
fn a_denied_agent_set_managed_is_permission_denied_and_nothing_is_forwarded() {
    let contract = common::contract();
    let comp = gated(true);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    let (name, params) = client.call_error(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.gaps_outer": 3 } }),
    );
    assert_eq!(name, "org.tetsu.comp.config.v1.PermissionDenied");
    assert_eq!(params, json!({}));
    assert_eq!(checks(&comp.capd), [format!("Check:{SET_MANAGED}")]);
    assert!(checks(&comp.sink).is_empty());
}

#[test]
fn an_agents_configure_outputs_asks_capd_and_runs_on_allow() {
    let contract = common::contract();
    let comp = gated(true);
    let mut client = Client::connect(&comp.socket);
    let reply = client.call_with(&contract, CONFIGURE_OUTPUTS, &json!({ "configs": {} }));
    assert_eq!(reply, json!({}));
    assert_eq!(checks(&comp.capd), [format!("Check:{CONFIGURE_OUTPUTS}")]);
    assert_eq!(checks(&comp.sink), ["ConfigureOutputs:{}"]);
}

#[test]
fn a_denied_agent_configure_outputs_is_permission_denied_and_nothing_is_forwarded() {
    let contract = common::contract();
    let comp = gated(true);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    let (name, params) = client.call_error(&contract, CONFIGURE_OUTPUTS, &json!({ "configs": {} }));
    assert_eq!(name, "org.tetsu.comp.output.v1.PermissionDenied");
    assert_eq!(params, json!({}));
    assert_eq!(checks(&comp.capd), [format!("Check:{CONFIGURE_OUTPUTS}")]);
    assert!(checks(&comp.sink).is_empty());
}

#[test]
fn an_owners_configure_outputs_passes_without_capd() {
    let contract = common::contract();
    let comp = gated(false);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    let reply = client.call_with(&contract, CONFIGURE_OUTPUTS, &json!({ "configs": {} }));
    assert_eq!(reply, json!({}));
    assert!(checks(&comp.capd).is_empty());
    assert_eq!(checks(&comp.sink).len(), 1);
}

#[test]
fn an_agents_reads_never_reach_capd() {
    let contract = common::contract();
    let comp = gated(true);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    let reply: GetConfigReply = serde_json::from_value(client.call(&contract, GET_CONFIG)).unwrap();
    assert_eq!(reply, config(""));
    client.send(WATCH_CONFIG, true);
    let initial = client.event(&contract, WATCH_CONFIG);
    assert_eq!(initial["kind"], "initial");
    assert!(checks(&comp.capd).is_empty());
}

#[test]
fn an_owners_set_managed_passes_without_capd() {
    let contract = common::contract();
    let comp = gated(false);
    comp.deny();
    let mut client = Client::connect(&comp.socket);
    let reply = client.call_with(
        &contract,
        SET_MANAGED,
        &json!({ "values": { "layout.gaps_outer": 3 } }),
    );
    assert_eq!(reply, json!({}));
    assert!(checks(&comp.capd).is_empty());
    assert_eq!(checks(&comp.sink).len(), 1);
}
