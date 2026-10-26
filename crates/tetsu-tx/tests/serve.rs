//! `org.tetsu.tx.v1` over a real Unix socket in a tempdir, with the engine
//! behind a fake that is stepped phase by phase. Every reply and error is
//! checked against the live IDL.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server;
use tetsu_bus::tx as wire;
use tetsu_bus_contract::{Contract, check_error, check_reply, load_dir};
use tetsu_tx::boot::entry::BootState;
use tetsu_tx::serve;
use tetsu_tx::serve::backend::{Backend, StatusView};
use tetsu_tx::serve::policy::Policy;
use tetsu_tx::store::record::{Record, TxKind};
use tetsu_tx::tx::TxError;
use tetsu_tx::tx::engine::{EngineEvent, EnginePhase, Report, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(200);

// ---- the fake engine -----------------------------------------------------

/// One scripted run: the events it reports, then its result. A gated run
/// waits for a token from the test after each event.
struct Run {
    events: Vec<EngineEvent>,
    gated: bool,
    result: Result<Report, TxError>,
}

/// The test's grip on a read of the records: it signals after the scan
/// and returns only once released.
struct ScanHold {
    scanned: Sender<()>,
    release: Receiver<()>,
}

struct Fake {
    runs: Mutex<VecDeque<Run>>,
    calls: Mutex<Vec<Request>>,
    gate: Mutex<Receiver<()>>,
    records: Mutex<Vec<Record>>,
    /// When set, a read of the records reports it has scanned, then waits.
    scan_hold: Mutex<Option<ScanHold>>,
    packages: Mutex<Result<Vec<String>, String>>,
    status: Mutex<Result<StatusView, String>>,
}

impl Backend for Fake {
    fn run(
        &self,
        id: &str,
        request: &Request,
        observer: &(dyn Fn(EngineEvent) + Sync),
    ) -> Result<Report, TxError> {
        let run = self
            .runs
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted run");
        self.calls.lock().unwrap().push(request.clone());
        for event in &run.events {
            observer(*event);
            if run.gated {
                // A dropped gate lets everything through.
                let _ = self.gate.lock().unwrap().recv();
            }
        }
        if let Ok(report) = &run.result {
            let record = Record {
                counter: report.counter,
                id: Some(id.to_owned()),
                kind: Some(report.kind),
                packages: report.packages.clone(),
                started_at: Some(4_000_000_000),
                ..Record::default()
            };
            self.records.lock().unwrap().insert(0, record);
        }
        run.result
    }

    fn transactions(&self) -> Result<Vec<Record>, String> {
        let scan = self.records.lock().unwrap().clone();
        if let Some(hold) = self.scan_hold.lock().unwrap().as_ref() {
            let _ = hold.scanned.send(());
            let _ = hold.release.recv();
        }
        Ok(scan)
    }

    fn user_packages(&self) -> Result<Vec<String>, String> {
        self.packages.lock().unwrap().clone()
    }

    fn status(&self) -> Result<StatusView, String> {
        self.status.lock().unwrap().clone()
    }
}

/// What the test keeps: the fake and the sender of its gate.
struct Rig {
    backend: Arc<Fake>,
    gate: Sender<()>,
}

impl Rig {
    fn new() -> Self {
        let (gate, receiver) = mpsc::channel();
        let backend = Arc::new(Fake {
            runs: Mutex::default(),
            calls: Mutex::default(),
            gate: Mutex::new(receiver),
            records: Mutex::default(),
            scan_hold: Mutex::new(None),
            packages: Mutex::new(Ok(Vec::new())),
            status: Mutex::new(Err("unscripted".into())),
        });
        Self { backend, gate }
    }

    fn script(&self, events: &[EngineEvent], gated: bool, result: Result<Report, TxError>) {
        self.backend.runs.lock().unwrap().push_back(Run {
            events: events.to_vec(),
            gated,
            result,
        });
    }

    fn calls(&self) -> Vec<Request> {
        self.backend.calls.lock().unwrap().clone()
    }
}

fn report(counter: u64, kind: TxKind, packages: &[&str]) -> Report {
    Report {
        counter,
        kind,
        packages: packages.iter().map(|p| (*p).to_owned()).collect(),
        changes: vec![],
        log: vec![],
        warnings: vec![],
    }
}

fn all_phases(counter: u64) -> Vec<EngineEvent> {
    vec![
        EngineEvent::Begun { counter },
        EngineEvent::Phase(EnginePhase::Packages),
        EngineEvent::Phase(EnginePhase::BootEntry),
        EngineEvent::Phase(EnginePhase::Commit),
    ]
}

const PHASES: [&str; 4] = ["snapshot", "packages", "boot_entry", "commit"];

// ---- policies ------------------------------------------------------------

/// Allows or denies everything and remembers the classes it was asked.
struct Fixed {
    allow: bool,
    asked: Mutex<Vec<String>>,
}

impl Fixed {
    fn new(allow: bool) -> Rc<Self> {
        Rc::new(Self {
            allow,
            asked: Mutex::default(),
        })
    }
}

/// The policy the server gets; the test keeps the `Fixed` to read it.
struct Shared(Rc<Fixed>);

impl Policy for Shared {
    async fn allows(&self, _peer: &Peer, class: &str) -> bool {
        self.0.asked.lock().unwrap().push(class.to_owned());
        self.0.allow
    }
}

// ---- the client ----------------------------------------------------------

/// What came back for one call.
#[derive(Debug)]
struct Reply {
    parameters: Value,
    continues: bool,
    error: Option<String>,
}

struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
    contract: Rc<Contract>,
}

impl Client {
    async fn connect(path: &Path, contract: &Rc<Contract>) -> Self {
        Self {
            stream: UnixStream::connect(path).await.unwrap(),
            buffer: Vec::new(),
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
        }
    }

    /// The next reply to a call of `method`, checked against the IDL.
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

    /// Nothing arrives for a while.
    async fn quiet(&mut self) {
        assert_eq!(self.message(QUIET).await, None);
    }

    /// One call without `more`, one reply.
    async fn call(&mut self, method: &str, parameters: Value) -> Reply {
        self.send(method, parameters, false).await;
        self.recv(method).await
    }

    /// A call with `more`, every reply up to the last one.
    async fn stream(&mut self, method: &str, parameters: Value) -> Vec<Reply> {
        self.send(method, parameters, true).await;
        let mut replies = Vec::new();
        loop {
            let reply = self.recv(method).await;
            let last = !reply.continues;
            replies.push(reply);
            if last {
                return replies;
            }
        }
    }
}

fn fqn(method: &str) -> String {
    format!("org.tetsu.tx.v1.{method}")
}

fn contract() -> Rc<Contract> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    Rc::new(load_dir(&dir).expect("the bus contract loads"))
}

/// Runs `test` against a server bound in a fresh directory.
fn with_server<P, F, Fut>(rig: &Rig, policy: P, test: F)
where
    P: Policy,
    F: FnOnce(PathBuf, Rc<Contract>) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tetsu").join("org.tetsu.tx.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let listener = server::bind_system(&path).unwrap();
        let backend = rig.backend.clone();
        let server = tokio::task::spawn_local(async move {
            serve::serve(listener, backend, Rc::new(policy)).await
        });
        test(path, contract()).await;
        server.abort();
    });
    // Runs left waiting at a gate go on: dropping the runtime waits for them.
    for _ in 0..8 {
        let _ = rig.gate.send(());
    }
    drop(runtime);
}

fn allow() -> Shared {
    Shared(Fixed::new(true))
}

/// The id of the one running transaction, through `ListTransactions`.
async fn running_id(client: &mut Client) -> String {
    let reply = client.call(&fqn("ListTransactions"), json!({})).await;
    let first = &reply.parameters["transactions"][0];
    assert_eq!(first["state"], "running", "{reply:?}");
    first["id"].as_str().unwrap().to_owned()
}

/// The `kind` of a watch reply's event, with the event.
fn event(reply: &Reply) -> (&str, &Value) {
    let event = &reply.parameters["event"];
    (event["kind"].as_str().unwrap(), event)
}

// ---- the service basics --------------------------------------------------

#[test]
fn get_info_lists_the_interface_and_the_description_is_the_idl() {
    let rig = Rig::new();
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        let info = client.call("org.varlink.service.GetInfo", json!({})).await;
        assert_eq!(
            info.parameters["interfaces"],
            json!(["org.varlink.service", wire::INTERFACE])
        );
        let description = client
            .call(
                "org.varlink.service.GetInterfaceDescription",
                json!({ "interface": wire::INTERFACE }),
            )
            .await;
        assert_eq!(description.parameters["description"], wire::IDL);
        let other = client
            .call(
                "org.varlink.service.GetInterfaceDescription",
                json!({ "interface": "org.example.v1" }),
            )
            .await;
        assert_eq!(
            other.error.as_deref(),
            Some("org.varlink.service.InterfaceNotFound")
        );
    });
}

#[test]
fn an_unknown_method_is_method_not_found() {
    let rig = Rig::new();
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        let reply = client.call(&fqn("Nope"), json!({})).await;
        assert_eq!(
            reply.error.as_deref(),
            Some("org.varlink.service.MethodNotFound")
        );
        assert_eq!(reply.parameters["method"], fqn("Nope"));
    });
}

#[test]
fn the_aur_methods_are_not_implemented_yet() {
    let rig = Rig::new();
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        for (method, params) in [
            ("ReviewAurPackages", json!({ "names": ["yay"] })),
            (
                "InstallFromAur",
                json!({ "packages": [{ "name": "yay", "commit": "abc" }] }),
            ),
        ] {
            client.send(&fqn(method), params, true).await;
            let reply = client.recv(&fqn(method)).await;
            assert_eq!(
                reply.error.as_deref(),
                Some("org.varlink.service.MethodNotImplemented"),
                "{method}"
            );
            assert_eq!(reply.parameters["method"], fqn(method));
        }
    });
    assert!(rig.calls().is_empty());
}

// ---- write methods -------------------------------------------------------

#[test]
fn every_write_method_streams_its_phases_then_the_transaction() {
    let cases: [(&str, Value, Request, TxKind, &str); 4] = [
        (
            "Update",
            json!({}),
            Request::Update,
            TxKind::Update,
            "update",
        ),
        (
            "Install",
            json!({ "packages": ["htop"] }),
            Request::Install(vec!["htop".into()]),
            TxKind::Install,
            "install",
        ),
        (
            "Remove",
            json!({ "packages": ["htop"] }),
            Request::Remove(vec!["htop".into()]),
            TxKind::Remove,
            "remove",
        ),
        (
            "Rollback",
            json!({ "to": "7" }),
            Request::Rollback(Some(7)),
            TxKind::Rollback,
            "rollback",
        ),
    ];
    for (method, params, request, kind, wire_kind) in cases {
        let rig = Rig::new();
        rig.script(&all_phases(5), false, Ok(report(5, kind, &["htop"])));
        with_server(&rig, allow(), |path, contract| async move {
            let mut client = Client::connect(&path, &contract).await;
            let replies = client.stream(&fqn(method), params).await;
            assert_eq!(replies.len(), 5, "{method}: {replies:?}");
            for (reply, phase) in replies.iter().zip(PHASES) {
                assert!(reply.continues, "{method}");
                assert_eq!(reply.parameters["progress"]["phase"], phase, "{method}");
            }
            let last = &replies[4].parameters["transaction"];
            assert_eq!(last["state"], "succeeded", "{method}");
            assert_eq!(last["kind"], wire_kind);
            assert_eq!(last["snapshot"], "5");
            assert!(last["id"].as_str().unwrap().starts_with("tx-"));
            // The connection serves the next call.
            let list = client.call(&fqn("ListTransactions"), json!({})).await;
            assert_eq!(list.parameters["transactions"][0]["id"], last["id"]);
        });
        assert_eq!(rig.calls(), [request], "{method}");
    }
}

#[test]
fn the_policy_is_asked_with_the_class_of_each_method() {
    let rig = Rig::new();
    let asked = Fixed::new(false);
    with_server(&rig, Shared(asked.clone()), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        for (method, params) in [
            ("Update", json!({})),
            ("Install", json!({ "packages": ["htop"] })),
            ("Remove", json!({ "packages": ["htop"] })),
            ("Rollback", json!({})),
        ] {
            client.send(&fqn(method), params, true).await;
            let reply = client.recv(&fqn(method)).await;
            assert_eq!(reply.error.as_deref(), Some(wire::PERMISSION_DENIED));
            assert_eq!(reply.parameters, json!({}));
        }
    });
    assert_eq!(
        *asked.asked.lock().unwrap(),
        [
            "system.update",
            "packages.install",
            "packages.remove",
            "system.rollback"
        ]
    );
    assert!(rig.calls().is_empty(), "a denied call runs nothing");
}

#[test]
fn a_denied_caller_still_reads_and_watches() {
    let rig = Rig::new();
    *rig.backend.status.lock().unwrap() = Ok(StatusView {
        booted: 1,
        default: 1,
        default_boot: BootState::Good,
    });
    with_server(
        &rig,
        Shared(Fixed::new(false)),
        |path, contract| async move {
            let mut client = Client::connect(&path, &contract).await;
            let list = client.call(&fqn("ListTransactions"), json!({})).await;
            assert_eq!(list.parameters["transactions"], json!([]));
            let status = client.call(&fqn("GetStatus"), json!({})).await;
            assert_eq!(status.parameters["default_boot"], "good");
            client
                .send(&fqn("WatchTransactions"), json!({}), true)
                .await;
            let initial = client.recv(&fqn("WatchTransactions")).await;
            assert_eq!(event(&initial).0, "initial");
        },
    );
}

#[test]
fn a_write_without_more_is_refused_and_runs_nothing() {
    let rig = Rig::new();
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        for (method, params) in [
            ("Update", json!({})),
            ("Install", json!({ "packages": ["htop"] })),
            ("Remove", json!({ "packages": ["htop"] })),
            ("Rollback", json!({})),
        ] {
            let reply = client.call(&fqn(method), params).await;
            assert_eq!(
                reply.error.as_deref(),
                Some("org.varlink.service.ExpectedMore"),
                "{method}"
            );
        }
    });
    assert!(rig.calls().is_empty());
}

#[test]
fn a_rollback_to_a_bad_id_never_reaches_the_engine() {
    let rig = Rig::new();
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        for bad in ["x", "012", ""] {
            client
                .send(&fqn("Rollback"), json!({ "to": bad }), true)
                .await;
            let reply = client.recv(&fqn("Rollback")).await;
            assert_eq!(reply.error.as_deref(), Some(wire::SNAPSHOT_NOT_FOUND));
            assert_eq!(reply.parameters, json!({ "id": bad }));
        }
        client
            .send(&fqn("Rollback"), json!({ "to": 5 }), true)
            .await;
        let reply = client.recv(&fqn("Rollback")).await;
        assert_eq!(
            reply.error.as_deref(),
            Some("org.varlink.service.InvalidParameter")
        );
        assert_eq!(reply.parameters["parameter"], "to");
    });
    assert!(rig.calls().is_empty());
}

#[test]
fn bad_parameters_name_the_bad_one() {
    let rig = Rig::new();
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        let cases = [
            ("Install", json!({}), "packages"),
            ("Install", json!({ "packages": "htop" }), "packages"),
            ("Install", json!({ "packages": [1] }), "packages"),
            ("Install", json!({ "packages": [] }), "packages"),
            ("Remove", json!({ "packages": [] }), "packages"),
            ("ListTransactions", json!({ "limit": -1 }), "limit"),
            ("ListTransactions", json!({ "limit": "3" }), "limit"),
            ("ListTransactions", json!({ "limit": 1.5 }), "limit"),
        ];
        for (method, params, parameter) in cases {
            client.send(&fqn(method), params.clone(), true).await;
            let reply = client.recv(&fqn(method)).await;
            assert_eq!(
                reply.error.as_deref(),
                Some("org.varlink.service.InvalidParameter"),
                "{method} {params}"
            );
            assert_eq!(reply.parameters["parameter"], parameter);
        }
    });
    assert!(rig.calls().is_empty());
}

#[test]
fn a_second_write_while_one_runs_names_the_running_one() {
    let rig = Rig::new();
    let begun = [EngineEvent::Begun { counter: 5 }];
    rig.script(&begun, true, Ok(report(5, TxKind::Update, &[])));
    let step = rig.gate.clone();
    with_server(&rig, allow(), |path, contract| async move {
        let mut first = Client::connect(&path, &contract).await;
        first.send(&fqn("Update"), json!({}), true).await;
        let snapshot = first.recv(&fqn("Update")).await;
        assert_eq!(snapshot.parameters["progress"]["phase"], "snapshot");

        let mut second = Client::connect(&path, &contract).await;
        let id = running_id(&mut second).await;
        second
            .send(&fqn("Install"), json!({ "packages": ["htop"] }), true)
            .await;
        let refused = second.recv(&fqn("Install")).await;
        assert_eq!(
            refused.error.as_deref(),
            Some(wire::TRANSACTION_IN_PROGRESS)
        );
        assert_eq!(refused.parameters, json!({ "id": id }));

        step.send(()).unwrap();
        let done = first.recv(&fqn("Update")).await;
        assert_eq!(done.parameters["transaction"]["id"], id);
        assert!(!done.continues);
    });
    assert_eq!(
        rig.calls(),
        [Request::Update],
        "the refused call ran nothing"
    );
}

#[test]
fn a_lock_held_by_the_cli_is_in_progress_with_no_id() {
    let rig = Rig::new();
    rig.script(&[], false, Err(TxError::InProgress));
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        client.send(&fqn("Update"), json!({}), true).await;
        let reply = client.recv(&fqn("Update")).await;
        assert_eq!(reply.error.as_deref(), Some(wire::TRANSACTION_IN_PROGRESS));
        assert_eq!(reply.parameters, json!({ "id": "" }));
        let list = client.call(&fqn("ListTransactions"), json!({})).await;
        assert_eq!(list.parameters["transactions"], json!([]));
    });
}

// ---- failures ------------------------------------------------------------

const NASTY: &str = "pacman: <b>boom</b>\n\u{1b}[31mred\u{1b}[0m & more";

/// A failing run of `Install htop` after `Begun` and the package phase,
/// seen by the caller and by a watcher: (the caller's last reply, the
/// watcher's `finished` transaction).
async fn failing_install(path: &Path, contract: &Rc<Contract>) -> (Reply, Value) {
    let mut watcher = Client::connect(path, contract).await;
    watcher
        .send(&fqn("WatchTransactions"), json!({}), true)
        .await;
    let initial = watcher.recv(&fqn("WatchTransactions")).await;
    assert_eq!(event(&initial).1["running"], json!([]));

    let mut caller = Client::connect(path, contract).await;
    caller
        .send(&fqn("Install"), json!({ "packages": ["htop"] }), true)
        .await;
    for phase in &PHASES[..2] {
        let reply = caller.recv(&fqn("Install")).await;
        assert_eq!(reply.parameters["progress"]["phase"], *phase);
    }
    let last = caller.recv(&fqn("Install")).await;
    assert!(!last.continues);

    let started = watcher.recv(&fqn("WatchTransactions")).await;
    assert_eq!(event(&started).0, "started");
    let id = event(&started).1["transaction"]["id"].clone();
    for phase in &PHASES[..2] {
        let progress = watcher.recv(&fqn("WatchTransactions")).await;
        assert_eq!(event(&progress).0, "progress");
        assert_eq!(event(&progress).1["id"], id);
        assert_eq!(event(&progress).1["progress"]["phase"], *phase);
    }
    let finished = watcher.recv(&fqn("WatchTransactions")).await;
    assert_eq!(event(&finished).0, "finished");
    let tx = event(&finished).1["transaction"].clone();
    assert_eq!(tx["id"], id);
    assert_eq!(tx["state"], "failed");
    (last, tx)
}

fn failing_events() -> [EngineEvent; 2] {
    [
        EngineEvent::Begun { counter: 2 },
        EngineEvent::Phase(EnginePhase::Packages),
    ]
}

#[test]
fn a_missing_package_ends_the_stream_with_its_error() {
    let rig = Rig::new();
    rig.script(
        &failing_events(),
        false,
        Err(TxError::PackageNotFound("nope".into())),
    );
    with_server(&rig, allow(), |path, contract| async move {
        let (last, tx) = failing_install(&path, &contract).await;
        assert_eq!(last.error.as_deref(), Some(wire::PACKAGE_NOT_FOUND));
        assert_eq!(last.parameters, json!({ "name": "nope" }));
        assert_eq!(tx["failure"], "package not found: nope");
    });
}

#[test]
fn a_base_package_ends_the_stream_with_not_user_package() {
    let rig = Rig::new();
    rig.script(
        &failing_events(),
        false,
        Err(TxError::NotUserPackage("bash".into())),
    );
    with_server(&rig, allow(), |path, contract| async move {
        let (last, _) = failing_install(&path, &contract).await;
        assert_eq!(last.error.as_deref(), Some(wire::NOT_USER_PACKAGE));
        assert_eq!(last.parameters, json!({ "name": "bash" }));
    });
}

#[test]
fn a_failed_transaction_carries_its_text_verbatim() {
    let rig = Rig::new();
    rig.script(&failing_events(), false, Err(TxError::failed(NASTY)));
    with_server(&rig, allow(), |path, contract| async move {
        let (last, tx) = failing_install(&path, &contract).await;
        assert_eq!(last.error.as_deref(), Some(wire::TRANSACTION_FAILED));
        assert_eq!(last.parameters["reason"], NASTY);
        assert_eq!(last.parameters["id"], tx["id"]);
        assert_eq!(tx["failure"], NASTY);
    });
}

// ---- running to the end ----------------------------------------------------

#[test]
fn a_transaction_runs_to_the_end_when_its_caller_leaves() {
    let rig = Rig::new();
    let events = [
        EngineEvent::Begun { counter: 5 },
        EngineEvent::Phase(EnginePhase::Packages),
    ];
    rig.script(&events, true, Ok(report(5, TxKind::Update, &[])));
    let step = rig.gate.clone();
    with_server(&rig, allow(), |path, contract| async move {
        let mut caller = Client::connect(&path, &contract).await;
        caller.send(&fqn("Update"), json!({}), true).await;
        let snapshot = caller.recv(&fqn("Update")).await;
        assert_eq!(snapshot.parameters["progress"]["phase"], "snapshot");
        drop(caller);

        let mut watcher = Client::connect(&path, &contract).await;
        watcher
            .send(&fqn("WatchTransactions"), json!({}), true)
            .await;
        let initial = watcher.recv(&fqn("WatchTransactions")).await;
        let (kind, body) = event(&initial);
        assert_eq!(kind, "initial");
        let running = &body["running"][0];
        assert_eq!(running["state"], "running");
        let id = running["id"].clone();

        step.send(()).unwrap();
        let progress = watcher.recv(&fqn("WatchTransactions")).await;
        assert_eq!(event(&progress).0, "progress");
        assert_eq!(event(&progress).1["id"], id);
        assert_eq!(event(&progress).1["progress"]["phase"], "packages");
        step.send(()).unwrap();
        let finished = watcher.recv(&fqn("WatchTransactions")).await;
        let (kind, body) = event(&finished);
        assert_eq!(kind, "finished");
        assert_eq!(body["transaction"]["id"], id);
        assert_eq!(body["transaction"]["state"], "succeeded");

        // A watching connection answers nothing else: ask on a new one.
        let mut reader = Client::connect(&path, &contract).await;
        let list = reader.call(&fqn("ListTransactions"), json!({})).await;
        assert_eq!(list.parameters["transactions"][0]["id"], id);
        assert_eq!(list.parameters["transactions"][0]["state"], "succeeded");
    });
}

#[test]
fn a_watcher_sees_started_progress_and_finished_after_an_empty_initial() {
    let rig = Rig::new();
    rig.script(&all_phases(3), false, Ok(report(3, TxKind::Update, &[])));
    with_server(&rig, allow(), |path, contract| async move {
        let mut watcher = Client::connect(&path, &contract).await;
        watcher
            .send(&fqn("WatchTransactions"), json!({}), true)
            .await;
        let initial = watcher.recv(&fqn("WatchTransactions")).await;
        assert_eq!(event(&initial).1["running"], json!([]));

        let mut caller = Client::connect(&path, &contract).await;
        caller.stream(&fqn("Update"), json!({})).await;

        let started = watcher.recv(&fqn("WatchTransactions")).await;
        assert_eq!(event(&started).0, "started");
        let id = event(&started).1["transaction"]["id"].clone();
        assert_eq!(event(&started).1["transaction"]["state"], "running");
        for phase in PHASES {
            let progress = watcher.recv(&fqn("WatchTransactions")).await;
            assert_eq!(event(&progress).0, "progress");
            assert_eq!(event(&progress).1["id"], id);
            assert_eq!(event(&progress).1["progress"]["phase"], phase);
        }
        let finished = watcher.recv(&fqn("WatchTransactions")).await;
        assert_eq!(event(&finished).0, "finished");
        assert_eq!(event(&finished).1["transaction"]["state"], "succeeded");
        watcher.quiet().await;
    });
}

// ---- reads -----------------------------------------------------------------

fn record(counter: u64, id: &str, kind: Option<TxKind>, started: u64) -> Record {
    Record {
        counter,
        id: Some(id.to_owned()),
        kind,
        started_at: Some(started),
        ..Record::default()
    }
}

#[test]
fn list_merges_committed_and_failed_newest_first_and_honours_the_limit() {
    let rig = Rig::new();
    *rig.backend.records.lock().unwrap() = vec![
        record(3, "tx-c", Some(TxKind::Install), 3000),
        record(2, "tx-b", Some(TxKind::Update), 2000),
        record(1, "tx-a", None, 1000),
    ];
    rig.script(
        &failing_events(),
        false,
        Err(TxError::PackageNotFound("nope".into())),
    );
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        client
            .send(&fqn("Install"), json!({ "packages": ["nope"] }), true)
            .await;
        loop {
            if !client.recv(&fqn("Install")).await.continues {
                break;
            }
        }
        let ids = |reply: &Reply| -> Vec<String> {
            reply.parameters["transactions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_owned())
                .collect()
        };
        let all = client.call(&fqn("ListTransactions"), json!({})).await;
        let listed = ids(&all);
        assert_eq!(listed.len(), 3, "the record without a kind is skipped");
        assert_eq!(listed[1..], ["tx-c", "tx-b"]);
        let failed = &all.parameters["transactions"][0];
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["failure"], "package not found: nope");

        let two = client
            .call(&fqn("ListTransactions"), json!({ "limit": 2 }))
            .await;
        assert_eq!(ids(&two), [listed[0].clone(), "tx-c".to_owned()]);
        let none = client
            .call(&fqn("ListTransactions"), json!({ "limit": 0 }))
            .await;
        assert_eq!(none.parameters["transactions"], json!([]));
        let null = client
            .call(&fqn("ListTransactions"), json!({ "limit": null }))
            .await;
        assert_eq!(ids(&null).len(), 3);
    });
}

#[test]
fn list_keeps_a_transaction_that_commits_and_clears_during_the_read() {
    let rig = Rig::new();
    rig.script(
        &[EngineEvent::Begun { counter: 7 }],
        true,
        Ok(report(7, TxKind::Update, &[])),
    );
    let step = rig.gate.clone();
    let (scanned, scan_seen) = mpsc::channel();
    let (release, hold) = mpsc::channel();
    *rig.backend.scan_hold.lock().unwrap() = Some(ScanHold {
        scanned,
        release: hold,
    });
    with_server(&rig, allow(), |path, contract| async move {
        let mut caller = Client::connect(&path, &contract).await;
        caller.send(&fqn("Update"), json!({}), true).await;
        caller.recv(&fqn("Update")).await;
        let mut watcher = Client::connect(&path, &contract).await;
        watcher
            .send(&fqn("WatchTransactions"), json!({}), true)
            .await;
        let initial = watcher.recv(&fqn("WatchTransactions")).await;
        let id = event(&initial).1["running"][0]["id"].clone();

        // The list scans the records while the transaction still runs.
        let mut reader = Client::connect(&path, &contract).await;
        reader
            .send(&fqn("ListTransactions"), json!({}), false)
            .await;
        while scan_seen.try_recv().is_err() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // It commits after the scan and leaves the hub before the list resumes.
        step.send(()).unwrap();
        let finished = watcher.recv(&fqn("WatchTransactions")).await;
        assert_eq!(event(&finished).0, "finished");
        release.send(()).unwrap();

        let list = reader.recv(&fqn("ListTransactions")).await;
        let listed = &list.parameters["transactions"];
        assert_eq!(listed.as_array().unwrap().len(), 1, "{list:?}");
        assert_eq!(listed[0]["id"], id);
    });
}

#[test]
fn the_user_layer_and_the_status_are_read_through_the_backend() {
    let rig = Rig::new();
    *rig.backend.packages.lock().unwrap() = Ok(vec!["htop".into(), "vim".into()]);
    *rig.backend.status.lock().unwrap() = Ok(StatusView {
        booted: 1,
        default: 2,
        default_boot: BootState::Trying,
    });
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        let packages = client.call(&fqn("ListUserPackages"), json!({})).await;
        assert_eq!(
            packages.parameters["packages"],
            json!([
                { "name": "htop", "source": "repo", "aur_commit": null },
                { "name": "vim", "source": "repo", "aur_commit": null }
            ])
        );
        let status = client.call(&fqn("GetStatus"), json!({})).await;
        assert_eq!(
            status.parameters,
            json!({ "booted": "1", "default": "2", "default_boot": "untried" })
        );
    });
}

#[test]
fn a_failed_read_is_service_unavailable() {
    let rig = Rig::new();
    *rig.backend.packages.lock().unwrap() = Err("no ESP".into());
    with_server(&rig, allow(), |path, contract| async move {
        let mut client = Client::connect(&path, &contract).await;
        for method in ["ListUserPackages", "GetStatus"] {
            let reply = client.call(&fqn(method), json!({})).await;
            assert_eq!(
                reply.error.as_deref(),
                Some("org.varlink.service.ServiceUnavailable"),
                "{method}"
            );
        }
    });
}
