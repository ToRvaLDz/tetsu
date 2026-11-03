//! The runner: a tool call through its server, a refusal as an outcome.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use tetsu_agent::contract;
use tetsu_agent::discovery::Route;
use tetsu_agent::runner::{BusRunner, LogSink, Outcome, ToolRunner};
use tetsu_agent::tools::{self, Tool};
use tetsu_bus::access::ExpectedServer;
use tetsu_bus::endpoint::Endpoint;
use tetsu_bus::peer::Peer;
use tetsu_bus::server::{self, ErrorReply, Handler, Request, Response, StreamItem};
use tokio::sync::mpsc;
use tokio::task::LocalSet;

const TIMEOUT: Duration = Duration::from_secs(5);
const GET_STATUS: &str = "org.tetsu.tx.v1.GetStatus";
const INSTALL: &str = "org.tetsu.tx.v1.Install";
const REMOVE: &str = "org.tetsu.tx.v1.Remove";
const ROLLBACK: &str = "org.tetsu.tx.v1.Rollback";
const UPDATE: &str = "org.tetsu.tx.v1.Update";
const LIST: &str = "org.tetsu.tx.v1.ListTransactions";
const REVIEW: &str = "org.tetsu.tx.v1.ReviewAurPackages";
const WATCH: &str = "org.tetsu.comp.window.v1.WatchWindows";
const SHOW: &str = "org.tetsu.notify.v1.Show";
const HOLD_LIMIT: usize = 8;

type Events = Rc<RefCell<Vec<String>>>;

fn tool(id: &str) -> Tool {
    let all = tools::build(&contract::load().unwrap()).unwrap();
    all.into_iter().find(|t| t.id == id).expect("a tool")
}

fn own_cgroup() -> String {
    let proc = fs::read_to_string("/proc/self/cgroup").unwrap();
    tetsu_bus::peer::parse_proc_cgroup(&proc).expect("a unified cgroup")
}

/// A fake service for every interface the tests use. Records each call as
/// `call:<method>:<params>` and the end of each hanging stream as
/// `gone:<tag>`.
struct Service {
    events: Events,
    holds: Rc<RefCell<usize>>,
}

impl Service {
    /// A stream that sends `first` and then stays open until the caller
    /// closes the connection.
    fn open(&self, tag: String, first: Value) -> Response {
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(StreamItem::Reply(first)).unwrap();
        let events = self.events.clone();
        tokio::task::spawn_local(async move {
            tx.closed().await;
            events.borrow_mut().push(format!("gone:{tag}"));
        });
        Response::Stream(rx)
    }
}

impl Handler for Service {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        let params = serde_json::to_string(&request.parameters).unwrap();
        self.events
            .borrow_mut()
            .push(format!("call:{}:{params}", request.method));
        match request.method.as_str() {
            GET_STATUS => {
                Response::Reply(json!({ "booted": "a", "default": "b", "default_boot": "good" }))
            }
            INSTALL => {
                let (tx, rx) = mpsc::unbounded_channel();
                tx.send(StreamItem::Reply(
                    json!({ "progress": { "phase": "build" } }),
                ))
                .unwrap();
                tx.send(StreamItem::Reply(
                    json!({ "progress": { "phase": "commit" } }),
                ))
                .unwrap();
                tx.send(StreamItem::Last(json!({ "transaction": { "id": "t1" } })))
                    .unwrap();
                Response::Stream(rx)
            }
            REMOVE => Response::Error(ErrorReply::new(
                "org.tetsu.tx.v1.PermissionDenied",
                json!({}),
            )),
            ROLLBACK if request.parameters["id"] == "big" => Response::Error(ErrorReply::new(
                &format!("org.tetsu.tx.v1.Bad\u{7}{}", "N".repeat(1000)),
                json!({ "blob": "x".repeat(100 * 1024) }),
            )),
            ROLLBACK => Response::Error(ErrorReply::new(
                "org.tetsu.tx.v1.SnapshotNotFound",
                json!({ "id": "x" }),
            )),
            LIST => Response::Reply(json!({ "transactions": "x".repeat(100 * 1024) })),
            REVIEW => Response::Reply(
                json!({ "reviews": [], "echo": request.parameters, "note": "RESULT-MARKER" }),
            ),
            UPDATE => self.open("Update".to_owned(), json!({ "progress": null })),
            WATCH => self.open(
                "Watch".to_owned(),
                json!({ "event": { "kind": "initial" } }),
            ),
            SHOW => {
                let n = {
                    let mut holds = self.holds.borrow_mut();
                    *holds += 1;
                    *holds
                };
                self.open(
                    format!("show{n}"),
                    json!({ "reply": { "kind": "shown", "id": format!("n{n}") } }),
                )
            }
            other => Response::method_not_found(other),
        }
    }
}

/// A fake capd: records `Intent`, answers it or fails it.
struct Capd {
    events: Events,
    fail: bool,
}

impl Handler for Capd {
    async fn call(&self, _peer: Rc<Peer>, request: Request) -> Response {
        let method = request.parameters["method"]
            .as_str()
            .unwrap_or("?")
            .to_owned();
        let reason = request.parameters["reason"]
            .as_str()
            .unwrap_or("?")
            .to_owned();
        self.events.borrow_mut().push(format!(
            "{}:{method}:{reason}",
            request.method.rsplit('.').next().unwrap()
        ));
        if self.fail {
            Response::Error(ErrorReply::new("org.tetsu.cap.v1.UnknownMethod", json!({})))
        } else {
            Response::Reply(Value::Null)
        }
    }
}

struct Env {
    /// The temporary directory; the test closure may capture only part of
    /// the `Env`, so the directory itself lives in `run`.
    dir: PathBuf,
    events: Events,
    logs: Events,
    runner: BusRunner,
}

fn run<F, Fut>(capd_up: bool, intent_fails: bool, test: F)
where
    F: FnOnce(Env) -> Fut,
    Fut: Future<Output = ()>,
{
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let events: Events = Rc::default();
        let logs: Events = Rc::default();
        let service = dir.path().join("svc").join("service");
        let listener = server::bind(&service).unwrap();
        let handler = Rc::new(Service {
            events: events.clone(),
            holds: Rc::default(),
        });
        tokio::task::spawn_local(server::serve(listener, handler));

        let capd = dir.path().join("capd").join("org.tetsu.cap.v1");
        if capd_up {
            let listener = server::bind(&capd).unwrap();
            let handler = Rc::new(Capd {
                events: events.clone(),
                fail: intent_fails,
            });
            tokio::task::spawn_local(server::serve(listener, handler));
        }
        let routes: BTreeMap<String, Route> = [
            "org.tetsu.tx.v1",
            "org.tetsu.comp.window.v1",
            "org.tetsu.notify.v1",
        ]
        .iter()
        .map(|i| ((*i).to_owned(), Route::Scanned(service.clone())))
        .collect();
        let sink_logs = logs.clone();
        let sink: LogSink = Rc::new(move |line| sink_logs.borrow_mut().push(line.to_owned()));
        let runner = BusRunner::new(
            routes,
            Endpoint::new(capd, ExpectedServer::Cgroup(own_cgroup())),
            sink,
        );
        test(Env {
            dir: dir.path().to_owned(),
            events,
            logs,
            runner,
        })
        .await;
    });
}

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, f).await.expect("in time")
}

async fn wait_for(events: &Events, wanted: &str) {
    within(async {
        while !events.borrow().iter().any(|e| e == wanted) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

fn events(env: &Env) -> Vec<String> {
    env.events.borrow().clone()
}

// 1. A read.

#[test]
fn a_read_returns_the_reply_with_no_intent() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(GET_STATUS), json!({}))).await;
        assert_eq!(
            outcome,
            Outcome::Ok(json!({ "booted": "a", "default": "b", "default_boot": "good" }))
        );
        assert_eq!(events(&env), [format!("call:{GET_STATUS}:{{}}")]);
    });
}

// 2, 3. The reason.

#[test]
fn a_gated_call_sends_the_intent_before_the_call_and_strips_the_reason() {
    run(true, false, |env| async move {
        let input = json!({ "packages": ["a"], "_reason": "the user asked for it" });
        let outcome = within(env.runner.run(&tool(INSTALL), input)).await;
        assert_eq!(
            outcome,
            Outcome::Ok(json!({
                "final": { "transaction": { "id": "t1" } },
                "intermediate_count": 2,
            }))
        );
        assert_eq!(
            events(&env),
            [
                format!("Intent:{INSTALL}:the user asked for it"),
                format!("call:{INSTALL}:{{\"packages\":[\"a\"]}}"),
            ]
        );
    });
}

#[test]
fn a_gated_call_without_a_reason_sends_no_intent() {
    run(true, false, |env| async move {
        within(env.runner.run(&tool(INSTALL), json!({ "packages": ["a"] }))).await;
        within(env.runner.run(
            &tool(INSTALL),
            json!({ "packages": ["a"], "_reason": "  " }),
        ))
        .await;
        within(
            env.runner
                .run(&tool(INSTALL), json!({ "packages": ["a"], "_reason": 7 })),
        )
        .await;
        assert!(
            events(&env).iter().all(|e| e.starts_with("call:")),
            "{:?}",
            events(&env)
        );
    });
}

#[test]
fn a_read_never_sends_an_intent_and_its_reason_is_stripped() {
    run(true, false, |env| async move {
        within(
            env.runner
                .run(&tool(GET_STATUS), json!({ "_reason": "why" })),
        )
        .await;
        assert_eq!(events(&env), [format!("call:{GET_STATUS}:{{}}")]);
    });
}

// 4. capd down, or refusing the intent.

#[test]
fn a_missing_capd_does_not_stop_the_call_and_is_logged_once() {
    run(false, false, |env| async move {
        let input = json!({ "packages": ["a"], "_reason": "why" });
        let outcome = within(env.runner.run(&tool(INSTALL), input)).await;
        assert!(matches!(outcome, Outcome::Ok(_)), "{outcome:?}");
        assert_eq!(
            events(&env),
            [format!("call:{INSTALL}:{{\"packages\":[\"a\"]}}")]
        );
        let logs = env.logs.borrow();
        assert_eq!(
            logs.iter().filter(|l| l.contains("intent")).count(),
            1,
            "{logs:?}"
        );
    });
}

#[test]
fn a_refused_intent_does_not_stop_the_call() {
    run(true, true, |env| async move {
        let input = json!({ "packages": ["a"], "_reason": "why" });
        let outcome = within(env.runner.run(&tool(INSTALL), input)).await;
        assert!(matches!(outcome, Outcome::Ok(_)), "{outcome:?}");
        assert_eq!(events(&env).len(), 2);
        assert_eq!(
            env.logs
                .borrow()
                .iter()
                .filter(|l| l.contains("intent"))
                .count(),
            1
        );
    });
}

// 5. Refusals and errors.

#[test]
fn a_permission_denied_of_the_interface_is_denied() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(REMOVE), json!({ "packages": ["a"] }))).await;
        assert_eq!(outcome, Outcome::Denied);
    });
}

#[test]
fn another_remote_error_keeps_its_name_and_parameters() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(ROLLBACK), json!({}))).await;
        assert_eq!(
            outcome,
            Outcome::Error {
                error: "org.tetsu.tx.v1.SnapshotNotFound".to_owned(),
                parameters: json!({ "id": "x" }),
            }
        );
    });
}

#[test]
fn a_missing_socket_or_route_is_unavailable() {
    run(true, false, |env| async move {
        let gone = env.dir.join("no-such-socket");
        let routes = BTreeMap::from([("org.tetsu.tx.v1".to_owned(), Route::Scanned(gone))]);
        let sink: LogSink = Rc::new(|_| {});
        let runner = BusRunner::new(
            routes,
            Endpoint::new("/nonexistent", ExpectedServer::Cgroup(own_cgroup())),
            sink,
        );
        let outcome = within(runner.run(&tool(GET_STATUS), json!({}))).await;
        assert!(matches!(outcome, Outcome::Unavailable(_)), "{outcome:?}");
        // No route for the interface at all.
        let outcome = within(runner.run(&tool(WATCH), json!({}))).await;
        assert!(matches!(outcome, Outcome::Unavailable(_)), "{outcome:?}");
    });
}

// 6. Input that is not an object.

#[test]
fn input_that_is_not_an_object_is_an_error_with_no_bus_call() {
    run(true, false, |env| async move {
        for input in [json!("text"), json!([1]), json!(3)] {
            let outcome = within(env.runner.run(&tool(INSTALL), input)).await;
            assert!(matches!(outcome, Outcome::Error { .. }), "{outcome:?}");
        }
        assert!(events(&env).is_empty());
        // No input at all reads as an empty object.
        let outcome = within(env.runner.run(&tool(GET_STATUS), Value::Null)).await;
        assert!(matches!(outcome, Outcome::Ok(_)));
    });
}

// 7, 8, 9. Stream modes.

#[test]
fn first_returns_the_initial_reply_and_closes_the_connection() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(WATCH), json!({}))).await;
        assert_eq!(
            outcome,
            Outcome::Ok(json!({ "event": { "kind": "initial" } }))
        );
        wait_for(&env.events, "gone:Watch").await;
    });
}

#[test]
fn hold_returns_the_first_reply_and_keeps_the_stream_open() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(SHOW), json!({ "title": "t" }))).await;
        assert_eq!(
            outcome,
            Outcome::Ok(json!({ "reply": { "kind": "shown", "id": "n1" } }))
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!events(&env).iter().any(|e| e.starts_with("gone:")));
    });
}

#[test]
fn the_ninth_hold_closes_the_oldest_and_dropping_the_runner_closes_all() {
    run(true, false, |env| async move {
        for _ in 0..=HOLD_LIMIT {
            within(env.runner.run(&tool(SHOW), json!({ "title": "t" }))).await;
        }
        wait_for(&env.events, "gone:show1").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        let gone = events(&env)
            .iter()
            .filter(|e| e.starts_with("gone:"))
            .count();
        assert_eq!(gone, 1, "only the oldest is dropped: {:?}", events(&env));

        let Env {
            events: log,
            runner,
            ..
        } = env;
        drop(runner);
        wait_for(&log, "gone:show9").await;
        let gone = log
            .borrow()
            .iter()
            .filter(|e| e.starts_with("gone:"))
            .count();
        assert_eq!(gone, 9);
    });
}

// 10. Bounded results.

#[test]
fn a_big_reply_is_replaced_by_the_truncated_form() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(LIST), json!({}))).await;
        let Outcome::Ok(value) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(value["truncated"], true);
        let bytes = value["bytes"].as_u64().unwrap();
        assert!(bytes > 100 * 1024);
        let head = value["head"].as_str().unwrap();
        assert!(head.starts_with("{\"transactions\":\"xxx"));
        assert!(serde_json::to_vec(&value).unwrap().len() < 64 * 1024);
    });
}

// 11. Cancelling.

#[test]
fn dropping_the_call_closes_the_connection_the_server_waits_on() {
    run(true, false, |env| async move {
        let update = tool(UPDATE);
        let call = env.runner.run(&update, json!({}));
        let result = tokio::time::timeout(Duration::from_millis(200), call).await;
        assert!(result.is_err(), "the server never ends the stream");
        wait_for(&env.events, "gone:Update").await;
    });
}

// 12. Log hygiene.

#[test]
fn the_log_names_the_tool_and_the_outcome_and_nothing_else() {
    run(true, false, |env| async move {
        let input = json!({
            "names": ["PARAM-MARKER"],
            "_reason": "REASON-MARKER",
        });
        let outcome = within(env.runner.run(&tool(REVIEW), input)).await;
        let Outcome::Ok(value) = outcome else {
            panic!("{outcome:?}")
        };
        assert_eq!(value["note"], "RESULT-MARKER");
        within(
            env.runner
                .run(&tool(REMOVE), json!({ "packages": ["PARAM-MARKER"] })),
        )
        .await;
        within(env.runner.run(&tool(ROLLBACK), json!({}))).await;

        let logs = env.logs.borrow().join("\n");
        for marker in [
            "PARAM-MARKER",
            "REASON-MARKER",
            "RESULT-MARKER",
            "SnapshotNotFound",
        ] {
            assert!(!logs.contains(marker), "{marker} leaked: {logs}");
        }
        assert!(logs.contains(REVIEW) && logs.contains("ok"), "{logs}");
        assert!(logs.contains(REMOVE) && logs.contains("denied"), "{logs}");
        assert!(logs.contains(ROLLBACK) && logs.contains("error"), "{logs}");
    });
}

// 13. Bounded errors and reasons.

#[test]
fn an_error_is_bounded_and_clean_like_a_result() {
    run(true, false, |env| async move {
        let outcome = within(env.runner.run(&tool(ROLLBACK), json!({ "id": "big" }))).await;
        let Outcome::Error { error, parameters } = outcome else {
            panic!("{outcome:?}")
        };
        assert!(error.len() <= 256, "{}", error.len());
        assert!(error.starts_with("org.tetsu.tx.v1.Bad"));
        assert!(!error.chars().any(char::is_control), "{error:?}");
        assert_eq!(parameters["truncated"], true);
        assert!(serde_json::to_vec(&parameters).unwrap().len() < 64 * 1024);
    });
}

#[test]
fn an_unavailable_text_is_cut_and_clean() {
    run(true, false, |env| async move {
        let gone = env.dir.join(format!("no-\u{7}{}", "d".repeat(1000)));
        let routes = BTreeMap::from([("org.tetsu.tx.v1".to_owned(), Route::Scanned(gone))]);
        let sink: LogSink = Rc::new(|_| {});
        let runner = BusRunner::new(
            routes,
            Endpoint::new("/nonexistent", ExpectedServer::Cgroup(own_cgroup())),
            sink,
        );
        let outcome = within(runner.run(&tool(GET_STATUS), json!({}))).await;
        let Outcome::Unavailable(text) = outcome else {
            panic!("{outcome:?}")
        };
        assert!(text.len() <= 256, "{}", text.len());
        assert!(!text.chars().any(char::is_control), "{text:?}");
    });
}

#[test]
fn a_long_reason_is_cut_to_a_kilobyte_at_a_char_boundary() {
    run(true, false, |env| async move {
        // 3-byte chars: 1024 is not a multiple of 3.
        let reason = "\u{20ac}".repeat(2000);
        let input = json!({ "packages": ["a"], "_reason": reason });
        within(env.runner.run(&tool(INSTALL), input)).await;
        let events = events(&env);
        let sent = events[0]
            .strip_prefix(&format!("Intent:{INSTALL}:"))
            .unwrap();
        assert!(sent.len() <= 1024 && sent.len() > 1000, "{}", sent.len());
        assert!(sent.chars().all(|c| c == '\u{20ac}'));
    });
}
