//! A session locker on Smithay client toolkit, for the end-to-end tests: it
//! behaves like swaylock, a lock surface on every output, a buffer on each.
#![allow(dead_code)]

use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, Proxy, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::session_lock::{
    SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
    SessionLockSurfaceConfigure,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

const TIMEOUT: Duration = Duration::from_secs(10);

/// A lock surface of this client.
struct Surf {
    surface: SessionLockSurface,
    output: wl_output::WlOutput,
    pool: Option<SlotPool>,
}

struct App {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    shm: Shm,
    seats: SeatState,
    lock_state: SessionLockState,
    lock: Option<SessionLock>,
    surfaces: Vec<Surf>,
    /// Configures: the output name and the size.
    configures: Vec<(Option<String>, (u32, u32))>,
    locked: bool,
    finished: bool,
    /// Keyboard enter (`true`) and leave (`false`) by surface.
    keys: Vec<(ObjectId, bool)>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
}

struct KeyboardData;

impl Dispatch2<wl_keyboard::WlKeyboard, App> for KeyboardData {
    fn event(
        &self,
        app: &mut App,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        match event {
            wl_keyboard::Event::Enter { surface, .. } => app.keys.push((surface.id(), true)),
            wl_keyboard::Event::Leave { surface, .. } => app.keys.push((surface.id(), false)),
            _ => {}
        }
    }
}

impl App {
    /// A lock surface for `output`, when the lock is live.
    fn add_surface(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput) {
        let Some(lock) = self.lock.as_ref() else {
            return;
        };
        if self.finished || self.surfaces.iter().any(|s| s.output == *output) {
            return;
        }
        let surface = self.compositor.create_surface(qh);
        let surface = lock.create_lock_surface(surface, output, qh);
        self.surfaces.push(Surf {
            surface,
            output: output.clone(),
            pool: None,
        });
    }
}

impl SessionLockHandler for App {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        self.locked = true;
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        self.finished = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _: u32,
    ) {
        let (w, h) = configure.new_size;
        let output = self
            .surfaces
            .iter()
            .find(|s| s.surface.wl_surface() == surface.wl_surface())
            .map(|s| s.output.clone());
        let name = output
            .as_ref()
            .and_then(|o| self.outputs.info(o))
            .and_then(|info| info.name);
        self.configures.push((name, (w, h)));
        let App { shm, surfaces, .. } = self;
        let Some(rec) = surfaces
            .iter_mut()
            .find(|s| s.surface.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let (wi, hi) = (w as i32, h as i32);
        let mut pool = SlotPool::new((wi * hi * 4) as usize, shm).unwrap();
        let (buffer, _canvas) = pool
            .create_buffer(wi, hi, wi * 4, wl_shm::Format::Argb8888)
            .unwrap();
        buffer.attach_to(surface.wl_surface()).unwrap();
        surface.wl_surface().damage_buffer(0, 0, wi, hi);
        surface.wl_surface().commit();
        rec.pool = Some(pool);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = Some(seat.get_keyboard(qh, KeyboardData));
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        _: Capability,
    ) {
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        self.add_surface(qh, &output);
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

delegate_registry!(App);
delegate_dispatch2!(App);

/// One locker client.
pub struct Locker {
    conn: Connection,
    queue: EventQueue<App>,
    app: App,
    /// A handle on the socket, to cut the connection like a crash.
    cut: Option<UnixStream>,
}

impl Locker {
    /// Connects to the Wayland socket at `socket`.
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        let cut = stream.try_clone().ok();
        Self::over(Connection::from_socket(stream).unwrap(), cut)
    }

    /// Connects like a client the compositor started: `WAYLAND_SOCKET` first.
    pub fn connect_env() -> Self {
        Self::over(Connection::connect_to_env().unwrap(), None)
    }

    fn over(conn: Connection, cut: Option<UnixStream>) -> Self {
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let mut app = App {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &qh),
            compositor: CompositorState::bind(&globals, &qh).unwrap(),
            shm: Shm::bind(&globals, &qh).unwrap(),
            seats: SeatState::new(&globals, &qh),
            lock_state: SessionLockState::new(&globals, &qh),
            lock: None,
            surfaces: Vec::new(),
            configures: Vec::new(),
            locked: false,
            finished: false,
            keys: Vec::new(),
            keyboard: None,
        };
        queue.roundtrip(&mut app).unwrap();
        Self {
            conn,
            queue,
            app,
            cut,
        }
    }

    /// Asks for the lock and puts a lock surface on every output, now and
    /// as outputs appear.
    pub fn lock(&mut self) {
        let qh = self.queue.handle();
        let lock = self.app.lock_state.lock(&qh).expect("ext_session_lock");
        self.app.lock = Some(lock);
        let outputs: Vec<_> = self.app.outputs.outputs().collect();
        for output in outputs {
            self.app.add_surface(&qh, &output);
        }
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Dispatches until `done` holds.
    fn until(&mut self, what: &str, mut done: impl FnMut(&App) -> bool) {
        let start = Instant::now();
        while !done(&self.app) {
            assert!(start.elapsed() < TIMEOUT, "timed out waiting for {what}");
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_locked(&mut self) {
        self.until("locked", |a| a.locked);
    }

    pub fn wait_finished(&mut self) {
        self.until("finished", |a| a.finished);
    }

    /// Round-trips for `span`, so a late event would show.
    pub fn settle(&mut self, span: Duration) {
        let start = Instant::now();
        while start.elapsed() < span {
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn is_locked(&self) -> bool {
        self.app.locked
    }

    pub fn is_finished(&self) -> bool {
        self.app.finished
    }

    /// Dispatches until `n` lock surfaces were configured.
    pub fn wait_for_configures(&mut self, n: usize) {
        self.until("configures", |a| a.configures.len() >= n);
    }

    /// The configures so far: the output name and the size.
    pub fn configures(&self) -> Vec<(Option<String>, (u32, u32))> {
        self.app.configures.clone()
    }

    /// Keyboard events of the lock surfaces: enter `true`, leave `false`.
    pub fn keyboard_events(&self) -> Vec<bool> {
        let ids: Vec<ObjectId> = self
            .app
            .surfaces
            .iter()
            .map(|s| s.surface.wl_surface().id())
            .collect();
        self.app
            .keys
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(_, enter)| *enter)
            .collect()
    }

    pub fn wait_for_keyboard_events(&mut self, n: usize) {
        let start = Instant::now();
        while self.keyboard_events().len() < n {
            assert!(
                start.elapsed() < TIMEOUT,
                "keyboard events: {:?}",
                self.keyboard_events()
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Unlocks and destroys the lock.
    pub fn unlock(&mut self) {
        if let Some(lock) = self.app.lock.take() {
            lock.unlock();
        }
        self.app.surfaces.clear();
        self.conn.flush().unwrap();
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Dies without unlocking: the connection is cut, nothing is sent.
    pub fn crash(mut self) {
        if let Some(cut) = self.cut.take() {
            let _ = cut.shutdown(std::net::Shutdown::Both);
        }
        // Dropping the lock would try to send `destroy`; the socket is cut.
        std::mem::forget(self);
    }
}
