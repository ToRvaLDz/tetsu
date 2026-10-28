//! ext-idle-notify: Smithay serves the notifications; activity comes in
//! through `State::note_activity`. See `crate::idle`.

use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};

use crate::state::State;

impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle.notifier
    }
}
