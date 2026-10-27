//! The per-call gate (#28): an agent's calls are checked by `tetsu-capd`
//! before they reach the server, and every error on the way refuses.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use tetsu_bus::access::{ExpectedServer, Groups, Scope};
use tetsu_bus::cap::{CheckReply, DeniedBy};
use tetsu_bus::endpoint::Endpoint;
use tetsu_bus::gate::{Authority, GateConfig, Gated, RootOnly};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(150);
const OWN: u32 = 1000;
const USER: &str = "/user.slice/user-1000.slice/user@1000.service";
const DO: &str = "org.example.v1.Do";
const STREAM: &str = "org.example.v1.Stream";
const READ: &str = "org.example.v1.GetThing";
const DENIED: &str = "org.example.v1.PermissionDenied";

type Events = Rc<RefCell<Vec<String>>>;

/// The server behind the gate: records its calls.
struct Inner {
    events: Events,
}

impl Handler for Inner {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        self.events
            .borrow_mut()
            .push(format!("inner:{}", request.method));
        if request.method == STREAM {
            let (tx, rx) = mpsc::unbounded_channel();
            tx.send(StreamItem::Last(json!({ "streamed": true })))
                .unwrap();
            Response::Stream(rx)
        } else {
            Response::Reply(json!({ "inner": true }))
        }
    }
}

#[derive(Clone)]
enum Answer {
    Reply(Value),
    Error(&'static str),
    /// The connection is dropped without a reply.
    Close,
    /// Never answers; reports when the caller goes away.
    Hang,
}

/// A fake `org.tetsu.cap.v1` that records each `Check`.
struct FakeCapd {
    events: Events,
    checks: Rc<RefCell<Vec<Request>>>,
    answer: Rc<RefCell<Answer>>,
    gone: mpsc::UnboundedSender<()>,
}

impl Handler for FakeCapd {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        self.events.borrow_mut().push("check".to_owned());
        self.checks.borrow_mut().push(request);
        match self.answer.borrow().clone() {
            Answer::Reply(value) => Response::Reply(value),
            Answer::Error(name) => Response::Error(ErrorReply::new(name, json!({}))),
            Answer::Close => {
                let (_tx, rx) = oneshot::channel::<Result<Value, ErrorReply>>();
                Response::Pending(rx)
            }
            Answer::Hang => {
                let (mut tx, rx) = oneshot::channel::<Result<Value, ErrorReply>>();
                let gone = self.gone.clone();
                tokio::task::spawn_local(async move {
                    tx.closed().await;
                    let _ = gone.send(());
                });
                Response::Pending(rx)
            }
        }
    }
}

struct Wheel(Vec<u32>);

impl Groups for Wheel {
    fn in_wheel(&self, uid: u32) -> bool {
        self.0.contains(&uid)
    }
}

/// An authority that records what it is asked and answers `yes`.
struct Auth {
    asks: Events,
    yes: bool,
}

impl Authority for Auth {
    async fn authorize(&self, peer: &Peer, method: &str) -> bool {
        self.asks
            .borrow_mut()
            .push(format!("{}:{method}", peer.uid));
        self.yes
    }
}

struct Env {
    _dir: TempDir,
    socket: PathBuf,
    events: Events,
    checks: Rc<RefCell<Vec<Request>>>,
    answer: Rc<RefCell<Answer>>,
    gone: mpsc::UnboundedReceiver<()>,
    logs: Events,
    asked_uids: Rc<RefCell<Vec<u32>>>,
    asks: Events,
}

fn own_cgroup() -> String {
    let proc = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

impl Env {
    fn gate(&self, scope: Scope, wheel: &[u32], yes: bool) -> Gated<Inner, Wheel, Auth> {
        self.gate_expecting(scope, wheel, yes, ExpectedServer::Cgroup(own_cgroup()))
    }

    fn gate_expecting(
        &self,
        scope: Scope,
        wheel: &[u32],
        yes: bool,
        expected: ExpectedServer,
    ) -> Gated<Inner, Wheel, Auth> {
        let socket = self.socket.clone();
        let asked = self.asked_uids.clone();
        let logs = self.logs.clone();
        Gated::with(
            Inner {
                events: self.events.clone(),
            },
            GateConfig {
                scope,
                own_uid: OWN,
                capd: Box::new(move |uid| {
                    asked.borrow_mut().push(uid);
                    Endpoint::new(socket.clone(), expected.clone())
                }),
                groups: Wheel(wheel.to_vec()),
                authority: Auth {
                    asks: self.asks.clone(),
                    yes,
                },
                log: Box::new(move |line| logs.borrow_mut().push(line.to_owned())),
            },
        )
    }

    fn set(&self, answer: Answer) {
        *self.answer.borrow_mut() = answer;
    }

    fn events(&self) -> Vec<String> {
        self.events.borrow().clone()
    }
}

fn run<F, Fut>(test: F)
where
    F: FnOnce(Env) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("tetsu").join("org.tetsu.cap.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let events: Events = Rc::default();
        let checks = Rc::default();
        let answer = Rc::new(RefCell::new(Answer::Reply(
            serde_json::to_value(CheckReply::allow()).unwrap(),
        )));
        let (gone_tx, gone) = mpsc::unbounded_channel();
        let capd = Rc::new(FakeCapd {
            events: events.clone(),
            checks: Rc::clone(&checks),
            answer: answer.clone(),
            gone: gone_tx,
        });
        let listener = server::bind(&socket).unwrap();
        tokio::task::spawn_local(server::serve(listener, capd));
        test(Env {
            _dir: dir,
            socket,
            events,
            checks,
            answer,
            gone,
            logs: Rc::default(),
            asked_uids: Rc::default(),
            asks: Rc::default(),
        })
        .await;
    });
}

fn peer(uid: u32, agent: bool) -> Rc<Peer> {
    let cgroup = if agent {
        format!("{USER}/tetsu.slice/tetsu-agent.slice/run-u1.service")
    } else {
        format!("{USER}/app.slice/app-foot-1.scope")
    };
    Rc::new(Peer {
        uid,
        gid: uid,
        pid: 4242,
        cgroup,
        agent,
        comm: "x".to_owned(),
    })
}

fn request(method: &str) -> Request {
    Request {
        method: method.to_owned(),
        parameters: json!({ "what": "this" }),
        oneway: false,
        more: false,
    }
}

/// The final response: a `Deferred` is awaited.
async fn settle(response: Response) -> Response {
    match response {
        Response::Deferred(rx) => tokio::time::timeout(TIMEOUT, rx)
            .await
            .expect("decided in time")
            .expect("a decision was sent"),
        other => other,
    }
}

async fn call<H: Handler>(gate: &H, peer: Rc<Peer>, method: &str) -> Response {
    settle(gate.call(peer, request(method)).await).await
}

fn error_of(response: Response) -> String {
    match response {
        Response::Error(e) => {
            assert_eq!(e.parameters, json!({}));
            e.error
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

fn reply_of(response: Response) -> Value {
    match response {
        Response::Reply(v) => v,
        other => panic!("expected a reply, got {other:?}"),
    }
}

// 1. A session agent is checked by capd before the call runs.

#[test]
fn an_allowed_call_reaches_the_server_after_the_check() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        let response = call(&gate, peer(OWN, true), DO).await;
        assert_eq!(reply_of(response), json!({ "inner": true }));
        assert_eq!(env.events(), ["check", &format!("inner:{DO}")]);
        let checks = env.checks.borrow();
        assert_eq!(checks[0].method, "org.tetsu.cap.v1.Check");
        assert_eq!(
            checks[0].parameters,
            json!({
                "peer": { "pid": 4242, "uid": 1000, "unit": "run-u1.service" },
                "method": DO,
                "args": { "what": "this" },
            })
        );
        assert!(env.logs.borrow().is_empty());
    });
}

#[test]
fn an_allowed_stream_is_returned_unchanged() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        match call(&gate, peer(OWN, true), STREAM).await {
            Response::Stream(mut rx) => {
                let item = rx.recv().await.unwrap();
                assert_eq!(item, StreamItem::Last(json!({ "streamed": true })));
            }
            other => panic!("expected a stream, got {other:?}"),
        }
    });
}

#[test]
fn a_denied_call_is_refused_for_every_reason() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        for by in [
            DeniedBy::Off,
            DeniedBy::User,
            DeniedBy::Suppressed,
            DeniedBy::Unavailable,
        ] {
            env.set(Answer::Reply(
                serde_json::to_value(CheckReply::deny(by)).unwrap(),
            ));
            let response = call(&gate, peer(OWN, true), DO).await;
            assert_eq!(error_of(response), DENIED, "{by:?}");
        }
        assert!(
            env.events().iter().all(|e| e == "check"),
            "{:?}",
            env.events()
        );
        assert_eq!(env.logs.borrow().len(), 4);
    });
}

// 2. Every error on the capd path refuses.

#[test]
fn every_error_on_the_capd_path_refuses() {
    run(|env| async move {
        let cases = [
            (
                "a malformed reply",
                Answer::Reply(json!({ "decision": "maybe" })),
            ),
            ("an empty reply", Answer::Reply(json!({}))),
            (
                "a remote error",
                Answer::Error("org.tetsu.cap.v1.UnknownMethod"),
            ),
            ("capd closing mid-call", Answer::Close),
        ];
        for (name, answer) in cases {
            env.set(answer);
            let gate = env.gate(Scope::Session, &[], false);
            let response = call(&gate, peer(OWN, true), DO).await;
            assert_eq!(error_of(response), DENIED, "{name}");
        }
        assert!(
            env.events().iter().all(|e| e == "check"),
            "{:?}",
            env.events()
        );
        assert_eq!(env.logs.borrow().len(), 4);
    });
}

#[test]
fn a_missing_capd_refuses() {
    run(|env| async move {
        let mut gate_env = env;
        gate_env.socket = gate_env.socket.with_file_name("nobody-home");
        let gate = gate_env.gate(Scope::Session, &[], false);
        let response = call(&gate, peer(OWN, true), DO).await;
        assert_eq!(error_of(response), DENIED);
        assert_eq!(gate_env.events(), Vec::<String>::new());
        assert_eq!(gate_env.logs.borrow().len(), 1);
    });
}

#[test]
fn a_capd_in_the_wrong_cgroup_gets_no_call_and_the_call_is_refused() {
    run(|env| async move {
        let wrong = ExpectedServer::Cgroup("/system.slice/impostor.service".to_owned());
        let gate = env.gate_expecting(Scope::Session, &[], false, wrong);
        let response = call(&gate, peer(OWN, true), DO).await;
        assert_eq!(error_of(response), DENIED);
        tokio::time::sleep(QUIET).await;
        assert_eq!(env.events(), Vec::<String>::new());
    });
}

// 3-5. Who is asked in a session service.

#[test]
fn a_session_agent_reads_without_asking_capd() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        let response = call(&gate, peer(OWN, true), READ).await;
        assert_eq!(reply_of(response), json!({ "inner": true }));
        assert_eq!(env.events(), [format!("inner:{READ}")]);
        assert!(env.asked_uids.borrow().is_empty());
    });
}

#[test]
fn a_session_non_agent_of_the_owner_or_root_is_served_without_capd() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        for uid in [OWN, 0] {
            let response = call(&gate, peer(uid, false), DO).await;
            assert_eq!(reply_of(response), json!({ "inner": true }));
        }
        assert_eq!(env.events().len(), 2);
        assert!(env.checks.borrow().is_empty());
    });
}

#[test]
fn a_session_peer_of_another_user_is_refused_even_on_a_read() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        for method in [DO, READ] {
            for agent in [false, true] {
                let response = call(&gate, peer(1001, agent), method).await;
                assert_eq!(error_of(response), DENIED, "{method} agent={agent}");
            }
        }
        assert_eq!(env.events(), Vec::<String>::new());
        assert_eq!(env.logs.borrow().len(), 4);
    });
}

// 6. System services.

#[test]
fn a_system_read_is_free_for_an_agent_and_for_another_user() {
    run(|env| async move {
        let gate = env.gate(Scope::System, &[], false);
        for p in [peer(OWN, true), peer(1001, false), peer(0, false)] {
            let response = call(&gate, p, READ).await;
            assert_eq!(reply_of(response), json!({ "inner": true }));
        }
        assert!(env.checks.borrow().is_empty());
        assert!(env.asks.borrow().is_empty());
    });
}

#[test]
fn a_system_agent_outside_wheel_is_refused_and_nobody_is_asked() {
    run(|env| async move {
        let gate = env.gate(Scope::System, &[1001], true);
        let response = call(&gate, peer(OWN, true), DO).await;
        assert_eq!(error_of(response), DENIED);
        assert_eq!(env.events(), Vec::<String>::new());
        assert!(env.asks.borrow().is_empty());
        assert!(env.asked_uids.borrow().is_empty());
        assert_eq!(env.logs.borrow().len(), 1);
    });
}

#[test]
fn a_system_agent_in_wheel_is_checked_by_the_capd_of_its_own_user() {
    run(|env| async move {
        let gate = env.gate(Scope::System, &[OWN], false);
        let response = call(&gate, peer(OWN, true), DO).await;
        assert_eq!(reply_of(response), json!({ "inner": true }));
        assert_eq!(env.events(), ["check", &format!("inner:{DO}")]);
        assert_eq!(*env.asked_uids.borrow(), [OWN]);
        assert!(
            env.asks.borrow().is_empty(),
            "polkit is never consulted for an agent"
        );
    });
}

#[test]
fn a_system_agent_in_wheel_uses_its_own_uid_not_the_daemons() {
    run(|env| async move {
        let gate = env.gate(Scope::System, &[1234], false);
        let response = call(&gate, peer(1234, true), DO).await;
        assert_eq!(reply_of(response), json!({ "inner": true }));
        assert_eq!(*env.asked_uids.borrow(), [1234]);
    });
}

#[test]
fn a_system_non_agent_goes_to_the_authority_and_never_to_capd() {
    run(|env| async move {
        let gate = env.gate(Scope::System, &[OWN], true);
        let response = call(&gate, peer(OWN, false), DO).await;
        assert_eq!(reply_of(response), json!({ "inner": true }));
        assert_eq!(*env.asks.borrow(), [format!("1000:{DO}")]);
        assert!(env.checks.borrow().is_empty());
        assert!(env.asked_uids.borrow().is_empty());
    });
}

#[test]
fn an_authority_that_says_no_refuses() {
    run(|env| async move {
        let gate = env.gate(Scope::System, &[OWN], false);
        let response = call(&gate, peer(OWN, false), DO).await;
        assert_eq!(error_of(response), DENIED);
        assert_eq!(env.events(), Vec::<String>::new());
        assert_eq!(env.logs.borrow().len(), 1);
    });
}

#[test]
fn root_only_accepts_root_and_nobody_else() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    rt.block_on(async {
        assert!(RootOnly.authorize(&peer(0, false), DO).await);
        assert!(!RootOnly.authorize(&peer(0, true), DO).await);
        assert!(!RootOnly.authorize(&peer(1000, false), DO).await);
        assert!(!RootOnly.authorize(&peer(1000, true), DO).await);
    });
}

// 7. Over a real socket: the caller going away withdraws the check.

/// Marks the resolved peer as the agent, then delegates to the gate. The
/// test connects from its own cgroup, which is not the agent's.
struct AsAgent<H: Handler>(H);

impl<H: Handler> Handler for AsAgent<H> {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        let peer = Rc::new(Peer {
            agent: true,
            ..(*peer).clone()
        });
        self.0.call(peer, request).await
    }
}

#[test]
fn a_caller_that_goes_away_withdraws_the_check_and_the_call_never_runs() {
    run(|mut env| async move {
        env.set(Answer::Hang);
        // The test is its own caller: its uid is the session owner's.
        let uid = rustix::process::geteuid().as_raw();
        let socket = env.socket.with_file_name("org.example.v1");
        let capd = env.socket.clone();
        let events = env.events.clone();
        let gate = Gated::with(
            Inner {
                events: events.clone(),
            },
            GateConfig {
                scope: Scope::Session,
                own_uid: uid,
                capd: Box::new(move |_| {
                    Endpoint::new(capd.clone(), ExpectedServer::Cgroup(own_cgroup()))
                }),
                groups: Wheel(vec![]),
                authority: RootOnly,
                log: Box::new(|_| {}),
            },
        );
        let listener = server::bind(&socket).unwrap();
        tokio::task::spawn_local(server::serve(listener, Rc::new(AsAgent(gate))));

        let mut client = UnixStream::connect(&socket).await.unwrap();
        let mut bytes = serde_json::to_vec(&json!({ "method": DO, "parameters": {} })).unwrap();
        bytes.push(0);
        client.write_all(&bytes).await.unwrap();
        // The check is pending at capd.
        tokio::time::sleep(QUIET).await;
        assert_eq!(env.events(), ["check"]);
        drop(client);

        tokio::time::timeout(TIMEOUT, env.gone.recv())
            .await
            .expect("capd saw the check connection close")
            .unwrap();
        tokio::time::sleep(QUIET).await;
        assert_eq!(env.events(), ["check"], "the call must never run");
    });
}

#[test]
fn a_real_connection_is_answered_with_the_refusal() {
    run(|env| async move {
        env.set(Answer::Reply(
            serde_json::to_value(CheckReply::deny(DeniedBy::Off)).unwrap(),
        ));
        let uid = rustix::process::geteuid().as_raw();
        let socket = env.socket.with_file_name("org.example.v1");
        let capd = env.socket.clone();
        let gate = Gated::with(
            Inner {
                events: env.events.clone(),
            },
            GateConfig {
                scope: Scope::Session,
                own_uid: uid,
                capd: Box::new(move |_| {
                    Endpoint::new(capd.clone(), ExpectedServer::Cgroup(own_cgroup()))
                }),
                groups: Wheel(vec![]),
                authority: RootOnly,
                log: Box::new(|_| {}),
            },
        );
        let listener = server::bind(&socket).unwrap();
        tokio::task::spawn_local(server::serve(listener, Rc::new(AsAgent(gate))));

        let mut client = UnixStream::connect(&socket).await.unwrap();
        let mut bytes = serde_json::to_vec(&json!({ "method": DO, "parameters": {} })).unwrap();
        bytes.push(0);
        client.write_all(&bytes).await.unwrap();
        let mut chunk = [0u8; 1024];
        let n = tokio::time::timeout(TIMEOUT, client.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_slice(&chunk[..n - 1]).unwrap();
        assert_eq!(reply, json!({ "error": DENIED, "parameters": {} }));
        assert_eq!(env.events(), ["check"]);
    });
}

// 8. One log line per refusal.

#[test]
fn a_refusal_writes_one_sanitized_line() {
    run(|env| async move {
        env.set(Answer::Reply(
            serde_json::to_value(CheckReply::deny(DeniedBy::User)).unwrap(),
        ));
        let gate = env.gate(Scope::Session, &[], false);
        let method = "org.example.v1.Do\nFORGED: ok\x1b[2J";
        let response = call(&gate, peer(OWN, true), method).await;
        assert_eq!(error_of(response), "org.example.v1.PermissionDenied");
        let logs = env.logs.borrow();
        assert_eq!(logs.len(), 1);
        assert!(
            logs[0].starts_with("tetsu-bus: refused org.example.v1.DoFORGED: ok[2J from pid 4242")
        );
        assert!(!logs[0].contains(char::is_control), "{:?}", logs[0]);
    });
}

#[test]
fn a_call_that_passes_writes_no_line() {
    run(|env| async move {
        let gate = env.gate(Scope::Session, &[], false);
        call(&gate, peer(OWN, true), DO).await;
        call(&gate, peer(OWN, false), DO).await;
        assert!(env.logs.borrow().is_empty());
    });
}
