//! The turn loop over a scripted backend and a scripted runner.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use serde_json::{Value, json};
use tetsu_agent::agent::{Agent, AgentError};
use tetsu_agent::backend::fake::FakeBackend;
use tetsu_agent::backend::{Event, Flags, Input, SessionConfig, StopReason, ToolResult};
use tetsu_agent::contract;
use tetsu_agent::runner::{LogSink, Outcome, ToolRunner};
use tetsu_agent::tools::{self, Tool};
use tetsu_agent::turn::{self, Context, Limits, Stop};

const STATUS: &str = "org.tetsu.tx.v1.GetStatus";
const STATUS_WIRE: &str = "org_tetsu_tx_v1_GetStatus";
const INSTALL: &str = "org.tetsu.tx.v1.Install";
const INSTALL_WIRE: &str = "org_tetsu_tx_v1_Install";

fn catalog() -> Vec<Tool> {
    tools::build(&contract::load().unwrap()).unwrap()
}

/// A runner that answers from a script and records its calls.
#[derive(Default)]
struct Scripted {
    outcomes: RefCell<HashMap<String, VecDeque<Outcome>>>,
    calls: RefCell<Vec<(String, Value)>>,
}

impl Scripted {
    fn answer(self, id: &str, outcomes: Vec<Outcome>) -> Self {
        self.outcomes
            .borrow_mut()
            .insert(id.to_owned(), outcomes.into());
        self
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.borrow().clone()
    }
}

impl ToolRunner for Scripted {
    async fn run(&self, tool: &Tool, input: Value) -> Outcome {
        self.calls.borrow_mut().push((tool.id.clone(), input));
        self.outcomes
            .borrow_mut()
            .get_mut(&tool.id)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Outcome::Ok(json!({})))
    }
}

fn call(id: &str, name: &str, input: Value) -> Event {
    Event::ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        input,
    }
}

fn done(stop_reason: StopReason) -> Event {
    Event::TurnDone { stop_reason }
}

fn text(s: &str) -> Event {
    Event::TextDelta(s.to_owned())
}

struct Run {
    outcome: turn::TurnOutcome,
    inputs: Vec<Input>,
    calls: Vec<(String, Value)>,
    logs: Vec<String>,
    observed: Vec<Event>,
}

fn run_with(backend: FakeBackend, runner: Scripted, limits: Limits) -> Run {
    let tools = catalog();
    let logs: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink_logs = logs.clone();
    let log: LogSink = Rc::new(move |line| sink_logs.borrow_mut().push(line.to_owned()));
    let recorder = backend.recorder();
    let mut backend = backend;
    let mut observed = Vec::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let outcome = runtime.block_on(async {
        let ctx = Context {
            runner: &runner,
            tools: &tools,
            limits,
            log: &log,
        };
        turn::run(&mut backend, &ctx, "hi".to_owned(), &mut |e| {
            observed.push(e.clone())
        })
        .await
    });
    let inputs = recorder.borrow().inputs.clone();
    let logs = logs.borrow().clone();
    Run {
        outcome,
        inputs,
        calls: runner.calls(),
        logs,
        observed,
    }
}

fn run(script: Vec<Vec<Event>>, runner: Scripted) -> Run {
    run_with(FakeBackend::new(script), runner, Limits::default())
}

fn result(call_id: &str, content: &str, is_error: bool) -> ToolResult {
    ToolResult {
        call_id: call_id.to_owned(),
        content: content.to_owned(),
        is_error,
    }
}

// 1. Text only.

#[test]
fn a_text_only_turn_collects_the_text_with_one_send() {
    let r = run(
        vec![vec![text("hel"), text("lo"), done(StopReason::EndTurn)]],
        Scripted::default(),
    );
    assert_eq!(r.outcome.text, "hello");
    assert_eq!(r.outcome.stop, Stop::Done(StopReason::EndTurn));
    assert_eq!(r.inputs, [Input::User("hi".to_owned())]);
    assert!(r.calls.is_empty());
}

// 2. Tool use.

#[test]
fn a_tool_call_runs_and_its_result_goes_back_with_the_same_call_id() {
    let runner = Scripted::default().answer(STATUS, vec![Outcome::Ok(json!({ "booted": "a" }))]);
    let r = run(
        vec![
            vec![
                call("c1", STATUS_WIRE, json!({})),
                done(StopReason::ToolUse),
            ],
            vec![text("fine"), done(StopReason::EndTurn)],
        ],
        runner,
    );
    assert_eq!(r.calls, [(STATUS.to_owned(), json!({}))]);
    assert_eq!(
        r.inputs,
        [
            Input::User("hi".to_owned()),
            Input::ToolResults(vec![result("c1", "{\"booted\":\"a\"}", false)]),
        ]
    );
    assert_eq!(r.outcome.text, "fine");
    assert_eq!(r.outcome.stop, Stop::Done(StopReason::EndTurn));
    assert_eq!(r.outcome.runs.len(), 1);
    assert_eq!(r.outcome.runs[0].tool, STATUS);
    assert_eq!(r.outcome.runs[0].outcome, "ok");
}

// 3. Several calls in one step.

#[test]
fn several_calls_in_one_step_run_in_order_and_return_in_one_message() {
    let r = run(
        vec![
            vec![
                call("c1", STATUS_WIRE, json!({})),
                call("c2", INSTALL_WIRE, json!({ "packages": ["a"] })),
                done(StopReason::ToolUse),
            ],
            vec![done(StopReason::EndTurn)],
        ],
        Scripted::default(),
    );
    let ids: Vec<&str> = r.calls.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, [STATUS, INSTALL]);
    assert_eq!(r.calls[1].1, json!({ "packages": ["a"] }));
    let Input::ToolResults(results) = &r.inputs[1] else {
        panic!("{:?}", r.inputs)
    };
    let call_ids: Vec<&str> = results.iter().map(|t| t.call_id.as_str()).collect();
    assert_eq!(call_ids, ["c1", "c2"]);
    assert_eq!(r.inputs.len(), 2);
}

// 4. Unknown tool.

#[test]
fn an_unknown_wire_name_is_an_error_result_and_its_name_is_not_logged() {
    let r = run(
        vec![
            vec![
                call("c1", "evil_NAME_MARKER", json!({})),
                done(StopReason::ToolUse),
            ],
            vec![done(StopReason::EndTurn)],
        ],
        Scripted::default(),
    );
    assert!(r.calls.is_empty());
    let Input::ToolResults(results) = &r.inputs[1] else {
        panic!("{:?}", r.inputs)
    };
    assert!(results[0].is_error);
    assert!(!results[0].content.contains("NAME_MARKER"));
    assert!(
        r.logs.iter().all(|l| !l.contains("NAME_MARKER")),
        "{:?}",
        r.logs
    );
    assert!(
        r.logs.iter().any(|l| l.contains("16 bytes")),
        "{:?}",
        r.logs
    );
}

// 5. A denied ask.

#[test]
fn a_denied_call_is_an_error_result_and_is_not_asked_again_in_the_turn() {
    let runner = Scripted::default().answer(INSTALL, vec![Outcome::Denied]);
    let r = run(
        vec![
            vec![
                call("c1", INSTALL_WIRE, json!({})),
                done(StopReason::ToolUse),
            ],
            vec![
                call("c2", INSTALL_WIRE, json!({})),
                done(StopReason::ToolUse),
            ],
            vec![text("understood"), done(StopReason::EndTurn)],
        ],
        runner,
    );
    assert_eq!(r.calls.len(), 1, "the second call must not touch the bus");
    let fixed = format!(
        "{INSTALL} was refused by the user or the capability policy; do not retry it in this turn"
    );
    assert_eq!(
        r.inputs[1],
        Input::ToolResults(vec![result("c1", &fixed, true)])
    );
    assert_eq!(
        r.inputs[2],
        Input::ToolResults(vec![result("c2", &fixed, true)])
    );
    assert_eq!(r.outcome.denied, [INSTALL]);
    assert_eq!(r.outcome.stop, Stop::Done(StopReason::EndTurn));
    assert_eq!(r.outcome.text, "understood");
}

#[test]
fn an_error_and_an_unavailable_server_are_error_results() {
    let runner = Scripted::default()
        .answer(
            INSTALL,
            vec![Outcome::Error {
                error: "org.tetsu.tx.v1.TransactionFailed".to_owned(),
                parameters: json!({ "id": "t" }),
            }],
        )
        .answer(STATUS, vec![Outcome::Unavailable("gone".to_owned())]);
    let r = run(
        vec![
            vec![
                call("c1", INSTALL_WIRE, json!({})),
                call("c2", STATUS_WIRE, json!({})),
                done(StopReason::ToolUse),
            ],
            vec![done(StopReason::EndTurn)],
        ],
        runner,
    );
    let Input::ToolResults(results) = &r.inputs[1] else {
        panic!("{:?}", r.inputs)
    };
    assert!(results.iter().all(|t| t.is_error));
    assert!(results[0].content.contains("TransactionFailed"));
    assert!(results[1].content.contains("gone"));
    assert!(r.outcome.denied.is_empty());
}

// 6. The step limit.

#[test]
fn the_step_limit_stops_the_turn_with_no_further_send() {
    let step = || vec![call("c", STATUS_WIRE, json!({})), done(StopReason::ToolUse)];
    let r = run_with(
        FakeBackend::new(vec![step(), step(), step(), step()]),
        Scripted::default(),
        Limits { max_steps: 2 },
    );
    assert_eq!(r.outcome.stop, Stop::StepLimit);
    assert_eq!(r.inputs.len(), 2);
    assert_eq!(r.calls.len(), 1, "the calls of the last round are not run");
}

// 7. Backend errors.

#[test]
fn a_backend_error_ends_the_turn_with_its_retry_flag() {
    for retryable in [true, false] {
        let r = run(
            vec![vec![
                text("partial"),
                Event::Error {
                    message: "boom".to_owned(),
                    retryable,
                },
            ]],
            Scripted::default(),
        );
        assert_eq!(r.outcome.stop, Stop::BackendError { retryable });
        assert_eq!(r.outcome.text, "partial");
    }
}

#[test]
fn a_backend_that_closes_without_ending_the_turn_is_a_stop() {
    let r = run(vec![vec![text("x")]], Scripted::default());
    assert_eq!(r.outcome.stop, Stop::Closed);
}

// 8. A backend that runs the tools itself.

#[test]
fn a_backend_that_runs_tools_itself_is_only_observed() {
    let flags = Flags {
        executes_tools_itself: true,
        ..Flags::default()
    };
    let backend = FakeBackend::new(vec![vec![
        call("c1", STATUS_WIRE, json!({})),
        Event::ToolResult {
            id: "c1".to_owned(),
            content: "{}".to_owned(),
            is_error: false,
        },
        text("done"),
        done(StopReason::EndTurn),
    ]])
    .with_flags(flags);
    let r = run_with(backend, Scripted::default(), Limits::default());
    assert!(r.calls.is_empty());
    assert_eq!(r.inputs.len(), 1);
    assert_eq!(r.outcome.stop, Stop::Done(StopReason::EndTurn));
    assert_eq!(r.outcome.text, "done");
    assert!(
        r.observed
            .iter()
            .any(|e| matches!(e, Event::ToolCall { .. }))
    );
    assert!(
        r.observed
            .iter()
            .any(|e| matches!(e, Event::ToolResult { .. }))
    );
}

// 9. A permission request is observed, never approved.

#[test]
fn a_permission_request_is_observed_and_nothing_is_sent_back() {
    let r = run(
        vec![vec![
            Event::PermissionRequest {
                id: "p1".to_owned(),
                tool: "Bash".to_owned(),
                input: json!({}),
            },
            done(StopReason::EndTurn),
        ]],
        Scripted::default(),
    );
    assert!(
        r.observed
            .iter()
            .any(|e| matches!(e, Event::PermissionRequest { .. }))
    );
    assert_eq!(r.inputs, [Input::User("hi".to_owned())]);
}

// 10. Usage.

#[test]
fn usage_is_accumulated() {
    let usage = |input, output, cost_usd, is_estimate| Event::Usage {
        input,
        output,
        cache_read: 1,
        cache_write: 2,
        cost_usd,
        is_estimate,
    };
    let r = run(
        vec![vec![
            usage(10, 5, Some(0.5), false),
            usage(20, 6, None, true),
            done(StopReason::EndTurn),
        ]],
        Scripted::default(),
    );
    let u = r.outcome.usage;
    assert_eq!(
        (u.input, u.output, u.cache_read, u.cache_write),
        (30, 11, 2, 4)
    );
    assert!((u.cost_usd - 0.5).abs() < f64::EPSILON);
    assert!(u.is_estimate);
}

// 11. No backend.

#[test]
fn with_no_backend_the_agent_reports_it_and_nothing_runs() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let log: LogSink = Rc::new(|_| {});
        let mut agent: Agent<FakeBackend, Scripted> = Agent::new(
            None,
            Scripted::default(),
            catalog(),
            SessionConfig::default(),
            log,
        );
        let err = agent.ask("hi".to_owned(), &mut |_| {}).await.unwrap_err();
        assert!(matches!(err, AgentError::NoBackend), "{err:?}");
        assert!(agent.runner().calls().is_empty());
    });
}

#[test]
fn the_session_starts_once_with_every_tool_and_no_credential() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let backend = FakeBackend::new(vec![
            vec![text("a"), done(StopReason::EndTurn)],
            vec![text("b"), done(StopReason::EndTurn)],
        ]);
        let recorder = backend.recorder();
        let log: LogSink = Rc::new(|_| {});
        let config = SessionConfig {
            model: "opaque-model".to_owned(),
            ..SessionConfig::default()
        };
        let mut agent = Agent::new(Some(backend), Scripted::default(), catalog(), config, log);
        let first = agent.ask("one".to_owned(), &mut |_| {}).await.unwrap();
        let second = agent.ask("two".to_owned(), &mut |_| {}).await.unwrap();
        assert_eq!((first.text.as_str(), second.text.as_str()), ("a", "b"));
        let rec = recorder.borrow();
        assert_eq!(rec.starts, 1);
        let (config, specs) = rec.started.clone().unwrap();
        assert_eq!(config.model, "opaque-model");
        assert_eq!(specs.len(), catalog().len());
        assert!(specs.iter().any(|s| s.name == INSTALL_WIRE));
        assert_eq!(
            rec.inputs,
            [Input::User("one".to_owned()), Input::User("two".to_owned())]
        );
    });
}

// 12. Log hygiene.

#[test]
fn model_text_never_reaches_the_log() {
    let runner = Scripted::default().answer(INSTALL, vec![Outcome::Denied]);
    let r = run(
        vec![
            vec![
                Event::ThinkingDelta("THINK-MARKER".to_owned()),
                text("TEXT-MARKER"),
                call("c1", INSTALL_WIRE, json!({ "packages": ["INPUT-MARKER"] })),
                call("c2", "bad_NAME-MARKER", json!({})),
                done(StopReason::ToolUse),
            ],
            vec![Event::Error {
                message: "ERROR-MARKER".to_owned(),
                retryable: false,
            }],
        ],
        runner,
    );
    let logs = r.logs.join("\n");
    for marker in [
        "THINK-MARKER",
        "TEXT-MARKER",
        "INPUT-MARKER",
        "NAME-MARKER",
        "ERROR-MARKER",
    ] {
        assert!(!logs.contains(marker), "{marker} leaked: {logs}");
    }
}
