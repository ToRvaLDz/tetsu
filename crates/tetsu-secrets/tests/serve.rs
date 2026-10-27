//! `org.tetsu.secrets.v1` over a real Unix socket in a tempdir. Every reply
//! and error is checked against the live IDL, and every raw byte the server
//! sends is scanned for the hidden value.

mod common;

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use common::{Rig, SENTINEL, with_server};
use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{Handler, Request, Response};
use tetsu_bus_contract::{Contract, check_error, check_reply, load_dir};
use tetsu_secrets::audit::{Action, Outcome};
use tetsu_secrets::service::SecretsService;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const TIMEOUT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(200);

#[derive(Debug)]
struct Reply {
    parameters: Value,
    continues: bool,
    error: Option<String>,
}

struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
    /// Every byte received, for the sentinel scan.
    raw: Vec<u8>,
    contract: Rc<Contract>,
}

impl Client {
    async fn connect(path: &Path, contract: &Rc<Contract>) -> Self {
        Self {
            stream: UnixStream::connect(path).await.unwrap(),
            buffer: Vec::new(),
            raw: Vec::new(),
            contract: contract.clone(),
        }
    }

    async fn send(&mut self, method: &str, parameters: Value, more: bool) {
        let call = json!({ "method": method, "parameters": parameters, "more": more });
        let mut bytes = serde_json::to_vec(&call).unwrap();
        bytes.push(0);
        self.stream.write_all(&bytes).await.unwrap();
    }

    async fn message(&mut self, within: Duration) -> Option<Value> {
        loop {
            if let Some(end) = self.buffer.iter().position(|b| *b == 0) {
                let message: Vec<u8> = self.buffer.drain(..=end).collect();
                return Some(serde_json::from_slice(&message[..end]).unwrap());
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(within, self.stream.read(&mut chunk))
                .await
                .ok()?
                .unwrap();
            assert!(n > 0, "server closed the connection");
            self.buffer.extend_from_slice(&chunk[..n]);
            self.raw.extend_from_slice(&chunk[..n]);
        }
    }

    async fn recv(&mut self, method: &str) -> Reply {
        let message = self.message(TIMEOUT).await.expect("a reply in time");
        let parameters = message.get("parameters").cloned().unwrap_or(json!({}));
        let error = message["error"].as_str().map(str::to_owned);
        let found = match &error {
            Some(name) => check_error(&self.contract, name, &parameters),
            None if method.starts_with("org.varlink.service.") => vec![],
            None => check_reply(&self.contract, method, &parameters),
        };
        assert_eq!(found, vec![], "{message}");
        Reply {
            parameters,
            continues: message["continues"] == true,
            error,
        }
    }

    async fn call(&mut self, method: &str, parameters: Value) -> Reply {
        let method = fqn(method);
        self.send(&method, parameters, false).await;
        self.recv(&method).await
    }

    async fn quiet(&mut self) {
        assert_eq!(self.message(QUIET).await, None);
    }

    fn assert_no_sentinel(&self) {
        let needle = SENTINEL.as_bytes();
        assert!(
            !self.raw.windows(needle.len()).any(|w| w == needle),
            "the server sent the hidden value"
        );
    }
}

fn fqn(method: &str) -> String {
    if method.starts_with("org.varlink.") {
        method.to_owned()
    } else {
        format!("org.tetsu.secrets.v1.{method}")
    }
}

fn contract() -> Rc<Contract> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    Rc::new(load_dir(&dir).expect("the bus contract loads"))
}

fn add_params(id: &str) -> Value {
    json!({
        "id": id,
        "kind": "header",
        "header_name": "X-Api-Key",
        "host": "api.example.com",
        "allowed_tools": ["web-fetch"],
        "value": SENTINEL
    })
}

fn error_name(reply: &Reply) -> &str {
    reply.error.as_deref().expect("an error reply")
}

#[test]
fn add_list_rotate_and_remove() {
    with_server(|path, _rig| async move {
        let contract = contract();
        let mut c = Client::connect(&path, &contract).await;
        let added = c.call("Add", add_params("demo")).await;
        assert_eq!(added.error, None);
        let secret = &added.parameters["secret"];
        assert_eq!(secret["id"], "demo");
        assert_eq!(secret["kind"], "header");
        assert_eq!(secret["header_name"], "x-api-key");
        assert_eq!(secret["host"], "api.example.com");
        assert_eq!(secret["allowed_tools"], json!(["web-fetch"]));
        assert_eq!(secret["use_count"], 0);

        let listed = c.call("ListSecrets", json!({})).await;
        assert_eq!(listed.parameters["secrets"], json!([secret]));

        let rotated = c
            .call("Rotate", json!({ "id": "demo", "value": "another-value" }))
            .await;
        assert_eq!(rotated.error, None);
        assert_eq!(rotated.parameters["secret"]["host"], "api.example.com");
        assert!(rotated.parameters["secret"]["rotated_at"].is_string());

        let removed = c.call("Remove", json!({ "id": "demo" })).await;
        assert_eq!(removed.error, None);
        let listed = c.call("ListSecrets", json!({})).await;
        assert_eq!(listed.parameters["secrets"], json!([]));
        c.assert_no_sentinel();
    });
}

#[test]
fn contract_errors_are_declared_ones() {
    with_server(|path, _rig| async move {
        let contract = contract();
        let mut c = Client::connect(&path, &contract).await;
        c.call("Add", add_params("demo")).await;

        let dup = c.call("Add", add_params("demo")).await;
        assert_eq!(error_name(&dup), "org.tetsu.secrets.v1.SecretExists");
        assert_eq!(dup.parameters, json!({ "id": "demo" }));

        for method in ["Rotate", "Remove"] {
            let params = json!({ "id": "ghost", "value": "x" });
            let gone = c.call(method, params).await;
            assert_eq!(error_name(&gone), "org.tetsu.secrets.v1.SecretNotFound");
            assert_eq!(gone.parameters, json!({ "id": "ghost" }));
        }
        c.assert_no_sentinel();
    });
}

#[test]
fn bad_parameters_are_named_and_never_echoed() {
    with_server(|path, rig: Rig| async move {
        let contract = contract();
        let mut c = Client::connect(&path, &contract).await;
        let cases = [
            ("host", json!({ "host": "https://api.example.com" })),
            ("id", json!({ "id": "Upper Case" })),
            ("header_name", json!({ "header_name": null })),
            ("kind", json!({ "kind": "oauth" })),
            ("allowed_tools", json!({ "allowed_tools": ["Not A Tool"] })),
            ("value", json!({ "value": "" })),
            ("value", json!({ "value": 7 })),
        ];
        for (parameter, patch) in cases {
            let mut params = add_params("demo");
            for (key, value) in patch.as_object().unwrap() {
                params[key] = value.clone();
            }
            // The value is the sentinel in every case but the two for `value`.
            let reply = c.call("Add", params).await;
            assert_eq!(error_name(&reply), "org.varlink.service.InvalidParameter");
            assert_eq!(reply.parameters, json!({ "parameter": parameter }));
        }
        let missing = c.call("Add", json!({ "id": "x", "value": SENTINEL })).await;
        assert_eq!(error_name(&missing), "org.varlink.service.InvalidParameter");
        let rotate = c
            .call("Rotate", json!({ "id": "BAD ID", "value": SENTINEL }))
            .await;
        assert_eq!(rotate.parameters, json!({ "parameter": "id" }));
        c.assert_no_sentinel();
        assert!(rig.broker.list(&common_caller()).await.unwrap().is_empty());
    });
}

fn common_caller() -> tetsu_secrets::caller::Caller {
    tetsu_secrets::caller::Caller {
        unit: None,
        pid: 1,
        exe: None,
        comm: "test".into(),
        agent: false,
    }
}

#[test]
fn watch_streams_initial_then_changes() {
    with_server(|path, _rig| async move {
        let contract = contract();
        let mut watcher = Client::connect(&path, &contract).await;
        let method = fqn("WatchSecrets");
        watcher.send(&method, json!({}), true).await;
        let initial = watcher.recv(&method).await;
        assert!(initial.continues);
        assert_eq!(initial.parameters["event"]["kind"], "initial");
        assert_eq!(initial.parameters["event"]["secrets"], json!([]));

        let mut writer = Client::connect(&path, &contract).await;
        writer.call("Add", add_params("demo")).await;
        let added = watcher.recv(&method).await;
        assert_eq!(added.parameters["event"]["kind"], "added");
        assert_eq!(added.parameters["event"]["secret"]["id"], "demo");
        writer
            .call("Rotate", json!({ "id": "demo", "value": "second" }))
            .await;
        let changed = watcher.recv(&method).await;
        assert_eq!(changed.parameters["event"]["kind"], "changed");
        writer.call("Remove", json!({ "id": "demo" })).await;
        let removed = watcher.recv(&method).await;
        assert_eq!(removed.parameters["event"]["kind"], "removed");
        assert_eq!(removed.parameters["event"]["id"], "demo");
        watcher.quiet().await;
        watcher.assert_no_sentinel();
    });
}

#[test]
fn watching_without_more_is_refused() {
    with_server(|path, _rig| async move {
        let contract = contract();
        let mut c = Client::connect(&path, &contract).await;
        let reply = c.call("WatchSecrets", json!({})).await;
        assert_eq!(error_name(&reply), "org.varlink.service.ExpectedMore");
    });
}

#[test]
fn a_dead_journal_is_vault_unavailable() {
    with_server(|path, rig: Rig| async move {
        let contract = contract();
        let mut c = Client::connect(&path, &contract).await;
        rig.sink.fail.set(true);
        let reply = c.call("Add", add_params("demo")).await;
        assert_eq!(error_name(&reply), "org.tetsu.secrets.v1.VaultUnavailable");
        rig.sink.fail.set(false);
        let listed = c.call("ListSecrets", json!({})).await;
        assert_eq!(listed.parameters["secrets"], json!([]));
        c.assert_no_sentinel();
    });
}

#[test]
fn the_service_describes_itself() {
    with_server(|path, _rig| async move {
        let contract = contract();
        let mut c = Client::connect(&path, &contract).await;
        let info = c.call("org.varlink.service.GetInfo", json!({})).await;
        assert_eq!(info.parameters["product"], "tetsu-secrets");
        assert!(
            info.parameters["interfaces"]
                .as_array()
                .unwrap()
                .contains(&json!("org.tetsu.secrets.v1"))
        );
        let described = c
            .call(
                "org.varlink.service.GetInterfaceDescription",
                json!({ "interface": "org.tetsu.secrets.v1" }),
            )
            .await;
        let text = described.parameters["description"].as_str().unwrap();
        assert!(text.contains("interface org.tetsu.secrets.v1"));
        let other = c
            .call(
                "org.varlink.service.GetInterfaceDescription",
                json!({ "interface": "org.nope" }),
            )
            .await;
        assert_eq!(error_name(&other), "org.varlink.service.InterfaceNotFound");
        let unknown = c.call("Nope", json!({})).await;
        assert_eq!(error_name(&unknown), "org.varlink.service.MethodNotFound");
    });
}

fn agent_peer() -> Rc<Peer> {
    Rc::new(Peer {
        uid: 1000,
        gid: 1000,
        pid: 4242,
        cgroup:
            "/user.slice/user-1000.slice/user@1000.service/tetsu-agent.slice/tetsu-agent.service"
                .into(),
        agent: true,
        comm: "tetsu-agent".into(),
    })
}

fn request(method: &str, parameters: Value) -> Request {
    Request {
        method: fqn(method),
        parameters,
        oneway: false,
        more: false,
    }
}

#[test]
fn the_agent_cannot_write_but_can_read() {
    with_server(|_path, rig: Rig| async move {
        let service = SecretsService::new(rig.broker.clone());
        let contract = contract();
        let user = Rc::new(Peer {
            agent: false,
            ..(*agent_peer()).clone()
        });
        let added = service
            .call(user.clone(), request("Add", add_params("demo")))
            .await;
        assert!(matches!(added, Response::Reply(_)), "{added:?}");

        for (method, params) in [
            ("Add", add_params("other")),
            ("Rotate", json!({ "id": "demo", "value": SENTINEL })),
            ("Remove", json!({ "id": "demo" })),
            // Even a call that would be invalid is a denial, not a hint.
            ("Add", json!({ "value": SENTINEL })),
        ] {
            let Response::Error(error) = service.call(agent_peer(), request(method, params)).await
            else {
                panic!("{method} from the agent was not refused");
            };
            assert_eq!(error.error, "org.tetsu.secrets.v1.PermissionDenied");
            assert_eq!(
                check_error(&contract, &error.error, &error.parameters),
                vec![]
            );
        }
        let denied = rig
            .sink
            .entries
            .borrow()
            .iter()
            .filter(|e| e.action == Action::Denied && e.outcome == Outcome::Denied)
            .count();
        assert_eq!(denied, 4);

        let Response::Reply(listed) = service
            .call(agent_peer(), request("ListSecrets", json!({})))
            .await
        else {
            panic!("the agent could not list");
        };
        assert_eq!(listed["secrets"].as_array().unwrap().len(), 1);
        assert_eq!(listed["secrets"][0]["id"], "demo");
        assert!(!listed.to_string().contains(SENTINEL));
    });
}
