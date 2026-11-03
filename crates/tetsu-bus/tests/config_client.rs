//! The `GetConfig` client against a fake compositor served by the facade.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::cap::ClientError;
use tetsu_bus::config::{
    self, ConfigEntry, Diagnostic, GET_CONFIG, GetConfigReply, INTERFACE, Origin,
};
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response};
use tetsu_bus_contract::{check_reply, load_dir};
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);

/// What the fake answers to `GetConfig`.
#[derive(Clone, Copy)]
enum Mode {
    Full,
    Deny,
    MissingEntries,
}

struct FakeCompositor(Mode);

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

fn checked(reply: &Value) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl");
    let contract = load_dir(&dir).expect("the contract loads");
    let found = check_reply(&contract, GET_CONFIG, reply);
    assert!(found.is_empty(), "{found:?} for {reply}");
}

impl Handler for FakeCompositor {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        if request.method != GET_CONFIG {
            return Response::method_not_found(&request.method);
        }
        match self.0 {
            Mode::Deny => Response::Error(ErrorReply::new(config::PERMISSION_DENIED, json!({}))),
            Mode::MissingEntries => Response::Reply(json!({ "diagnostics": [] })),
            Mode::Full => {
                let reply = serde_json::to_value(fake_reply()).unwrap();
                checked(&reply);
                Response::Reply(reply)
            }
        }
    }
}

fn run<F: std::future::Future<Output = ()>>(f: impl FnOnce(PathBuf) -> F) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("run").join("comp");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, f(path));
}

fn serve(path: &std::path::Path, mode: Mode) {
    let listener = server::bind(path).unwrap();
    tokio::task::spawn_local(server::serve(listener, Rc::new(FakeCompositor(mode))));
}

#[test]
fn the_method_name_is_the_interface_plus_the_method() {
    assert_eq!(GET_CONFIG, format!("{INTERFACE}.GetConfig"));
}

#[test]
fn get_config_returns_what_the_compositor_sent() {
    run(|path| async move {
        serve(&path, Mode::Full);
        let got = tokio::time::timeout(TIMEOUT, config::get_config(&path))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got, fake_reply());
    });
}

#[test]
fn get_config_surfaces_a_remote_error() {
    run(|path| async move {
        serve(&path, Mode::Deny);
        let error = tokio::time::timeout(TIMEOUT, config::get_config(&path))
            .await
            .unwrap()
            .unwrap_err();
        match error {
            ClientError::Remote { error, .. } => assert_eq!(error, config::PERMISSION_DENIED),
            other => panic!("not a remote error: {other:?}"),
        }
    });
}

#[test]
fn get_config_fails_with_not_found_when_nobody_listens() {
    run(|path| async move {
        let error = config::get_config(&path.with_file_name("absent"))
            .await
            .unwrap_err();
        match error {
            ClientError::Io(e) => assert_eq!(e.kind(), ErrorKind::NotFound),
            other => panic!("not an I/O error: {other:?}"),
        }
    });
}

#[test]
fn get_config_rejects_a_reply_without_entries() {
    run(|path| async move {
        serve(&path, Mode::MissingEntries);
        let error = tokio::time::timeout(TIMEOUT, config::get_config(&path))
            .await
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, ClientError::Io(_)), "{error:?}");
    });
}
