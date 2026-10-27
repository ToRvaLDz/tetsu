//! Statistics for the focus-to-bar latency measurement. Pure, no I/O.
#![allow(dead_code)]

use std::fmt;
use std::time::Duration;

/// The bus publish tick (#57).
pub const TICK: Duration = Duration::from_millis(16);
/// Trials when the environment does not say.
pub const DEFAULT_TRIALS: usize = 60;
/// Accepted range for the trial count.
const TRIALS_RANGE: std::ops::RangeInclusive<usize> = 1..=10_000;

/// The statistics of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub n: usize,
    pub min: Duration,
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
    pub mean: Duration,
}

/// The nearest-rank percentile of sorted samples: the `ceil(p * n)`-th.
fn nearest_rank(sorted: &[Duration], percent: usize) -> Duration {
    let rank = (sorted.len() * percent).div_ceil(100).max(1);
    sorted[rank - 1]
}

impl Summary {
    /// `None` when there are no samples.
    pub fn of(samples: &[Duration]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort();
        let total: Duration = sorted.iter().sum();
        Some(Self {
            n: sorted.len(),
            min: sorted[0],
            p50: nearest_rank(&sorted, 50),
            p95: nearest_rank(&sorted, 95),
            max: sorted[sorted.len() - 1],
            mean: total / sorted.len() as u32,
        })
    }
}

fn millis(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1000.0)
}

/// The latency budget: the 95th percentile bound is the ticket's 50 ms,
/// the max bound is tolerance for a loaded runner.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub p95: Duration,
    pub max: Duration,
}

impl Budget {
    /// `Err` names the failing statistic with its value and bound.
    pub fn check(&self, s: &Summary) -> Result<(), String> {
        if s.p95 >= self.p95 {
            return Err(format!(
                "p95 {} is not under the budget of {}",
                millis(s.p95),
                millis(self.p95)
            ));
        }
        if s.max >= self.max {
            return Err(format!(
                "max {} is not under the tolerance of {}",
                millis(s.max),
                millis(self.max)
            ));
        }
        Ok(())
    }
}

impl fmt::Display for Summary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "focus-to-bar latency over {} trials: min {}, p50 {}, p95 {}, max {}, mean {} \
             (budget p95 < 50 ms, max < 100 ms)",
            self.n,
            millis(self.min),
            millis(self.p50),
            millis(self.p95),
            millis(self.max),
            millis(self.mean),
        )
    }
}

/// The wait before trial `i`, spread over 0 to 16 ms so trials cannot lock
/// onto the publish tick phase.
pub fn phase_offset(i: usize) -> Duration {
    Duration::from_millis((i as u64 * 7) % 17)
}

/// The trial count from `TETSU_FOCUS_LATENCY_TRIALS` (`None`: unset).
pub fn trials_from(value: Option<&str>) -> Result<usize, String> {
    let Some(text) = value else {
        return Ok(DEFAULT_TRIALS);
    };
    match text.parse::<usize>() {
        Ok(n) if TRIALS_RANGE.contains(&n) => Ok(n),
        _ => Err(format!(
            "TETSU_FOCUS_LATENCY_TRIALS={text:?} is not a number in {}..={}",
            TRIALS_RANGE.start(),
            TRIALS_RANGE.end()
        )),
    }
}
