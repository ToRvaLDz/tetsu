//! The `org.tetsu.comp.render.v1` Rust types read and write the contract's
//! own event fixtures unchanged, and what they write passes the contract.

use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_bus::render::{
    DropReason, ExplicitSync, GetRenderReply, PendingDevice, Render, RenderDevice, RenderEvent,
    RenderKind, SetExplicitSyncParams, SetRenderDeviceParams, SetRenderReply, SyncMode,
};
use tetsu_bus_contract::{Contract, check_reply, load_dir};

const INTERFACE: &str = "org.tetsu.comp.render.v1";

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

fn round_trips(name: &str) -> RenderEvent {
    let wire = fixture(name);
    let event: RenderEvent = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&event).unwrap(), wire, "{name}");
    event
}

fn check(method: &str, reply: &Value) {
    let found = check_reply(&contract(), &format!("{INTERFACE}.{method}"), reply);
    assert!(found.is_empty(), "{method}: {found:?} for {reply}");
}

fn device() -> RenderDevice {
    RenderDevice {
        id: "dev-1".into(),
        node: "/dev/dri/renderD128".into(),
        driver: "i915".into(),
        driver_version: None,
        name: Some("Intel Iris Xe Graphics".into()),
        output_ids: vec!["out-7".into()],
        power_mw: Some(4200),
        switch_failures: 0,
    }
}

fn render() -> Render {
    Render {
        device_id: "dev-1".into(),
        automatic: true,
        devices: vec![device()],
        explicit_sync: ExplicitSync {
            mode: SyncMode::Auto,
            supported: true,
            enabled: true,
            reason: None,
        },
        pending_device: None,
        pending_explicit_sync: None,
    }
}

fn pending() -> Render {
    Render {
        automatic: false,
        pending_device: Some(PendingDevice {
            automatic: false,
            device_id: Some("dev-2".into()),
        }),
        pending_explicit_sync: Some(SyncMode::On),
        ..render()
    }
}

#[test]
fn the_events_match_the_fixtures() {
    let initial = round_trips("RenderEvent.initial");
    assert_eq!(initial, RenderEvent::initial(render()));
    let changed = round_trips("RenderEvent.changed");
    assert_eq!(changed, RenderEvent::changed(pending()));
    let dropped = round_trips("RenderEvent.pending_dropped");
    assert_eq!(
        dropped,
        RenderEvent::pending_dropped(DropReason::DeviceGone)
    );
}

#[test]
fn an_unknown_kind_or_reason_maps_to_unknown() {
    let event: RenderEvent =
        serde_json::from_value(json!({ "kind": "moved", "reason": "bored" })).unwrap();
    assert_eq!(event.kind, RenderKind::Unknown);
    assert_eq!(event.reason, Some(DropReason::Unknown));
    let sync: ExplicitSync = serde_json::from_value(
        json!({ "mode": "maybe", "supported": false, "enabled": false, "reason": null }),
    )
    .unwrap();
    assert_eq!(sync.mode, SyncMode::Unknown);
}

#[test]
fn the_nullable_fields_are_written_as_null_never_omitted() {
    let bare = RenderDevice {
        name: None,
        power_mw: None,
        ..device()
    };
    let wire = serde_json::to_value(&bare).unwrap();
    for key in ["driver_version", "name", "power_mw"] {
        assert_eq!(wire.get(key), Some(&Value::Null), "{key}");
    }
    let wire = serde_json::to_value(render()).unwrap();
    for key in ["pending_device", "pending_explicit_sync"] {
        assert_eq!(wire.get(key), Some(&Value::Null), "{key}");
    }
    assert_eq!(wire["explicit_sync"].get("reason"), Some(&Value::Null));
    let wire = serde_json::to_value(PendingDevice {
        automatic: true,
        device_id: None,
    })
    .unwrap();
    assert_eq!(wire.get("device_id"), Some(&Value::Null));
}

#[test]
fn the_replies_pass_the_contract() {
    let reply = GetRenderReply { render: pending() };
    check("GetRender", &serde_json::to_value(&reply).unwrap());
    let reply = GetRenderReply {
        render: Render::default(),
    };
    check("GetRender", &serde_json::to_value(&reply).unwrap());
    for pending in [true, false] {
        let reply = serde_json::to_value(SetRenderReply { pending }).unwrap();
        assert_eq!(reply, json!({ "pending": pending }));
        check("SetRenderDevice", &reply);
        check("SetExplicitSync", &reply);
    }
    let events = [
        RenderEvent::initial(render()),
        RenderEvent::changed(pending()),
        RenderEvent::pending_dropped(DropReason::DeviceGone),
        RenderEvent::pending_dropped(DropReason::OverriddenByUser),
    ];
    for event in events {
        let reply = json!({ "event": serde_json::to_value(&event).unwrap() });
        check("WatchRender", &reply);
    }
}

#[test]
fn set_render_device_params_read_the_wire_form() {
    let absent: SetRenderDeviceParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(absent.device_id, None);
    let null: SetRenderDeviceParams = serde_json::from_value(json!({ "device_id": null })).unwrap();
    assert_eq!(null.device_id, None);
    let named: SetRenderDeviceParams = serde_json::from_value(json!({ "device_id": "x" })).unwrap();
    assert_eq!(named.device_id.as_deref(), Some("x"));
    assert!(serde_json::from_value::<SetRenderDeviceParams>(json!({ "device_id": 3 })).is_err());
}

#[test]
fn set_explicit_sync_params_read_only_the_three_modes() {
    for (text, mode) in [
        ("auto", SyncMode::Auto),
        ("on", SyncMode::On),
        ("off", SyncMode::Off),
    ] {
        let params: SetExplicitSyncParams =
            serde_json::from_value(json!({ "mode": text })).unwrap();
        assert_eq!(params.mode, mode);
    }
    assert!(serde_json::from_value::<SetExplicitSyncParams>(json!({ "mode": "maybe" })).is_err());
    assert!(serde_json::from_value::<SetExplicitSyncParams>(json!({})).is_err());
}
