//! `tetsu comp config` against a fake compositor served by the facade.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::time::Duration;

use serde_json::json;
use tetsu_bus::config::{self, ConfigEntry, Diagnostic, GET_CONFIG, GetConfigReply, Origin};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response};
use tetsu_bus_contract::{check_reply, load_dir};
use tetsu_cli::comp::{self, Command};
use tokio::sync::oneshot;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
enum Mode {
    Full,
    Deny,
    /// Takes the call and never answers.
    Silent,
}

struct FakeCompositor {
    mode: Mode,
    held: RefCell<Vec<oneshot::Sender<Result<serde_json::Value, ErrorReply>>>>,
}

impl FakeCompositor {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            held: RefCell::new(Vec::new()),
        }
    }
}

fn entry(key: &str, origin: Origin, writable: bool, method: Option<&str>) -> ConfigEntry {
    ConfigEntry {
        key: key.into(),
        value: json!(7),
        origin,
        default_value: Some(json!(1)),
        managed_value: (origin != Origin::Default).then(|| json!(5)),
        user_value: (origin == Origin::User).then(|| json!(7)),
        writable,
        method: method.map(str::to_owned),
    }
}

fn fake_reply() -> GetConfigReply {
    GetConfigReply {
        entries: vec![
            entry("a.default", Origin::Default, true, None),
            entry("b.managed", Origin::Managed, true, None),
            entry("c.user", Origin::User, true, None),
            entry(
                "d.device",
                Origin::Managed,
                false,
                Some("org.tetsu.comp.render.v1.SetRenderDevice"),
            ),
            entry("e.file_only", Origin::Default, false, None),
        ],
        diagnostics: vec![
            Diagnostic {
                line: Some(3),
                key: Some("c.user".into()),
                message: "expected an integer".into(),
            },
            Diagnostic {
                line: None,
                key: None,
                message: "the file is not valid TOML".into(),
            },
        ],
    }
}

fn checked(reply: &serde_json::Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tetsu-bus/idl");
    let contract = load_dir(&dir).expect("the contract loads");
    let found = check_reply(&contract, GET_CONFIG, reply);
    assert!(found.is_empty(), "{found:?} for {reply}");
}

impl Handler for FakeCompositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != GET_CONFIG {
            return Response::method_not_found(&request.method);
        }
        match self.mode {
            Mode::Deny => Response::Error(ErrorReply::new(config::PERMISSION_DENIED, json!({}))),
            Mode::Silent => {
                let (tx, rx) = oneshot::channel();
                self.held.borrow_mut().push(tx);
                Response::Pending(rx)
            }
            Mode::Full => {
                let reply = serde_json::to_value(fake_reply()).unwrap();
                checked(&reply);
                Response::Reply(reply)
            }
        }
    }
}

fn serve(path: &Path, mode: Mode) {
    let listener = server::bind(path).unwrap();
    tokio::task::spawn_local(server::serve(listener, Rc::new(FakeCompositor::new(mode))));
}

fn in_runtime<F: std::future::Future<Output = ()>>(f: impl FnOnce(PathBuf) -> F) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("comp");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, f(path));
}

struct Outcome {
    status: u8,
    out: String,
    err: String,
}

async fn run(json: bool, path: &Path, timeout: Duration) -> Outcome {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let status = comp::run(Command::Config { json }, path, timeout, &mut out, &mut err).await;
    Outcome {
        status,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

#[test]
fn the_table_lists_every_key_then_the_diagnostics() {
    in_runtime(|path| async move {
        serve(&path, Mode::Full);
        let got = run(false, &path, TIMEOUT).await;
        assert_eq!((got.status, got.err.as_str()), (0, ""));
        for e in fake_reply().entries {
            assert!(got.out.contains(&e.key), "{}: {}", e.key, got.out);
        }
        assert!(got.out.contains("user (overrides managed)"), "{}", got.out);
        assert!(got.out.contains("line 3: c.user: expected an integer"));
        assert!(got.out.contains("the file is not valid TOML"));
    });
}

#[test]
fn json_is_the_fake_reply() {
    in_runtime(|path| async move {
        serve(&path, Mode::Full);
        let got = run(true, &path, TIMEOUT).await;
        assert_eq!(got.status, 0);
        let back: GetConfigReply = serde_json::from_str(&got.out).unwrap();
        assert_eq!(back, fake_reply());
    });
}

#[test]
fn nobody_listening_is_not_running() {
    in_runtime(|path| async move {
        let got = run(false, &path.with_file_name("absent"), TIMEOUT).await;
        assert_eq!(got.status, 1);
        assert!(
            got.err.starts_with("tetsu: tetsu-comp is not running"),
            "{}",
            got.err
        );
        assert_eq!(got.out, "");
    });
}

#[test]
fn a_refusal_is_reported() {
    in_runtime(|path| async move {
        serve(&path, Mode::Deny);
        let got = run(false, &path, TIMEOUT).await;
        assert_eq!(got.status, 1);
        assert!(got.err.starts_with("tetsu: not allowed:"), "{}", got.err);
    });
}

#[test]
fn a_silent_compositor_times_out() {
    in_runtime(|path| async move {
        serve(&path, Mode::Silent);
        let got = run(false, &path, Duration::from_millis(200)).await;
        assert_eq!(got.status, 1);
        assert!(
            got.err
                .starts_with("tetsu: tetsu-comp did not answer within"),
            "{}",
            got.err
        );
    });
}

/// Serves the fake on a thread of its own, so a child process can call it.
fn serve_on_thread(path: PathBuf) {
    let (ready, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        LocalSet::new().block_on(&runtime, async move {
            serve(&path, Mode::Full);
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
    });
    wait.recv_timeout(TIMEOUT).expect("the fake is bound");
}

fn tetsu(runtime_dir: &Path) -> Process {
    let mut process = Process::new(env!("CARGO_BIN_EXE_tetsu"));
    process
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("TETSU_COMP_SOCKET")
        .env("XDG_RUNTIME_DIR", runtime_dir);
    process
}

#[test]
fn the_binary_shows_the_config_found_through_the_environment() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("fake-comp");
    serve_on_thread(socket.clone());
    let out = tetsu(dir.path())
        .args(["comp", "config"])
        .env("TETSU_COMP_SOCKET", &socket)
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for e in fake_reply().entries {
        assert!(stdout.contains(&e.key), "{}: {stdout}", e.key);
    }
}

#[test]
fn the_binary_without_a_compositor_says_so_and_fails() {
    let empty = tempfile::tempdir().unwrap();
    let out = tetsu(empty.path())
        .args(["comp", "config"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("tetsu-comp is not running"), "{stderr}");
}

#[test]
fn the_binary_rejects_an_unknown_comp_command() {
    let empty = tempfile::tempdir().unwrap();
    let out = tetsu(empty.path()).args(["comp", "frob"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
