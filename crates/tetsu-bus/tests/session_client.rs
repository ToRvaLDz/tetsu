//! The `WatchSession` client against a fake compositor served by the facade.

use std::rc::Rc;
use std::time::Duration;

use serde_json::json;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response, StreamItem};
use tetsu_bus::session::{self, SessionKind};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

struct FakeCompositor;

impl Handler for FakeCompositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.tetsu.comp.session.v1.WatchSession" => {
                let (tx, rx) = mpsc::unbounded_channel();
                tx.send(StreamItem::Reply(
                    json!({ "event": { "kind": "initial", "locked": true } }),
                ))
                .unwrap();
                tx.send(StreamItem::Reply(
                    json!({ "event": { "kind": "unlocked" } }),
                ))
                .unwrap();
                // Keep the stream open, as a compositor does.
                tokio::task::spawn_local(async move { tx.closed().await });
                Response::Stream(rx)
            }
            other => Response::method_not_found(other),
        }
    }
}

#[test]
fn watch_session_yields_initial_then_deltas() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("comp");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let listener = server::bind(&path).unwrap();
        tokio::task::spawn_local(server::serve(listener, Rc::new(FakeCompositor)));

        let mut watch = session::watch(&path).await.unwrap();
        let first = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.kind, SessionKind::Initial);
        assert_eq!(first.locked, Some(true));
        let second = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.kind, SessionKind::Unlocked);
    });
}

#[test]
fn watch_session_fails_when_nobody_listens() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        assert!(session::watch(&dir.path().join("absent")).await.is_err());
    });
}
