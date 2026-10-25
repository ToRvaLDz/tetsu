//! Where the lock state comes from: `tetsu-comp`'s `WatchSession`, an
//! explicit development fallback when no Tetsu compositor is configured, and
//! fail closed when one is configured but cannot be reached.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::json;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response, StreamItem};
use tetsu_notifyd::lock::{self, LockSource};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

async fn next(rx: &mut mpsc::UnboundedReceiver<bool>) -> bool {
    tokio::time::timeout(TIMEOUT, rx.recv())
        .await
        .expect("a lock state in time")
        .expect("the watcher is alive")
}

#[test]
fn the_source_follows_the_environment() {
    assert_eq!(LockSource::from_env(None), LockSource::Unmanaged);
    assert_eq!(LockSource::from_env(Some("".into())), LockSource::Unmanaged);
    assert_eq!(
        LockSource::from_env(Some("/run/user/1000/tetsu/comp-0".into())),
        LockSource::Compositor(PathBuf::from("/run/user/1000/tetsu/comp-0"))
    );
}

#[test]
fn without_a_tetsu_compositor_the_session_counts_as_unlocked() {
    LocalSet::new().block_on(&runtime(), async {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::task::spawn_local(lock::watch(LockSource::Unmanaged, tx));
        assert!(!next(&mut rx).await);
    });
}

#[test]
fn an_unreachable_compositor_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let source = LockSource::Compositor(dir.path().join("absent"));
    LocalSet::new().block_on(&runtime(), async {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::task::spawn_local(lock::watch(source, tx));
        assert!(next(&mut rx).await);
    });
}

/// Streams `initial` (unlocked) and `locked`, then ends the stream.
struct Compositor;

impl Handler for Compositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        let (tx, rx) = mpsc::unbounded_channel();
        if request.method == "org.tetsu.comp.session.v1.WatchSession" {
            tx.send(StreamItem::Reply(
                json!({ "event": { "kind": "initial", "locked": false } }),
            ))
            .unwrap();
            tx.send(StreamItem::Last(json!({ "event": { "kind": "locked" } })))
                .unwrap();
        }
        Response::Stream(rx)
    }
}

#[test]
fn the_compositor_drives_the_state_and_a_lost_stream_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("comp");
    LocalSet::new().block_on(&runtime(), async {
        let listener = server::bind(&path).unwrap();
        tokio::task::spawn_local(server::serve(listener, Rc::new(Compositor)));
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::task::spawn_local(lock::watch(LockSource::Compositor(path), tx));
        let states = [
            next(&mut rx).await,
            next(&mut rx).await,
            next(&mut rx).await,
        ];
        // unlocked from `initial`, locked from the event, still locked when
        // the stream ends; then it reconnects and starts over.
        assert_eq!(states, [false, true, true]);
        assert!(!next(&mut rx).await);
    });
}
