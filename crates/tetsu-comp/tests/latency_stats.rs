//! The statistics of the focus-to-bar latency measurement: pure, no I/O.

#[path = "common/latency.rs"]
mod latency;

use std::time::Duration;

use latency::{Budget, Summary, TICK, phase_offset, trials_from};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

const BUDGET: Budget = Budget {
    p95: Duration::from_millis(50),
    max: Duration::from_millis(100),
};

#[test]
fn no_samples_have_no_summary() {
    assert!(Summary::of(&[]).is_none());
}

#[test]
fn one_sample_is_every_statistic() {
    let s = Summary::of(&[ms(7)]).unwrap();
    assert_eq!(
        (s.n, s.min, s.p50, s.p95, s.max),
        (1, ms(7), ms(7), ms(7), ms(7))
    );
    assert_eq!(s.mean, ms(7));
}

#[test]
fn percentiles_are_nearest_rank_in_any_order() {
    // 1..=20 ms, shuffled with a fixed stride coprime to 20.
    let samples: Vec<Duration> = (0..20).map(|i| ms(1 + (i * 7) % 20)).collect();
    let s = Summary::of(&samples).unwrap();
    assert_eq!(s.n, 20);
    assert_eq!(s.min, ms(1));
    assert_eq!(s.p50, ms(10));
    assert_eq!(s.p95, ms(19));
    assert_eq!(s.max, ms(20));
}

#[test]
fn mean_is_the_arithmetic_mean() {
    let s = Summary::of(&[ms(2), ms(4), ms(9)]).unwrap();
    assert_eq!(s.mean, ms(5));
}

#[test]
fn budget_accepts_within_and_names_the_failing_statistic() {
    let ok = Summary::of(&[ms(5), ms(10), ms(20)]).unwrap();
    assert_eq!(BUDGET.check(&ok), Ok(()));

    let slow_p95 = Summary::of(&[ms(60)]).unwrap();
    let error = BUDGET.check(&slow_p95).unwrap_err();
    assert!(error.contains("p95"), "{error}");
    assert!(error.contains("60.0 ms"), "{error}");
    assert!(error.contains("50.0 ms"), "{error}");

    // p95 within budget, but one stall beyond the max.
    let mut samples = vec![ms(5); 99];
    samples.push(ms(150));
    let stalled = Summary::of(&samples).unwrap();
    let error = BUDGET.check(&stalled).unwrap_err();
    assert!(error.contains("max"), "{error}");
    assert!(error.contains("150.0 ms"), "{error}");
    assert!(error.contains("100.0 ms"), "{error}");
}

#[test]
fn display_is_the_report_line() {
    let s = Summary {
        n: 60,
        min: Duration::from_micros(1100),
        p50: Duration::from_micros(8900),
        p95: Duration::from_micros(16700),
        max: Duration::from_micros(18200),
        mean: Duration::from_micros(9000),
    };
    assert_eq!(
        format!("{s}"),
        "focus-to-bar latency over 60 trials: min 1.1 ms, p50 8.9 ms, p95 16.7 ms, \
         max 18.2 ms, mean 9.0 ms (budget p95 < 50 ms, max < 100 ms)"
    );
}

#[test]
fn phase_offsets_stay_below_a_tick_and_cover_both_halves() {
    let offsets: Vec<Duration> = (0..60).map(phase_offset).collect();
    assert!(offsets.iter().all(|o| *o < ms(17)), "{offsets:?}");
    assert!(offsets.iter().any(|o| *o < TICK / 2));
    assert!(offsets.iter().any(|o| *o >= TICK / 2));
}

#[test]
fn trial_count_comes_from_the_environment_value() {
    assert_eq!(trials_from(None), Ok(60));
    assert_eq!(trials_from(Some("1000")), Ok(1000));
    assert_eq!(trials_from(Some("1")), Ok(1));
    assert_eq!(trials_from(Some("10000")), Ok(10_000));
    for bad in ["0", "abc", "20000", "", "-3"] {
        assert!(trials_from(Some(bad)).is_err(), "{bad:?}");
    }
}
