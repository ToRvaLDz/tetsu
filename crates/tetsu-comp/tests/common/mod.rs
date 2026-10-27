//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tetsu_bus::output::{ListOutputsReply, OutputEvent};
use tetsu_bus::window::{ListWindowsReply, WindowEvent};
use tetsu_bus::workspace::{ListWorkspacesReply, WorkspaceEvent};
use tetsu_bus_contract::{Contract, check_reply, load_dir};

pub mod comp;

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

pub fn check_output_event(contract: &Contract, event: &OutputEvent) {
    let reply = json!({ "event": wire(event) });
    check(contract, "org.tetsu.comp.output.v1.WatchOutputs", &reply);
}

pub fn check_outputs_list(contract: &Contract, list: &ListOutputsReply) {
    check(
        contract,
        "org.tetsu.comp.output.v1.ListOutputs",
        &wire(list),
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

const TIMEOUT: Duration = Duration::from_secs(10);

/// A raw NUL-framed Varlink client.
pub struct Client {
    pub stream: UnixStream,
    pub buffer: Vec<u8>,
}

impl Client {
    pub fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).unwrap();
        stream.set_read_timeout(Some(TIMEOUT)).unwrap();
        Self {
            stream,
            buffer: Vec::new(),
        }
    }

    pub fn send(&mut self, method: &str, more: bool) {
        let mut call = json!({ "method": method, "parameters": {} });
        if more {
            call["more"] = json!(true);
        }
        let mut bytes = serde_json::to_vec(&call).unwrap();
        bytes.push(0);
        self.stream.write_all(&bytes).unwrap();
    }

    /// The next message, or `None` at end of file.
    pub fn try_recv(&mut self) -> Option<Value> {
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

    pub fn recv(&mut self) -> Value {
        self.try_recv().expect("the server closed the connection")
    }

    /// One reply, checked against `method`'s outputs.
    pub fn call(&mut self, contract: &Contract, method: &str) -> Value {
        self.send(method, false);
        let reply = self.recv();
        let parameters = reply["parameters"].clone();
        check(contract, method, &parameters);
        parameters
    }

    /// The next streamed reply of `method`, checked; returns its event.
    pub fn event(&mut self, contract: &Contract, method: &str) -> Value {
        let reply = self.recv();
        assert_eq!(reply["continues"], true, "{reply}");
        check(contract, method, &reply["parameters"]);
        reply["parameters"]["event"].clone()
    }
}
