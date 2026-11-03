//! The session lock as a pure state machine: no Smithay types, lock objects
//! are opaque ids and outputs are model ids. See `glue` for the Smithay side.

pub mod glue;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use tetsu_bus::session::SessionEvent;

use crate::trust::Trust;

/// How many times the locker is started again per `RELAUNCH_WINDOW`.
const RELAUNCH_BUDGET: usize = 3;
/// The window of the relaunch budget.
const RELAUNCH_WINDOW: Duration = Duration::from_secs(60);
/// Least time between two relaunches.
const RELAUNCH_GAP: Duration = Duration::from_secs(1);

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
///
/// Fail closed: the only way back to `Unlocked` is `unlock` by the owner of a
/// confirmed lock. An owner that dies, a refused takeover, an output nobody
/// presented on: all of them leave the session locked.
#[derive(Debug)]
pub struct LockMachine {
    phase: Phase,
    owner: Option<LockId>,
    /// Every output that exists, locked or not.
    outputs: BTreeSet<String>,
    /// Outputs that presented a frame rendered in the locked scene since the
    /// session locked.
    proven: BTreeSet<String>,
    /// Lock surfaces: lock, output, surface.
    surfaces: Vec<(LockId, String, SurfaceKey)>,
    events: Vec<SessionEvent>,
    /// When the locker was started again.
    launches: Vec<Instant>,
}

impl LockMachine {
    /// Unlocked, with no outputs.
    pub fn new() -> Self {
        Self {
            phase: Phase::Unlocked,
            owner: None,
            outputs: BTreeSet::new(),
            proven: BTreeSet::new(),
            surfaces: Vec::new(),
            events: Vec::new(),
            launches: Vec::new(),
        }
    }

    /// The phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether anything is locked (locking, locked or defunct).
    pub fn is_locked(&self) -> bool {
        self.phase != Phase::Unlocked
    }

    /// The lock object that owns the session.
    pub fn owner(&self) -> Option<LockId> {
        self.owner
    }

    /// An output exists (always called, locked or not).
    pub fn output_added(&mut self, output: &str) {
        self.outputs.insert(output.to_owned());
    }

    /// An output went away.
    pub fn output_removed(&mut self, output: &str) {
        self.outputs.remove(output);
        self.proven.remove(output);
        self.surfaces.retain(|(_, o, _)| o != output);
    }

    /// A lock request from a client of the given trust.
    pub fn request(&mut self, id: LockId, trust: Trust) -> Decision {
        match self.phase {
            Phase::Locking | Phase::Locked => Decision::Refuse,
            Phase::Defunct if trust != Trust::Locker => Decision::Refuse,
            Phase::Defunct => {
                self.take(id);
                Decision::Accept
            }
            Phase::Unlocked => {
                self.proven.clear();
                self.take(id);
                self.events.push(SessionEvent::locked());
                Decision::Accept
            }
        }
    }

    fn take(&mut self, id: LockId) {
        self.phase = Phase::Locking;
        self.owner = Some(id);
        self.surfaces.retain(|(lock, ..)| *lock == id);
    }

    /// An output presented a frame; `locked_frame` says it was rendered in
    /// the locked scene.
    pub fn presented(&mut self, output: &str, locked_frame: bool) {
        if locked_frame && self.is_locked() && self.outputs.contains(output) {
            self.proven.insert(output.to_owned());
        }
    }

    /// The owner to send `locked` to, once, when every output presented a
    /// locked frame.
    pub fn confirm(&mut self) -> Option<LockId> {
        if self.phase != Phase::Locking || !self.outputs.is_subset(&self.proven) {
            return None;
        }
        self.phase = Phase::Locked;
        self.owner
    }

    /// A lock object went away.
    pub fn owner_died(&mut self, id: LockId) {
        if self.owner == Some(id) && matches!(self.phase, Phase::Locking | Phase::Locked) {
            self.defunct();
        }
    }

    fn defunct(&mut self) {
        self.phase = Phase::Defunct;
        self.owner = None;
    }

    /// `unlock_and_destroy` from `id`; true when the session unlocked.
    pub fn unlock(&mut self, id: LockId) -> bool {
        if self.owner != Some(id) {
            return false;
        }
        match self.phase {
            Phase::Locked => {
                self.phase = Phase::Unlocked;
                self.owner = None;
                self.proven.clear();
                self.surfaces.clear();
                self.events.push(SessionEvent::unlocked());
                true
            }
            // Smithay answers an unlock before `locked` with a protocol
            // error that kills the client: the lock stays.
            Phase::Locking => {
                self.defunct();
                false
            }
            Phase::Unlocked | Phase::Defunct => false,
        }
    }

    /// A lock surface of lock `lock` exists on `output`.
    pub fn add_surface(&mut self, lock: LockId, output: &str, key: SurfaceKey) {
        self.surfaces.push((lock, output.to_owned(), key));
    }

    /// A lock surface went away.
    pub fn remove_surface(&mut self, key: SurfaceKey) {
        self.surfaces.retain(|(.., k)| *k != key);
    }

    /// What `output` may show.
    pub fn scene(&self, output: &str) -> Scene {
        if self.phase == Phase::Unlocked {
            return Scene::Normal;
        }
        let surface = self.owner.and_then(|owner| {
            self.surfaces
                .iter()
                .find(|(lock, o, _)| *lock == owner && o == output)
                .map(|(.., key)| *key)
        });
        Scene::Locked { surface }
    }

    /// Whether the compositor should start the locker now; counts it.
    pub fn relaunch_due(&mut self, now: Instant) -> bool {
        if self.phase != Phase::Defunct {
            return false;
        }
        self.launches
            .retain(|t| now.saturating_duration_since(*t) < RELAUNCH_WINDOW);
        let spaced = self
            .launches
            .last()
            .is_none_or(|t| now.saturating_duration_since(*t) >= RELAUNCH_GAP);
        if self.launches.len() >= RELAUNCH_BUDGET || !spaced {
            return false;
        }
        self.launches.push(now);
        true
    }

    /// The bus events since the last call.
    pub fn flush(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Default for LockMachine {
    fn default() -> Self {
        Self::new()
    }
}
