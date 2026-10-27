//! The dynamic client: calls any method by name and returns JSON.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::access::ExpectedServer;
use tetsu_bus::cap::ClientError;
use tetsu_bus::dynamic::Client;
use tetsu_bus::endpoint::Endpoint;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

/// A server that records each method it is called with.
struct Fake {
    calls: Rc<RefCell<Vec<Request>>>,
}

impl Handler for Fake {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        self.calls.borrow_mut().push(request.clone());
        match request.method.as_str() {
            "org.example.v1.Echo" => Response::Reply(request.parameters),
            "org.example.v1.Empty" => Response::Reply(Value::Null),
            "org.example.v1.Fail" => Response::Error(ErrorReply::new(
                "org.example.v1.Broken",
                json!({ "why": "marker" }),
            )),
            "org.example.v1.Many" => {
                let (tx, rx) = mpsc::unbounded_channel();
                tx.send(StreamItem::Reply(json!({ "n": 1 }))).unwrap();
                tx.send(StreamItem::Reply(json!({ "n": 2 }))).unwrap();
                tx.send(StreamItem::Last(json!({ "n": 3 }))).unwrap();
                Response::Stream(rx)
            }
            "org.example.v1.Cut" => {
                let (tx, rx) = mpsc::channel(4);
                tx.send(StreamItem::Reply(json!({ "n": 1 }))).await.unwrap();
                // Dropped without a final reply: the connection is closed.
                Response::BoundedStream(rx)
            }
            "org.varlink.service.GetInfo" => Response::Reply(json!({
                "vendor": "v", "product": "p", "version": "1", "url": "u",
                "interfaces": ["org.varlink.service", "org.example.v1"],
            })),
            other => Response::method_not_found(other),
        }
    }
}

fn own_cgroup() -> String {
    let proc = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

struct Env {
    socket: std::path::PathBuf,
    calls: Rc<RefCell<Vec<Request>>>,
}

fn run<F, Fut>(test: F)
where
    F: FnOnce(Env) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("tetsu").join("org.example.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let calls: Rc<RefCell<Vec<Request>>> = Rc::default();
        let listener = server::bind(&socket).unwrap();
        let handler = Rc::new(Fake {
            calls: calls.clone(),
        });
        tokio::task::spawn_local(server::serve(listener, handler));
        test(Env { socket, calls }).await;
    });
}

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, f).await.expect("in time")
}

#[test]
fn a_reply_comes_back_as_json() {
    run(|env| async move {
        let mut client = Client::connect(&env.socket).await.unwrap();
        let params = json!({ "a": 1, "b": ["x"] });
        let reply = within(client.call("org.example.v1.Echo", &params))
            .await
            .unwrap();
        assert_eq!(reply, params);
        assert_eq!(env.calls.borrow()[0].method, "org.example.v1.Echo");
    });
}

#[test]
fn a_reply_without_parameters_is_an_empty_object() {
    run(|env| async move {
        let mut client = Client::connect(&env.socket).await.unwrap();
        let reply = within(client.call("org.example.v1.Empty", &json!({})))
            .await
            .unwrap();
        assert_eq!(reply, json!({}));
    });
}

#[test]
fn a_remote_error_keeps_its_name_and_parameters() {
    run(|env| async move {
        let mut client = Client::connect(&env.socket).await.unwrap();
        let err = within(client.call("org.example.v1.Fail", &json!({})))
            .await
            .unwrap_err();
        match err {
            ClientError::Remote { error, parameters } => {
                assert_eq!(error, "org.example.v1.Broken");
                assert_eq!(parameters, json!({ "why": "marker" }));
            }
            other => panic!("expected a remote error, got {other:?}"),
        }
    });
}

#[test]
fn a_stream_yields_every_reply_with_continues_and_then_ends() {
    run(|env| async move {
        let client = Client::connect(&env.socket).await.unwrap();
        let mut stream = within(client.call_more("org.example.v1.Many", &json!({})))
            .await
            .unwrap();
        let mut seen = Vec::new();
        while let Some(item) = within(stream.next()).await.unwrap() {
            seen.push((item.value, item.continues));
        }
        assert_eq!(
            seen,
            [
                (json!({ "n": 1 }), true),
                (json!({ "n": 2 }), true),
                (json!({ "n": 3 }), false),
            ]
        );
        // After the end it stays ended.
        assert!(within(stream.next()).await.unwrap().is_none());
    });
}

#[test]
fn end_of_file_in_the_middle_of_a_stream_is_an_error() {
    run(|env| async move {
        let client = Client::connect(&env.socket).await.unwrap();
        let mut stream = within(client.call_more("org.example.v1.Cut", &json!({})))
            .await
            .unwrap();
        let first = within(stream.next()).await.unwrap().unwrap();
        assert!(first.continues);
        match within(stream.next()).await {
            Err(ClientError::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
            other => panic!("expected end of file, got {other:?}"),
        }
    });
}

#[test]
fn get_info_lists_the_interfaces() {
    run(|env| async move {
        let mut client = Client::connect(&env.socket).await.unwrap();
        let info = within(client.get_info()).await.unwrap();
        assert_eq!(info.interfaces, ["org.varlink.service", "org.example.v1"]);
        assert_eq!(env.calls.borrow()[0].method, "org.varlink.service.GetInfo");
    });
}

#[test]
fn a_wrong_server_is_refused_and_receives_no_call() {
    run(|env| async move {
        let wrong = Endpoint::new(
            env.socket.clone(),
            ExpectedServer::Cgroup("/not/this/cgroup".to_owned()),
        );
        match Client::connect_to(&wrong).await {
            Err(ClientError::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("expected PermissionDenied, got {other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(env.calls.borrow().is_empty());

        let right = Endpoint::new(env.socket.clone(), ExpectedServer::Cgroup(own_cgroup()));
        let mut client = Client::connect_to(&right).await.unwrap();
        let reply = within(client.call("org.example.v1.Empty", &json!({})))
            .await
            .unwrap();
        assert_eq!(reply, json!({}));
    });
}
