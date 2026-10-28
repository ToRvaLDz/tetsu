//! The compositor's idle timeout on an injected clock: every decision is
//! taken on `Instant`s the test picks, with no sleeps.

use std::time::{Duration, Instant};

use tetsu_comp::idle::{IdleTimer, timeout_of};

const TIMEOUT: Duration = Duration::from_secs(10);

fn secs(s: u64) -> Duration {
    Duration::from_secs(s)
}

#[test]
fn not_due_before_the_timeout_and_due_at_it() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(Some(TIMEOUT), t0);
    assert!(!timer.due(t0, true));
    assert!(!timer.due(t0 + secs(9), true));
    assert!(!timer.due(t0 + TIMEOUT - Duration::from_millis(1), true));
    assert!(timer.due(t0 + TIMEOUT, true));
}

#[test]
fn due_only_once_per_idle_period() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(Some(TIMEOUT), t0);
    assert!(timer.due(t0 + secs(11), true));
    assert!(!timer.due(t0 + secs(12), true));
    assert!(!timer.due(t0 + secs(100), true));
}

#[test]
fn activity_rearms_from_the_new_activity() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(Some(TIMEOUT), t0);
    timer.activity(t0 + secs(5));
    assert!(!timer.due(t0 + secs(10), true));
    assert!(!timer.due(t0 + secs(14), true));
    assert!(timer.due(t0 + secs(15), true));

    timer.activity(t0 + secs(20));
    assert!(!timer.due(t0 + secs(29), true));
    assert!(timer.due(t0 + secs(30), true));
}

#[test]
fn never_due_while_not_unlocked_and_the_period_is_kept() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(Some(TIMEOUT), t0);
    assert!(!timer.due(t0 + secs(10), false));
    assert!(!timer.due(t0 + secs(30), false));
    // Unlocked again with no activity: due on the next poll.
    assert!(timer.due(t0 + secs(31), true));
}

#[test]
fn no_timeout_is_never_due() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(None, t0);
    assert!(!timer.due(t0 + secs(1), true));
    assert!(!timer.due(t0 + secs(86_400 * 7), true));
}

#[test]
fn a_lower_timeout_than_the_idle_time_is_due_on_the_next_poll() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(Some(secs(60)), t0);
    assert!(!timer.due(t0 + secs(20), true));
    timer.set_timeout(Some(TIMEOUT));
    assert!(timer.due(t0 + secs(21), true));
}

#[test]
fn turning_the_timeout_on_counts_from_the_last_activity() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(None, t0);
    timer.activity(t0 + secs(5));
    assert!(!timer.due(t0 + secs(30), true));
    timer.set_timeout(Some(secs(30)));
    assert!(!timer.due(t0 + secs(34), true));
    assert!(timer.due(t0 + secs(35), true));
}

#[test]
fn a_new_timeout_after_firing_waits_for_activity() {
    let t0 = Instant::now();
    let mut timer = IdleTimer::new(Some(TIMEOUT), t0);
    assert!(timer.due(t0 + TIMEOUT, true));
    timer.set_timeout(Some(secs(5)));
    assert!(!timer.due(t0 + secs(20), true));
    timer.activity(t0 + secs(21));
    assert!(timer.due(t0 + secs(26), true));
}

#[test]
fn zero_seconds_turns_idle_off() {
    assert_eq!(timeout_of(0), None);
    assert_eq!(timeout_of(30), Some(secs(30)));
}
