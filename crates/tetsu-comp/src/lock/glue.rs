//! The Smithay side of the session lock: lock objects and surfaces, the
//! liveness poll, the relaunch of the locker, and the lock-first rules of
//! focus and pointer. The decisions are `LockMachine`'s.

use std::time::Instant;

use smithay::desktop::utils::send_frames_surface_tree;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::session_lock::{LockSurface, SessionLocker};

use super::{Decision, LockId, Phase, Scene, SurfaceKey};
use crate::decor::Frame;
use crate::state::State;
use crate::trust::{Trust, spawn_trusted, trust_of};

/// What an output shows.
pub enum View {
    /// Windows and layers, with the borders to draw behind the windows.
    Normal(Vec<Frame>),
    /// A solid colour and, if there is one, this lock surface.
    Locked(Option<WlSurface>),
}

struct Surf {
    key: SurfaceKey,
    lock: LockId,
    output: String,
    surface: LockSurface,
}

/// Lock objects, lock surfaces and the locker command.
pub struct LockGlue {
    next: u64,
    /// Accepted lock objects.
    locks: Vec<(LockId, ExtSessionLockV1)>,
    /// The accepted request that waits for its confirmation.
    pending: Option<(LockId, SessionLocker)>,
    surfaces: Vec<Surf>,
    /// `[lock] command`.
    command: Vec<String>,
}

impl LockGlue {
    /// A glue that starts `command` as the locker.
    pub fn new(command: Vec<String>) -> Self {
        Self {
            next: 1,
            locks: Vec::new(),
            pending: None,
            surfaces: Vec::new(),
            command,
        }
    }

    /// Replaces the locker's argv; it is used from the next launch.
    pub fn set_command(&mut self, command: Vec<String>) {
        self.command = command;
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn lock_id_of(&self, lock: &ExtSessionLockV1) -> Option<LockId> {
        self.locks
            .iter()
            .find(|(_, l)| l == lock)
            .map(|(id, _)| *id)
    }
}

impl State {
    /// A client asked for the lock. The machine decides; a refused request
    /// drops the locker, which sends `finished`.
    pub(crate) fn lock_request(&mut self, locker: SessionLocker) {
        let lock = locker.ext_session_lock().clone();
        let trust = lock.client().map_or(Trust::Untrusted, |c| trust_of(&c));
        let id = LockId(self.lock_glue.next_id());
        let was_locked = self.lock.is_locked();
        match self.lock.request(id, trust) {
            Decision::Refuse => {
                eprintln!("tetsu-comp: lock refused ({trust:?})");
                drop(locker);
            }
            Decision::Accept => {
                self.lock_glue.locks.push((id, lock));
                self.lock_glue.pending = Some((id, locker));
                if !was_locked {
                    self.lock_begins();
                }
                self.lock_confirm();
            }
        }
    }

    /// Locking began: nothing may keep a grab, the keyboard and pointer go
    /// to the lock surfaces (none yet: to nobody).
    fn lock_begins(&mut self) {
        if let Some(pointer) = self.seat.get_pointer()
            && pointer.is_grabbed()
        {
            let time = smithay::backend::input::InputTime::from_millis(
                u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX),
            );
            pointer.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), time);
        }
        self.lock_refocus();
    }

    /// Keyboard and pointer focus follow the scene.
    fn lock_refocus(&mut self) {
        self.sync_focus();
        let at = self.pointer_location();
        let time = smithay::backend::input::InputTime::from_millis(
            u32::try_from(self.start_time.elapsed().as_millis()).unwrap_or(u32::MAX),
        );
        self.move_pointer(at, time);
    }

    /// Sends `locked` when the machine says every output showed the lock.
    fn lock_confirm(&mut self) {
        let Some(id) = self.lock.confirm() else {
            return;
        };
        match self.lock_glue.pending.take() {
            Some((pending, locker)) if pending == id => locker.lock(),
            // No locker to answer: stay locked, with nobody to unlock.
            _ => self.lock.owner_died(id),
        }
    }

    /// A lock surface for `wl_output`, sized to the output.
    pub(crate) fn lock_new_surface(&mut self, surface: LockSurface, wl_output: &WlOutput) {
        let Some(lock) = self.lock_glue.lock_id_of(surface.ext_session_lock()) else {
            return;
        };
        let Some(output) = Output::from_resource(wl_output) else {
            return;
        };
        let Some(id) = self
            .outputs
            .iter()
            .find(|o| o.output == output)
            .map(|o| o.id.clone())
        else {
            return;
        };
        self.configure_lock_surface(&output, &surface);
        let key = SurfaceKey(self.lock_glue.next_id());
        self.lock.add_surface(lock, &id, key);
        self.lock_glue.surfaces.push(Surf {
            key,
            lock,
            output: id,
            surface,
        });
        self.lock_refocus();
    }

    /// Puts the output's logical size into the surface's pending state; the
    /// caller (or Smithay, for a new surface) sends the configure.
    fn configure_lock_surface(&self, output: &Output, surface: &LockSurface) {
        let size = self
            .space
            .output_geometry(output)
            .map_or((0, 0), |g| (g.size.w, g.size.h));
        let size = (
            u32::try_from(size.0).unwrap_or(0),
            u32::try_from(size.1).unwrap_or(0),
        );
        surface.with_pending_state(|state| state.size = Some(size.into()));
    }

    /// The output changed size: its lock surface follows.
    pub(crate) fn lock_output_resized(&mut self, output: &Output) {
        let Some(id) = self
            .outputs
            .iter()
            .find(|o| &o.output == output)
            .map(|o| o.id.clone())
        else {
            return;
        };
        let surfaces: Vec<LockSurface> = self
            .lock_glue
            .surfaces
            .iter()
            .filter(|s| s.output == id)
            .map(|s| s.surface.clone())
            .collect();
        for surface in surfaces {
            self.configure_lock_surface(output, &surface);
            surface.send_configure();
        }
    }

    /// `unlock_and_destroy` from the owner, already checked by Smithay.
    pub(crate) fn lock_unlock(&mut self) {
        let Some(owner) = self.lock.owner() else {
            eprintln!("tetsu-comp: an unlock with no owner; staying locked");
            return;
        };
        if self.lock.unlock(owner) {
            // A new idle period starts at the unlock.
            self.idle.timer.activity(Instant::now());
            self.lock_glue.locks.retain(|(id, _)| *id != owner);
            self.lock_glue.surfaces.clear();
            self.lock_glue.pending = None;
            self.lock_refocus();
            self.relayout();
        }
    }

    /// An output went away.
    pub(crate) fn lock_output_removed(&mut self, id: &str) {
        self.lock.output_removed(id);
        self.lock_glue.surfaces.retain(|s| s.output != id);
    }

    /// An output presented a frame; `locked_frame` says it was rendered in
    /// the locked scene.
    pub fn output_presented(&mut self, output: &Output, locked_frame: bool) {
        let Some(id) = self
            .outputs
            .iter()
            .find(|o| &o.output == output)
            .map(|o| o.id.clone())
        else {
            return;
        };
        self.lock.presented(&id, locked_frame);
        self.lock_confirm();
    }

    /// What `output` shows: the one answer the backends draw by.
    pub fn view(&self, output: &Output) -> View {
        let Some(entry) = self.outputs.iter().find(|o| &o.output == output) else {
            return View::Normal(Vec::new());
        };
        match crate::render::scene(&self.lock, &entry.id) {
            Scene::Normal => View::Normal(self.frames_on(output)),
            Scene::Locked { surface } => View::Locked(surface.and_then(|key| {
                self.lock_glue
                    .surfaces
                    .iter()
                    .find(|s| s.key == key && s.surface.alive())
                    .map(|s| s.surface.wl_surface().clone())
            })),
        }
    }

    /// The lock surface of the owner on `output`, for the pointer and frames.
    fn lock_surface_on(&self, output: &Output) -> Option<WlSurface> {
        match self.view(output) {
            View::Locked(surface) => surface,
            View::Normal(_) => None,
        }
    }

    /// The keyboard focus while locked: the lock surface of the focused
    /// output, else any lock surface of the owner, else nobody.
    pub(crate) fn lock_keyboard_target(&self) -> Option<WlSurface> {
        let focused = self.engine.model().focused_output();
        let mut outputs: Vec<&crate::state::OutputEntry> = self.outputs.iter().collect();
        outputs.sort_by_key(|o| Some(o.id.as_str()) != focused);
        outputs
            .into_iter()
            .find_map(|entry| self.lock_surface_on(&entry.output))
    }

    /// The pointer focus while locked: the lock surface of the output under
    /// the pointer, never anything else.
    pub(crate) fn lock_surface_under(
        &self,
        location: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(location).next()?.clone();
        let surface = self.lock_surface_on(&output)?;
        let origin = self.space.output_geometry(&output)?.loc.to_f64();
        Some((surface, origin))
    }

    /// Frame callbacks of a locked output: only its lock surface.
    pub(crate) fn send_lock_frames(&self, output: &Output) {
        if let Some(surface) = self.lock_surface_on(output) {
            send_frames_surface_tree(
                &surface,
                output,
                self.start_time.elapsed(),
                Some(std::time::Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }
    }

    /// Starts the locker unless a live one owns the session.
    pub(crate) fn start_locker(&mut self) {
        if matches!(self.lock.phase(), Phase::Locking | Phase::Locked) {
            return;
        }
        let command = self.lock_glue.command.clone();
        if command.is_empty() {
            eprintln!("tetsu-comp: no [lock] command to start");
            return;
        }
        if let Err(e) = spawn_trusted(self, &command, Trust::Locker) {
            eprintln!("tetsu-comp: cannot start the locker {command:?}: {e}");
        }
    }

    /// The publish tick's share: liveness, dead surfaces, the relaunch, the
    /// confirmation.
    pub(crate) fn lock_tick(&mut self) {
        if let Some(owner) = self.lock.owner()
            && self
                .lock_glue
                .locks
                .iter()
                .find(|(id, _)| *id == owner)
                .is_none_or(|(_, lock)| !lock.is_alive())
        {
            eprintln!("tetsu-comp: the locker is gone; the session stays locked");
            self.lock.owner_died(owner);
            self.lock_glue.locks.retain(|(id, _)| *id != owner);
            self.lock_glue.pending = None;
            self.lock_drop_surfaces_of(owner);
            self.lock_refocus();
        }
        let dead: Vec<SurfaceKey> = self
            .lock_glue
            .surfaces
            .iter()
            .filter(|s| !s.surface.alive())
            .map(|s| s.key)
            .collect();
        if !dead.is_empty() {
            for key in dead {
                self.lock.remove_surface(key);
                self.lock_glue.surfaces.retain(|s| s.key != key);
            }
            self.lock_refocus();
        }
        if !self.lock_glue.command.is_empty() && self.lock.relaunch_due(Instant::now()) {
            self.start_locker();
        }
        self.lock_confirm();
    }

    fn lock_drop_surfaces_of(&mut self, lock: LockId) {
        let keys: Vec<SurfaceKey> = self
            .lock_glue
            .surfaces
            .iter()
            .filter(|s| s.lock == lock)
            .map(|s| s.key)
            .collect();
        for key in keys {
            self.lock.remove_surface(key);
        }
        self.lock_glue.surfaces.retain(|s| s.lock != lock);
    }
}
