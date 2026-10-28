//! Idle: the compositor's own idle timeout as a pure timer, polled with a
//! clock the caller passes in. Smithay's idle notifier cannot take an
//! injected clock, so it only serves `ext-idle-notify-v1` to clients.

pub mod glue;

use std::time::{Duration, Instant};

use smithay::backend::input::{InputBackend, InputEvent};

use crate::lock::Phase;

/// The timeout `secs` stands for: `0` turns idle off.
pub fn timeout_of(secs: u32) -> Option<Duration> {
    (secs > 0).then(|| Duration::from_secs(u64::from(secs)))
}

/// Whether `event` is user activity that re-arms idle: a device that
/// comes or goes (a dock replug) or a switch (a lid closing) is not.
pub fn is_activity<B: InputBackend>(event: &InputEvent<B>) -> bool {
    !matches!(
        event,
        InputEvent::DeviceAdded { .. }
            | InputEvent::DeviceRemoved { .. }
            | InputEvent::SwitchToggle { .. }
    )
}

/// Whether idle may start the locker in `phase`: only when nothing is
/// locked. A defunct lock belongs to the relaunch budget.
pub fn may_lock(phase: Phase) -> bool {
    phase == Phase::Unlocked
}

/// When the session has been idle long enough.
///
/// Due a full timeout after the last activity, then again every timeout
/// while no activity comes: a locker that failed or crashed is retried
/// (fail-closed). It is never due while the session is not unlocked, and a
/// poll while locked does not use up the period.
#[derive(Debug, Clone)]
pub struct IdleTimer {
    timeout: Option<Duration>,
    /// The last activity, the last firing, or when the timer was made.
    last: Instant,
}

impl IdleTimer {
    /// Armed from `now`.
    pub fn new(timeout: Option<Duration>, now: Instant) -> Self {
        Self { timeout, last: now }
    }

    /// Input happened at `now`: a new idle period starts.
    pub fn activity(&mut self, now: Instant) {
        self.last = now;
    }

    /// A new timeout, counted from the last activity or firing; `None`
    /// turns it off.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) {
        self.timeout = timeout;
    }

    /// Whether the session went idle at `now`; once true, it re-arms from
    /// `now`.
    pub fn due(&mut self, now: Instant, unlocked: bool) -> bool {
        let Some(timeout) = self.timeout else {
            return false;
        };
        if !unlocked || now.saturating_duration_since(self.last) < timeout {
            return false;
        }
        self.last = now;
        true
    }
}
