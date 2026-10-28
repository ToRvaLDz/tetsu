//! Idle: the compositor's own idle timeout as a pure timer, polled with a
//! clock the caller passes in. See `glue` for the Smithay side.

use std::time::{Duration, Instant};

/// The timeout `secs` stands for: `0` turns idle off.
pub fn timeout_of(_secs: u32) -> Option<Duration> {
    None
}

/// When the session has been idle long enough.
#[derive(Debug, Clone)]
pub struct IdleTimer {
    timeout: Option<Duration>,
    last: Instant,
    fired: bool,
}

impl IdleTimer {
    /// Armed from `now`.
    pub fn new(timeout: Option<Duration>, now: Instant) -> Self {
        Self {
            timeout,
            last: now,
            fired: false,
        }
    }

    /// Input happened at `now`.
    pub fn activity(&mut self, _now: Instant) {}

    /// A new timeout.
    pub fn set_timeout(&mut self, _timeout: Option<Duration>) {}

    /// Whether the session went idle.
    pub fn due(&mut self, _now: Instant, _unlocked: bool) -> bool {
        false
    }
}
