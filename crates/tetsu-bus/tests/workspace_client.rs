//! The `WatchWorkspaces` client against a fake compositor served by the facade.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tetsu_bus::workspace::{self, Workspace, WorkspaceEvent, WorkspaceKind};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const WATCH: &str = "org.tetsu.comp.workspace.v1.WatchWorkspaces";

/// How the fake ends (or does not end) its stream.
#[derive(Clone, Copy)]
enum Ending {
    StayOpen,
    Close,
    Deny,
}

struct FakeCompositor(Ending);

fn reply(event: &WorkspaceEvent) -> Value {
    json!({ "event": serde_json::to_value(event).unwrap() })
}

fn one() -> Workspace {
    Workspace {
        id: "w1".into(),
        name: None,
        output_id: Some("o1".into()),
        index: 0,
    }
}

impl Handler for FakeCompositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != WATCH {
            return Response::method_not_found(&request.method);
        }
        if matches!(self.0, Ending::Deny) {
            return Response::Error(ErrorReply::new(workspace::PERMISSION_DENIED, json!({})));
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let active = BTreeMap::from([("o1".to_owned(), "w1".to_owned())]);
        let initial = WorkspaceEvent::initial(vec![one()], active, None);
        tx.send(StreamItem::Reply(reply(&initial))).unwrap();
        let activated = WorkspaceEvent::activated(Some("w1".into()), "o1");
        tx.send(StreamItem::Reply(reply(&activated))).unwrap();
        let focused = WorkspaceEvent::focused(Some("w1".into()));
        match self.0 {
            Ending::Close => tx.send(StreamItem::Last(reply(&focused))).unwrap(),
            _ => {
                tx.send(StreamItem::Reply(reply(&focused))).unwrap();
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
fn watch_workspaces_yields_initial_then_deltas() {
    run(|path| async move {
        serve(&path, Ending::StayOpen);
        let mut watch = workspace::watch(&path).await.unwrap();
        let kinds = [
            WorkspaceKind::Initial,
            WorkspaceKind::Activated,
            WorkspaceKind::Focused,
        ];
        for kind in kinds {
            let event = tokio::time::timeout(TIMEOUT, watch.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(event.kind, kind);
        }
    });
}

#[test]
fn watch_workspaces_fails_when_nobody_listens() {
    run(|path| async move {
        assert!(
            workspace::watch(&path.with_file_name("absent"))
                .await
                .is_err()
        );
    });
}

#[test]
fn watch_workspaces_reports_end_as_eof() {
    run(|path| async move {
        serve(&path, Ending::Close);
        let mut watch = workspace::watch(&path).await.unwrap();
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
fn watch_workspaces_surfaces_remote_error() {
    run(|path| async move {
        serve(&path, Ending::Deny);
        let mut watch = workspace::watch(&path).await.unwrap();
        let error = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap_err();
        assert!(error.to_string().contains("PermissionDenied"), "{error}");
    });
}
