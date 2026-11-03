//! ext-session-lock: the handler hands every request to the lock glue, which
//! asks the lock machine. See `crate::lock`.

use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};

use crate::state::State;

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.lock_request(confirmation);
    }

    fn unlock(&mut self) {
        self.lock_unlock();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        self.lock_new_surface(surface, &output);
    }
}
