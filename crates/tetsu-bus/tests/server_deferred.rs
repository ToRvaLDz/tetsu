//! `Response::Deferred`: a handler decides the whole response later (a
//! reply, an error or a stream), while the connection keeps being read.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait to be sure something does not happen.
const QUIET: Duration = Duration::from_millis(150);

/// What the test sees of the handler.
struct Ctl {
    /// The caller went away while the response was deferred.
    gone: mpsc::UnboundedReceiver<()>,
    /// The effect of a deferred call ran.
    effects: mpsc::UnboundedReceiver<()>,
    /// Lets the pending deferred call decide.
    release: Rc<Notify>,
}

/// `Later` answers `how` (reply, error, stream, drop, nested) once released;
/// `Echo` answers at once.
struct Test {
    gone: mpsc::UnboundedSender<()>,
    effects: mpsc::UnboundedSender<()>,
    release: Rc<Notify>,
}

impl Handler for Test {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.example.Echo" => Response::Reply(json!({ "value": request.parameters["value"] })),
            "org.example.Later" => {
                let (tx, rx) = oneshot::channel();
                let how = request.parameters["how"]
                    .as_str()
                    .unwrap_or("reply")
                    .to_owned();
                let (gone, effects, release) = (
                    self.gone.clone(),
                    self.effects.clone(),
                    self.release.clone(),
                );
                tokio::task::spawn_local(async move {
                    tokio::select! {
                        () = tx.closed() => {
                            let _ = gone.send(());
                        }
                        () = release.notified() => {
                            let _ = effects.send(());
                            decide(&how, tx);
                        }
                    }
                });
                Response::Deferred(rx)
            }
            other => Response::method_not_found(other),
        }
    }
}

fn decide(how: &str, tx: oneshot::Sender<Response>) {
    match how {
        "reply" => {
            let _ = tx.send(Response::Reply(json!({ "late": true })));
        }
        "error" => {
            let error = ErrorReply::new("org.example.Broken", json!({ "why": "late" }));
            let _ = tx.send(Response::Error(error));
        }
        "stream" => {
            let (items, stream) = mpsc::unbounded_channel();
            items.send(StreamItem::Reply(json!({ "i": 1 }))).unwrap();
            items.send(StreamItem::Last(json!({ "i": 2 }))).unwrap();
            let _ = tx.send(Response::Stream(stream));
        }
        "nested" => {
            let (inner_tx, inner_rx) = oneshot::channel();
            let _ = inner_tx.send(Response::Reply(json!({ "must": "not be sent" })));
            let _ = tx.send(Response::Deferred(inner_rx));
        }
        _ => drop(tx),
    }
}

struct Client(UnixStream, Vec<u8>);

impl Client {
    async fn connect(path: &Path) -> Self {
        Self(UnixStream::connect(path).await.unwrap(), Vec::new())
    }

    async fn send(&mut self, message: Value) {
        let mut bytes = serde_json::to_vec(&message).unwrap();
        bytes.push(0);
        self.0.write_all(&bytes).await.unwrap();
    }

    /// The next message, or `None` once the server closed the connection.
    async fn next(&mut self) -> Option<Value> {
        loop {
            if let Some(end) = self.1.iter().position(|b| *b == 0) {
                let message: Vec<u8> = self.1.drain(..=end).collect();
                return Some(serde_json::from_slice(&message[..end]).unwrap());
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(TIMEOUT, self.0.read(&mut chunk))
                .await
                .expect("an answer in time")
                .unwrap_or(0);
            if n == 0 {
                return None;
            }
            self.1.extend_from_slice(&chunk[..n]);
        }
    }

    async fn recv(&mut self) -> Value {
        self.next().await.expect("server closed the connection")
    }

    async fn call_later(&mut self, how: &str, more: bool, oneway: bool) {
        self.send(json!({
            "method": "org.example.Later",
            "parameters": { "how": how },
            "more": more,
            "oneway": oneway,
        }))
        .await;
    }
}

fn with_server<F, Fut>(test: F)
where
    F: FnOnce(PathBuf, Ctl) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tetsu").join("org.example.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (gone_tx, gone) = mpsc::unbounded_channel();
        let (effects_tx, effects) = mpsc::unbounded_channel();
        let release = Rc::new(Notify::new());
        let listener = server::bind(&path).unwrap();
        let handler = Rc::new(Test {
            gone: gone_tx,
            effects: effects_tx,
            release: release.clone(),
        });
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        tokio::task::spawn_local(async move {
            tokio::select! {
                result = server::serve(listener, handler) => result.unwrap(),
                _ = stop_rx => {}
            }
        });
        test(
            path,
            Ctl {
                gone,
                effects,
                release,
            },
        )
        .await;
        let _ = stop_tx.send(());
    });
}

#[test]
fn a_deferred_reply_is_sent_once_decided() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("reply", false, false).await;
        tokio::time::sleep(QUIET).await;
        ctl.release.notify_one();
        assert_eq!(
            client.recv().await,
            json!({ "parameters": { "late": true } })
        );
    });
}

#[test]
fn a_deferred_error_is_sent_once_decided() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("error", false, false).await;
        ctl.release.notify_one();
        assert_eq!(
            client.recv().await,
            json!({ "error": "org.example.Broken", "parameters": { "why": "late" } })
        );
    });
}

#[test]
fn a_deferred_stream_is_relayed_to_a_call_with_more() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("stream", true, false).await;
        ctl.release.notify_one();
        assert_eq!(
            client.recv().await,
            json!({ "parameters": { "i": 1 }, "continues": true })
        );
        assert_eq!(client.recv().await, json!({ "parameters": { "i": 2 } }));
    });
}

#[test]
fn a_deferred_stream_to_a_call_without_more_is_expected_more() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("stream", false, false).await;
        ctl.release.notify_one();
        assert_eq!(
            client.recv().await,
            json!({ "error": "org.varlink.service.ExpectedMore", "parameters": {} })
        );
    });
}

#[test]
fn a_caller_that_closes_while_deferred_is_seen_by_the_handler() {
    with_server(|path, mut ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("reply", false, false).await;
        tokio::time::sleep(QUIET).await;
        drop(client);
        tokio::time::timeout(TIMEOUT, ctl.gone.recv())
            .await
            .expect("the handler saw the caller go")
            .unwrap();
    });
}

#[test]
fn a_call_pipelined_meanwhile_is_answered_after() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("reply", false, false).await;
        client
            .send(json!({ "method": "org.example.Echo", "parameters": { "value": 3 } }))
            .await;
        tokio::time::sleep(QUIET).await;
        ctl.release.notify_one();
        assert_eq!(
            client.recv().await,
            json!({ "parameters": { "late": true } })
        );
        assert_eq!(client.recv().await, json!({ "parameters": { "value": 3 } }));
    });
}

#[test]
fn a_sender_dropped_without_an_answer_closes_the_connection() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("drop", false, false).await;
        ctl.release.notify_one();
        assert_eq!(client.next().await, None);
    });
}

#[test]
fn a_deferred_that_resolves_to_a_deferred_closes_the_connection() {
    with_server(|path, ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("nested", false, false).await;
        ctl.release.notify_one();
        assert_eq!(client.next().await, None);
    });
}

#[test]
fn a_oneway_call_is_decided_before_its_effect_and_gets_no_reply() {
    with_server(|path, mut ctl| async move {
        let mut client = Client::connect(&path).await;
        client.call_later("reply", false, true).await;
        tokio::time::sleep(QUIET).await;
        assert!(
            ctl.effects.try_recv().is_err(),
            "the effect ran before the decision"
        );
        ctl.release.notify_one();
        tokio::time::timeout(TIMEOUT, ctl.effects.recv())
            .await
            .expect("the effect ran once decided")
            .unwrap();
        // No reply was written: the first message is the next call's.
        client
            .send(json!({ "method": "org.example.Echo", "parameters": { "value": 9 } }))
            .await;
        assert_eq!(client.recv().await, json!({ "parameters": { "value": 9 } }));
    });
}
