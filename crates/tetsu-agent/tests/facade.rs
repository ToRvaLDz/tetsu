//! End to end: the agent lists, runs and is refused through the real
//! capability gate, with a fake capd and a scripted backend.

mod common;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::Duration;

use common::{Decision, World, start};
use serde_json::json;
use tetsu_agent::agent::Agent;
use tetsu_agent::backend::fake::{FakeBackend, Recorded};
use tetsu_agent::backend::{Event, Input, SessionConfig, StopReason, ToolResult};
use tetsu_agent::contract;
use tetsu_agent::discovery::{self, Catalog, Dirs, Pinned};
use tetsu_agent::runner::{BusRunner, LogSink};
use tetsu_agent::tools;
use tetsu_agent::turn::Stop;
use tokio::task::LocalSet;

const TX: &str = "org.tetsu.tx.v1";
const INSTALL: &str = "org.tetsu.tx.v1.Install";
const INSTALL_WIRE: &str = "org_tetsu_tx_v1_Install";
const STATUS_WIRE: &str = "org_tetsu_tx_v1_GetStatus";

fn run<F, Fut>(test: F)
where
    F: FnOnce(World) -> Fut,
    Fut: Future<Output = ()>,
{
    let root = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    LocalSet::new().block_on(&runtime, async {
        let world = start(root.path());
        test(world).await;
    });
}

async fn discover(world: &World) -> Catalog {
    let all = tools::build(&contract::load().unwrap()).unwrap();
    let dirs = Dirs::new(vec![world.scan_dir.clone()]).with_timeout(Duration::from_secs(2));
    discovery::discover(&all, &dirs, &Pinned::new(Vec::new())).await
}

type Recorder = Rc<RefCell<Recorded>>;

/// An agent over what was discovered, with a scripted model.
async fn agent(
    world: &World,
    script: Vec<Vec<Event>>,
) -> (Agent<FakeBackend, BusRunner>, Recorder) {
    let catalog = discover(world).await;
    let log: LogSink = Rc::new(|_| {});
    let runner = BusRunner::new(catalog.routes, world.capd.clone(), log.clone());
    let backend = FakeBackend::new(script);
    let recorder = backend.recorder();
    let agent = Agent::new(
        Some(backend),
        runner,
        catalog.tools,
        SessionConfig::default(),
        log,
    );
    (agent, recorder)
}

fn call_event(name: &str, input: serde_json::Value) -> Vec<Event> {
    vec![
        Event::ToolCall {
            id: "c1".to_owned(),
            name: name.to_owned(),
            input,
        },
        Event::TurnDone {
            stop_reason: StopReason::ToolUse,
        },
    ]
}

fn finish() -> Vec<Event> {
    vec![
        Event::TextDelta("ok".to_owned()),
        Event::TurnDone {
            stop_reason: StopReason::EndTurn,
        },
    ]
}

// 1. Discovery lists every method of the interface served, and nothing else.

#[test]
fn discovery_lists_every_method_of_the_served_interface() {
    run(|world| async move {
        let catalog = discover(&world).await;
        let contract = contract::load().unwrap();
        let expected: BTreeSet<String> = contract.interfaces[TX]
            .methods
            .keys()
            .map(|m| format!("{TX}.{m}"))
            .collect();
        let listed: BTreeSet<String> = catalog.tools.iter().map(|t| t.id.clone()).collect();
        assert_eq!(listed, expected);
        assert!(catalog.skipped.is_empty(), "{:?}", catalog.skipped);
    });
}

// 2. An allowed gated call: Intent, then Check, then the call.

#[test]
fn an_allowed_call_goes_intent_then_check_then_the_service() {
    run(|world| async move {
        let script = vec![
            call_event(
                INSTALL_WIRE,
                json!({ "packages": ["htop"], "_reason": "the user wants it" }),
            ),
            finish(),
        ];
        let (mut agent, recorder) = agent(&world, script).await;
        let outcome = agent
            .ask("install htop".to_owned(), &mut |_| {})
            .await
            .unwrap();
        assert_eq!(outcome.stop, Stop::Done(StopReason::EndTurn));
        assert!(outcome.denied.is_empty());
        assert_eq!(
            recorder.borrow().inputs[1],
            Input::ToolResults(vec![ToolResult {
                call_id: "c1".to_owned(),
                content: "{\"final\":{\"transaction\":{\"id\":\"t1\"}},\"intermediate_count\":0}"
                    .to_owned(),
                is_error: false,
            }])
        );
        assert_eq!(
            world.events.borrow().as_slice(),
            [
                format!("Intent:{INSTALL}:the user wants it"),
                format!("Check:{INSTALL}:{{\"packages\":[\"htop\"]}}"),
                "inner:Install:{\"packages\":[\"htop\"]}".to_owned(),
            ]
        );
        assert!(world.gate_log.borrow().is_empty());
    });
}

// 3. A denied ask: the inner service never runs; the turn ends normally.

#[test]
fn a_denied_ask_never_reaches_the_service_and_the_turn_ends_normally() {
    run(|world| async move {
        *world.decision.borrow_mut() = Decision::DenyByUser;
        let script = vec![
            call_event(
                INSTALL_WIRE,
                json!({ "packages": ["htop"], "_reason": "why" }),
            ),
            finish(),
        ];
        let (mut agent, recorder) = agent(&world, script).await;
        let outcome = agent
            .ask("install htop".to_owned(), &mut |_| {})
            .await
            .unwrap();
        assert_eq!(outcome.stop, Stop::Done(StopReason::EndTurn));
        assert_eq!(outcome.denied, [INSTALL]);
        let inputs = recorder.borrow().inputs.clone();
        let Input::ToolResults(results) = &inputs[1] else {
            panic!("{inputs:?}")
        };
        assert!(results[0].is_error && results[0].content.contains("refused"));
        assert_eq!(outcome.text, "ok");
        let events = world.events.borrow();
        assert!(
            events.iter().all(|e| !e.starts_with("inner:")),
            "{events:?}"
        );
        assert_eq!(events.iter().filter(|e| e.starts_with("Check:")).count(), 1);
        assert_eq!(
            world.gate_log.borrow().len(),
            1,
            "the gate logged the refusal"
        );
    });
}

// 4. A free read passes with no Check.

#[test]
fn a_free_read_passes_with_no_check() {
    run(|world| async move {
        let script = vec![call_event(STATUS_WIRE, json!({})), finish()];
        let (mut agent, _) = agent(&world, script).await;
        let outcome = agent.ask("status".to_owned(), &mut |_| {}).await.unwrap();
        assert_eq!(outcome.stop, Stop::Done(StopReason::EndTurn));
        assert_eq!(world.events.borrow().as_slice(), ["inner:GetStatus"]);
    });
}
