//! The Done-when of #32 over the real `org.tetsu.notify.v1` socket, with the
//! screen replaced by a driver that reads the view and clicks like the user.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::server;
use tetsu_notifyd::core::{ARM_DELAY, NoteView, Source, View};
use tetsu_notifyd::daemon::{Command, Daemon, UiMsg};
use tetsu_notifyd::varlink::NotifyService;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(200);

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

    async fn try_recv(&mut self, within: Duration) -> Option<Value> {
        loop {
            if let Some(end) = self.1.iter().position(|b| *b == 0) {
                let message: Vec<u8> = self.1.drain(..=end).collect();
                return Some(serde_json::from_slice(&message[..end]).unwrap());
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(within, self.0.read(&mut chunk))
                .await
                .ok()?
                .unwrap();
            assert!(n > 0, "server closed the connection");
            self.1.extend_from_slice(&chunk[..n]);
        }
    }

    async fn recv(&mut self) -> Value {
        self.try_recv(TIMEOUT).await.expect("a reply in time")
    }

    async fn show_approval(&mut self, title: &str) -> String {
        self.send(json!({
            "method": "org.tetsu.notify.v1.Show",
            "parameters": {
                "title": title,
                "urgency": "critical",
                "actions": [
                    { "id": "approve", "label": "Approve" },
                    { "id": "deny", "label": "Deny" }
                ]
            },
            "more": true
        }))
        .await;
        let shown = self.recv().await;
        assert_eq!(shown["parameters"]["reply"]["kind"], "shown", "{shown}");
        assert_eq!(shown["continues"], true);
        shown["parameters"]["reply"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

struct Screen {
    rx: mpsc::UnboundedReceiver<UiMsg>,
    commands: mpsc::UnboundedSender<Command>,
    view: View,
}

impl Screen {
    /// Waits until the drawn view satisfies `pred`.
    async fn until(&mut self, pred: impl Fn(&View) -> bool) -> View {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        while !pred(&self.view) {
            let msg = tokio::time::timeout_at(deadline, self.rx.recv())
                .await
                .expect("the view in time")
                .unwrap();
            if let UiMsg::View(view) = msg {
                self.view = view;
            }
        }
        self.view.clone()
    }

    async fn popup(&mut self, title: &str) -> NoteView {
        let view = self
            .until(|v| v.popups.iter().any(|n| n.title == title))
            .await;
        view.popups.into_iter().find(|n| n.title == title).unwrap()
    }

    fn click(&self, note: &NoteView, action: &str) {
        self.commands
            .send(Command::Click {
                note: note.id,
                action: action.into(),
            })
            .unwrap();
    }
}

fn run(test: impl AsyncFnOnce(PathBuf, Screen)) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tetsu").join("org.tetsu.notify.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let (daemon, commands) = Daemon::new(ui_tx);
        tokio::task::spawn_local(daemon.run());
        commands.send(Command::Lock(false)).unwrap();
        let listener = server::bind(&path).unwrap();
        let service = Rc::new(NotifyService::new(commands.clone()));
        tokio::task::spawn_local(server::serve(listener, service));
        let screen = Screen {
            rx: ui_rx,
            commands,
            view: View::default(),
        };
        test(path, screen).await;
    });
}

async fn armed() {
    tokio::time::sleep(ARM_DELAY + Duration::from_millis(100)).await;
}

#[test]
fn an_approval_shows_in_the_tetsu_frame_and_answers_its_caller_only() {
    run(async |path, mut screen| {
        let mut alice = Client::connect(&path).await;
        let mut bob = Client::connect(&path).await;
        alice.show_approval("Alice asks").await;
        bob.show_approval("Bob asks").await;
        let note = screen.popup("Alice asks").await;
        assert!(matches!(note.source, Source::Tetsu(_)));
        assert_eq!(note.buttons.len(), 2);

        armed().await;
        screen.click(&note, "approve");
        let reply = alice.recv().await;
        assert_eq!(reply["parameters"]["reply"]["kind"], "invoked");
        assert_eq!(reply["parameters"]["reply"]["action_id"], "approve");
        assert!(reply.get("continues").is_none_or(|c| c == false));
        assert!(bob.try_recv(QUIET).await.is_none(), "Bob hears nothing");
    });
}

#[test]
fn a_click_while_locked_is_refused_and_works_after_unlock() {
    run(async |path, mut screen| {
        let mut alice = Client::connect(&path).await;
        alice.show_approval("Locked ask").await;
        let note = screen.popup("Locked ask").await;
        screen.commands.send(Command::Lock(true)).unwrap();
        screen.until(|v| v.locked).await;
        armed().await;
        screen.click(&note, "approve");
        assert!(
            alice.try_recv(QUIET).await.is_none(),
            "refused while locked"
        );

        screen.commands.send(Command::Lock(false)).unwrap();
        let note = screen.popup("Locked ask").await;
        armed().await;
        screen.click(&note, "deny");
        let reply = alice.recv().await;
        assert_eq!(reply["parameters"]["reply"]["action_id"], "deny");
    });
}

#[test]
fn watch_pending_counts_approvals_from_every_caller() {
    run(async |path, mut screen| {
        let mut watcher = Client::connect(&path).await;
        watcher
            .send(json!({ "method": "org.tetsu.notify.v1.WatchPending", "more": true }))
            .await;
        let initial = watcher.recv().await;
        assert_eq!(initial["parameters"]["event"]["kind"], "initial");
        assert_eq!(initial["parameters"]["event"]["count"], 0);

        let mut alice = Client::connect(&path).await;
        let mut bob = Client::connect(&path).await;
        alice.show_approval("One").await;
        assert_eq!(watcher.recv().await["parameters"]["event"]["count"], 1);
        bob.show_approval("Two").await;
        let changed = watcher.recv().await;
        assert_eq!(changed["parameters"]["event"]["kind"], "changed");
        assert_eq!(changed["parameters"]["event"]["count"], 2);

        let mut reader = Client::connect(&path).await;
        reader
            .send(json!({ "method": "org.tetsu.notify.v1.GetPending" }))
            .await;
        assert_eq!(reader.recv().await["parameters"]["count"], 2);

        let note = screen.popup("One").await;
        armed().await;
        screen.click(&note, "approve");
        assert_eq!(watcher.recv().await["parameters"]["event"]["count"], 1);
    });
}

#[test]
fn an_approval_pushed_into_the_panel_stays_pending_and_answerable() {
    run(async |path, mut screen| {
        let mut first = Client::connect(&path).await;
        first.show_approval("First").await;
        let mut others = Vec::new();
        for title in ["Second", "Third", "Fourth"] {
            let mut client = Client::connect(&path).await;
            client.show_approval(title).await;
            others.push(client);
        }
        let view = screen
            .until(|v| v.pending.len() == 4 && !v.popups.iter().any(|n| n.title == "First"))
            .await;
        let note = view.pending.iter().find(|n| n.title == "First").unwrap();
        assert!(first.try_recv(QUIET).await.is_none(), "no dismissed");

        screen.commands.send(Command::PanelOpened).unwrap();
        armed().await;
        screen.click(note, "approve");
        let reply = first.recv().await;
        assert_eq!(reply["parameters"]["reply"]["action_id"], "approve");
    });
}

#[test]
fn a_caller_disconnecting_withdraws_its_approval() {
    run(async |path, mut screen| {
        let mut alice = Client::connect(&path).await;
        alice.show_approval("Going away").await;
        screen.popup("Going away").await;
        drop(alice);
        let view = screen
            .until(|v| v.popups.is_empty() && v.pending.is_empty())
            .await;
        assert!(view.popups.is_empty());
    });
}

#[test]
fn close_ends_the_stream_and_unknown_ids_are_not_found() {
    run(async |path, mut screen| {
        let mut alice = Client::connect(&path).await;
        let id = alice.show_approval("Closing").await;
        screen.popup("Closing").await;
        let mut other_connection = Client::connect(&path).await;
        other_connection
            .send(json!({ "method": "org.tetsu.notify.v1.Close", "parameters": { "id": id } }))
            .await;
        assert!(other_connection.recv().await.get("error").is_none());
        let reply = alice.recv().await;
        assert_eq!(reply["parameters"]["reply"]["kind"], "dismissed");
        assert_eq!(reply["parameters"]["reply"]["reason"], "closed");

        other_connection
            .send(json!({ "method": "org.tetsu.notify.v1.Close", "parameters": { "id": id } }))
            .await;
        let reply = other_connection.recv().await;
        assert_eq!(reply["error"], "org.tetsu.notify.v1.NotificationNotFound");
        assert_eq!(reply["parameters"]["id"], id);
    });
}

#[test]
fn oneway_show_draws_without_replying_and_open_panel_reaches_the_screen() {
    run(async |path, mut screen| {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({
                "method": "org.tetsu.notify.v1.Show",
                "parameters": { "title": "Fire and forget" },
                "oneway": true
            }))
            .await;
        screen.popup("Fire and forget").await;
        client
            .send(json!({ "method": "org.tetsu.notify.v1.OpenPanel" }))
            .await;
        assert!(client.recv().await.get("error").is_none());
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            let msg = tokio::time::timeout_at(deadline, screen.rx.recv())
                .await
                .expect("OpenPanel in time")
                .unwrap();
            if matches!(msg, UiMsg::OpenPanel) {
                break;
            }
        }
    });
}

#[test]
fn bad_calls_get_varlink_errors() {
    run(async |path, _screen| {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({ "method": "org.tetsu.notify.v1.Show", "parameters": { "title": "x" } }))
            .await;
        assert_eq!(
            client.recv().await["error"],
            "org.varlink.service.ExpectedMore"
        );
        client
            .send(json!({ "method": "org.tetsu.notify.v1.Show", "parameters": {}, "more": true }))
            .await;
        let reply = client.recv().await;
        assert_eq!(reply["error"], "org.varlink.service.InvalidParameter");
        client
            .send(json!({ "method": "org.tetsu.notify.v1.Invoke", "parameters": {} }))
            .await;
        assert_eq!(
            client.recv().await["error"],
            "org.varlink.service.MethodNotFound"
        );
    });
}
