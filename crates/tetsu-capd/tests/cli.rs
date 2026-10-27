//! `tetsu cap` over a fake service, and once over a real one.

use std::io;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use tetsu_bus::cap::{self, Capability, ClientError, Grant, Layer, SetParams, State};
use tetsu_bus::server;
use tetsu_capd::audit::RecordingSink;
use tetsu_capd::cli::{CapApi, Command, parse, run};
use tetsu_capd::clock::SystemClock;
use tetsu_capd::engine::Engine;
use tetsu_capd::service::CapService;
use tetsu_capd::store::{Layers, Store};
use tokio::task::LocalSet;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn the_commands_parse() {
    assert_eq!(parse(&args(&["list"])), Ok(Command::List));
    assert_eq!(parse(&args(&["-h"])), Ok(Command::Help));
    assert_eq!(parse(&args(&["--help"])), Ok(Command::Help));
    assert_eq!(
        parse(&args(&["set", "packages.install", "notify"])),
        Ok(Command::Set {
            class: "packages.install".into(),
            scope: None,
            state: Grant::Notify,
        })
    );
    assert_eq!(
        parse(&args(&["set", "mail.send", "off", "--scope", "work"])),
        Ok(Command::Set {
            class: "mail.send".into(),
            scope: Some("work".into()),
            state: Grant::Off,
        })
    );
    assert_eq!(
        parse(&args(&["revoke", "packages.install"])),
        Ok(Command::Revoke {
            class: "packages.install".into(),
            scope: None,
        })
    );
    assert_eq!(
        parse(&args(&["revoke", "mail.send", "--scope", "work"])),
        Ok(Command::Revoke {
            class: "mail.send".into(),
            scope: Some("work".into()),
        })
    );
}

#[test]
fn usage_errors_carry_the_usage_text() {
    for bad in [
        &[][..],
        &["nope"],
        &["list", "extra"],
        &["set"],
        &["set", "a.b"],
        &["set", "a.b", "maybe"],
        &["set", "a.b", "unset"],
        &["set", "a.b", "ask", "--scope"],
        &["set", "a.b", "ask", "--other", "x"],
        &["set", "a.b", "ask", "x"],
        &["revoke"],
        &["revoke", "a.b", "c"],
    ] {
        let error = parse(&args(bad)).unwrap_err();
        assert!(error.contains("usage: tetsu cap"), "{bad:?}: {error}");
    }
}

struct Fake {
    caps: Vec<Capability>,
    set_result: Result<(), (&'static str, serde_json::Value)>,
    list_fails: Option<io::ErrorKind>,
    sets: Vec<SetParams>,
}

impl Fake {
    fn new() -> Self {
        Self {
            caps: Vec::new(),
            set_result: Ok(()),
            list_fails: None,
            sets: Vec::new(),
        }
    }
}

impl CapApi for Fake {
    async fn list(&mut self) -> Result<Vec<Capability>, ClientError> {
        match self.list_fails {
            Some(kind) => Err(ClientError::Io(kind.into())),
            None => Ok(self.caps.clone()),
        }
    }

    async fn set(&mut self, params: &SetParams) -> Result<(), ClientError> {
        self.sets.push(params.clone());
        self.set_result
            .clone()
            .map_err(|(error, parameters)| ClientError::Remote {
                error: error.to_owned(),
                parameters,
            })
    }
}

fn capability(class: &str, state: State, layer: Option<Layer>) -> Capability {
    Capability {
        class: class.into(),
        scope: None,
        state,
        layer,
        destructive: false,
        methods: vec![],
        asks: 0,
        approvals: 0,
        denials: 0,
        suppressions: 0,
        last_used_at: None,
        suppressed_until_at: None,
    }
}

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn go(command: Command, api: &mut Fake) -> (u8, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = block_on(run(command, api, &mut out, &mut err));
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn list_prints_an_aligned_table() {
    let mut api = Fake::new();
    let mut install = capability("packages.install", State::Ask, Some(Layer::Managed));
    install.asks = 3;
    install.approvals = 2;
    install.denials = 1;
    install.suppressions = 4;
    install.last_used_at = Some("2026-10-25T09:31:00.000Z".into());
    install.suppressed_until_at = Some("2026-10-26T09:31:00.000Z".into());
    let mut update = capability("system.update", State::Unset, None);
    update.destructive = true;
    let mut scoped = capability("mail.send", State::Off, Some(Layer::User));
    scoped.scope = Some("work".into());
    api.caps = vec![install, update, scoped];
    let (code, out, err) = go(Command::List, &mut api);
    assert_eq!((code, err.as_str()), (0, ""));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    let header = lines[0];
    for title in [
        "CLASS",
        "SCOPE",
        "STATE",
        "LAYER",
        "ASKS",
        "LAST USED",
        "SUPPRESSED UNTIL",
    ] {
        assert!(header.contains(title), "{header}");
    }
    // Columns line up under their titles.
    let state = header.find("STATE").unwrap();
    assert!(lines[1][state..].starts_with("ask"), "{}", lines[1]);
    assert!(lines[2][state..].starts_with("unset"), "{}", lines[2]);
    assert!(lines[3][state..].starts_with("off"), "{}", lines[3]);
    let layer = header.find("LAYER").unwrap();
    assert!(lines[1][layer..].starts_with("managed"));
    assert!(lines[2][layer..].starts_with('-'));
    let scope = header.find("SCOPE").unwrap();
    assert!(lines[3][scope..].starts_with("work"));
    assert!(lines[2].contains('!'), "destructive marker: {}", lines[2]);
    assert!(!lines[1].contains('!'));
    assert!(lines[1].contains("2026-10-25T09:31:00.000Z"));
    assert!(lines[1].contains("2026-10-26T09:31:00.000Z"));
}

#[test]
fn set_and_revoke_print_one_line_and_send_the_call() {
    let mut api = Fake::new();
    let (code, out, _) = go(
        Command::Set {
            class: "packages.install".into(),
            scope: None,
            state: Grant::Notify,
        },
        &mut api,
    );
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(out.contains("packages.install") && out.contains("notify"));
    assert_eq!(api.sets[0].state, Some(Grant::Notify));

    let (code, out, _) = go(
        Command::Revoke {
            class: "packages.install".into(),
            scope: Some("work".into()),
        },
        &mut api,
    );
    assert_eq!(code, 0);
    assert_eq!(out.lines().count(), 1, "{out}");
    assert_eq!(api.sets[1].state, None);
    assert_eq!(api.sets[1].scope.as_deref(), Some("work"));
}

fn set_command(class: &str, state: Grant) -> Command {
    Command::Set {
        class: class.into(),
        scope: None,
        state,
    }
}

#[test]
fn errors_are_one_clear_line_and_exit_one() {
    let cases = [
        (
            "org.tetsu.cap.v1.OverriddenByUser",
            serde_json::json!({ "class": "aur.review" }),
            "~/.config/tetsu/capabilities.toml",
        ),
        (
            "org.tetsu.cap.v1.UnknownClass",
            serde_json::json!({ "class": "aur.review" }),
            "unknown class",
        ),
        (
            "org.tetsu.cap.v1.PermissionDenied",
            serde_json::json!({}),
            "not allowed",
        ),
    ];
    for (error, parameters, expect) in cases {
        let mut api = Fake::new();
        api.set_result = Err((error, parameters));
        let (code, out, err) = go(set_command("aur.review", Grant::Ask), &mut api);
        assert_eq!((code, out.as_str()), (1, ""), "{error}");
        assert!(err.contains(expect), "{error}: {err}");
        assert_eq!(err.lines().count(), 1, "{err}");
    }
}

#[test]
fn an_unreachable_daemon_says_so() {
    for kind in [io::ErrorKind::NotFound, io::ErrorKind::ConnectionRefused] {
        let mut api = Fake::new();
        api.list_fails = Some(kind);
        let (code, _, err) = go(Command::List, &mut api);
        assert_eq!(code, 1);
        assert!(err.contains("tetsu-capd is not running"), "{err}");
    }
}

#[test]
fn automatic_on_a_destructive_class_says_it_still_asks() {
    let mut api = Fake::new();
    let (code, out, _) = go(set_command("system.update", Grant::Automatic), &mut api);
    assert_eq!(code, 0);
    assert!(out.contains("still asks"), "{out}");
    let (_, out, _) = go(set_command("packages.install", Grant::Automatic), &mut api);
    assert!(!out.contains("still asks"), "{out}");
}

mod offline {
    use std::io;

    use tetsu_bus::notify::{NotifyReply, ShowParams};
    use tetsu_capd::ask::{NotifyChannel, ShowEvents};

    pub struct Nothing;

    impl ShowEvents for Nothing {
        async fn next(&mut self) -> io::Result<NotifyReply> {
            Err(io::ErrorKind::UnexpectedEof.into())
        }
    }

    pub struct Offline;

    impl NotifyChannel for Offline {
        type Events = Nothing;

        async fn show(&self, _: ShowParams) -> io::Result<Nothing> {
            Err(io::ErrorKind::ConnectionRefused.into())
        }

        async fn notice(&self, _: ShowParams) -> io::Result<()> {
            Ok(())
        }
    }
}

#[test]
fn the_command_changes_the_managed_file_of_a_real_service() {
    let dir = tempfile::tempdir().unwrap();
    let layers = Layers {
        image: dir.path().join("image.toml"),
        managed: dir.path().join("state/capabilities.toml"),
        user: dir.path().join("user.toml"),
    };
    let managed: PathBuf = layers.managed.clone();
    let socket = dir.path().join("run/org.tetsu.cap.v1");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let engine = Engine::new(
            offline::Offline,
            SystemClock,
            Rc::new(RecordingSink::default()),
            Store::new(layers),
            vec![Duration::ZERO; 3],
        );
        let service = CapService::new(Rc::new(engine), rustix::process::geteuid().as_raw());
        tokio::task::spawn_local(server::serve(
            server::bind(&socket).unwrap(),
            Rc::new(service),
        ));

        let mut client = cap::Client::connect(&socket).await.unwrap();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let command = set_command("packages.install", Grant::Notify);
        assert_eq!(run(command, &mut client, &mut out, &mut err).await, 0);
        let text = std::fs::read_to_string(&managed).unwrap();
        assert!(text.contains("\"packages.install\" = \"notify\""), "{text}");

        let revoke = Command::Revoke {
            class: "packages.install".into(),
            scope: None,
        };
        assert_eq!(run(revoke, &mut client, &mut out, &mut err).await, 0);
        let text = std::fs::read_to_string(&managed).unwrap();
        assert!(!text.contains("packages.install"), "{text}");

        let mut out = Vec::new();
        assert_eq!(run(Command::List, &mut client, &mut out, &mut err).await, 0);
        assert!(String::from_utf8(out).unwrap().contains("packages.install"));
        assert!(err.is_empty(), "{}", String::from_utf8_lossy(&err));
    });
}
