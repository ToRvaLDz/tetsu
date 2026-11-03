//! The `org.tetsu.comp.config.v1` Rust types read and write the contract's
//! own event fixtures unchanged, and what they write passes the contract.

use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_bus::config::{
    ConfigEntry, ConfigEvent, ConfigKind, Diagnostic, GetConfigReply, Origin, SetManagedParams,
};
use tetsu_bus_contract::{Contract, check_reply, load_dir};

const INTERFACE: &str = "org.tetsu.comp.config.v1";

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

fn round_trips(name: &str) -> ConfigEvent {
    let wire = fixture(name);
    let event: ConfigEvent = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&event).unwrap(), wire, "{name}");
    event
}

fn check(method: &str, reply: &Value) {
    let found = check_reply(&contract(), &format!("{INTERFACE}.{method}"), reply);
    assert!(found.is_empty(), "{method}: {found:?} for {reply}");
}

fn device() -> ConfigEntry {
    ConfigEntry {
        key: "render.device".into(),
        value: json!("dev-1"),
        origin: Origin::Managed,
        default_value: None,
        managed_value: Some(json!("dev-1")),
        user_value: None,
        writable: false,
        method: Some("org.tetsu.comp.render.v1.SetRenderDevice".into()),
    }
}

fn idle() -> ConfigEntry {
    ConfigEntry {
        key: "idle.timeout_ms".into(),
        value: json!(300_000),
        origin: Origin::User,
        default_value: Some(json!(600_000)),
        managed_value: Some(json!(450_000)),
        user_value: Some(json!(300_000)),
        writable: true,
        method: None,
    }
}

fn diagnostic() -> Diagnostic {
    Diagnostic {
        line: Some(12),
        key: Some("idle.timeout_ms".into()),
        message: "expected an integer".into(),
    }
}

#[test]
fn the_events_match_the_fixtures() {
    let initial = round_trips("ConfigEvent.initial");
    assert_eq!(
        initial,
        ConfigEvent::initial(vec![idle(), device()], vec![diagnostic()])
    );
    let changed = round_trips("ConfigEvent.changed");
    assert_eq!(changed, ConfigEvent::changed(device()));
    let diagnostics = round_trips("ConfigEvent.diagnostics");
    assert_eq!(diagnostics.kind, ConfigKind::Diagnostics);
    assert_eq!(diagnostics.diagnostics.as_ref().unwrap().len(), 2);
    let removed = round_trips("ConfigEvent.removed");
    assert_eq!(removed, ConfigEvent::removed("idle.timeout_ms"));
}

#[test]
fn an_unknown_kind_maps_to_unknown() {
    let event: ConfigEvent = serde_json::from_value(json!({ "kind": "moved" })).unwrap();
    assert_eq!(event.kind, ConfigKind::Unknown);
}

#[test]
fn the_nullable_fields_are_written_as_null_never_omitted() {
    let wire = serde_json::to_value(device()).unwrap();
    for key in ["default_value", "user_value"] {
        assert_eq!(wire.get(key), Some(&Value::Null), "{key}");
    }
    let bare = Diagnostic {
        line: None,
        key: None,
        message: "x".into(),
    };
    let wire = serde_json::to_value(&bare).unwrap();
    assert_eq!(wire.get("line"), Some(&Value::Null));
    assert_eq!(wire.get("key"), Some(&Value::Null));
}

#[test]
fn the_replies_pass_the_contract() {
    let reply = GetConfigReply {
        entries: vec![idle(), device()],
        diagnostics: vec![diagnostic()],
    };
    check("GetConfig", &serde_json::to_value(&reply).unwrap());
    let events = [
        ConfigEvent::initial(vec![idle()], vec![]),
        ConfigEvent::changed(device()),
        ConfigEvent::diagnostics(vec![diagnostic()]),
        ConfigEvent::removed("idle.timeout_ms"),
    ];
    for event in events {
        let reply = json!({ "event": serde_json::to_value(&event).unwrap() });
        check("WatchConfig", &reply);
    }
}

#[test]
fn set_managed_params_read_the_wire_form() {
    let params: SetManagedParams =
        serde_json::from_value(json!({ "values": { "a": 1 }, "remove": ["b"] })).unwrap();
    assert_eq!(params.values["a"], json!(1));
    assert_eq!(params.remove, Some(vec!["b".to_owned()]));
    let bare: SetManagedParams = serde_json::from_value(json!({ "values": {} })).unwrap();
    assert!(bare.remove.is_none());
}
