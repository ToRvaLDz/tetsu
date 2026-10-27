//! `org.tetsu.cap.v1` wire types and clients, and the `notify` Show client,
//! against test servers on tempdir sockets.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::cap::{self, CapabilityEvent, CheckParams, CheckReply, ClientError, SetParams};
use tetsu_bus::notify::{self, DismissReason, NotifyKind, ShowParams, Urgency};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tetsu_bus_contract::{check_error, check_reply, load_dir};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture(kind: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/events/org.tetsu.cap.v1")
        .join(format!("CapabilityEvent.{kind}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn contract() -> tetsu_bus_contract::Contract {
    load_dir(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl")).unwrap()
}

#[test]
fn capability_events_round_trip_the_fixtures() {
    for kind in ["initial", "changed"] {
        let wire = fixture(kind);
        let event: CapabilityEvent = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&event).unwrap(), wire, "{kind}");
    }
}

#[test]
fn serialized_replies_pass_the_contract() {
    let contract = contract();
    let event: CapabilityEvent = serde_json::from_value(fixture("initial")).unwrap();
    let list = json!({ "capabilities": event.capabilities });
    assert_eq!(
        check_reply(&contract, "org.tetsu.cap.v1.ListCapabilities", &list),
        vec![]
    );
    let watch = json!({ "event": event });
    assert_eq!(
        check_reply(&contract, "org.tetsu.cap.v1.WatchCapabilities", &watch),
        vec![]
    );
    for reply in [
        CheckReply::allow(),
        CheckReply::deny(cap::DeniedBy::Suppressed),
    ] {
        let wire = serde_json::to_value(reply).unwrap();
        assert_eq!(
            check_reply(&contract, "org.tetsu.cap.v1.Check", &wire),
            vec![]
        );
    }
    assert_eq!(
        check_error(
            &contract,
            cap::OVERRIDDEN_BY_USER,
            &json!({ "class": "a.b" })
        ),
        vec![]
    );
}

#[test]
fn params_deserialize_from_the_wire() {
    let check: CheckParams = serde_json::from_value(json!({
        "peer": { "pid": 7, "uid": 1000, "unit": "tetsu-agent.service" },
        "method": "org.tetsu.tx.v1.Install",
        "args": { "packages": ["htop"] }
    }))
    .unwrap();
    assert_eq!(check.peer.pid, 7);
    assert_eq!(check.args["packages"][0], "htop");
    let set: SetParams = serde_json::from_value(json!({ "class": "a.b" })).unwrap();
    assert_eq!((set.scope, set.state), (None, None));
    let set: SetParams =
        serde_json::from_value(json!({ "class": "a.b", "scope": "x", "state": "notify" })).unwrap();
    assert_eq!(set.state, Some(cap::Grant::Notify));
    assert!(
        serde_json::from_value::<SetParams>(json!({ "class": "a", "state": "unset" })).is_err()
    );
}

/// ListCapabilities and Set; the last Set seen is reported.
struct FakeCap {
    sets: mpsc::UnboundedSender<Value>,
}

impl Handler for FakeCap {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.tetsu.cap.v1.ListCapabilities" => {
                let initial = fixture("initial");
                Response::Reply(json!({ "capabilities": initial["capabilities"] }))
            }
            "org.tetsu.cap.v1.WatchCapabilities" => {
                let (tx, rx) = mpsc::unbounded_channel();
                for kind in ["initial", "changed"] {
                    tx.send(StreamItem::Reply(json!({ "event": fixture(kind) })))
                        .unwrap();
                }
                tokio::task::spawn_local(async move { tx.closed().await });
                Response::Stream(rx)
            }
            "org.tetsu.cap.v1.Set" => {
                if request.parameters["class"] == "mail.send" {
                    return Response::Error(ErrorReply::new(
                        cap::OVERRIDDEN_BY_USER,
                        json!({ "class": "mail.send" }),
                    ));
                }
                self.sets.send(request.parameters.clone()).unwrap();
                Response::Reply(Value::Null)
            }
            other => Response::method_not_found(other),
        }
    }
}

fn run<F: Future<Output = ()>>(test: F) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, test);
}

#[test]
fn the_client_lists_watches_and_sets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("cap");
    run(async {
        let (sets_tx, mut sets) = mpsc::unbounded_channel();
        let listener = server::bind(&path).unwrap();
        tokio::task::spawn_local(server::serve(listener, Rc::new(FakeCap { sets: sets_tx })));

        let mut client = cap::Client::connect(&path).await.unwrap();
        let list = client.list().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].class, "packages.install");
        assert_eq!(list[1].state, cap::State::Unset);
        assert_eq!(list[1].layer, None);

        let mut client = cap::Client::connect(&path).await.unwrap();
        client
            .set(&SetParams {
                class: "packages.install".into(),
                scope: None,
                state: None,
            })
            .await
            .unwrap();
        let seen = sets.recv().await.unwrap();
        assert_eq!(seen["class"], "packages.install");
        assert_eq!(seen["state"], Value::Null);

        client
            .set(&SetParams {
                class: "packages.install".into(),
                scope: Some("s".into()),
                state: Some(cap::Grant::Off),
            })
            .await
            .unwrap();
        let seen = sets.recv().await.unwrap();
        assert_eq!(
            (seen["scope"].as_str(), seen["state"].as_str()),
            (Some("s"), Some("off"))
        );

        let error = client
            .set(&SetParams {
                class: "mail.send".into(),
                scope: None,
                state: Some(cap::Grant::Ask),
            })
            .await
            .unwrap_err();
        match error {
            ClientError::Remote { error, parameters } => {
                assert_eq!(error, cap::OVERRIDDEN_BY_USER);
                assert_eq!(parameters["class"], "mail.send");
            }
            other => panic!("{other:?}"),
        }

        let mut watch = cap::Client::connect(&path)
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();
        let first = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.kind, cap::EventKind::Initial);
        let second = tokio::time::timeout(TIMEOUT, watch.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.kind, cap::EventKind::Changed);
    });
}

#[test]
fn the_client_fails_when_nobody_listens() {
    let dir = tempfile::tempdir().unwrap();
    run(async {
        let error = cap::Client::connect(&dir.path().join("absent"))
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::Io(_)));
    });
}

/// Show: `shown`, then the scripted answer, or just `shown` and a close.
struct FakeNotify {
    params: mpsc::UnboundedSender<Value>,
    gone: mpsc::UnboundedSender<()>,
}

impl Handler for FakeNotify {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != "org.tetsu.notify.v1.Show" {
            return Response::method_not_found(&request.method);
        }
        self.params.send(request.parameters.clone()).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(StreamItem::Reply(shown_reply())).unwrap();
        match request.parameters["title"].as_str() {
            Some("answer") => {
                let invoked = json!({ "reply": { "kind": "invoked", "action_id": "approve" } });
                tx.send(StreamItem::Last(invoked)).unwrap();
            }
            Some("eof") => {
                // A bounded stream whose sender is dropped closes the
                // connection, as a notifyd that went away does.
                let (btx, brx) = mpsc::channel(4);
                btx.try_send(StreamItem::Reply(shown_reply())).unwrap();
                drop(btx);
                return Response::BoundedStream(brx);
            }
            _ => {
                let gone = self.gone.clone();
                tokio::task::spawn_local(async move {
                    tx.closed().await;
                    let _ = gone.send(());
                });
            }
        }
        Response::Stream(rx)
    }
}

fn shown_reply() -> Value {
    json!({ "reply": { "kind": "shown", "id": "n1" } })
}

fn show_params(title: &str) -> ShowParams {
    ShowParams {
        title: title.into(),
        body: None,
        urgency: Some(Urgency::Critical),
        actions: Some(vec![notify::Action::new("approve", "Approve")]),
        timeout_ms: None,
    }
}

#[test]
fn show_yields_shown_then_invoked_and_ends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("notify");
    run(async {
        let (params_tx, mut params) = mpsc::unbounded_channel();
        let (gone_tx, mut gone) = mpsc::unbounded_channel();
        let listener = server::bind(&path).unwrap();
        let fake = FakeNotify {
            params: params_tx,
            gone: gone_tx,
        };
        tokio::task::spawn_local(server::serve(listener, Rc::new(fake)));

        let mut stream = notify::show(&path, &show_params("answer")).await.unwrap();
        let sent = params.recv().await.unwrap();
        assert_eq!(sent["urgency"], "critical");
        assert_eq!(sent["actions"][0]["id"], "approve");
        assert!(sent.get("body").is_none() && sent.get("timeout_ms").is_none());
        let shown = stream.next().await.unwrap();
        assert_eq!(
            (shown.kind, shown.id.as_deref()),
            (NotifyKind::Shown, Some("n1"))
        );
        let invoked = stream.next().await.unwrap();
        assert_eq!(invoked.kind, NotifyKind::Invoked);
        assert_eq!(invoked.action_id.as_deref(), Some("approve"));
        let end = stream.next().await.unwrap_err();
        assert_eq!(end.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(gone.try_recv().is_err());
        let _ = DismissReason::User;
    });
}

#[test]
fn show_reports_an_eof_before_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("notify");
    run(async {
        let (params_tx, _params) = mpsc::unbounded_channel();
        let (gone_tx, _gone) = mpsc::unbounded_channel();
        let listener = server::bind(&path).unwrap();
        let fake = FakeNotify {
            params: params_tx,
            gone: gone_tx,
        };
        tokio::task::spawn_local(server::serve(listener, Rc::new(fake)));

        let mut stream = notify::show(&path, &show_params("eof")).await.unwrap();
        assert_eq!(stream.next().await.unwrap().kind, NotifyKind::Shown);
        let end = tokio::time::timeout(TIMEOUT, stream.next()).await.unwrap();
        assert_eq!(end.unwrap_err().kind(), std::io::ErrorKind::UnexpectedEof);
    });
}

#[test]
fn dropping_the_show_stream_closes_the_servers_sender() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("notify");
    run(async {
        let (params_tx, _params) = mpsc::unbounded_channel();
        let (gone_tx, mut gone) = mpsc::unbounded_channel();
        let listener = server::bind(&path).unwrap();
        let fake = FakeNotify {
            params: params_tx,
            gone: gone_tx,
        };
        tokio::task::spawn_local(server::serve(listener, Rc::new(fake)));

        let mut stream = notify::show(&path, &show_params("hold")).await.unwrap();
        assert_eq!(stream.next().await.unwrap().kind, NotifyKind::Shown);
        drop(stream);
        tokio::time::timeout(TIMEOUT, gone.recv())
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn show_oneway_sends_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("notify");
    run(async {
        let (params_tx, mut params) = mpsc::unbounded_channel();
        let (gone_tx, _gone) = mpsc::unbounded_channel();
        let listener = server::bind(&path).unwrap();
        let fake = FakeNotify {
            params: params_tx,
            gone: gone_tx,
        };
        tokio::task::spawn_local(server::serve(listener, Rc::new(fake)));

        let mut low = show_params("note");
        low.urgency = Some(Urgency::Low);
        low.actions = None;
        notify::show_oneway(&path, &low).await.unwrap();
        let sent = tokio::time::timeout(TIMEOUT, params.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sent["title"], "note");
        assert_eq!(sent["urgency"], "low");
    });
}
