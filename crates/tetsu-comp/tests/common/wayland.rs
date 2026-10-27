//! A small xdg-shell client on Smithay client toolkit, for the end-to-end
//! tests.

use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, Proxy, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

/// A configured size; a side is `None` when the compositor left it open.
type Size = (Option<NonZeroU32>, Option<NonZeroU32>);

struct App {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    configures: Vec<(ObjectId, Size)>,
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

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {}

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _: u32,
    ) {
        let id = window.wl_surface().id();
        self.configures.push((id, configure.new_size));
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
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
    registry_handlers![OutputState];
}

delegate_registry!(App);
delegate_dispatch2!(App);

/// A mapped toplevel.
pub struct Toplevel {
    window: Window,
}

/// One connection to the compositor under test.
pub struct WlClient {
    _conn: Connection,
    queue: EventQueue<App>,
    app: App,
    compositor: CompositorState,
    xdg: XdgShell,
    pool: Option<SlotPool>,
    buffers: Vec<smithay_client_toolkit::shm::slot::Buffer>,
}

impl WlClient {
    /// Connects to the Wayland socket at `socket`.
    pub fn connect(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
        let qh = queue.handle();
        let mut app = App {
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &qh),
            shm: Shm::bind(&globals, &qh).unwrap(),
            configures: Vec::new(),
        };
        let compositor = CompositorState::bind(&globals, &qh).unwrap();
        let xdg = XdgShell::bind(&globals, &qh).unwrap();
        queue.roundtrip(&mut app).unwrap();
        Self {
            _conn: conn,
            queue,
            app,
            compositor,
            xdg,
            pool: None,
            buffers: Vec::new(),
        }
    }

    /// Maps a toplevel: commits it bare, waits for the first configure,
    /// then attaches a buffer of the size it was given.
    pub fn map(&mut self, app_id: &str, title: &str) -> Toplevel {
        let qh = self.queue.handle();
        let surface = self.compositor.create_surface(&qh);
        let window = self
            .xdg
            .create_window(surface, WindowDecorations::None, &qh);
        window.set_app_id(app_id);
        window.set_title(title);
        window.commit();
        let top = Toplevel { window };
        while self.sizes(&top).is_empty() {
            self.queue.blocking_dispatch(&mut self.app).unwrap();
        }
        let (w, h) = self.sizes(&top)[0];
        let (w, h) = (w as i32, h as i32);
        let pool = self
            .pool
            .insert(SlotPool::new((w * h * 4) as usize, &self.app.shm).unwrap());
        let (buffer, _canvas) = pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)
            .unwrap();
        buffer.attach_to(top.window.wl_surface()).unwrap();
        top.window.commit();
        self.buffers.push(buffer);
        self.queue.roundtrip(&mut self.app).unwrap();
        top
    }

    /// The sizes the compositor configured the toplevel to, in order.
    pub fn sizes(&self, top: &Toplevel) -> Vec<(u32, u32)> {
        let id = top.window.wl_surface().id();
        self.app
            .configures
            .iter()
            .filter(|(surface, _)| *surface == id)
            .filter_map(|(_, (w, h))| Some(((*w)?.get(), (*h)?.get())))
            .collect()
    }

    /// Dispatches until the toplevel's latest configured size is `size`.
    pub fn wait_for_size(&mut self, top: &Toplevel, size: (u32, u32)) {
        let start = Instant::now();
        while self.sizes(top).last() != Some(&size) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "no configure to {size:?}: {:?}",
                self.sizes(top)
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Sets the title and commits.
    pub fn set_title(&mut self, top: &Toplevel, title: &str) {
        top.window.set_title(title);
        top.window.commit();
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Destroys the toplevel.
    pub fn destroy(&mut self, top: Toplevel) {
        drop(top);
        self.buffers.clear();
        self.queue.roundtrip(&mut self.app).unwrap();
    }
}
