//! The ask: rendering and the round-trip over a scripted notify channel.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_bus::notify::{DismissReason, NotifyReply, ShowParams, Urgency};
use tetsu_capd::ask::{Answer, AskRequest, BACKOFF, NotifyChannel, ShowEvents, render, run_ask};
use tetsu_capd::decide::Buttons;

const METHOD: &str = "org.tetsu.tx.v1.Install";

fn request<'a>(args: &'a Value, reason: Option<&'a str>, buttons: Buttons) -> AskRequest<'a> {
    AskRequest {
        method: METHOD,
        class: "packages.install",
        args,
        reason,
        buttons,
    }
}

fn body(params: &ShowParams) -> Vec<String> {
    params
        .body
        .as_deref()
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_ask_names_the_method_class_and_arguments() {
    let args = json!({ "packages": ["htop", "firefox"], "reinstall": true });
    let params = render(&request(&args, None, Buttons::Two));
    assert_eq!(params.title, "The agent wants to run a method");
    assert_eq!(
        body(&params),
        vec![
            "Method: org.tetsu.tx.v1.Install",
            "Class: packages.install",
            "Arguments:",
            "  packages: [\"htop\",\"firefox\"]",
            "  reinstall: true",
        ]
    );
    assert_eq!(params.urgency, Some(Urgency::Critical));
    assert_eq!(params.timeout_ms, None);
}

#[test]
fn a_string_argument_is_quoted() {
    let args = json!({ "name": "htop" });
    let lines = body(&render(&request(&args, None, Buttons::Two)));
    assert!(lines.contains(&"  name: \"htop\"".to_owned()), "{lines:?}");
}

#[test]
fn the_reason_line_is_there_only_with_an_intent() {
    let args = json!({});
    let none = render(&request(&args, None, Buttons::Two));
    assert!(!none.body.unwrap().contains("Reason"));
    let some = render(&request(&args, Some("to build it"), Buttons::Two));
    assert!(
        body(&some).contains(&"Reason given by the agent (unverified): \"to build it\"".to_owned())
    );
}

#[test]
fn a_value_cannot_forge_a_line() {
    let args = json!({
        "evil": "x\nMethod: org.tetsu.tx.v1.Update\nClass: system.update",
        "bad\nClass: system.update": "y",
    });
    let params = render(&request(&args, Some("r\nMethod: forged"), Buttons::Two));
    let lines = body(&params);
    let count = |prefix: &str| lines.iter().filter(|l| l.starts_with(prefix)).count();
    assert_eq!(count("Method:"), 1, "{lines:?}");
    assert_eq!(count("Class:"), 1, "{lines:?}");
    assert_eq!(count("Reason"), 1, "{lines:?}");
    assert_eq!(count("Arguments:"), 1, "{lines:?}");
}

#[test]
fn control_characters_and_line_separators_are_stripped() {
    let args = json!({ "v": "a\u{7f}b\u{2028}c\u{202e}d\u{85}e" });
    let lines = body(&render(&request(&args, None, Buttons::Two)));
    assert!(lines.contains(&"  v: \"abcde\"".to_owned()), "{lines:?}");
}

#[test]
fn a_long_value_is_cut_visibly() {
    let args = json!({ "v": "x".repeat(500) });
    let lines = body(&render(&request(&args, None, Buttons::Two)));
    let line = lines.iter().find(|l| l.starts_with("  v: ")).unwrap();
    // The JSON rendering is 502 characters: 200 stay, 302 are cut.
    let expected = format!("  v: \"{}… (302 more characters)", "x".repeat(199));
    assert_eq!(line, &expected);
}

fn many_args(count: usize) -> Value {
    let mut args = serde_json::Map::new();
    for i in 0..count {
        args.insert(format!("a{i:02}"), json!(i));
    }
    Value::Object(args)
}

#[test]
fn more_than_ten_arguments_say_how_many_are_hidden() {
    let args = many_args(13);
    let lines = body(&render(&request(&args, None, Buttons::Two)));
    let shown = lines.iter().filter(|l| l.starts_with("  a")).count();
    assert_eq!(shown, 10, "{lines:?}");
    assert_eq!(lines.last().unwrap(), "… (3 more arguments not shown)");
}

#[test]
fn exactly_ten_arguments_hide_nothing() {
    let args = many_args(10);
    let lines = body(&render(&request(&args, None, Buttons::Two)));
    assert!(!lines.iter().any(|l| l.contains("not shown")), "{lines:?}");
}

#[test]
fn an_argument_line_cannot_start_at_column_zero() {
    let args = json!({
        "Reason given by the agent (unverified)": "ok",
        "Method": "org.tetsu.tx.v1.Update",
    });
    let lines = body(&render(&request(&args, None, Buttons::Two)));
    let at_zero = |prefix: &str| lines.iter().filter(|l| l.starts_with(prefix)).count();
    assert_eq!(at_zero("Reason"), 0, "{lines:?}");
    assert_eq!(at_zero("Method:"), 1, "{lines:?}");
    let after = lines.iter().skip_while(|l| *l != "Arguments:").skip(1);
    assert!(after.clone().count() == 2 && after.into_iter().all(|l| l.starts_with("  ")));
}

#[test]
fn always_allow_is_not_offered_from_a_cut_view() {
    let ids = |args: &Value| -> Vec<String> {
        render(&request(args, None, Buttons::Three))
            .actions
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect()
    };
    let full = ["allow_once", "always_allow", "deny"];
    let cut = ["allow_once", "deny"];
    assert_eq!(ids(&json!({ "v": "short" })), full);
    assert_eq!(ids(&many_args(10)), full);
    assert_eq!(ids(&many_args(11)), cut);
    assert_eq!(ids(&json!({ "v": "x".repeat(500) })), cut);
}

#[test]
fn a_cut_view_keeps_the_two_button_set() {
    let args = json!({ "v": "x".repeat(500) });
    let ids: Vec<String> = render(&request(&args, None, Buttons::Two))
        .actions
        .unwrap()
        .into_iter()
        .map(|a| a.id)
        .collect();
    assert_eq!(ids, ["approve", "deny"]);
}

#[test]
fn the_buttons_follow_the_ask_kind() {
    let args = json!({});
    let ids = |buttons| -> Vec<String> {
        render(&request(&args, None, buttons))
            .actions
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect()
    };
    assert_eq!(ids(Buttons::Three), ["allow_once", "always_allow", "deny"]);
    assert_eq!(ids(Buttons::Two), ["approve", "deny"]);
}

#[test]
fn the_backoff_is_one_two_four_seconds() {
    assert_eq!(
        BACKOFF,
        [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4)
        ]
    );
}

/// One scripted show: a connect failure, or the replies before the stream
/// ends.
enum Script {
    ConnectFails,
    Replies(Vec<NotifyReply>),
}

struct Events(VecDeque<NotifyReply>);

impl ShowEvents for Events {
    async fn next(&mut self) -> io::Result<NotifyReply> {
        self.0
            .pop_front()
            .ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
    }
}

struct Scripted {
    scripts: RefCell<VecDeque<Script>>,
    shows: Cell<usize>,
}

impl Scripted {
    fn new(scripts: Vec<Script>) -> Self {
        Self {
            scripts: RefCell::new(scripts.into()),
            shows: Cell::new(0),
        }
    }
}

impl NotifyChannel for Scripted {
    type Events = Events;

    async fn show(&self, _params: ShowParams) -> io::Result<Events> {
        self.shows.set(self.shows.get() + 1);
        match self.scripts.borrow_mut().pop_front() {
            Some(Script::Replies(replies)) => Ok(Events(replies.into())),
            _ => Err(io::ErrorKind::ConnectionRefused.into()),
        }
    }

    async fn notice(&self, _params: ShowParams) -> io::Result<()> {
        Ok(())
    }
}

fn run(channel: &Scripted) -> (Answer, Vec<usize>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let args = json!({});
    let params = render(&request(&args, None, Buttons::Two));
    let mut seen = Vec::new();
    let answer = runtime.block_on(run_ask(
        channel,
        &params,
        &[Duration::ZERO; 3],
        &mut |attempt| seen.push(attempt),
    ));
    (answer, seen)
}

fn shown() -> NotifyReply {
    NotifyReply::shown("n1")
}

#[test]
fn invoked_actions_are_returned() {
    for id in ["approve", "allow_once", "always_allow", "deny"] {
        let channel = Scripted::new(vec![Script::Replies(vec![
            shown(),
            NotifyReply::invoked(id),
        ])]);
        let (answer, seen) = run(&channel);
        assert_eq!(answer, Answer::Invoked(id.to_owned()));
        assert_eq!(seen, vec![0]);
    }
}

#[test]
fn dismissals_are_returned_with_their_reason() {
    for reason in [
        DismissReason::User,
        DismissReason::Expired,
        DismissReason::Closed,
    ] {
        let channel = Scripted::new(vec![Script::Replies(vec![
            shown(),
            NotifyReply::dismissed(reason),
        ])]);
        assert_eq!(run(&channel).0, Answer::Dismissed(reason));
    }
}

#[test]
fn a_stream_that_ends_before_an_answer_is_shown_again() {
    let channel = Scripted::new(vec![
        Script::Replies(vec![shown()]),
        Script::Replies(vec![shown()]),
        Script::Replies(vec![shown()]),
        Script::Replies(vec![shown(), NotifyReply::invoked("approve")]),
    ]);
    let (answer, seen) = run(&channel);
    assert_eq!(answer, Answer::Invoked("approve".into()));
    assert_eq!(seen, vec![0, 1, 2, 3], "four shows in all");
    assert_eq!(channel.shows.get(), 4);
}

#[test]
fn four_failures_are_unavailable_and_connect_failures_count() {
    let channel = Scripted::new(vec![
        Script::ConnectFails,
        Script::Replies(vec![shown()]),
        Script::ConnectFails,
        Script::Replies(vec![]),
        // A fifth show would answer; it must never be reached.
        Script::Replies(vec![shown(), NotifyReply::invoked("approve")]),
    ]);
    let (answer, seen) = run(&channel);
    assert_eq!(answer, Answer::Unavailable);
    assert_eq!(seen, vec![0, 1, 2, 3]);
    assert_eq!(channel.shows.get(), 4);
}
