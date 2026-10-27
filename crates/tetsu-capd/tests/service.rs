//! The service: `Handler::call` directly, then over a tempdir socket
//! against a fake `notify.v1` server.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tetsu_bus::cap::{self, EventKind};
use tetsu_bus::notify::NotifyReply;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tetsu_bus_contract::{Contract, check_error, check_reply, load_dir};
use tetsu_capd::audit::RecordingSink;
use tetsu_capd::auth::TX_CGROUP;
use tetsu_capd::clock::SystemClock;
use tetsu_capd::engine::Engine;
use tetsu_capd::notify::BusNotify;
use tetsu_capd::service::CapService;
use tetsu_capd::store::{Layers, Store};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const INSTALL: &str = "org.tetsu.tx.v1.Install";

fn contract() -> Contract {
    load_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl")).unwrap()
}

fn run<F: Future>(test: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, test)
}

fn layers(dir: &TempDir) -> Layers {
    Layers {
        image: dir.path().join("image.toml"),
        managed: dir.path().join("state/managed.toml"),
        user: dir.path().join("user.toml"),
    }
}

fn peer(uid: u32, cgroup: &str, agent: bool) -> Rc<Peer> {
    Rc::new(Peer {
        uid,
        gid: uid,
        pid: 4242,
        cgroup: cgroup.into(),
        agent,
        comm: "x".into(),
    })
}

const SHELL: &str = "/user.slice/user-1000.slice/session-2.scope";
const AGENT: &str =
    "/user.slice/user-1000.slice/user@1000.service/tetsu-agent.slice/tetsu-agent.service";

/// A notify channel that is never there.
mod offline {
    use std::io;

    use tetsu_bus::notify::{NotifyReply, ShowParams};
    use tetsu_capd::ask::{NotifyChannel, ShowEvents};

    pub struct Nothing;

    impl ShowEvents for Nothing {
        async fn next(&mut self) -> io::Result<NotifyReply> {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }

    pub struct Offline;

    impl NotifyChannel for Offline {
        type Events = Nothing;

        async fn show(&self, _: ShowParams) -> io::Result<Nothing> {
            Err(io::ErrorKind::ConnectionRefused.into())
        }

        async fn notice(&self, _: ShowParams) -> io::Result<()> {
            Err(io::ErrorKind::ConnectionRefused.into())
        }
    }
}

type Direct = CapService<offline::Offline, SystemClock, Rc<RecordingSink>>;

fn direct(dir: &TempDir) -> Direct {
    let engine = Engine::new(
        offline::Offline,
        SystemClock,
        Rc::new(RecordingSink::default()),
        Store::new(layers(dir)),
        vec![Duration::ZERO; 3],
    );
    CapService::new(Rc::new(engine), 1000)
}

/// Calls and resolves a pending reply.
async fn call(
    service: &Direct,
    peer: &Rc<Peer>,
    method: &str,
    parameters: Value,
) -> Result<Value, ErrorReply> {
    let request = Request {
        method: method.into(),
        parameters,
        oneway: false,
        more: false,
    };
    match service.call(peer.clone(), request).await {
        Response::Reply(value) => Ok(value),
        Response::Error(error) => Err(error),
        Response::Pending(rx) => rx.await.expect("an answer"),
        other => panic!("unexpected {other:?}"),
    }
}

fn valid_reply(method: &str, reply: &Value) {
    let violations = check_reply(&contract(), method, reply);
    assert!(violations.is_empty(), "{method}: {violations:?}");
}

fn valid_error(error: &ErrorReply) {
    let violations = check_error(&contract(), &error.error, &error.parameters);
    assert!(violations.is_empty(), "{}: {violations:?}", error.error);
}

fn check_args() -> Value {
    json!({
        "peer": { "pid": 4242, "uid": 1000, "unit": "tetsu-agent.service" },
        "method": INSTALL,
        "args": { "packages": ["htop"] }
    })
}

#[test]
fn check_from_the_tx_daemon_passes() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("user.toml"),
            "[class]\n\"packages.install\" = \"off\"\n",
        )
        .unwrap();
        let service = direct(&dir);
        let tx = peer(0, TX_CGROUP, false);
        let reply = call(&service, &tx, "org.tetsu.cap.v1.Check", check_args())
            .await
            .unwrap();
        assert_eq!(reply, json!({ "decision": "deny", "denied_by": "off" }));
        valid_reply("org.tetsu.cap.v1.Check", &reply);
    });
}

#[test]
fn check_from_a_shell_or_the_agent_is_refused() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let service = direct(&dir);
        for caller in [peer(1000, SHELL, false), peer(1000, AGENT, true)] {
            let error = call(&service, &caller, "org.tetsu.cap.v1.Check", check_args())
                .await
                .unwrap_err();
            assert_eq!(error.error, cap::PERMISSION_DENIED);
            valid_error(&error);
        }
    });
}

#[test]
fn set_is_refused_for_the_agent_and_works_for_a_shell() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let service = direct(&dir);
        let params = json!({ "class": "packages.install", "state": "ask" });
        let error = call(
            &service,
            &peer(1000, AGENT, true),
            "org.tetsu.cap.v1.Set",
            params.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.error, cap::PERMISSION_DENIED);
        valid_error(&error);
        assert!(!dir.path().join("state/managed.toml").exists());

        let reply = call(
            &service,
            &peer(1000, SHELL, false),
            "org.tetsu.cap.v1.Set",
            params,
        )
        .await
        .unwrap();
        valid_reply("org.tetsu.cap.v1.Set", &reply);
        assert!(dir.path().join("state/managed.toml").exists());
    });
}

#[test]
fn set_errors_are_the_contract_errors() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("user.toml"),
            "[class]\n\"aur.review\" = \"ask\"\n",
        )
        .unwrap();
        let service = direct(&dir);
        let shell = peer(1000, SHELL, false);
        let set = |p: Value| call(&service, &shell, "org.tetsu.cap.v1.Set", p);

        let e = set(json!({ "class": "made.up", "state": "ask" }))
            .await
            .unwrap_err();
        assert_eq!(
            (e.error.as_str(), &e.parameters["class"]),
            (cap::UNKNOWN_CLASS, &json!("made.up"))
        );
        valid_error(&e);

        let e = set(json!({ "class": "aur.review", "state": "off" }))
            .await
            .unwrap_err();
        assert_eq!(e.error, cap::OVERRIDDEN_BY_USER);
        valid_error(&e);

        let e = set(json!({ "class": "packages.install", "scope": "x", "state": "ask" }))
            .await
            .unwrap_err();
        assert_eq!(e.error, "org.varlink.service.InvalidParameter");
        assert_eq!(e.parameters["parameter"], "scope");
        valid_error(&e);

        let e = set(json!({ "class": "packages.install", "state": "unset" }))
            .await
            .unwrap_err();
        assert_eq!(e.parameters["parameter"], "state");
        let e = set(json!({ "state": "ask" })).await.unwrap_err();
        assert_eq!(e.parameters["parameter"], "class");
    });
}

#[test]
fn intent_checks_the_method_and_answers_empty() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let service = direct(&dir);
        let agent = peer(1000, AGENT, true);
        let e = call(
            &service,
            &agent,
            "org.tetsu.cap.v1.Intent",
            json!({ "method": "org.tetsu.tx.v1.Nope", "reason": "x" }),
        )
        .await
        .unwrap_err();
        assert_eq!(e.error, cap::UNKNOWN_METHOD);
        assert_eq!(e.parameters["method"], "org.tetsu.tx.v1.Nope");
        valid_error(&e);
        let reply = call(
            &service,
            &agent,
            "org.tetsu.cap.v1.Intent",
            json!({ "method": INSTALL, "reason": "x" }),
        )
        .await
        .unwrap();
        valid_reply("org.tetsu.cap.v1.Intent", &reply);
    });
}

#[test]
fn bad_parameters_and_unknown_methods_are_varlink_errors() {
    run(async {
        let dir = tempfile::tempdir().unwrap();
        let service = direct(&dir);
        let tx = peer(0, TX_CGROUP, false);
        for (broken, parameter) in [
            (json!({ "method": INSTALL, "args": {} }), "peer"),
            (
                json!({ "peer": { "pid": 1, "uid": 1, "unit": "u" }, "args": {} }),
                "method",
            ),
            (
                json!({ "peer": { "pid": 1, "uid": 1, "unit": "u" }, "method": INSTALL, "args": 3 }),
                "args",
            ),
        ] {
            let e = call(&service, &tx, "org.tetsu.cap.v1.Check", broken)
                .await
                .unwrap_err();
            assert_eq!(e.error, "org.varlink.service.InvalidParameter");
            assert_eq!(e.parameters["parameter"], parameter);
        }
        let e = call(&service, &tx, "org.tetsu.cap.v1.Nope", json!({}))
            .await
            .unwrap_err();
        assert_eq!(e.error, "org.varlink.service.MethodNotFound");
        let request = Request {
            method: "org.tetsu.cap.v1.WatchCapabilities".into(),
            parameters: Value::Null,
            oneway: false,
            more: false,
        };
        match service.call(tx, request).await {
            Response::Error(e) => assert_eq!(e.error, "org.varlink.service.ExpectedMore"),
            other => panic!("{other:?}"),
        }
    });
}

// Over a socket.

struct Raw(UnixStream, Vec<u8>);

impl Raw {
    async fn connect(path: &Path) -> Self {
        Self(UnixStream::connect(path).await.unwrap(), Vec::new())
    }

    async fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(0);
        self.0.write_all(&bytes).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        loop {
            if let Some(end) = self.1.iter().position(|b| *b == 0) {
                let message: Vec<u8> = self.1.drain(..=end).collect();
                return serde_json::from_slice(&message[..end]).unwrap();
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(TIMEOUT, self.0.read(&mut chunk))
                .await
                .expect("a reply in time")
                .unwrap();
            assert!(n > 0, "the server closed the connection");
            self.1.extend_from_slice(&chunk[..n]);
        }
    }
}

/// The fake notifyd: `Show` sends `shown`, then waits for the test's
/// answer, and reports when its caller goes away.
struct FakeNotifyd {
    shows: mpsc::UnboundedSender<Value>,
    answer: RefCell<Option<oneshot::Receiver<&'static str>>>,
    gone: mpsc::UnboundedSender<()>,
}

impl Handler for FakeNotifyd {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != "org.tetsu.notify.v1.Show" {
            return Response::method_not_found(&request.method);
        }
        self.shows.send(request.parameters.clone()).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(StreamItem::Reply(
            json!({ "reply": NotifyReply::shown("n1") }),
        ))
        .unwrap();
        let answer = self.answer.borrow_mut().take();
        let gone = self.gone.clone();
        tokio::task::spawn_local(async move {
            match answer {
                Some(answer) => tokio::select! {
                    Ok(id) = answer => {
                        let reply = json!({ "reply": NotifyReply::invoked(id) });
                        let _ = tx.send(StreamItem::Last(reply));
                    }
                    () = tx.closed() => {
                        let _ = gone.send(());
                    }
                },
                None => {
                    tx.closed().await;
                    let _ = gone.send(());
                }
            }
        });
        Response::Stream(rx)
    }
}

struct Wired {
    _dir: TempDir,
    cap_path: PathBuf,
    shows: mpsc::UnboundedReceiver<Value>,
    gone: mpsc::UnboundedReceiver<()>,
    answer: Option<oneshot::Sender<&'static str>>,
}

/// Serves capd and a fake notifyd in a fresh tempdir.
fn wire() -> Wired {
    let dir = tempfile::tempdir().unwrap();
    let cap_path = dir.path().join("run/org.tetsu.cap.v1");
    let notify_path = dir.path().join("run/org.tetsu.notify.v1");
    let (shows_tx, shows) = mpsc::unbounded_channel();
    let (gone_tx, gone) = mpsc::unbounded_channel();
    let (answer, answer_rx) = oneshot::channel();
    let fake = FakeNotifyd {
        shows: shows_tx,
        answer: RefCell::new(Some(answer_rx)),
        gone: gone_tx,
    };
    tokio::task::spawn_local(server::serve(
        server::bind(&notify_path).unwrap(),
        Rc::new(fake),
    ));
    let engine = Engine::new(
        BusNotify::new(notify_path),
        SystemClock,
        Rc::new(RecordingSink::default()),
        Store::new(layers(&dir)),
        vec![Duration::ZERO; 3],
    );
    let service = CapService::allow_all_checks(Rc::new(engine), 1000);
    tokio::task::spawn_local(server::serve(
        server::bind(&cap_path).unwrap(),
        Rc::new(service),
    ));
    Wired {
        _dir: dir,
        cap_path,
        shows,
        gone,
        answer: Some(answer),
    }
}

fn check_call() -> Value {
    json!({ "method": "org.tetsu.cap.v1.Check", "parameters": check_args() })
}

#[test]
fn list_and_watch_work_over_the_socket() {
    run(async {
        let w = wire();
        let mut client = cap::Client::connect(&w.cap_path).await.unwrap();
        let list = client.list().await.unwrap();
        assert!(
            list.iter()
                .any(|c| c.class == "packages.install" && c.state == cap::State::Unset)
        );
        let violations = check_reply(
            &contract(),
            "org.tetsu.cap.v1.ListCapabilities",
            &json!({ "capabilities": list }),
        );
        assert!(violations.is_empty(), "{violations:?}");

        let mut watch = cap::Client::connect(&w.cap_path)
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();
        let first = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.kind, EventKind::Initial);
        client
            .set(&cap::SetParams {
                class: "aur.review".into(),
                scope: None,
                state: Some(cap::Grant::Off),
            })
            .await
            .unwrap();
        let second = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.kind, EventKind::Changed);
        assert_eq!(second.capability.unwrap().state, cap::State::Off);
    });
}

#[test]
fn get_info_and_the_interface_description_are_served() {
    run(async {
        let w = wire();
        let mut raw = Raw::connect(&w.cap_path).await;
        raw.send(json!({ "method": "org.varlink.service.GetInfo" }))
            .await;
        let info = raw.recv().await;
        assert_eq!(info["parameters"]["product"], "tetsu-capd");
        assert!(
            info["parameters"]["interfaces"]
                .as_array()
                .unwrap()
                .contains(&json!(cap::INTERFACE))
        );
        raw.send(json!({
            "method": "org.varlink.service.GetInterfaceDescription",
            "parameters": { "interface": cap::INTERFACE }
        }))
        .await;
        assert_eq!(raw.recv().await["parameters"]["description"], cap::IDL);
    });
}

#[test]
fn a_check_waits_for_the_users_answer() {
    run(async {
        let mut w = wire();
        let mut raw = Raw::connect(&w.cap_path).await;
        raw.send(check_call()).await;
        let shown = tokio::time::timeout(TIMEOUT, w.shows.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(shown["title"].as_str().unwrap().contains(INSTALL));
        w.answer.take().unwrap().send("allow_once").unwrap();
        let reply = raw.recv().await;
        assert_eq!(
            reply["parameters"],
            json!({ "decision": "allow", "denied_by": null })
        );
        valid_reply("org.tetsu.cap.v1.Check", &reply["parameters"]);
    });
}

#[test]
fn closing_a_pending_check_withdraws_the_notification() {
    run(async {
        let mut w = wire();
        let mut raw = Raw::connect(&w.cap_path).await;
        raw.send(check_call()).await;
        tokio::time::timeout(TIMEOUT, w.shows.recv())
            .await
            .unwrap()
            .unwrap();
        drop(raw);
        tokio::time::timeout(TIMEOUT, w.gone.recv())
            .await
            .expect("notifyd sees its Show connection close")
            .unwrap();
    });
}
