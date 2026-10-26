//! The facade server over a real Unix socket, spoken to in raw Varlink
//! (JSON objects terminated by NUL) so the wire format is tested too.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Echo, Count (streams `n` replies) and Hold (streams one reply, then
/// stays open and reports when the caller is gone).
struct Test {
    gone: mpsc::UnboundedSender<()>,
}

impl Handler for Test {
    async fn call(&self, peer: Rc<Peer>, request: Request) -> Response {
        match request.method.as_str() {
            "org.example.Echo" => Response::Reply(json!({
                "value": request.parameters["value"],
                "pid": peer.pid,
                "oneway": request.oneway,
            })),
            "org.example.Count" => {
                let n = request.parameters["n"].as_i64().unwrap_or(0);
                let (tx, rx) = mpsc::unbounded_channel();
                for i in 1..n {
                    tx.send(StreamItem::Reply(json!({ "i": i }))).unwrap();
                }
                tx.send(StreamItem::Last(json!({ "i": n }))).unwrap();
                Response::Stream(rx)
            }
            "org.example.Hold" => {
                let (tx, rx) = mpsc::unbounded_channel();
                tx.send(StreamItem::Reply(json!({ "held": true }))).unwrap();
                let gone = self.gone.clone();
                tokio::task::spawn_local(async move {
                    tx.closed().await;
                    let _ = gone.send(());
                });
                Response::Stream(rx)
            }
            "org.example.Fail" => Response::Error(ErrorReply::new(
                "org.example.Broken",
                json!({ "why": "asked to" }),
            )),
            other => Response::method_not_found(other),
        }
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

    async fn recv(&mut self) -> Value {
        loop {
            if let Some(end) = self.1.iter().position(|b| *b == 0) {
                let message: Vec<u8> = self.1.drain(..=end).collect();
                return serde_json::from_slice(&message[..end]).unwrap();
            }
            let mut chunk = [0u8; 4096];
            let n = tokio::time::timeout(TIMEOUT, self.0.read(&mut chunk))
                .await
                .expect("reply in time")
                .unwrap();
            assert!(n > 0, "server closed the connection");
            self.1.extend_from_slice(&chunk[..n]);
        }
    }
}

fn socket_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("tetsu").join("org.example.v1")
}

/// Runs `test` against a server bound in a fresh directory.
fn with_server<F, Fut>(test: F)
where
    F: FnOnce(PathBuf, mpsc::UnboundedReceiver<()>) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (gone_tx, gone_rx) = mpsc::unbounded_channel();
        let listener = server::bind(&path).unwrap();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let handler = Rc::new(Test { gone: gone_tx });
        tokio::task::spawn_local(async move {
            tokio::select! {
                result = server::serve(listener, handler) => result.unwrap(),
                _ = stop_rx => {}
            }
        });
        test(path, gone_rx).await;
        let _ = stop_tx.send(());
    });
}

#[test]
fn bind_creates_a_private_directory_and_socket() {
    with_server(|path, _| async move {
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        let socket_mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        assert_eq!(socket_mode & 0o777, 0o600);
    });
}

#[test]
fn bind_replaces_a_stale_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        drop(server::bind(&path).unwrap());
        server::bind(&path).expect("a stale socket is replaced");
    });
}

#[test]
fn a_single_reply_carries_the_peer() {
    with_server(|path, _| async move {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({ "method": "org.example.Echo", "parameters": { "value": 7 } }))
            .await;
        let reply = client.recv().await;
        assert_eq!(reply["parameters"]["value"], 7);
        assert_eq!(reply["parameters"]["pid"], std::process::id());
        assert_eq!(reply["parameters"]["oneway"], false);
        assert!(reply.get("continues").is_none_or(|c| c == false));
    });
}

#[test]
fn calls_on_one_connection_are_answered_in_order() {
    with_server(|path, _| async move {
        let mut client = Client::connect(&path).await;
        for value in 1..=3 {
            client
                .send(json!({ "method": "org.example.Echo", "parameters": { "value": value } }))
                .await;
        }
        for value in 1..=3 {
            assert_eq!(client.recv().await["parameters"]["value"], value);
        }
    });
}

#[test]
fn a_oneway_call_gets_no_reply() {
    with_server(|path, _| async move {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({ "method": "org.example.Echo", "parameters": { "value": 1 }, "oneway": true }))
            .await;
        client
            .send(json!({ "method": "org.example.Echo", "parameters": { "value": 2 } }))
            .await;
        assert_eq!(client.recv().await["parameters"]["value"], 2);
    });
}

#[test]
fn a_stream_continues_then_ends() {
    with_server(|path, _| async move {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({ "method": "org.example.Count", "parameters": { "n": 3 }, "more": true }))
            .await;
        for i in 1..=2 {
            let reply = client.recv().await;
            assert_eq!(reply["parameters"]["i"], i);
            assert_eq!(reply["continues"], true);
        }
        let last = client.recv().await;
        assert_eq!(last["parameters"]["i"], 3);
        assert!(last.get("continues").is_none_or(|c| c == false));
        // The connection is usable again after the stream ended.
        client
            .send(json!({ "method": "org.example.Echo", "parameters": { "value": 9 } }))
            .await;
        assert_eq!(client.recv().await["parameters"]["value"], 9);
    });
}

#[test]
fn a_streaming_reply_to_a_call_without_more_is_refused() {
    with_server(|path, _| async move {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({ "method": "org.example.Count", "parameters": { "n": 2 } }))
            .await;
        assert_eq!(
            client.recv().await["error"],
            "org.varlink.service.ExpectedMore"
        );
    });
}

#[test]
fn disconnecting_during_a_stream_reaches_the_handler() {
    with_server(|path, mut gone| async move {
        let mut client = Client::connect(&path).await;
        client
            .send(json!({ "method": "org.example.Hold", "more": true }))
            .await;
        assert_eq!(client.recv().await["parameters"]["held"], true);
        drop(client);
        tokio::time::timeout(TIMEOUT, gone.recv())
            .await
            .expect("the handler sees the caller go")
            .unwrap();
    });
}

#[test]
fn errors_and_unknown_methods_are_varlink_errors() {
    with_server(|path, _| async move {
        let mut client = Client::connect(&path).await;
        client.send(json!({ "method": "org.example.Fail" })).await;
        let reply = client.recv().await;
        assert_eq!(reply["error"], "org.example.Broken");
        assert_eq!(reply["parameters"]["why"], "asked to");
        client.send(json!({ "method": "org.example.Nope" })).await;
        let reply = client.recv().await;
        assert_eq!(reply["error"], "org.varlink.service.MethodNotFound");
        assert_eq!(reply["parameters"]["method"], "org.example.Nope");
    });
}

#[test]
fn bind_refuses_a_socket_someone_still_answers_on() {
    with_server(|path, _| async move {
        let error = server::bind(&path).expect_err("a live socket is kept");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        assert!(path.exists());
    });
}

#[test]
fn dropping_the_listener_removes_only_its_own_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let first = server::bind(&path).unwrap();
        drop(first);
        assert!(!path.exists(), "its own socket is removed");

        let old = server::bind(&path).unwrap();
        // Another instance took the path over (the old socket was stale).
        std::fs::remove_file(&path).unwrap();
        let _new = server::bind(&path).unwrap();
        drop(old);
        assert!(path.exists(), "someone else's socket is left alone");
    });
}

#[test]
fn bind_refuses_a_directory_others_can_enter() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let error = server::bind(&path).expect_err("a 0755 directory is refused");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    });
}

fn current_thread() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn mode_of(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn bind_system_creates_a_public_socket_in_a_world_readable_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    current_thread().block_on(async {
        let _listener = server::bind_system(&path).unwrap();
        assert_eq!(mode_of(path.parent().unwrap()), 0o755);
        assert_eq!(mode_of(&path), 0o666);
    });
}

#[test]
fn bind_system_refuses_a_directory_others_can_write() {
    for mode in [0o775, 0o757, 0o777] {
        let dir = tempfile::tempdir().unwrap();
        let path = socket_path(&dir);
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        std::fs::set_permissions(
            path.parent().unwrap(),
            std::fs::Permissions::from_mode(mode),
        )
        .unwrap();
        current_thread().block_on(async {
            let error = server::bind_system(&path).expect_err("a writable directory is refused");
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{mode:o}"
            );
        });
        assert!(!path.exists());
    }
}

#[test]
fn bind_system_accepts_an_existing_directory_of_this_user() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o750),
    )
    .unwrap();
    current_thread().block_on(async {
        server::bind_system(&path).expect("a directory of ours that nobody can write is fine");
    });
}

#[test]
fn bind_system_replaces_a_stale_socket_and_keeps_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    current_thread().block_on(async {
        drop(server::bind_system(&path).unwrap());
        let live = server::bind_system(&path).expect("a stale socket is replaced");
        let error = server::bind_system(&path).expect_err("a live socket is kept");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);
        drop(live);
    });
}

#[test]
fn a_system_socket_file_goes_with_its_listener() {
    let dir = tempfile::tempdir().unwrap();
    let path = socket_path(&dir);
    current_thread().block_on(async {
        let listener = server::bind_system(&path).unwrap();
        assert!(path.exists());
        drop(listener);
        assert!(!path.exists());
    });
}
