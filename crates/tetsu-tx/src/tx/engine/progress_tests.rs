//! What the engine reports to its observer, and the id it writes.

use std::sync::Mutex;

use super::tests::{Env, pkgs};
use super::*;
use crate::store::lock::TxLock;

const AFTER_HTOP: &str = "bash 5.2-1\ntetsu-base 1-1\nvim 9.1-1\nhtop 3-1\n";

/// An observer that keeps what it sees.
#[derive(Default)]
struct Seen(Mutex<Vec<EngineEvent>>);

impl Seen {
    fn observe(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }

    fn events(&self) -> Vec<EngineEvent> {
        self.0.lock().unwrap().clone()
    }
}

fn run_observed(
    env: &Env,
    request: &Request,
    id: Option<&str>,
) -> (Result<Report, TxError>, Vec<EngineEvent>) {
    let seen = Seen::default();
    let observer = |event| seen.observe(event);
    let ctx = RunCtx {
        id: id.map(str::to_owned),
        observer: &observer,
    };
    let result = env.engine(&env.runner).run_with(request, ctx);
    (result, seen.events())
}

fn full(counter: u64) -> Vec<EngineEvent> {
    vec![
        EngineEvent::Begun { counter },
        EngineEvent::Phase(EnginePhase::Packages),
        EngineEvent::Phase(EnginePhase::BootEntry),
        EngineEvent::Phase(EnginePhase::Commit),
    ]
}

#[test]
fn update_reports_every_phase_in_order() {
    let env = Env::new();
    let (result, events) = run_observed(&env, &Request::Update, None);
    assert_eq!(result.unwrap().counter, 2);
    assert_eq!(events, full(2));
}

#[test]
fn install_reports_every_phase_in_order() {
    let env = Env::with(super::tests::QI, AFTER_HTOP);
    let (result, events) = run_observed(&env, &Request::Install(pkgs(&["htop"])), None);
    assert_eq!(result.unwrap().counter, 2);
    assert_eq!(events, full(2));
}

#[test]
fn rollback_has_no_package_phase() {
    let env = Env::two_snapshots();
    let (result, events) = run_observed(&env, &Request::Rollback(Some(1)), None);
    assert_eq!(result.unwrap().counter, 3);
    assert_eq!(
        events,
        [
            EngineEvent::Begun { counter: 3 },
            EngineEvent::Phase(EnginePhase::BootEntry),
            EngineEvent::Phase(EnginePhase::Commit),
        ]
    );
}

#[test]
fn a_package_failure_stops_the_phases() {
    let env = Env::new();
    env.runner.on_tail(
        &["pacman", "-Si", "nope"],
        crate::tx::fake::Reply::Fail("error: package 'nope' was not found".into()),
    );
    let (result, events) = run_observed(&env, &Request::Install(pkgs(&["nope"])), None);
    assert!(matches!(result, Err(TxError::PackageNotFound(_))));
    assert_eq!(
        events,
        [
            EngineEvent::Begun { counter: 2 },
            EngineEvent::Phase(EnginePhase::Packages),
        ]
    );
}

#[test]
fn a_held_lock_observes_nothing() {
    let env = Env::new();
    let _held = TxLock::acquire(&env.lock).unwrap();
    let (result, events) = run_observed(&env, &Request::Update, None);
    assert!(matches!(result, Err(TxError::InProgress)));
    assert!(events.is_empty());
}

#[test]
fn a_rollback_to_a_missing_snapshot_observes_nothing() {
    let env = Env::two_snapshots();
    let (result, events) = run_observed(&env, &Request::Rollback(Some(9)), None);
    assert!(matches!(result, Err(TxError::SnapshotNotFound(9))));
    assert!(events.is_empty());
}

#[test]
fn the_record_carries_the_id_given() {
    let env = Env::new();
    let (result, _) = run_observed(&env, &Request::Update, Some("tx-00ff"));
    assert_eq!(result.unwrap().counter, 2);
    assert_eq!(env.record(2).id.as_deref(), Some("tx-00ff"));
}

#[test]
fn run_without_a_context_writes_no_id() {
    let env = Env::new();
    env.run(&Request::Update).unwrap();
    assert_eq!(env.record(2).id, None);
}
