//! Fixed-path clients check who answers before they send anything (#28
//! item 6), and the `Check` client of `org.tetsu.cap.v1`.

use std::cell::RefCell;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::cap::{
    CheckParams, CheckPeer, CheckReply, Client, ClientError, Decision, DeniedBy, UNKNOWN_METHOD,
};
use tetsu_bus::endpoint::{Endpoint, ExpectedServer, connect_verified};
use tetsu_bus::notify::{self, NotifyKind, ShowParams};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tetsu_bus_contract::{check_reply, load_dir};
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(150);
const CHECK: &str = "org.tetsu.cap.v1.Check";

/// Records every call, answers `Check` as scripted and `Show` with a
/// stream that starts with `shown`.
struct Fake {
    calls: Rc<RefCell<Vec<Request>>>,
    check: Rc<RefCell<Answer>>,
    gone: mpsc::UnboundedSender<()>,
}

#[derive(Clone)]
enum Answer {
    Reply(Value),
    Error(&'static str),
    /// Never answers; reports when the caller goes away.
    Hang,
}

impl Handler for Fake {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        self.calls.borrow_mut().push(request.clone());
        match request.method.as_str() {
            CHECK => match self.check.borrow().clone() {
                Answer::Reply(value) => Response::Reply(value),
                Answer::Error(name) => Response::Error(ErrorReply::new(name, json!({}))),
                Answer::Hang => {
                    let (mut tx, rx) = oneshot::channel::<Result<Value, ErrorReply>>();
                    let gone = self.gone.clone();
                    tokio::task::spawn_local(async move {
                        tx.closed().await;
                        let _ = gone.send(());
                    });
                    Response::Pending(rx)
                }
            },
            "org.tetsu.notify.v1.Show" => {
                let (tx, rx) = mpsc::unbounded_channel();
                tx.send(StreamItem::Reply(
                    json!({ "reply": { "kind": "shown", "id": "n1" } }),
                ))
                .unwrap();
                tokio::task::spawn_local(async move {
                    tx.closed().await;
                });
                Response::Stream(rx)
            }
            other => Response::method_not_found(other),
        }
    }
}

struct World {
    path: PathBuf,
    calls: Rc<RefCell<Vec<Request>>>,
    check: Rc<RefCell<Answer>>,
    gone: mpsc::UnboundedReceiver<()>,
}

fn own_cgroup() -> String {
    let proc = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

fn own() -> ExpectedServer {
    ExpectedServer::Cgroup(own_cgroup())
}

fn elsewhere() -> ExpectedServer {
    ExpectedServer::Cgroup("/system.slice/somebody-else.service".to_owned())
}

fn with_fake<F, Fut>(test: F)
where
    F: FnOnce(World) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tetsu").join("org.example.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let check = Rc::new(RefCell::new(Answer::Reply(json!({
            "decision": "allow", "denied_by": null
        }))));
        let (gone_tx, gone) = mpsc::unbounded_channel();
        let handler = Rc::new(Fake {
            calls: calls.clone(),
            check: check.clone(),
            gone: gone_tx,
        });
        let listener = server::bind(&path).unwrap();
        tokio::task::spawn_local(server::serve(listener, handler));
        test(World {
            path,
            calls,
            check,
            gone,
        })
        .await;
    });
}

fn params() -> CheckParams {
    CheckParams {
        peer: CheckPeer {
            pid: 4242,
            uid: 1000,
            unit: "run-u1.service".to_owned(),
        },
        method: "org.tetsu.notify.v1.Show".to_owned(),
        args: json!({ "title": "hi" }),
    }
}

fn endpoint(path: &Path, expected: ExpectedServer) -> Endpoint {
    Endpoint::new(path, expected)
}

// connect_verified

#[test]
fn a_server_in_the_expected_cgroup_is_connected() {
    with_fake(|world| async move {
        connect_verified(&endpoint(&world.path, own()))
            .await
            .unwrap();
    });
}

#[test]
fn a_server_in_another_cgroup_is_refused_and_gets_no_call() {
    with_fake(|world| async move {
        let ep = endpoint(&world.path, elsewhere());
        let err = connect_verified(&ep).await.expect_err("refused");
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        let err = Client::connect_to(&ep).await.expect_err("refused");
        assert!(matches!(err, ClientError::Io(e) if e.kind() == ErrorKind::PermissionDenied));
        tokio::time::sleep(QUIET).await;
        assert!(world.calls.borrow().is_empty());
    });
}

#[test]
fn a_missing_socket_is_an_error() {
    with_fake(|world| async move {
        let missing = world.path.with_file_name("nobody-home");
        assert!(connect_verified(&endpoint(&missing, own())).await.is_err());
    });
}

// cap::Client::check

#[test]
fn check_sends_the_idl_shape_and_reads_allow() {
    with_fake(|world| async move {
        let mut client = Client::connect_to(&endpoint(&world.path, own()))
            .await
            .unwrap();
        let reply = client.check(&params()).await.unwrap();
        assert_eq!(reply, CheckReply::allow());
        let calls = world.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, CHECK);
        assert_eq!(
            calls[0].parameters,
            json!({
                "peer": { "pid": 4242, "uid": 1000, "unit": "run-u1.service" },
                "method": "org.tetsu.notify.v1.Show",
                "args": { "title": "hi" },
            })
        );
        assert!(!calls[0].oneway && !calls[0].more);
    });
}

#[test]
fn check_reads_every_denied_by() {
    with_fake(|world| async move {
        let contract = load_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl")).unwrap();
        for by in [
            DeniedBy::Off,
            DeniedBy::User,
            DeniedBy::Suppressed,
            DeniedBy::Unavailable,
        ] {
            let wire = serde_json::to_value(CheckReply::deny(by)).unwrap();
            assert_eq!(check_reply(&contract, CHECK, &wire), vec![]);
            *world.check.borrow_mut() = Answer::Reply(wire);
            let mut client = Client::connect_to(&endpoint(&world.path, own()))
                .await
                .unwrap();
            let reply = client.check(&params()).await.unwrap();
            assert_eq!(reply.decision, Decision::Deny);
            assert_eq!(reply.denied_by, Some(by));
        }
        let allow = serde_json::to_value(CheckReply::allow()).unwrap();
        assert_eq!(check_reply(&contract, CHECK, &allow), vec![]);
    });
}

#[test]
fn a_remote_error_is_a_remote_error() {
    with_fake(|world| async move {
        *world.check.borrow_mut() = Answer::Error(UNKNOWN_METHOD);
        let mut client = Client::connect_to(&endpoint(&world.path, own()))
            .await
            .unwrap();
        let err = client.check(&params()).await.unwrap_err();
        assert!(matches!(err, ClientError::Remote { ref error, .. } if error == UNKNOWN_METHOD));
    });
}

#[test]
fn a_malformed_reply_is_an_error_never_an_allow() {
    with_fake(|world| async move {
        for bad in [
            json!({ "decision": "maybe", "denied_by": null }),
            json!({ "denied_by": null }),
            json!({}),
            json!({ "decision": 1 }),
        ] {
            *world.check.borrow_mut() = Answer::Reply(bad.clone());
            let mut client = Client::connect_to(&endpoint(&world.path, own()))
                .await
                .unwrap();
            assert!(client.check(&params()).await.is_err(), "{bad}");
        }
    });
}

#[test]
fn dropping_a_pending_check_closes_the_connection() {
    with_fake(|mut world| async move {
        *world.check.borrow_mut() = Answer::Hang;
        let ep = endpoint(&world.path, own());
        let pending = async move {
            let mut client = Client::connect_to(&ep).await.unwrap();
            let _ = client.check(&params()).await;
        };
        // The future owns the client; dropping it closes the connection.
        let _ = tokio::time::timeout(QUIET, pending).await;
        tokio::time::timeout(TIMEOUT, world.gone.recv())
            .await
            .expect("the server saw the connection close")
            .unwrap();
    });
}

// notify::show_at

fn show() -> ShowParams {
    ShowParams {
        title: "hi".to_owned(),
        body: None,
        urgency: None,
        actions: None,
        timeout_ms: None,
    }
}

#[test]
fn show_at_reaches_the_expected_server() {
    with_fake(|world| async move {
        let mut stream = notify::show_at(&endpoint(&world.path, own()), &show())
            .await
            .unwrap();
        assert_eq!(stream.next().await.unwrap().kind, NotifyKind::Shown);
        assert_eq!(world.calls.borrow()[0].method, "org.tetsu.notify.v1.Show");
        assert!(world.calls.borrow()[0].more);
    });
}

#[test]
fn show_oneway_at_reaches_the_expected_server() {
    with_fake(|world| async move {
        notify::show_oneway_at(&endpoint(&world.path, own()), &show())
            .await
            .unwrap();
        tokio::time::sleep(QUIET).await;
        assert!(world.calls.borrow()[0].oneway);
    });
}

#[test]
fn show_at_and_show_oneway_at_refuse_a_wrong_server_before_sending() {
    with_fake(|world| async move {
        let ep = endpoint(&world.path, elsewhere());
        let err = notify::show_at(&ep, &show()).await.expect_err("refused");
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        let err = notify::show_oneway_at(&ep, &show())
            .await
            .expect_err("refused");
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        tokio::time::sleep(QUIET).await;
        assert!(world.calls.borrow().is_empty());
    });
}
