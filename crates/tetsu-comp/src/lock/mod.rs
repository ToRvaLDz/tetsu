//! The session lock as a pure state machine: no Smithay types, lock objects
//! are opaque ids and outputs are model ids. See `glue` for the Smithay side.

use std::time::Instant;

use tetsu_bus::session::SessionEvent;

use crate::trust::Trust;

/// An `ext_session_lock_v1` object, opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LockId(pub u64);

/// A lock surface, opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceKey(pub u64);

/// What a lock request gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The lock object stays; `locked` follows once every output is clear.
    Accept,
    /// The lock object gets `finished`.
    Refuse,
}

/// Where the machine is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is locked.
    Unlocked,
    /// A lock was accepted and is not confirmed yet.
    Locking,
    /// Confirmed.
    Locked,
    /// The owner is gone without unlocking: still locked.
    Defunct,
}

/// What one output may show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scene {
    /// Windows, layers, everything.
    Normal,
    /// A solid colour and, if there is one, only this lock surface.
    Locked {
        /// The current owner's lock surface on this output.
        surface: Option<SurfaceKey>,
    },
}

/// The lock state machine.
#[derive(Debug)]
pub struct LockMachine;

impl LockMachine {
    /// Unlocked, with no outputs.
    pub fn new() -> Self {
        todo!()
    }

    /// The phase.
    pub fn phase(&self) -> Phase {
        todo!()
    }

    /// Whether anything is locked (locking, locked or defunct).
    pub fn is_locked(&self) -> bool {
        todo!()
    }

    /// The lock object that owns the session.
    pub fn owner(&self) -> Option<LockId> {
        todo!()
    }

    /// An output exists (always called, locked or not).
    pub fn output_added(&mut self, _output: &str) {
        todo!()
    }

    /// An output went away.
    pub fn output_removed(&mut self, _output: &str) {
        todo!()
    }

    /// A lock request from a client of the given trust.
    pub fn request(&mut self, _id: LockId, _trust: Trust) -> Decision {
        todo!()
    }

    /// An output presented a frame; `locked_frame` says it was rendered in
    /// the locked scene.
    pub fn presented(&mut self, _output: &str, _locked_frame: bool) {
        todo!()
    }

    /// The owner to send `locked` to, once, when every output presented a
    /// locked frame.
    pub fn confirm(&mut self) -> Option<LockId> {
        todo!()
    }

    /// A lock object went away.
    pub fn owner_died(&mut self, _id: LockId) {
        todo!()
    }

    /// `unlock_and_destroy` from `id`; true when the session unlocked.
    pub fn unlock(&mut self, _id: LockId) -> bool {
        todo!()
    }

    /// A lock surface of lock `lock` exists on `output`.
    pub fn add_surface(&mut self, _lock: LockId, _output: &str, _key: SurfaceKey) {
        todo!()
    }

    /// A lock surface went away.
    pub fn remove_surface(&mut self, _key: SurfaceKey) {
        todo!()
    }

    /// What `output` may show.
    pub fn scene(&self, _output: &str) -> Scene {
        todo!()
    }

    /// Whether the compositor should start the locker now; counts it.
    pub fn relaunch_due(&mut self, _now: Instant) -> bool {
        todo!()
    }

    /// The bus events since the last call.
    pub fn flush(&mut self) -> Vec<SessionEvent> {
        todo!()
    }
}

impl Default for LockMachine {
    fn default() -> Self {
        Self::new()
    }
}
