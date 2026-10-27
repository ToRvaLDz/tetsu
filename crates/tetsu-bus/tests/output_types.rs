//! The `org.tetsu.comp.output.v1` Rust types read and write the contract's
//! own event fixtures unchanged, and what they write passes the contract.

use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_bus::output::{ListOutputsReply, Mode, Output, OutputEvent, OutputKind, Transform};
use tetsu_bus_contract::{Contract, check_reply, load_dir};

const INTERFACE: &str = "org.tetsu.comp.output.v1";

fn contract() -> Contract {
    load_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl")).expect("the contract loads")
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/events")
        .join(INTERFACE)
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn round_trips(name: &str) -> OutputEvent {
    let wire = fixture(name);
    let event: OutputEvent = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&event).unwrap(), wire, "{name}");
    event
}

fn modes() -> Vec<Mode> {
    vec![
        Mode {
            width: 3840,
            height: 2160,
            refresh_mhz: 60000,
            preferred: true,
        },
        Mode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60000,
            preferred: false,
        },
    ]
}

fn output(scale: f64, transform: Transform) -> Output {
    Output {
        id: "out-7".into(),
        name: "DP-1".into(),
        make: Some("LG Electronics".into()),
        model: Some("27UL850".into()),
        serial: Some("0x0004a1b2".into()),
        enabled: true,
        modes: modes(),
        current_mode: Some(0),
        scale,
        transform,
        x: 0,
        y: 0,
    }
}

fn check(method: &str, reply: &Value) {
    let found = check_reply(&contract(), &format!("{INTERFACE}.{method}"), reply);
    assert!(found.is_empty(), "{method}: {found:?} for {reply}");
}

#[test]
fn the_events_match_the_fixtures() {
    let initial = round_trips("OutputEvent.initial");
    assert_eq!(
        initial,
        OutputEvent::initial(vec![output(1.5, Transform::Normal)])
    );
    let added = round_trips("OutputEvent.added");
    assert_eq!(added, OutputEvent::added(output(1.5, Transform::Normal)));
    let changed = round_trips("OutputEvent.changed");
    assert_eq!(
        changed,
        OutputEvent::changed(output(2.0, Transform::Rotate90))
    );
    let removed = round_trips("OutputEvent.removed");
    assert_eq!(removed, OutputEvent::removed("out-7"));
}

#[test]
fn unknown_kind_and_transform_map_to_unknown() {
    let event: OutputEvent = serde_json::from_value(json!({ "kind": "moved" })).unwrap();
    assert_eq!(event.kind, OutputKind::Unknown);
    let mut wire = serde_json::to_value(output(1.0, Transform::Normal)).unwrap();
    wire["transform"] = json!("mirrored");
    let parsed: Output = serde_json::from_value(wire).unwrap();
    assert_eq!(parsed.transform, Transform::Unknown);
}

#[test]
fn the_nullable_fields_are_written_as_null_never_omitted() {
    let mut bare = output(1.0, Transform::Normal);
    bare.make = None;
    bare.model = None;
    bare.serial = None;
    bare.current_mode = None;
    let wire = serde_json::to_value(&bare).unwrap();
    for key in ["make", "model", "serial", "current_mode"] {
        assert_eq!(wire.get(key), Some(&Value::Null), "{key}");
    }
    let parsed: Output = serde_json::from_value(wire).unwrap();
    assert_eq!(parsed, bare);
}

#[test]
fn every_transform_is_written_as_its_idl_name() {
    let names = [
        (Transform::Normal, "normal"),
        (Transform::Rotate90, "rotate_90"),
        (Transform::Rotate180, "rotate_180"),
        (Transform::Rotate270, "rotate_270"),
        (Transform::Flipped, "flipped"),
        (Transform::Flipped90, "flipped_90"),
        (Transform::Flipped180, "flipped_180"),
        (Transform::Flipped270, "flipped_270"),
    ];
    for (transform, name) in names {
        assert_eq!(serde_json::to_value(transform).unwrap(), json!(name));
    }
}

#[test]
fn the_replies_pass_the_contract() {
    let list = ListOutputsReply {
        outputs: vec![output(1.0, Transform::Normal)],
    };
    check("ListOutputs", &serde_json::to_value(&list).unwrap());
    let empty = ListOutputsReply { outputs: vec![] };
    check("ListOutputs", &serde_json::to_value(&empty).unwrap());

    let events = [
        OutputEvent::initial(vec![output(1.0, Transform::Normal)]),
        OutputEvent::added(output(1.0, Transform::Normal)),
        OutputEvent::changed(output(2.0, Transform::Flipped270)),
        OutputEvent::removed("out-7"),
    ];
    for event in events {
        let reply = json!({ "event": serde_json::to_value(&event).unwrap() });
        check("WatchOutputs", &reply);
    }
}
