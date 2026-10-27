//! The facade's Rust types read and write the contract's own event
//! fixtures unchanged, and map unknown enum values to `Unknown`.

use std::path::PathBuf;

use serde_json::{Value, json};
use tetsu_bus::notify::{
    Action, DismissReason, NotifyKind, NotifyReply, PendingEvent, PendingKind, ShowParams, Urgency,
};
use tetsu_bus::session::{SessionEvent, SessionKind};
use tetsu_bus::tx::{
    DefaultBoot, ListParams, PackageChange, PackageSource, PackagesParams, Phase, Progress,
    RollbackParams, Status, Transaction, TxEvent, TxEventKind, TxKind, TxState, UserPackage,
};
use tetsu_bus::window::{Placement, Window, WindowEvent, WindowKind};
use tetsu_bus::workspace::{Workspace, WorkspaceEvent, WorkspaceKind};

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

fn running_tx() -> Transaction {
    Transaction {
        id: "tx-8".into(),
        kind: TxKind::Install,
        packages: vec!["htop".into()],
        state: TxState::Running,
        started_at: "2026-01-15T09:30:00.000Z".into(),
        finished_at: None,
        snapshot: None,
        changes: vec![],
        failure: None,
    }
}

#[test]
fn tx_events_match_the_fixtures() {
    let initial: TxEvent = round_trips("org.tetsu.tx.v1", "TxEvent.initial");
    assert_eq!(initial, TxEvent::initial(vec![running_tx()]));
    let started: TxEvent = round_trips("org.tetsu.tx.v1", "TxEvent.started");
    assert_eq!(started, TxEvent::started(running_tx()));
    let progress: TxEvent = round_trips("org.tetsu.tx.v1", "TxEvent.progress");
    assert_eq!(
        progress,
        TxEvent::progress(
            "tx-8",
            Progress {
                phase: Phase::Packages,
                percent: Some(40),
                message: Some("downloading htop".into()),
            }
        )
    );
    let finished: TxEvent = round_trips("org.tetsu.tx.v1", "TxEvent.finished");
    let done = Transaction {
        state: TxState::Succeeded,
        finished_at: Some("2026-01-15T09:31:12.481Z".into()),
        snapshot: Some("snap-120".into()),
        changes: vec![PackageChange {
            name: "htop".into(),
            from: None,
            to: Some("3.4.1-1".into()),
        }],
        ..running_tx()
    };
    assert_eq!(finished, TxEvent::finished(done));
}

#[test]
fn a_transaction_keeps_its_nullable_fields_as_null() {
    let wire = serde_json::to_value(running_tx()).unwrap();
    assert_eq!(wire["finished_at"], Value::Null);
    assert_eq!(wire["snapshot"], Value::Null);
    assert_eq!(wire["failure"], Value::Null);
}

#[test]
fn tx_unknown_enum_values_map_to_unknown() {
    let event: TxEvent = serde_json::from_value(json!({ "kind": "paused" })).unwrap();
    assert_eq!(event.kind, TxEventKind::Unknown);
    let mut tx = serde_json::to_value(running_tx()).unwrap();
    tx["kind"] = json!("downgrade");
    tx["state"] = json!("paused");
    let tx: Transaction = serde_json::from_value(tx).unwrap();
    assert_eq!(tx.kind, TxKind::Unknown);
    assert_eq!(tx.state, TxState::Unknown);
    let progress: Progress = serde_json::from_value(json!({ "phase": "warp" })).unwrap();
    assert_eq!(progress.phase, Phase::Unknown);
    let package: UserPackage =
        serde_json::from_value(json!({ "name": "a", "source": "git", "aur_commit": null }))
            .unwrap();
    assert_eq!(package.source, PackageSource::Unknown);
    let status: Status =
        serde_json::from_value(json!({ "booted": "1", "default": "2", "default_boot": "maybe" }))
            .unwrap();
    assert_eq!(status.default_boot, DefaultBoot::Unknown);
}

#[test]
fn tx_parameters_parse_with_and_without_optionals() {
    let install: PackagesParams =
        serde_json::from_value(json!({ "packages": ["htop", "vim"], "future_field": 1 })).unwrap();
    assert_eq!(install.packages, ["htop", "vim"]);
    let rollback: RollbackParams = serde_json::from_value(json!({ "to": "7" })).unwrap();
    assert_eq!(rollback.to.as_deref(), Some("7"));
    let newest: RollbackParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(newest.to, None);
    let null: RollbackParams = serde_json::from_value(json!({ "to": null })).unwrap();
    assert_eq!(null.to, None);
    let limited: ListParams = serde_json::from_value(json!({ "limit": 5 })).unwrap();
    assert_eq!(limited.limit, Some(5));
    let all: ListParams = serde_json::from_value(json!({})).unwrap();
    assert_eq!(all.limit, None);
}

fn win() -> Window {
    Window {
        id: "win-42".into(),
        app_id: Some("foot".into()),
        title: Some("tmux".into()),
        pid: Some(4242),
        workspace_id: Some("ws-3".into()),
        placement: Placement::Tiled,
        fullscreen: false,
        xwayland: false,
    }
}

#[test]
fn window_events_match_the_fixtures() {
    const I: &str = "org.tetsu.comp.window.v1";
    let initial: WindowEvent = round_trips(I, "WindowEvent.initial");
    assert_eq!(
        initial,
        WindowEvent::initial(vec![win()], Some("win-42".into()))
    );
    let opened: WindowEvent = round_trips(I, "WindowEvent.opened");
    assert_eq!(opened, WindowEvent::opened(win()));
    let changed: WindowEvent = round_trips(I, "WindowEvent.changed");
    let expected = Window {
        title: Some("tmux: 2 panes".into()),
        fullscreen: true,
        ..win()
    };
    assert_eq!(changed, WindowEvent::changed(expected));
    let closed: WindowEvent = round_trips(I, "WindowEvent.closed");
    assert_eq!(closed, WindowEvent::closed("win-42"));
    let focused: WindowEvent = round_trips(I, "WindowEvent.focused");
    assert_eq!(focused, WindowEvent::focused(None));
}

#[test]
fn a_window_keeps_its_nullable_fields_as_null() {
    let bare = Window {
        app_id: None,
        title: None,
        pid: None,
        workspace_id: None,
        ..win()
    };
    let wire = serde_json::to_value(bare).unwrap();
    for field in ["app_id", "title", "pid", "workspace_id"] {
        assert_eq!(wire[field], Value::Null, "{field}");
    }
    let focused = serde_json::to_value(WindowEvent::focused(Some("win-1".into()))).unwrap();
    assert_eq!(focused, json!({ "kind": "focused", "focused_id": "win-1" }));
}

#[test]
fn window_unknown_values_map_to_unknown() {
    let event: WindowEvent = serde_json::from_value(json!({ "kind": "moved" })).unwrap();
    assert_eq!(event.kind, WindowKind::Unknown);
    let mut wire = serde_json::to_value(win()).unwrap();
    wire["placement"] = json!("scrolling");
    let window: Window = serde_json::from_value(wire).unwrap();
    assert_eq!(window.placement, Placement::Unknown);
}

#[test]
fn workspace_events_match_the_fixtures() {
    const I: &str = "org.tetsu.comp.workspace.v1";
    let dev = Workspace {
        id: "ws-3".into(),
        name: Some("dev".into()),
        output_id: Some("out-7".into()),
        index: 0,
    };
    let unnamed = Workspace {
        id: "ws-4".into(),
        name: None,
        output_id: Some("out-7".into()),
        index: 1,
    };
    let initial: WorkspaceEvent = round_trips(I, "WorkspaceEvent.initial");
    let active = [("out-7".to_owned(), "ws-3".to_owned())].into();
    assert_eq!(
        initial,
        WorkspaceEvent::initial(vec![dev.clone(), unnamed], active, Some("ws-3".into()))
    );
    let added: WorkspaceEvent = round_trips(I, "WorkspaceEvent.added");
    assert_eq!(added, WorkspaceEvent::added(dev.clone()));
    let changed: WorkspaceEvent = round_trips(I, "WorkspaceEvent.changed");
    assert_eq!(
        changed,
        WorkspaceEvent::changed(Workspace { index: 1, ..dev })
    );
    let removed: WorkspaceEvent = round_trips(I, "WorkspaceEvent.removed");
    assert_eq!(removed, WorkspaceEvent::removed("ws-4"));
    let activated: WorkspaceEvent = round_trips(I, "WorkspaceEvent.activated");
    assert_eq!(
        activated,
        WorkspaceEvent::activated(Some("ws-3".into()), "out-7")
    );
    let focused: WorkspaceEvent = round_trips(I, "WorkspaceEvent.focused");
    assert_eq!(focused, WorkspaceEvent::focused(Some("ws-3".into())));
}

#[test]
fn an_activated_event_without_a_workspace_writes_null() {
    let wire = serde_json::to_value(WorkspaceEvent::activated(None, "out-7")).unwrap();
    assert_eq!(
        wire,
        json!({ "kind": "activated", "id": null, "output_id": "out-7" })
    );
}

#[test]
fn workspace_unknown_kind_maps_to_unknown() {
    let event: WorkspaceEvent = serde_json::from_value(json!({ "kind": "renamed" })).unwrap();
    assert_eq!(event.kind, WorkspaceKind::Unknown);
}
