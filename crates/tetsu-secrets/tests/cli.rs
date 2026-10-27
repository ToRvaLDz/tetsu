//! `tetsu secrets`: parsing, reading the value, and the commands against a
//! live in-process broker. No output ever holds the value.

mod common;

use std::io::{self, Cursor};

use common::{SENTINEL, with_server};
use tetsu_secrets::cli::{Command, Io, Prompt, ValueError, log_argv, parse, read_value, run};
use tetsu_secrets::model::{Host, KindSpec, SecretId, ToolId};

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

fn id(text: &str) -> SecretId {
    SecretId::parse(text).unwrap()
}

#[test]
fn add_parses_its_flags() {
    let got = parse(&args(
        "add demo --kind header:X-Api-Key --host api.example.com:8443 --allow web-fetch --allow git",
    ))
    .unwrap();
    assert_eq!(
        got,
        Command::Add {
            id: id("demo"),
            kind: KindSpec::parse("header:x-api-key").unwrap(),
            host: Host::parse("api.example.com:8443").unwrap(),
            allow: vec![
                ToolId::parse("web-fetch").unwrap(),
                ToolId::parse("git").unwrap()
            ],
        }
    );
    let eq = parse(&args(
        "add demo --kind=bearer --host=a.example.com --allow=a,b",
    ))
    .unwrap();
    let Command::Add { allow, .. } = eq else {
        panic!("not an add");
    };
    assert_eq!(allow.len(), 2);
}

#[test]
fn the_other_commands_parse() {
    assert_eq!(parse(&args("list")).unwrap(), Command::List);
    assert_eq!(
        parse(&args("rm demo")).unwrap(),
        Command::Rm { id: id("demo") }
    );
    assert_eq!(
        parse(&args("remove demo")).unwrap(),
        Command::Rm { id: id("demo") }
    );
    assert_eq!(
        parse(&args("rotate demo")).unwrap(),
        Command::Rotate { id: id("demo") }
    );
    assert_eq!(
        parse(&args("log")).unwrap(),
        Command::Log {
            follow: false,
            lines: None
        }
    );
    assert_eq!(
        parse(&args("log -f -n 20")).unwrap(),
        Command::Log {
            follow: true,
            lines: Some(20)
        }
    );
}

#[test]
fn usage_errors() {
    let bad = [
        "",
        "frob",
        "add",
        "add demo",
        "add demo --kind bearer",
        "add demo --kind bearer --host",
        "add demo --kind oauth --host a.example.com",
        "add demo --kind bearer --host https://a.example.com",
        "add Bad_Id --kind bearer --host a.example.com",
        "add demo --kind bearer --host a.example.com extra",
        "add demo --kind bearer --host a.example.com --frob",
        "rm",
        "rm a b",
        "rotate",
        "list now",
        "log -n",
        "log -n x",
        "log --follow=1",
    ];
    for line in bad {
        assert!(parse(&args(line)).is_err(), "{line:?} parsed");
    }
}

#[test]
fn no_flag_takes_the_value() {
    for line in [
        format!("add demo --kind bearer --host a.example.com --value {SENTINEL}"),
        format!("add demo --kind bearer --host a.example.com --value={SENTINEL}"),
        format!("rotate demo --value {SENTINEL}"),
        format!("rotate demo {SENTINEL}"),
        format!("add demo {SENTINEL} --kind bearer --host a.example.com"),
    ] {
        let e = parse(&args(&line)).unwrap_err();
        assert!(!e.contains(SENTINEL), "usage error echoes the value: {e}");
    }
}

#[test]
fn the_log_command_is_journalctl_for_the_audit_tag() {
    assert_eq!(
        log_argv(false, None),
        ["--user", "-t", "tetsu-secrets", "-o", "short-iso"]
    );
    assert_eq!(
        log_argv(true, Some(50)),
        [
            "--user",
            "-t",
            "tetsu-secrets",
            "-o",
            "short-iso",
            "-f",
            "-n",
            "50"
        ]
    );
}

// ---- the value ----------------------------------------------------------

struct Scripted {
    answers: Vec<&'static str>,
    labels: Vec<String>,
}

impl Scripted {
    fn new(answers: &[&'static str]) -> Self {
        Self {
            answers: answers.iter().rev().copied().collect(),
            labels: Vec::new(),
        }
    }
}

impl Prompt for Scripted {
    fn read_hidden(&mut self, label: &str) -> io::Result<String> {
        self.labels.push(label.to_owned());
        Ok(self.answers.pop().expect("a scripted answer").to_owned())
    }
}

#[test]
fn a_terminal_prompts_twice() {
    let mut prompt = Scripted::new(&[SENTINEL, SENTINEL]);
    let got = read_value(true, &mut prompt, &mut Cursor::new(Vec::new())).unwrap();
    assert_eq!(got.expose(), SENTINEL);
    assert_eq!(prompt.labels.len(), 2);
}

#[test]
fn a_mismatch_or_an_empty_entry_is_refused() {
    let mut prompt = Scripted::new(&["one", "two"]);
    let e = read_value(true, &mut prompt, &mut Cursor::new(Vec::new())).unwrap_err();
    assert!(matches!(e, ValueError::Mismatch), "{e}");
    let mut prompt = Scripted::new(&["", ""]);
    let e = read_value(true, &mut prompt, &mut Cursor::new(Vec::new())).unwrap_err();
    assert!(matches!(e, ValueError::Empty), "{e}");
}

#[test]
fn a_pipe_is_read_whole_without_one_trailing_newline() {
    let mut prompt = Scripted::new(&[]);
    let mut input = Cursor::new(format!("{SENTINEL}\n").into_bytes());
    assert_eq!(
        read_value(false, &mut prompt, &mut input).unwrap().expose(),
        SENTINEL
    );
    assert!(prompt.labels.is_empty());
    let mut input = Cursor::new(b"a\nb\n\n".to_vec());
    assert_eq!(
        read_value(false, &mut prompt, &mut input).unwrap().expose(),
        "a\nb\n"
    );
    let mut input = Cursor::new(b"no-newline".to_vec());
    assert_eq!(
        read_value(false, &mut prompt, &mut input).unwrap().expose(),
        "no-newline"
    );
}

#[test]
fn an_empty_or_nul_pipe_is_refused() {
    let mut prompt = Scripted::new(&[]);
    for bytes in [&b""[..], &b"\n"[..]] {
        let e = read_value(false, &mut prompt, &mut Cursor::new(bytes.to_vec())).unwrap_err();
        assert!(matches!(e, ValueError::Empty), "{e}");
    }
    let e = read_value(false, &mut prompt, &mut Cursor::new(b"a\0b".to_vec())).unwrap_err();
    assert!(matches!(e, ValueError::Nul), "{e}");
}

// ---- the commands -------------------------------------------------------

struct Output {
    code: u8,
    out: String,
    err: String,
}

async fn exec(line: &str, socket: &std::path::Path, tty: bool, stdin: &[u8]) -> Output {
    let command = parse(&args(line)).expect("a valid command line");
    let mut prompt = Scripted::new(&[SENTINEL, SENTINEL]);
    let mut input = Cursor::new(stdin.to_vec());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = {
        let mut io = Io {
            stdin: &mut input,
            stdin_is_tty: tty,
            prompt: &mut prompt,
            out: &mut out,
            err: &mut err,
        };
        run(command, socket, &mut io).await
    };
    Output {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

fn piped() -> Vec<u8> {
    format!("{SENTINEL}\n").into_bytes()
}

#[test]
fn add_list_rotate_and_rm_against_a_live_broker() {
    with_server(|socket, rig| async move {
        let add = "add demo --kind header:x-api-key --host api.example.com --allow web-fetch";
        let added = exec(add, &socket, false, &piped()).await;
        assert_eq!(added.code, 0, "{}", added.err);
        assert!(added.out.contains("demo"));

        let listed = exec("list", &socket, false, b"").await;
        assert_eq!(listed.code, 0, "{}", listed.err);
        for word in ["demo", "header:x-api-key", "api.example.com", "web-fetch"] {
            assert!(
                listed.out.contains(word),
                "{word} missing in {}",
                listed.out
            );
        }
        let header = listed.out.lines().next().unwrap();
        assert!(header.starts_with("ID"), "{header}");

        let rotated = exec("rotate demo", &socket, true, b"").await;
        assert_eq!(rotated.code, 0, "{}", rotated.err);

        let dup = exec(add, &socket, false, &piped()).await;
        assert_eq!(dup.code, 1);
        assert!(
            dup.err.contains("demo") && dup.err.contains("exists"),
            "{}",
            dup.err
        );

        let ghost = exec("rm ghost", &socket, false, b"").await;
        assert_eq!(ghost.code, 1);
        assert!(ghost.err.contains("ghost"), "{}", ghost.err);

        let removed = exec("rm demo", &socket, false, b"").await;
        assert_eq!(removed.code, 0, "{}", removed.err);
        let listed = exec("list", &socket, false, b"").await;
        assert!(!listed.out.contains("demo"));

        for shown in [
            &added.out,
            &added.err,
            &listed.out,
            &rotated.out,
            &rotated.err,
            &dup.err,
            &ghost.err,
        ] {
            assert!(!shown.contains(SENTINEL));
        }
        assert!(rig.sink.entries.borrow().len() >= 6);
    });
}

#[test]
fn an_empty_pipe_never_reaches_the_broker() {
    with_server(|socket, rig| async move {
        let out = exec(
            "add demo --kind bearer --host a.example.com",
            &socket,
            false,
            b"\n",
        )
        .await;
        assert_eq!(out.code, 1);
        assert!(out.err.contains("empty"), "{}", out.err);
        assert!(rig.sink.entries.borrow().is_empty());
    });
}

#[test]
fn no_broker_is_a_readable_failure() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(exec("list", &dir.path().join("none.sock"), false, b""));
    assert_eq!(out.code, 1);
    assert!(out.err.contains("tetsu-secrets"), "{}", out.err);
}
