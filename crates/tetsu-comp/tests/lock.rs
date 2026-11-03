//! The session lock state machine and the scene it decides, with no
//! compositor around it.

mod common;

use std::time::{Duration, Instant};

use tetsu_bus::session::{SessionEvent, SessionKind};
use tetsu_comp::input::{Bound, allowed_while_locked};
use tetsu_comp::lock::{Decision, LockId, LockMachine, Phase, Scene, SurfaceKey};
use tetsu_comp::render::scene;
use tetsu_comp::trust::Trust;

const A: LockId = LockId(1);
const B: LockId = LockId(2);

/// A machine that knows `n` outputs, `out-1..`.
fn machine(n: usize) -> LockMachine {
    let mut m = LockMachine::new();
    for i in 1..=n {
        m.output_added(&format!("out-{i}"));
    }
    m
}

/// Every output presents a locked frame and the lock confirms.
fn confirmed(m: &mut LockMachine, n: usize) -> Option<LockId> {
    for i in 1..=n {
        m.presented(&format!("out-{i}"), true);
    }
    m.confirm()
}

/// A locked machine owned by `A`.
fn locked(n: usize) -> LockMachine {
    let mut m = machine(n);
    assert_eq!(m.request(A, Trust::Untrusted), Decision::Accept);
    assert_eq!(confirmed(&mut m, n), Some(A));
    m.flush();
    m
}

/// A defunct machine: `A` locked and died.
fn defunct(n: usize) -> LockMachine {
    let mut m = locked(n);
    m.owner_died(A);
    m
}

#[test]
fn the_first_lock_is_accepted_from_any_trust() {
    for trust in [Trust::Untrusted, Trust::Locker, Trust::Input] {
        let mut m = machine(1);
        assert_eq!(m.phase(), Phase::Unlocked);
        assert_eq!(m.request(A, trust), Decision::Accept, "{trust:?}");
        assert_eq!(m.phase(), Phase::Locking);
        assert_eq!(m.owner(), Some(A));
        assert!(m.is_locked());
    }
}

#[test]
fn a_second_request_while_locking_or_locked_is_refused() {
    let mut m = machine(1);
    m.request(A, Trust::Untrusted);
    assert_eq!(m.request(B, Trust::Locker), Decision::Refuse);
    assert_eq!(m.owner(), Some(A));
    assert_eq!(m.phase(), Phase::Locking);

    assert_eq!(confirmed(&mut m, 1), Some(A));
    assert_eq!(m.request(B, Trust::Locker), Decision::Refuse);
    assert_eq!(m.owner(), Some(A));
    assert_eq!(m.phase(), Phase::Locked);
}

#[test]
fn only_the_owner_unlocks_and_only_once_locked() {
    let mut m = locked(1);
    assert!(!m.unlock(B));
    assert_eq!(m.phase(), Phase::Locked);
    assert_eq!(m.owner(), Some(A));
    assert!(m.unlock(A));
    assert_eq!(m.phase(), Phase::Unlocked);
    assert!(!m.is_locked());
    assert_eq!(m.owner(), None);
}

#[test]
fn an_unlock_before_locked_goes_defunct_and_stays_locked() {
    // Smithay answers it with a protocol error that kills the client.
    let mut m = machine(1);
    m.request(A, Trust::Untrusted);
    assert!(!m.unlock(A));
    assert_eq!(m.phase(), Phase::Defunct);
    assert!(m.is_locked());
}

#[test]
fn locked_waits_for_a_locked_frame_on_every_output() {
    let mut m = machine(2);
    m.request(A, Trust::Untrusted);
    assert_eq!(m.confirm(), None);
    m.presented("out-1", true);
    assert_eq!(m.confirm(), None);
    m.presented("out-2", true);
    assert_eq!(m.confirm(), Some(A));
    assert_eq!(m.phase(), Phase::Locked);
    // Once.
    assert_eq!(m.confirm(), None);
}

#[test]
fn a_frame_rendered_before_the_lock_never_confirms() {
    let mut m = machine(2);
    m.presented("out-1", false);
    m.request(A, Trust::Untrusted);
    m.presented("out-1", false);
    m.presented("out-2", true);
    assert_eq!(m.confirm(), None);
    m.presented("out-1", true);
    assert_eq!(m.confirm(), Some(A));
}

#[test]
fn a_locked_frame_from_before_this_lock_does_not_count() {
    let mut m = locked(1);
    assert!(m.unlock(A));
    m.output_added("out-2");
    m.request(B, Trust::Untrusted);
    // out-1 presented a locked frame in the previous lock.
    assert_eq!(m.confirm(), None);
}

#[test]
fn with_no_outputs_the_lock_confirms_at_once() {
    let mut m = LockMachine::new();
    m.request(A, Trust::Untrusted);
    assert_eq!(m.confirm(), Some(A));
}

#[test]
fn an_output_added_while_locking_delays_the_confirmation() {
    let mut m = machine(1);
    m.request(A, Trust::Untrusted);
    m.presented("out-1", true);
    m.output_added("out-2");
    assert_eq!(m.confirm(), None);
    m.presented("out-2", true);
    assert_eq!(m.confirm(), Some(A));
}

#[test]
fn a_removed_output_leaves_the_pending_set() {
    let mut m = machine(2);
    m.request(A, Trust::Untrusted);
    m.presented("out-1", true);
    m.output_removed("out-2");
    assert_eq!(m.confirm(), Some(A));
}

#[test]
fn an_output_added_while_locked_is_solid_at_once() {
    let mut m = locked(1);
    m.output_added("out-2");
    assert_eq!(m.phase(), Phase::Locked);
    assert_eq!(scene(&m, "out-2"), Scene::Locked { surface: None });
}

#[test]
fn owner_death_in_locking_or_locked_goes_defunct_and_stays_locked() {
    let mut m = machine(2);
    m.request(A, Trust::Untrusted);
    m.owner_died(A);
    assert_eq!(m.phase(), Phase::Defunct);
    assert!(m.is_locked());

    let mut m = locked(2);
    m.add_surface(A, "out-1", SurfaceKey(10));
    m.owner_died(A);
    assert_eq!(m.phase(), Phase::Defunct);
    assert!(m.is_locked());
    for output in ["out-1", "out-2", "out-unknown"] {
        assert_eq!(scene(&m, output), Scene::Locked { surface: None });
    }
}

#[test]
fn the_death_of_a_stranger_changes_nothing() {
    let mut m = locked(1);
    m.owner_died(B);
    assert_eq!(m.phase(), Phase::Locked);
    assert_eq!(m.owner(), Some(A));
}

#[test]
fn in_defunct_only_a_locker_can_take_over() {
    for trust in [Trust::Untrusted, Trust::Input] {
        let mut m = defunct(1);
        assert_eq!(m.request(B, trust), Decision::Refuse, "{trust:?}");
        assert_eq!(m.phase(), Phase::Defunct);
        assert_eq!(m.owner(), None);
    }
    let mut m = defunct(1);
    assert_eq!(m.request(B, Trust::Locker), Decision::Accept);
    assert_eq!(m.phase(), Phase::Locking);
    assert_eq!(m.owner(), Some(B));
    // The outputs showed the locked scene all along.
    assert_eq!(m.confirm(), Some(B));
    assert!(m.unlock(B));
    assert_eq!(m.phase(), Phase::Unlocked);
}

#[test]
fn a_takeover_waits_for_outputs_that_never_showed_the_lock() {
    let mut m = defunct(1);
    m.output_added("out-2");
    assert_eq!(m.request(B, Trust::Locker), Decision::Accept);
    assert_eq!(m.confirm(), None);
    m.presented("out-2", true);
    assert_eq!(m.confirm(), Some(B));
}

#[test]
fn the_scene_is_normal_only_while_unlocked() {
    let mut m = machine(1);
    assert_eq!(scene(&m, "out-1"), Scene::Normal);
    m.request(A, Trust::Untrusted);
    assert_eq!(scene(&m, "out-1"), Scene::Locked { surface: None });
    confirmed(&mut m, 1);
    assert_eq!(scene(&m, "out-1"), Scene::Locked { surface: None });
    m.unlock(A);
    assert_eq!(scene(&m, "out-1"), Scene::Normal);
}

#[test]
fn the_scene_picks_the_owners_surface_on_that_output_only() {
    let mut m = machine(2);
    m.request(A, Trust::Untrusted);
    m.add_surface(A, "out-1", SurfaceKey(1));
    assert_eq!(
        scene(&m, "out-1"),
        Scene::Locked {
            surface: Some(SurfaceKey(1))
        }
    );
    assert_eq!(scene(&m, "out-2"), Scene::Locked { surface: None });
    m.remove_surface(SurfaceKey(1));
    assert_eq!(scene(&m, "out-1"), Scene::Locked { surface: None });
}

#[test]
fn the_surface_of_a_refused_or_old_lock_is_never_chosen() {
    let mut m = machine(1);
    m.request(A, Trust::Untrusted);
    // B was refused; it still made a surface before the refusal reached it.
    assert_eq!(m.request(B, Trust::Locker), Decision::Refuse);
    m.add_surface(B, "out-1", SurfaceKey(2));
    assert_eq!(scene(&m, "out-1"), Scene::Locked { surface: None });

    // A dies, the locker B2 takes over; A's old surface is not shown.
    m.add_surface(A, "out-1", SurfaceKey(1));
    m.owner_died(A);
    assert_eq!(m.request(LockId(3), Trust::Locker), Decision::Accept);
    assert_eq!(scene(&m, "out-1"), Scene::Locked { surface: None });
    m.add_surface(LockId(3), "out-1", SurfaceKey(3));
    assert_eq!(
        scene(&m, "out-1"),
        Scene::Locked {
            surface: Some(SurfaceKey(3))
        }
    );
}

#[test]
fn relaunch_is_due_only_when_defunct() {
    let t0 = Instant::now();
    let mut m = machine(1);
    assert!(!m.relaunch_due(t0));
    m.request(A, Trust::Untrusted);
    assert!(!m.relaunch_due(t0));
    m.owner_died(A);
    assert!(m.relaunch_due(t0));
}

#[test]
fn relaunch_is_at_most_three_per_minute_and_a_second_apart() {
    let t0 = Instant::now();
    let at = |ms: u64| t0 + Duration::from_millis(ms);
    let mut m = defunct(1);
    assert!(m.relaunch_due(at(0)));
    // Too soon after the last one.
    assert!(!m.relaunch_due(at(500)));
    assert!(m.relaunch_due(at(1_000)));
    assert!(!m.relaunch_due(at(1_500)));
    assert!(m.relaunch_due(at(2_000)));
    // The budget of three is used up.
    assert!(!m.relaunch_due(at(3_000)));
    assert!(!m.relaunch_due(at(59_000)));
    // The first one aged out of the 60 s window.
    assert!(m.relaunch_due(at(60_000)));
    assert!(!m.relaunch_due(at(60_500)));
}

fn kinds(events: &[SessionEvent]) -> Vec<SessionKind> {
    events.iter().map(|e| e.kind).collect()
}

#[test]
fn events_are_locked_at_the_start_and_unlocked_at_the_end() {
    let contract = common::contract();
    let mut m = machine(1);
    assert!(m.flush().is_empty());
    m.request(A, Trust::Untrusted);
    let events = m.flush();
    assert_eq!(kinds(&events), [SessionKind::Locked]);
    // Confirming adds nothing: the bus said locked when locking began.
    confirmed(&mut m, 1);
    assert!(m.flush().is_empty());
    m.unlock(A);
    let more = m.flush();
    assert_eq!(kinds(&more), [SessionKind::Unlocked]);
    for event in events.iter().chain(&more) {
        common::check_session_event(&contract, event);
    }
    common::check_session_event(&contract, &SessionEvent::initial(true));
}

#[test]
fn refusals_and_takeovers_emit_nothing() {
    let mut m = machine(1);
    m.request(A, Trust::Untrusted);
    m.flush();
    assert_eq!(m.request(B, Trust::Locker), Decision::Refuse);
    assert!(!m.unlock(B));
    assert!(m.flush().is_empty());
    confirmed(&mut m, 1);
    m.owner_died(A);
    assert!(m.flush().is_empty(), "defunct stays locked: no event");
    assert_eq!(m.request(B, Trust::Untrusted), Decision::Refuse);
    assert_eq!(m.request(B, Trust::Locker), Decision::Accept);
    assert!(m.flush().is_empty(), "a takeover is no new lock");
    confirmed(&mut m, 1);
    m.unlock(B);
    assert_eq!(kinds(&m.flush()), [SessionKind::Unlocked]);
}

#[test]
fn only_the_safe_binds_pass_while_locked() {
    use tetsu_comp::engine::Action;
    assert!(allowed_while_locked(Bound::SwitchVt(2)));
    assert!(allowed_while_locked(Bound::Lock));
    assert!(!allowed_while_locked(Bound::Quit));
    for action in [Action::FocusNext, Action::Close, Action::ToggleFloating] {
        assert!(!allowed_while_locked(Bound::Engine(action)));
    }
}
