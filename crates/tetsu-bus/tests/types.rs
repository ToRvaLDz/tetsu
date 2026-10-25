//! The facade's Rust types read and write the contract's own event
//! fixtures unchanged, and map unknown enum values to `Unknown`.

use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_bus::notify::{
    Action, DismissReason, NotifyKind, NotifyReply, PendingEvent, PendingKind, ShowParams, Urgency,
};
use tetsu_bus::session::{SessionEvent, SessionKind};

fn fixture(interface: &str, name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/events")
        .join(interface)
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn round_trips<T>(interface: &str, name: &str) -> T
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let wire = fixture(interface, name);
    let value: T = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&value).unwrap(), wire, "{name}");
    value
}

#[test]
fn notify_replies_match_the_fixtures() {
    let shown: NotifyReply = round_trips("org.tetsu.notify.v1", "NotifyReply.shown");
    assert_eq!(shown, NotifyReply::shown("n-19"));
    let invoked: NotifyReply = round_trips("org.tetsu.notify.v1", "NotifyReply.invoked");
    assert_eq!(invoked, NotifyReply::invoked("open"));
    let dismissed: NotifyReply = round_trips("org.tetsu.notify.v1", "NotifyReply.dismissed");
    assert_eq!(dismissed, NotifyReply::dismissed(DismissReason::Expired));
}

#[test]
fn pending_events_match_the_fixtures() {
    let initial: PendingEvent = round_trips("org.tetsu.notify.v1", "PendingEvent.initial");
    assert_eq!(initial, PendingEvent::initial(2));
    let changed: PendingEvent = round_trips("org.tetsu.notify.v1", "PendingEvent.changed");
    assert_eq!(changed, PendingEvent::changed(3));
}

#[test]
fn session_events_match_the_fixtures() {
    let initial: SessionEvent = round_trips("org.tetsu.comp.session.v1", "SessionEvent.initial");
    assert_eq!(initial.kind, SessionKind::Initial);
    assert_eq!(initial.locked, Some(false));
    let locked: SessionEvent = round_trips("org.tetsu.comp.session.v1", "SessionEvent.locked");
    assert_eq!(locked.kind, SessionKind::Locked);
    let unlocked: SessionEvent = round_trips("org.tetsu.comp.session.v1", "SessionEvent.unlocked");
    assert_eq!(unlocked.kind, SessionKind::Unlocked);
}

#[test]
fn unknown_enum_values_map_to_unknown() {
    let event: SessionEvent = serde_json::from_value(json!({ "kind": "suspended" })).unwrap();
    assert_eq!(event.kind, SessionKind::Unknown);
    let reply: NotifyReply = serde_json::from_value(json!({ "kind": "snoozed" })).unwrap();
    assert_eq!(reply.kind, NotifyKind::Unknown);
    let event: PendingEvent = serde_json::from_value(json!({ "kind": "x", "count": 1 })).unwrap();
    assert_eq!(event.kind, PendingKind::Unknown);
}

#[test]
fn show_parameters_parse_with_and_without_optionals() {
    let full: ShowParams = serde_json::from_value(json!({
        "title": "Install firefox?",
        "body": "From the agent",
        "urgency": "critical",
        "actions": [{ "id": "approve", "label": "Approve" }, { "id": "deny", "label": "Deny" }],
        "timeout_ms": 5000,
        "future_field": 1
    }))
    .unwrap();
    assert_eq!(full.title, "Install firefox?");
    assert_eq!(full.urgency, Some(Urgency::Critical));
    assert_eq!(
        full.actions.as_deref(),
        Some(
            &[
                Action::new("approve", "Approve"),
                Action::new("deny", "Deny")
            ][..]
        )
    );
    assert_eq!(full.timeout_ms, Some(5000));

    let bare: ShowParams = serde_json::from_value(json!({ "title": "Hi" })).unwrap();
    assert_eq!(bare.body, None);
    assert_eq!(bare.urgency, None);
    assert_eq!(bare.actions, None);
    assert_eq!(bare.timeout_ms, None);
}
