//! The comp bus service behind the real gate, with a fake capd, on a thread
//! of its own. The test thread speaks raw Varlink to it.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use smithay::reexports::rustix;
use tetsu_bus::access::{EtcFiles, ExpectedServer, Scope};
use tetsu_bus::cap::{CheckReply, DeniedBy};
use tetsu_bus::endpoint::Endpoint;
use tetsu_bus::gate::{GateConfig, RootOnly};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response};
use tetsu_comp::bus::{Command, CommandSink, Hub, Snapshot, handler};

/// What the fakes saw, in order.
pub type Log = Arc<Mutex<Vec<String>>>;

fn own_cgroup() -> String {
    let proc = fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

/// A fake `org.tetsu.cap.v1`: records each `Check` and answers as told.
struct FakeCapd {
    log: Log,
    allow: Arc<Mutex<bool>>,
}

impl Handler for FakeCapd {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != "org.tetsu.cap.v1.Check" {
            return Response::method_not_found(&request.method);
        }
        let method = request.parameters["method"].as_str().unwrap_or("?");
        self.log.lock().unwrap().push(format!("Check:{method}"));
        let reply = if *self.allow.lock().unwrap() {
            CheckReply::allow()
        } else {
            CheckReply::deny(DeniedBy::User)
        };
        Response::Reply(serde_json::to_value(reply).unwrap())
    }
}

/// Marks the resolved peer as the agent: the test connects from its own
/// cgroup, which is not the agent's.
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

/// A running gated service.
pub struct GatedComp {
    pub socket: PathBuf,
    /// The `Check` calls capd saw.
    pub capd: Log,
    /// The commands that reached the compositor loop, one line each.
    pub sink: Log,
    /// Whether capd allows.
    pub allow: Arc<Mutex<bool>>,
    _dir: tempfile::TempDir,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl GatedComp {
    /// Starts the service; every caller is the agent when `as_agent`.
    pub fn start(as_agent: bool, snapshot: Snapshot) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tetsu/comp/wayland-test");
        let capd_path = dir.path().join("capd/org.tetsu.cap.v1");
        let capd: Log = Arc::default();
        let sink: Log = Arc::default();
        let allow = Arc::new(Mutex::new(true));
        let (stop, stopped) = mpsc::channel::<()>();
        let (ready_tx, ready) = mpsc::channel::<()>();
        let (cl, sl, al, sock, cp) = (
            capd.clone(),
            sink.clone(),
            allow.clone(),
            socket.clone(),
            capd_path,
        );
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            tokio::task::LocalSet::new().block_on(&runtime, async move {
                let listener = server::bind(&cp).unwrap();
                tokio::task::spawn_local(server::serve(
                    listener,
                    Rc::new(FakeCapd { log: cl, allow: al }),
                ));
                let endpoint = Endpoint::new(&cp, ExpectedServer::Cgroup(own_cgroup()));
                let commands: CommandSink = Arc::new(move |command| {
                    let (line, reply) = match command {
                        Command::SetManaged(call) => (
                            format!(
                                "SetManaged:{}",
                                serde_json::to_string(&call.values).unwrap()
                            ),
                            call.reply,
                        ),
                        Command::ConfigureOutputs(call) => (
                            format!(
                                "ConfigureOutputs:{}",
                                serde_json::to_string(&call.configs).unwrap()
                            ),
                            call.reply,
                        ),
                    };
                    sl.lock().unwrap().push(line);
                    let _ = reply.send(Ok(()));
                    Ok(())
                });
                let hub = Rc::new(RefCell::new(Hub::new(&snapshot)));
                let gate = handler(
                    hub,
                    Some(commands),
                    GateConfig {
                        scope: Scope::Session,
                        own_uid: rustix::process::geteuid().as_raw(),
                        capd: Box::new(move |_| endpoint.clone()),
                        groups: EtcFiles::new("/"),
                        authority: RootOnly,
                        log: Box::new(|_| {}),
                    },
                );
                let listener = server::bind(&sock).unwrap();
                if as_agent {
                    tokio::task::spawn_local(server::serve(listener, Rc::new(AsAgent(gate))));
                } else {
                    tokio::task::spawn_local(server::serve(listener, Rc::new(gate)));
                }
                ready_tx.send(()).unwrap();
                // Wait for the stop without blocking the runtime.
                loop {
                    if !matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            });
        });
        ready.recv().unwrap();
        Self {
            socket,
            capd,
            sink,
            allow,
            _dir: dir,
            stop: Some(stop),
            thread: Some(thread),
        }
    }

    pub fn deny(&self) {
        *self.allow.lock().unwrap() = false;
    }
}

impl Drop for GatedComp {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
