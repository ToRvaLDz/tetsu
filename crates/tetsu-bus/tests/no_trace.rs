//! The facade never traces message bodies (`@sensitive` values travel in
//! them): a global subscriber at TRACE sees no event that carries one.

use std::fmt::Debug;
use std::rc::Rc;
use std::sync::{Mutex, Once};

use serde_json::json;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, Handler, Request, Response};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::task::LocalSet;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

const SENTINEL: &str = "S3NT1NEL-NOTRACE";

static CAPTURED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INSTALL: Once = Once::new();

/// Records every event and span field as text.
struct Capture;

struct Text(String);

impl Visit for Text {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

fn keep(text: Text) {
    CAPTURED.lock().unwrap().push(text.0);
}

impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let mut text = Text(String::new());
        attrs.record(&mut text);
        keep(text);
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, values: &Record<'_>) {
        let mut text = Text(String::new());
        values.record(&mut text);
        keep(text);
    }

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut text = Text(String::new());
        event.record(&mut text);
        keep(text);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

fn install() {
    INSTALL.call_once(|| {
        tracing::subscriber::set_global_default(Capture).unwrap();
    });
}

fn captured() -> Vec<String> {
    CAPTURED.lock().unwrap().clone()
}

struct Echo;

impl Handler for Echo {
    async fn call(&self, _: Rc<Peer>, request: Request) -> Response {
        Response::Reply(json!({ "echo": request.parameters }))
    }
}

#[test]
fn the_server_traces_no_message_body() {
    install();
    tracing::trace!("control event");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tetsu").join("org.example.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let listener = server::bind(&path).unwrap();
        tokio::task::spawn_local(server::serve(listener, Rc::new(Echo)));
        let mut stream = UnixStream::connect(&path).await.unwrap();
        let call = json!({
            "method": "org.example.Echo",
            "parameters": { "value": SENTINEL }
        });
        let mut bytes = serde_json::to_vec(&call).unwrap();
        bytes.push(0);
        stream.write_all(&bytes).await.unwrap();
        let mut reply = Vec::new();
        let mut chunk = [0u8; 4096];
        while !reply.contains(&0) {
            let n = stream.read(&mut chunk).await.unwrap();
            assert!(n > 0, "server closed the connection");
            reply.extend_from_slice(&chunk[..n]);
        }
        assert!(String::from_utf8_lossy(&reply).contains(SENTINEL));
    });
    let events = captured();
    assert!(
        events.iter().any(|e| e.contains("control event")),
        "the capturing subscriber works"
    );
    let leaked: Vec<_> = events.iter().filter(|e| e.contains(SENTINEL)).collect();
    assert!(leaked.is_empty(), "a message body was traced: {leaked:#?}");
}
