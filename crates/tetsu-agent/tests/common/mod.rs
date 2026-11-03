//! Fakes for the end-to-end test: a capd that records `Intent` and `Check`,
//! a service behind the real gate, and the marker that makes the test
//! process an agent.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::{Value, json};
use tetsu_bus::access::{EtcFiles, ExpectedServer, Scope};
use tetsu_bus::cap::{CheckReply, DeniedBy};
use tetsu_bus::endpoint::Endpoint;
use tetsu_bus::gate::{GateConfig, Gated, RootOnly};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response, StreamItem};
use tokio::sync::mpsc;

/// The order in which capd and the service saw things.
pub type Events = Rc<RefCell<Vec<String>>>;

/// Whether the fake capd allows a `Check`.
#[derive(Clone, Copy)]
pub enum Decision {
    Allow,
    DenyByUser,
}

/// The cgroup of the test process, which serves every fake.
pub fn own_cgroup() -> String {
    let proc = fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

/// A fake `org.tetsu.cap.v1`: records `Intent` and `Check` in order.
pub struct FakeCapd {
    pub events: Events,
    pub decision: Rc<RefCell<Decision>>,
}

impl Handler for FakeCapd {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        let p = &request.parameters;
        match request.method.as_str() {
            "org.tetsu.cap.v1.Intent" => {
                self.events.borrow_mut().push(format!(
                    "Intent:{}:{}",
                    p["method"].as_str().unwrap_or("?"),
                    p["reason"].as_str().unwrap_or("?")
                ));
                Response::Reply(Value::Null)
            }
            "org.tetsu.cap.v1.Check" => {
                self.events.borrow_mut().push(format!(
                    "Check:{}:{}",
                    p["method"].as_str().unwrap_or("?"),
                    serde_json::to_string(&p["args"]).unwrap()
                ));
                let reply = match *self.decision.borrow() {
                    Decision::Allow => CheckReply::allow(),
                    Decision::DenyByUser => CheckReply::deny(DeniedBy::User),
                };
                Response::Reply(serde_json::to_value(reply).unwrap())
            }
            other => Response::method_not_found(other),
        }
    }
}

/// A fake `org.tetsu.tx.v1`: a read and `Install`. Records what runs.
pub struct FakeTx {
    pub events: Events,
}

impl Handler for FakeTx {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "v", "product": "p", "version": "1", "url": "u",
                "interfaces": ["org.varlink.service", "org.tetsu.tx.v1"],
            })),
            "org.tetsu.tx.v1.GetStatus" => {
                self.events.borrow_mut().push("inner:GetStatus".to_owned());
                Response::Reply(json!({ "booted": "a", "default": "a", "default_boot": "good" }))
            }
            "org.tetsu.tx.v1.Install" => {
                self.events.borrow_mut().push(format!(
                    "inner:Install:{}",
                    serde_json::to_string(&request.parameters).unwrap()
                ));
                let (tx, rx) = mpsc::unbounded_channel();
                tx.send(StreamItem::Last(json!({ "transaction": { "id": "t1" } })))
                    .unwrap();
                Response::Stream(rx)
            }
            other => Response::method_not_found(other),
        }
    }
}

/// Marks the resolved peer as the agent, then delegates to the gate. The
/// test connects from its own cgroup, which is not the agent's.
pub struct AsAgent<H: Handler>(pub H);

impl<H: Handler> Handler for AsAgent<H> {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        let peer = Rc::new(Peer {
            agent: true,
            ..(*peer).clone()
        });
        self.0.call(peer, request).await
    }
}

/// Everything the test talks to.
pub struct World {
    pub events: Events,
    pub decision: Rc<RefCell<Decision>>,
    pub gate_log: Events,
    pub scan_dir: PathBuf,
    pub capd: Endpoint,
}

/// Starts the fake capd and the gated service under `root`. Must run on a
/// `LocalSet`.
pub fn start(root: &std::path::Path) -> World {
    let events: Events = Rc::default();
    let decision = Rc::new(RefCell::new(Decision::Allow));
    let gate_log: Events = Rc::default();
    let scan_dir = root.join("tetsu");

    let capd_path = root.join("capd").join("org.tetsu.cap.v1");
    let listener = server::bind(&capd_path).unwrap();
    let capd = Rc::new(FakeCapd {
        events: events.clone(),
        decision: decision.clone(),
    });
    tokio::task::spawn_local(server::serve(listener, capd));
    let capd_endpoint = Endpoint::new(&capd_path, ExpectedServer::Cgroup(own_cgroup()));

    let gate_capd = capd_endpoint.clone();
    let sink = gate_log.clone();
    let gate = Gated::with(
        FakeTx {
            events: events.clone(),
        },
        GateConfig {
            scope: Scope::Session,
            own_uid: rustix::process::geteuid().as_raw(),
            capd: Box::new(move |_| gate_capd.clone()),
            groups: EtcFiles::new("/"),
            authority: RootOnly,
            log: Box::new(move |line| sink.borrow_mut().push(line.to_owned())),
        },
    );
    let listener = server::bind(&scan_dir.join("org.tetsu.tx.v1")).unwrap();
    tokio::task::spawn_local(server::serve(listener, Rc::new(AsAgent(gate))));

    World {
        events,
        decision,
        gate_log,
        scan_dir,
        capd: capd_endpoint,
    }
}
