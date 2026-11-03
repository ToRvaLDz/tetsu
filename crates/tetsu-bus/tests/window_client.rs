//! The `WatchWindows` client against a fake compositor served by the facade.

use std::io::ErrorKind;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tetsu_bus::window::{self, Placement, Window, WindowEvent, WindowKind};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const WATCH: &str = "org.tetsu.comp.window.v1.WatchWindows";

/// How the fake ends (or does not end) its stream.
#[derive(Clone, Copy)]
enum Ending {
    StayOpen,
    Close,
    Deny,
}

struct FakeCompositor(Ending);

fn reply(event: &WindowEvent) -> Value {
    json!({ "event": serde_json::to_value(event).unwrap() })
}

fn one() -> Window {
    Window {
        id: "a".into(),
        app_id: Some("app".into()),
        title: None,
        pid: Some(42),
        workspace_id: Some("w1".into()),
        placement: Placement::Tiled,
        fullscreen: false,
        xwayland: false,
    }
}

impl Handler for FakeCompositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != WATCH {
            return Response::method_not_found(&request.method);
        }
        if matches!(self.0, Ending::Deny) {
            return Response::Error(ErrorReply::new(window::PERMISSION_DENIED, json!({})));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let initial = WindowEvent::initial(vec![], None);
        tx.send(StreamItem::Reply(reply(&initial))).unwrap();
        let focused = WindowEvent::focused(Some("a".into()));
        tx.send(StreamItem::Reply(reply(&focused))).unwrap();
        let opened = WindowEvent::opened(one());
        match self.0 {
            Ending::Close => tx.send(StreamItem::Last(reply(&opened))).unwrap(),
            _ => {
                tx.send(StreamItem::Reply(reply(&opened))).unwrap();
                // Keep the stream open, as a compositor does.
                tokio::task::spawn_local(async move { tx.closed().await });
            }
        }
        Response::Stream(rx)
    }
}

fn run<F: std::future::Future<Output = ()>>(f: impl FnOnce(std::path::PathBuf) -> F) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("comp");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, f(path));
}

fn serve(path: &std::path::Path, ending: Ending) {
    let listener = server::bind(path).unwrap();
    tokio::task::spawn_local(server::serve(listener, Rc::new(FakeCompositor(ending))));
}

#[test]
fn watch_windows_yields_initial_then_deltas() {
    run(|path| async move {
        serve(&path, Ending::StayOpen);
        let mut watch = window::watch(&path).await.unwrap();
        let kinds = [WindowKind::Initial, WindowKind::Focused, WindowKind::Opened];
        for kind in kinds {
            let event = tokio::time::timeout(TIMEOUT, watch.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event.kind, kind);
            if kind == WindowKind::Focused {
                assert_eq!(event.focused_id, Some(Some("a".to_owned())));
            }
        }
    });
}

#[test]
fn watch_windows_fails_when_nobody_listens() {
    run(|path| async move {
        assert!(window::watch(&path.with_file_name("absent")).await.is_err());
    });
}

#[test]
fn watch_windows_reports_end_as_eof() {
    run(|path| async move {
        serve(&path, Ending::Close);
        let mut watch = window::watch(&path).await.unwrap();
        for _ in 0..3 {
            tokio::time::timeout(TIMEOUT, watch.next())
                .await
                .unwrap()
                .unwrap();
        }
        let end = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(end.kind(), ErrorKind::UnexpectedEof);
    });
}

#[test]
fn watch_windows_surfaces_remote_error() {
    run(|path| async move {
        serve(&path, Ending::Deny);
        let mut watch = window::watch(&path).await.unwrap();
        let error = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("PermissionDenied"), "{error}");
    });
}
