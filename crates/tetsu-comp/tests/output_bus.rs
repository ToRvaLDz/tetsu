//! `ConfigureOutputs` on the bus thread, over a real socket: the call
//! reaches the loop as a command and the loop's answer comes back.

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::Client;
use serde_json::{Value, json};
use tetsu_comp::bus::{Command, CommandSink, Publisher, Snapshot};
use tetsu_comp::model::Model;
use tetsu_comp::outputs::configure::ConfigureError;

const CONFIGURE: &str = "org.tetsu.comp.output.v1.ConfigureOutputs";

fn socket(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("tetsu/comp/wayland-test")
}

fn start(dir: &tempfile::TempDir, commands: CommandSink) -> Publisher {
    Publisher::spawn_with(
        &socket(dir),
        Snapshot::of(&Model::default()),
        Some(commands),
    )
    .unwrap()
}

#[test]
fn the_call_reaches_the_sink_with_its_configs_and_an_ok_is_an_empty_reply() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let log = seen.clone();
    let commands: CommandSink = Arc::new(move |command| {
        let Command::ConfigureOutputs(call) = command else {
            panic!("expected ConfigureOutputs");
        };
        log.lock().unwrap().push(json!(call.configs));
        let _ = call.reply.send(Ok(()));
        Ok(())
    });
    let _publisher = start(&dir, commands);
    let mut client = Client::connect(&socket(&dir));
    let params = json!({ "configs": { "out-1": {
        "scale": 2.0, "x": 0,
        "mode": { "width": 1920, "height": 1080, "refresh_mhz": 60000 }
    }}});
    let reply = client.call_with(&contract, CONFIGURE, &params);
    assert_eq!(reply, json!({}));
    let seen = seen.lock().unwrap();
    let config = &seen[0]["out-1"];
    assert_eq!(config["scale"], json!(2.0));
    assert_eq!(config["x"], json!(0));
    assert_eq!(config["y"], Value::Null);
    assert_eq!(config["mode"]["refresh_mhz"], json!(60000));
}

#[test]
fn an_error_from_the_loop_comes_back_verbatim() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(|command| {
        let Command::ConfigureOutputs(call) = command else {
            panic!("expected ConfigureOutputs");
        };
        let _ = call
            .reply
            .send(Err(ConfigureError::NotFound("out-9".into()).reply()));
        Ok(())
    });
    let _publisher = start(&dir, commands);
    let mut client = Client::connect(&socket(&dir));
    let (name, params) =
        client.call_error(&contract, CONFIGURE, &json!({ "configs": { "out-9": {} } }));
    assert_eq!(name, "org.tetsu.comp.output.v1.OutputNotFound");
    assert_eq!(params, json!({ "id": "out-9" }));
}

#[test]
fn bad_parameters_are_an_invalid_parameter() {
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(|_| panic!("nothing should be forwarded"));
    let _publisher = start(&dir, commands);
    let mut client = Client::connect(&socket(&dir));
    client.send_with(CONFIGURE, &json!({ "configs": 3 }), false);
    let reply = client.recv();
    assert_eq!(reply["error"], "org.varlink.service.InvalidParameter");
    assert_eq!(reply["parameters"]["parameter"], "configs");
}

#[test]
fn a_loop_that_is_gone_is_a_state_write_failure() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(Err);
    let _publisher = start(&dir, commands);
    let mut client = Client::connect(&socket(&dir));
    let (name, _) = client.call_error(&contract, CONFIGURE, &json!({ "configs": {} }));
    assert_eq!(name, "org.tetsu.comp.config.v1.StateWriteFailed");
}

#[test]
fn a_dropped_reply_is_a_state_write_failure() {
    let contract = common::contract();
    let dir = tempfile::tempdir().unwrap();
    let commands: CommandSink = Arc::new(|_| Ok(()));
    let _publisher = start(&dir, commands);
    let mut client = Client::connect(&socket(&dir));
    let (name, _) = client.call_error(&contract, CONFIGURE, &json!({ "configs": {} }));
    assert_eq!(name, "org.tetsu.comp.config.v1.StateWriteFailed");
}
