//! The bus task against a fake compositor on the facade's server. Every
//! event the fake sends is checked against the IDL first, so the fake cannot
//! drift from the contract.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bar::bus::{self, CompSource};
use tetsu_bar::model::workspaces::WsCell;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response, StreamItem};
use tetsu_bus::workspace::{Workspace, WorkspaceEvent};
use tetsu_bus_contract::{check_reply, load_dir};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_millis(10);
const WATCH: &str = "org.tetsu.comp.workspace.v1.WatchWorkspaces";

type Strip = Option<Vec<WsCell>>;

/// One connection's worth of events, and whether the stream then ends.
struct Script {
    events: Vec<Value>,
    ends: bool,
}

/// Checks `event` against the IDL and wraps it as a reply.
fn reply(event: &WorkspaceEvent) -> Value {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    let contract = load_dir(&dir).expect("the bus contract loads");
    let value = json!({ "event": serde_json::to_value(event).unwrap() });
    let violations = check_reply(&contract, WATCH, &value);
    assert!(violations.is_empty(), "{violations:?}");
    value
}

fn script(events: &[WorkspaceEvent], ends: bool) -> Script {
    Script {
        events: events.iter().map(reply).collect(),
        ends,
    }
}

struct FakeCompositor {
    scripts: RefCell<VecDeque<Script>>,
}

impl Handler for FakeCompositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != WATCH {
            return Response::method_not_found(&request.method);
        }
        let next = self.scripts.borrow_mut().pop_front();
        let (tx, rx) = mpsc::unbounded_channel();
        let Script { mut events, ends } = next.unwrap_or(Script {
            events: Vec::new(),
            ends: false,
        });
        let last = if ends { events.pop() } else { None };
        for event in events {
            tx.send(StreamItem::Reply(event)).unwrap();
        }
        match last {
            Some(event) => tx.send(StreamItem::Last(event)).unwrap(),
            None => {
                tokio::task::spawn_local(async move { tx.closed().await });
            }
        }
        Response::Stream(rx)
    }
}

fn serve(path: &Path, scripts: Vec<Script>) {
    let listener = server::bind(path).unwrap();
    let fake = FakeCompositor {
        scripts: RefCell::new(scripts.into()),
    };
    tokio::task::spawn_local(server::serve(listener, Rc::new(fake)));
}

fn ws(id: &str, index: i64) -> Workspace {
    Workspace {
        id: id.into(),
        name: None,
        output_id: Some("o1".into()),
        index,
    }
}

fn initial(workspaces: Vec<Workspace>, active: &str) -> WorkspaceEvent {
    let active = BTreeMap::from([("o1".to_owned(), active.to_owned())]);
    WorkspaceEvent::initial(workspaces, active, None)
}

fn cell(label: &str, active: bool, focused: bool) -> WsCell {
    WsCell {
        label: label.into(),
        active,
        focused,
    }
}

fn run<F: Future<Output = ()>>(f: impl FnOnce(PathBuf) -> F) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("comp");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, f(path));
}

async fn next(rx: &mut mpsc::UnboundedReceiver<Strip>) -> Strip {
    tokio::time::timeout(TIMEOUT, rx.recv())
        .await
        .expect("a strip in time")
        .expect("the task is still running")
}

fn start(path: &Path) -> (mpsc::UnboundedReceiver<Strip>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let source = CompSource::Compositor(path.to_owned());
    (rx, tokio::task::spawn_local(bus::run(source, tx, RETRY)))
}

#[test]
fn from_env_reads_the_socket_variable() {
    assert_eq!(CompSource::from_env(None), CompSource::Unmanaged);
    assert_eq!(
        CompSource::from_env(Some(OsString::new())),
        CompSource::Unmanaged
    );
    assert_eq!(
        CompSource::from_env(Some("/run/comp".into())),
        CompSource::Compositor("/run/comp".into())
    );
}

#[test]
fn initial_activated_focused_produce_the_strips_in_order() {
    run(|path| async move {
        let events = [
            initial(vec![ws("w1", 0), ws("w2", 1)], "w1"),
            WorkspaceEvent::activated(Some("w2".into()), "o1"),
            WorkspaceEvent::focused(Some("w2".into())),
        ];
        serve(&path, vec![script(&events, false)]);
        let (mut rx, _task) = start(&path);
        let expected = [
            vec![cell("1", true, false), cell("2", false, false)],
            vec![cell("1", false, false), cell("2", true, false)],
            vec![cell("1", false, false), cell("2", true, true)],
        ];
        for strip in expected {
            assert_eq!(next(&mut rx).await, Some(strip));
        }
    });
}

#[test]
fn a_strip_equal_to_the_last_one_is_not_sent_again() {
    run(|path| async move {
        let events = [
            initial(vec![ws("w1", 0)], "w1"),
            WorkspaceEvent::changed(ws("w1", 0)),
            WorkspaceEvent::focused(Some("w1".into())),
        ];
        serve(&path, vec![script(&events, false)]);
        let (mut rx, _task) = start(&path);
        assert_eq!(next(&mut rx).await, Some(vec![cell("1", true, false)]));
        assert_eq!(next(&mut rx).await, Some(vec![cell("1", true, true)]));
    });
}

#[test]
fn an_ended_stream_clears_the_strip_and_a_fresh_initial_restores_it() {
    run(|path| async move {
        let first = [initial(vec![ws("w1", 0)], "w1")];
        let second = [initial(vec![ws("w1", 0), ws("w2", 1)], "w2")];
        serve(&path, vec![script(&first, true), script(&second, false)]);
        let (mut rx, _task) = start(&path);
        assert_eq!(next(&mut rx).await, Some(vec![cell("1", true, false)]));
        assert_eq!(next(&mut rx).await, None);
        assert_eq!(
            next(&mut rx).await,
            Some(vec![cell("1", false, false), cell("2", true, false)])
        );
    });
}

#[test]
fn nobody_listening_gives_none_once_then_the_strip_when_the_fake_binds() {
    run(|path| async move {
        let (mut rx, _task) = start(&path);
        assert_eq!(next(&mut rx).await, None);
        // Several retries pass without a second `None`.
        tokio::time::sleep(RETRY * 10).await;
        serve(
            &path,
            vec![script(&[initial(vec![ws("w1", 0)], "w1")], false)],
        );
        assert_eq!(next(&mut rx).await, Some(vec![cell("1", true, false)]));
    });
}

#[test]
fn unmanaged_sends_one_none_and_returns() {
    run(|_| async move {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::time::timeout(TIMEOUT, bus::run(CompSource::Unmanaged, tx, RETRY))
            .await
            .expect("the task returns");
        assert_eq!(rx.recv().await, Some(None));
        assert_eq!(rx.recv().await, None);
    });
}

#[test]
fn dropping_the_receiver_ends_the_task() {
    run(|path| async move {
        serve(
            &path,
            vec![script(&[initial(vec![ws("w1", 0)], "w1")], false)],
        );
        let (mut rx, task) = start(&path);
        assert!(next(&mut rx).await.is_some());
        drop(rx);
        tokio::time::timeout(TIMEOUT, task)
            .await
            .expect("the task ends")
            .unwrap();
    });
}
