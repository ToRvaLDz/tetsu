//! Idle: the compositor's own idle timeout as a pure timer, polled with a
//! clock the caller passes in. Smithay's idle notifier cannot take an
//! injected clock, so it only serves `ext-idle-notify-v1` to clients.

pub mod glue;

use std::time::{Duration, Instant};

/// The timeout `secs` stands for: `0` turns idle off.
pub fn timeout_of(secs: u32) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(u64::from(secs)))
}

/// When the session has been idle long enough.
///
/// Due once per idle period: after it fired, only activity arms it again.
/// It is never due while the session is not unlocked, and a poll while
/// locked does not use up the period.
#[derive(Debug, Clone)]
pub struct IdleTimer {
    timeout: Option<Duration>,
    /// The last activity, or when the timer was made.
    last: Instant,
    /// Whether this idle period was already reported.
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

    /// Input happened at `now`: a new idle period starts.
    pub fn activity(&mut self, now: Instant) {
        self.last = now;
        self.fired = false;
    }

    /// A new timeout, counted from the last activity; `None` turns it off.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Whether the session went idle at `now`; true once per idle period.
    pub fn due(&mut self, now: Instant, unlocked: bool) -> bool {
        let Some(timeout) = self.timeout else {
            return false;
        };
        if !unlocked || self.fired || now.saturating_duration_since(self.last) < timeout {
            return false;
        }
        self.fired = true;
        true
    }
}
