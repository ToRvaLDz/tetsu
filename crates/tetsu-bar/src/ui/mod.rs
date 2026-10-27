//! The surfaces: one layer-shell bar per output, drawn in software (#12
//! stack: SCTK, tiny-skia, cosmic-text). The UI thread owns the [`Bar`]
//! model; every input goes through [`Bar::apply`], and a frame is drawn
//! only when it returns a new bar. Between events the only wakeups are the
//! clock and sampling timers.

pub mod draw;
pub mod layout;
mod wayland;

use std::io;
use std::path::Path;

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::reexports::calloop::channel::{self, Channel};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
};
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::SlotPool;

use crate::bus::Strip;
use crate::clock;
use crate::model::{Bar, Change};
use crate::sources::{SAMPLE_INTERVAL, battery, network};
use draw::Painter;
use wayland::Globals;

const NAMESPACE: &str = "tetsu-bar";
/// Logical height of the bar, and its exclusive zone.
pub const BAR_HEIGHT: u32 = 28;
/// Where the readers look: the real `/sys` and `/proc`.
const ROOT: &str = "/";
/// First size of the shm pool; it grows on demand.
const POOL_BYTES: usize = 2560 * BAR_HEIGHT as usize * 4;

/// What the UI thread is told.
#[derive(Debug)]
pub enum Msg {
    /// A new workspace strip (or `None`) from the bus thread.
    Strip(Strip),
    /// Stop: SIGTERM or SIGINT.
    Quit,
}

/// Why a frame was drawn, for `TETSU_BAR_TRACE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The workspace strip changed.
    Bus,
    /// The minute rolled over.
    Tick,
    /// A battery or network sample changed.
    Source,
    /// A surface was configured.
    Configure,
    /// A surface changed scale.
    Scale,
}

impl Reason {
    fn name(self) -> &'static str {
        match self {
            Self::Bus => "bus",
            Self::Tick => "tick",
            Self::Source => "source",
            Self::Configure => "configure",
            Self::Scale => "scale",
        }
    }
}

/// One layer surface and what is known about it.
#[derive(Debug)]
struct Surface {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    /// Logical width from the last configure; zero until then.
    width: u32,
    scale: i32,
}

impl Surface {
    fn is(&self, surface: &wl_surface::WlSurface) -> bool {
        self.layer.wl_surface() == surface
    }
}

/// The UI state on the main thread.
pub struct Ui {
    globals: Globals,
    pool: SlotPool,
    painter: Painter,
    bar: Bar,
    surfaces: Vec<Surface>,
    trace: bool,
    quit: bool,
}

impl std::fmt::Debug for Ui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ui")
            .field("bar", &self.bar)
            .finish_non_exhaustive()
    }
}

/// The receiving end for the bus thread's messages.
pub type UiChannel = Channel<Msg>;

/// A channel whose sender the bus thread forwards [`Msg`] into.
pub fn channel() -> (channel::Sender<Msg>, UiChannel) {
    channel::channel()
}

fn io_err(e: impl ToString) -> io::Error {
    io::Error::other(e.to_string())
}

/// Runs the bars until a quit message or a Wayland failure. With `trace`,
/// prints one `draw <reason>` line per redraw to stderr.
pub fn run(messages: UiChannel, trace: bool) -> io::Result<()> {
    let conn = Connection::connect_to_env()
        .map_err(|e| io_err(format!("cannot connect to the Wayland display: {e}")))?;
    let (globals, queue) = registry_queue_init::<Ui>(&conn).map_err(io_err)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, Ui> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    WaylandSource::new(conn, queue)
        .insert(handle.clone())
        .map_err(io_err)?;
    let shm = Shm::bind(&globals, &qh).map_err(io_err)?;
    let pool = SlotPool::new(POOL_BYTES, &shm).map_err(io_err)?;
    let ui_globals = Globals {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).map_err(io_err)?,
        layer_shell: LayerShell::bind(&globals, &qh)
            .map_err(|e| io_err(format!("the compositor has no layer-shell: {e}")))?,
        shm,
    };
    let root = Path::new(ROOT);
    let mut ui = Ui {
        globals: ui_globals,
        pool,
        painter: Painter::new(),
        bar: Bar {
            clock: clock::label(&clock::now()),
            battery: battery::read(root),
            network: network::read(root),
            workspaces: None,
        },
        surfaces: Vec::new(),
        trace,
        quit: false,
    };
    insert_sources(&handle, messages)?;
    while !ui.quit {
        event_loop.dispatch(None, &mut ui)?;
    }
    Ok(())
}

fn insert_sources(handle: &LoopHandle<'static, Ui>, messages: UiChannel) -> io::Result<()> {
    handle
        .insert_source(messages, |event, _, ui: &mut Ui| {
            if let channel::Event::Msg(msg) = event {
                match msg {
                    Msg::Strip(strip) => ui.apply([Change::Workspaces(strip)], Reason::Bus),
                    Msg::Quit => ui.quit = true,
                }
            }
        })
        .map_err(io_err)?;
    // One timer, re-armed to the next minute boundary after each firing.
    let first = clock::until_next_minute(&clock::now());
    handle
        .insert_source(Timer::from_duration(first), |_, _, ui: &mut Ui| {
            let now = clock::now();
            ui.apply([Change::Tick(clock::label(&now))], Reason::Tick);
            TimeoutAction::ToDuration(clock::until_next_minute(&now))
        })
        .map_err(io_err)?;
    handle
        .insert_source(
            Timer::from_duration(SAMPLE_INTERVAL),
            |_, _, ui: &mut Ui| {
                let root = Path::new(ROOT);
                // The minute timer runs on the monotonic clock and stalls across
                // suspend; re-reading the wall clock here bounds the staleness.
                let changes = [
                    Change::Tick(clock::label(&clock::now())),
                    Change::Battery(battery::read(root)),
                    Change::Network(network::read(root)),
                ];
                ui.apply(changes, Reason::Source);
                TimeoutAction::ToDuration(SAMPLE_INTERVAL)
            },
        )
        .map_err(io_err)?;
    Ok(())
}

impl Ui {
    /// Applies the changes in order and draws every bar once, if any of
    /// them changed what is shown.
    fn apply(&mut self, changes: impl IntoIterator<Item = Change>, reason: Reason) {
        let mut bar = self.bar.clone();
        let mut changed = false;
        for change in changes {
            if let Some(next) = bar.apply(change) {
                bar = next;
                changed = true;
            }
        }
        if !changed {
            return;
        }
        self.bar = bar;
        if self.trace {
            eprintln!("draw {}", reason.name());
        }
        for index in 0..self.surfaces.len() {
            self.draw(index);
        }
    }

    /// A bar on `output`, drawn once the compositor configures it.
    fn add_surface(&mut self, output: wl_output::WlOutput, qh: &QueueHandle<Self>) {
        if self.surfaces.iter().any(|s| s.output == output) {
            return;
        }
        let scale = self
            .globals
            .outputs
            .info(&output)
            .map_or(1, |info| info.scale_factor.max(1));
        let surface = self.globals.compositor.create_surface(qh);
        let layer = self.globals.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Top,
            Some(NAMESPACE),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
        layer.set_size(0, BAR_HEIGHT);
        layer.set_exclusive_zone(BAR_HEIGHT as i32);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.commit();
        self.surfaces.push(Surface {
            layer,
            output,
            width: 0,
            scale,
        });
    }

    fn remove_output(&mut self, output: &wl_output::WlOutput) {
        self.surfaces.retain(|s| &s.output != output);
    }

    fn remove_layer(&mut self, layer: &LayerSurface) {
        self.surfaces.retain(|s| &s.layer != layer);
    }

    fn configured(&mut self, layer: &LayerSurface, width: u32, reason: Reason) {
        let Some(index) = self.surfaces.iter().position(|s| &s.layer == layer) else {
            return;
        };
        self.surfaces[index].width = width.max(1);
        self.trace_frame(reason);
        self.draw(index);
    }

    fn set_scale(&mut self, surface: &wl_surface::WlSurface, factor: i32) {
        let Some(index) = self.surfaces.iter().position(|s| s.is(surface)) else {
            return;
        };
        let factor = factor.max(1);
        if self.surfaces[index].scale == factor {
            return;
        }
        self.surfaces[index].scale = factor;
        self.trace_frame(Reason::Scale);
        self.draw(index);
    }

    fn trace_frame(&self, reason: Reason) {
        if self.trace {
            eprintln!("draw {}", reason.name());
        }
    }

    fn draw(&mut self, index: usize) {
        let surface = &self.surfaces[index];
        if surface.width == 0 {
            return;
        }
        let scale = surface.scale;
        let pixmap = self.painter.bar(
            &self.bar,
            surface.width as f32,
            BAR_HEIGHT as f32,
            scale as u32,
        );
        if let Some(pixmap) = pixmap {
            present(&mut self.pool, &surface.layer, &pixmap, scale);
        }
    }
}

/// Copies a pixmap into a fresh shm buffer and commits it.
fn present(pool: &mut SlotPool, layer: &LayerSurface, pixmap: &tiny_skia::Pixmap, scale: i32) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let Ok((buffer, canvas)) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) else {
        eprintln!("tetsu-bar: no shm buffer for a {w}x{h} surface");
        return;
    };
    draw::to_argb8888(pixmap, canvas);
    let surface = layer.wl_surface();
    surface.set_buffer_scale(scale);
    surface.damage_buffer(0, 0, w, h);
    if buffer.attach_to(surface).is_ok() {
        layer.commit();
    }
}
