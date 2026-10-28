//! The Smithay side of idle: `ext-idle-notify-v1` for clients, and the one
//! entry point input reports activity through.

use std::time::{Duration, Instant};

use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::wayland::idle_notify::IdleNotifierState;

use super::{IdleTimer, SECOND, may_lock, timeout_in};
use crate::state::State;

/// The compositor's idle timer and the clients' idle notifier.
pub struct IdleGlue {
    /// The `[lock] idle_timeout_s` timer, polled on the publish tick.
    pub(crate) timer: IdleTimer,
    /// `ext_idle_notifier_v1`, seen by every client.
    pub(crate) notifier: IdleNotifierState<State>,
    /// `[lock] idle_timeout_s`, kept to count it again in a new unit.
    timeout_s: u32,
    /// How long one idle second lasts: a second, shorter only on the
    /// headless test hook.
    unit: Duration,
    /// Whether activity was forwarded to the notifier in this loop
    /// iteration: it re-arms every client timer, so once is enough.
    forwarded: bool,
}

impl IdleGlue {
    /// Creates the global; the timer is armed from now.
    pub fn new(
        dh: &DisplayHandle,
        loop_handle: LoopHandle<'static, State>,
        timeout_s: u32,
    ) -> Self {
        Self {
            timer: IdleTimer::new(timeout_in(timeout_s, SECOND), Instant::now()),
            notifier: IdleNotifierState::new(dh, loop_handle),
            timeout_s,
            unit: SECOND,
            forwarded: false,
        }
    }

    /// A new `[lock] idle_timeout_s`, counted from the last activity.
    pub(crate) fn set_timeout_s(&mut self, timeout_s: u32) {
        self.timeout_s = timeout_s;
        self.timer.set_timeout(timeout_in(timeout_s, self.unit));
    }

    /// How long one idle second lasts (the headless test hook).
    pub(crate) fn set_unit(&mut self, unit: Duration) {
        self.unit = unit;
        self.set_timeout_s(self.timeout_s);
    }
}

impl State {
    /// User activity: every input path calls this first. The compositor
    /// timer re-arms each time, the clients' notifications at most once
    /// per loop iteration.
    pub fn note_activity(&mut self) {
        self.idle.timer.activity(Instant::now());
        if !self.idle.forwarded {
            self.idle.forwarded = true;
            self.idle.notifier.notify_activity(&self.seat);
        }
    }

    /// The publish tick's share: once the session went idle, the locker
    /// starts, as the Lock bind starts it.
    pub(crate) fn idle_tick(&mut self) {
        if self
            .idle
            .timer
            .due(Instant::now(), may_lock(self.lock.phase()))
        {
            eprintln!("tetsu-comp: idle; starting the locker");
            self.start_locker();
        }
    }

    /// The end of a loop iteration: the next activity is forwarded again.
    pub fn idle_loop_done(&mut self) {
        self.idle.forwarded = false;
    }
}
