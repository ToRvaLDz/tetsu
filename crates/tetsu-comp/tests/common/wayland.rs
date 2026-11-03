//! A small xdg-shell client on Smithay client toolkit, for the end-to-end
//! tests.
#![allow(dead_code)]

use std::num::NonZeroU32;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface,
};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::xdg::shell::client::xdg_toplevel::XdgToplevel;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shell::xdg::window::{
    DecorationMode, Window, WindowConfigure, WindowDecorations, WindowHandler,
};
use smithay_client_toolkit::shm::slot::Buffer;
use smithay_client_toolkit::shm::slot::SlotPool;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

/// A configured size; a side is `None` when the compositor left it open.
type Size = (Option<NonZeroU32>, Option<NonZeroU32>);

struct App {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    seats: SeatState,
    compositor: CompositorState,
    layer_shell: Option<LayerShell>,
    configures: Vec<(ObjectId, Size, bool)>,
    /// The decoration mode of each toplevel configure.
    modes: Vec<(ObjectId, DecorationMode)>,
    /// Whether to put a bar on every `wl_output`, as tetsu-bar does.
    mimic_bar: bool,
    layers: Vec<LayerRec>,
    layer_buffers: Vec<(ObjectId, SlotPool, Buffer)>,
    /// Layer configures: the surface and the size it got.
    layer_configures: Vec<(ObjectId, (u32, u32))>,
    closed: Vec<ObjectId>,
    /// Frame callbacks that arrived, by surface.
    frames: Vec<ObjectId>,
    /// Keyboard enter (`true`) and leave (`false`) by surface.
    keys: Vec<(ObjectId, bool)>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// `wl_output`s kept by name, to use after their global is gone.
    kept: Vec<(String, wl_output::WlOutput)>,
}

/// A layer surface this client made.
struct LayerRec {
    surface: LayerSurface,
    /// The name of the output it was made for, if any.
    output: Option<String>,
    /// The size it attaches when the compositor leaves a side open.
    fallback: (u32, u32),
    bar: bool,
}

/// Keyboard events of the seat.
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

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.closed.push(layer.wl_surface().id());
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let id = layer.wl_surface().id();
        self.layer_configures.push((id.clone(), configure.new_size));
        let fallback = self
            .layers
            .iter()
            .find(|l| l.surface == *layer)
            .map_or((1, 1), |l| l.fallback);
        let w = if configure.new_size.0 == 0 {
            fallback.0
        } else {
            configure.new_size.0
        } as i32;
        let h = if configure.new_size.1 == 0 {
            fallback.1
        } else {
            configure.new_size.1
        } as i32;
        let mut pool = SlotPool::new((w * h * 4) as usize, &self.shm).unwrap();
        let (buffer, _canvas) = pool
            .create_buffer(w, h, w * 4, wl_shm::Format::Argb8888)
            .unwrap();
        buffer.attach_to(layer.wl_surface()).unwrap();
        layer.wl_surface().damage_buffer(0, 0, w, h);
        layer
            .wl_surface()
            .frame(qh, FrameCallbackData(layer.wl_surface().clone()));
        layer.commit();
        self.layer_buffers.retain(|(surface, ..)| *surface != id);
        self.layer_buffers.push((id, pool, buffer));
    }
}

impl App {
    fn output_name(&self, output: &wl_output::WlOutput) -> Option<String> {
        self.outputs.info(output).and_then(|info| info.name)
    }

    /// A tetsu-bar shaped layer surface on `output`.
    fn add_bar(&mut self, qh: &QueueHandle<Self>, output: &wl_output::WlOutput) {
        let Some(shell) = self.layer_shell.as_ref() else {
            return;
        };
        let surface = self.compositor.create_surface(qh);
        let layer =
            shell.create_layer_surface(qh, surface, Layer::Top, Some("tetsu-bar"), Some(output));
        layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, 28);
        layer.set_exclusive_zone(28);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        let name = self.output_name(output);
        self.layers.push(LayerRec {
            surface: layer,
            output: name,
            fallback: (1920, 28),
            bar: true,
        });
    }
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
    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.frames.push(surface.id());
    }
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
        let tiled = configure.is_tiled_left();
        self.modes.push((id.clone(), configure.decoration_mode));
        self.configures.push((id, configure.new_size, tiled));
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if self.mimic_bar {
            self.add_bar(qh, &output);
        }
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

/// The interface names of the globals the compositor advertises.
pub fn globals(socket: &Path) -> Vec<String> {
    let stream = UnixStream::connect(socket).unwrap();
    let conn = Connection::from_socket(stream).unwrap();
    let (globals, _queue) = registry_queue_init::<App>(&conn).unwrap();
    globals
        .contents()
        .clone_list()
        .into_iter()
        .map(|g| g.interface)
        .collect()
}

/// A mapped toplevel.
pub struct Toplevel {
    window: Window,
}

impl Toplevel {
    /// Its `xdg_toplevel`, for protocols the helper does not wrap.
    pub fn xdg_toplevel(&self) -> &XdgToplevel {
        self.window.xdg_toplevel()
    }

    /// The id of its `wl_surface`.
    pub fn surface_id(&self) -> ObjectId {
        self.window.wl_surface().id()
    }
}

/// One connection to the compositor under test.
pub struct WlClient {
    conn: Connection,
    queue: EventQueue<App>,
    app: App,
    xdg: XdgShell,
    pool: Option<SlotPool>,
    buffers: Vec<smithay_client_toolkit::shm::slot::Buffer>,
}

impl WlClient {
    /// The connection, for a helper with its own event queue.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

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
            seats: SeatState::new(&globals, &qh),
            compositor: CompositorState::bind(&globals, &qh).unwrap(),
            layer_shell: LayerShell::bind(&globals, &qh).ok(),
            configures: Vec::new(),
            modes: Vec::new(),
            mimic_bar: false,
            layers: Vec::new(),
            layer_buffers: Vec::new(),
            layer_configures: Vec::new(),
            closed: Vec::new(),
            frames: Vec::new(),
            keys: Vec::new(),
            keyboard: None,
            kept: Vec::new(),
        };
        let xdg = XdgShell::bind(&globals, &qh).unwrap();
        queue.roundtrip(&mut app).unwrap();
        Self {
            conn,
            queue,
            app,
            xdg,
            pool: None,
            buffers: Vec::new(),
        }
    }

    /// Maps a toplevel: commits it bare, waits for the first configure,
    /// then attaches a buffer of the size it was given.
    pub fn map(&mut self, app_id: &str, title: &str) -> Toplevel {
        self.map_with(app_id, title, None, WindowDecorations::None)
    }

    /// Like `map`, with the decorations the window is created with.
    pub fn map_decorated(
        &mut self,
        app_id: &str,
        title: &str,
        decorations: WindowDecorations,
    ) -> Toplevel {
        self.map_with(app_id, title, None, decorations)
    }

    /// Like `map`, for a window that may be configured without a size: it
    /// attaches a buffer of `fallback` then.
    pub fn map_floating(&mut self, app_id: &str, title: &str, fallback: (i32, i32)) -> Toplevel {
        self.map_with(app_id, title, Some(fallback), WindowDecorations::None)
    }

    fn map_with(
        &mut self,
        app_id: &str,
        title: &str,
        fallback: Option<(i32, i32)>,
        decorations: WindowDecorations,
    ) -> Toplevel {
        let qh = self.queue.handle();
        let surface = self.app.compositor.create_surface(&qh);
        let window = self.xdg.create_window(surface, decorations, &qh);
        window.set_app_id(app_id);
        window.set_title(title);
        window.commit();
        let top = Toplevel { window };
        while self.configure_count(&top) == 0 {
            self.queue.blocking_dispatch(&mut self.app).unwrap();
        }
        let (w, h) = match (self.sizes(&top).first().copied(), fallback) {
            (Some((w, h)), _) => (w as i32, h as i32),
            (None, Some(size)) => size,
            (None, None) => panic!("configured without a size and no fallback"),
        };
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

    fn configure_count(&self, top: &Toplevel) -> usize {
        let id = top.window.wl_surface().id();
        self.app.configures.iter().filter(|c| c.0 == id).count()
    }

    /// Whether the last configure of the toplevel had the tiled states.
    pub fn states(&self, top: &Toplevel) -> bool {
        let id = top.window.wl_surface().id();
        self.app
            .configures
            .iter()
            .rfind(|c| c.0 == id)
            .is_some_and(|c| c.2)
    }

    /// The decoration mode of each configure of the toplevel, in order.
    pub fn decoration_modes(&self, top: &Toplevel) -> Vec<DecorationMode> {
        let id = top.window.wl_surface().id();
        self.app
            .modes
            .iter()
            .filter(|(surface, _)| *surface == id)
            .map(|(_, mode)| *mode)
            .collect()
    }

    /// Asks for a decoration mode (`None`: no preference).
    pub fn request_decoration(&mut self, top: &Toplevel, mode: Option<DecorationMode>) {
        top.window.request_decoration_mode(mode);
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Dispatches until the toplevel has had more than `n` configures.
    pub fn wait_for_configure_after(&mut self, top: &Toplevel, n: usize) {
        let start = Instant::now();
        while self.configure_count(top) <= n {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "no configure after the {n}th"
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// How many configures the toplevel has had.
    pub fn configures(&self, top: &Toplevel) -> usize {
        self.configure_count(top)
    }

    /// Round-trips to the compositor.
    pub fn sync(&mut self) {
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// The sizes the compositor configured the toplevel to, in order.
    pub fn sizes(&self, top: &Toplevel) -> Vec<(u32, u32)> {
        let id = top.window.wl_surface().id();
        self.app
            .configures
            .iter()
            .filter(|(surface, ..)| *surface == id)
            .filter_map(|(_, (w, h), _)| Some(((*w)?.get(), (*h)?.get())))
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

    // ---- layer-shell ----

    /// Whether the compositor advertised `zwlr_layer_shell_v1`.
    pub fn has_layer_shell(&self) -> bool {
        self.app.layer_shell.is_some()
    }

    /// Puts a bar on every `wl_output`, now and as outputs appear, the way
    /// tetsu-bar does: `top`, anchored top, left and right, 28 px high and
    /// as wide as the output, exclusive zone 28, no keyboard.
    pub fn bar_on_every_output(&mut self) {
        self.app.mimic_bar = true;
        let qh = self.queue.handle();
        let outputs: Vec<_> = self.app.outputs.outputs().collect();
        for output in outputs {
            self.app.add_bar(&qh, &output);
        }
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// A bar on the output called `name` only.
    pub fn bar_on(&mut self, name: &str) {
        let qh = self.queue.handle();
        let output = self
            .app
            .outputs
            .outputs()
            .find(|o| self.app.output_name(o).as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no output {name}"));
        self.app.add_bar(&qh, &output);
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Remembers the `wl_output` called `name`, for `bar_on_kept` later.
    pub fn keep_output(&mut self, name: &str) {
        let output = self
            .app
            .outputs
            .outputs()
            .find(|o| self.app.output_name(o).as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no output {name}"));
        self.app.kept.push((name.to_owned(), output));
    }

    /// A bar on a kept `wl_output`, even if the compositor retired it since.
    pub fn bar_on_kept(&mut self, name: &str) {
        let qh = self.queue.handle();
        let output = self
            .app
            .kept
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| o.clone())
            .unwrap_or_else(|| panic!("no kept output {name}"));
        self.app.add_bar(&qh, &output);
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Drops the bars: their surfaces are destroyed.
    pub fn destroy_bars(&mut self) {
        let gone: Vec<ObjectId> = self
            .app
            .layers
            .iter()
            .filter(|l| l.bar)
            .map(|l| l.surface.wl_surface().id())
            .collect();
        self.app.layers.retain(|l| !l.bar);
        self.app.layer_buffers.retain(|(id, ..)| !gone.contains(id));
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// An `overlay` layer that asks for exclusive keyboard focus.
    pub fn overlay_exclusive(&mut self) -> Overlay {
        let qh = self.queue.handle();
        let shell = self.app.layer_shell.as_ref().expect("no layer shell");
        let surface = self.app.compositor.create_surface(&qh);
        let layer = shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("lock"), None);
        layer.set_size(200, 100);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.commit();
        let id = layer.wl_surface().id();
        self.app.layers.push(LayerRec {
            surface: layer,
            output: None,
            fallback: (200, 100),
            bar: false,
        });
        self.wait_until("the overlay is configured", |app| {
            app.layer_configures.iter().any(|(s, _)| *s == id)
        });
        // The buffer it attached maps it; wait for the compositor to see it.
        self.sync();
        Overlay { id }
    }

    /// Destroys an overlay.
    pub fn destroy_overlay(&mut self, overlay: Overlay) {
        self.app
            .layers
            .retain(|l| l.surface.wl_surface().id() != overlay.id);
        self.app.layer_buffers.retain(|(id, ..)| *id != overlay.id);
        self.queue.roundtrip(&mut self.app).unwrap();
    }

    /// Dispatches until `done` holds.
    fn wait_until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let start = Instant::now();
        while !done(&self.app) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Dispatches until `n` layer surfaces have been configured.
    pub fn wait_for_layers(&mut self, n: usize) {
        self.wait_until(&format!("{n} configured layers"), |app| {
            let mut ids: Vec<&ObjectId> = app.layer_configures.iter().map(|(id, _)| id).collect();
            ids.sort_by_key(|id| id.to_string());
            ids.dedup();
            ids.len() >= n
        });
    }

    /// The first configure of each layer surface, with the name of the
    /// output it was made for, sorted by that name.
    pub fn layer_sizes(&self) -> Vec<(Option<String>, (u32, u32))> {
        let mut seen: Vec<&ObjectId> = Vec::new();
        let mut sizes = Vec::new();
        for (id, size) in &self.app.layer_configures {
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            let output = self
                .app
                .layers
                .iter()
                .find(|l| l.surface.wl_surface().id() == *id)
                .and_then(|l| l.output.clone());
            sizes.push((output, *size));
        }
        sizes.sort();
        sizes
    }

    /// Frame callbacks received by the layer surface made for `output`.
    pub fn frames_on(&self, output: &str) -> usize {
        let ids: Vec<ObjectId> = self
            .app
            .layers
            .iter()
            .filter(|l| l.output.as_deref() == Some(output))
            .map(|l| l.surface.wl_surface().id())
            .collect();
        self.app.frames.iter().filter(|f| ids.contains(f)).count()
    }

    /// Dispatches until the layer surface of `output` got a frame callback.
    pub fn wait_for_frame_on(&mut self, output: &str) {
        let start = Instant::now();
        while self.frames_on(output) == 0 {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "no frame callback on {output}"
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Layer surfaces the compositor closed.
    pub fn closed_count(&self) -> usize {
        self.app.closed.len()
    }

    /// Dispatches until `n` layer surfaces were closed.
    pub fn wait_for_closed(&mut self, n: usize) {
        self.wait_until(&format!("{n} closed layers"), |app| app.closed.len() >= n);
    }

    /// The names of the `wl_output`s the client knows.
    pub fn output_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .app
            .outputs
            .outputs()
            .filter_map(|o| self.app.output_name(&o))
            .collect();
        names.sort();
        names
    }

    /// Dispatches until the client knows exactly these outputs.
    pub fn wait_for_outputs(&mut self, names: &[&str]) {
        let mut wanted: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
        wanted.sort();
        let start = Instant::now();
        while self.output_names() != wanted {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "outputs are {:?}, not {wanted:?}",
                self.output_names()
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Keyboard enter (`true`) and leave (`false`) events of a toplevel.
    pub fn keyboard_events(&self, top: &Toplevel) -> Vec<bool> {
        let id = top.window.wl_surface().id();
        self.app
            .keys
            .iter()
            .filter(|(surface, _)| *surface == id)
            .map(|(_, entered)| *entered)
            .collect()
    }

    /// Dispatches until the toplevel has had `n` keyboard events.
    pub fn wait_for_keyboard_events(&mut self, top: &Toplevel, n: usize) {
        let start = Instant::now();
        while self.keyboard_events(top).len() < n {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "keyboard events: {:?}",
                self.keyboard_events(top)
            );
            self.queue.roundtrip(&mut self.app).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A mapped overlay layer.
pub struct Overlay {
    id: ObjectId,
}
