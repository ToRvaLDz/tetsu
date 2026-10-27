//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};
use tetsu_bus::window::{ListWindowsReply, WindowEvent};
use tetsu_bus::workspace::{ListWorkspacesReply, WorkspaceEvent};
use tetsu_bus_contract::{Contract, check_reply, load_dir};

pub fn contract() -> Contract {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    load_dir(&dir).expect("the bus contract loads")
}

/// Fails the test when `reply` does not satisfy `method`'s outputs.
pub fn check(contract: &Contract, method: &str, reply: &Value) {
    let found = check_reply(contract, method, reply);
    assert!(found.is_empty(), "{method}: {found:?} for {reply}");
}

fn wire<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

pub fn check_window_event(contract: &Contract, event: &WindowEvent) {
    let reply = json!({ "event": wire(event) });
    check(contract, "org.tetsu.comp.window.v1.WatchWindows", &reply);
}

pub fn check_workspace_event(contract: &Contract, event: &WorkspaceEvent) {
    let reply = json!({ "event": wire(event) });
    check(
        contract,
        "org.tetsu.comp.workspace.v1.WatchWorkspaces",
        &reply,
    );
}

pub fn check_windows_list(contract: &Contract, list: &ListWindowsReply) {
    check(
        contract,
        "org.tetsu.comp.window.v1.ListWindows",
        &wire(list),
    );
}

pub fn check_workspaces_list(contract: &Contract, list: &ListWorkspacesReply) {
    check(
        contract,
        "org.tetsu.comp.workspace.v1.ListWorkspaces",
        &wire(list),
    );
}
