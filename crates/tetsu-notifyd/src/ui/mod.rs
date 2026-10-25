//! The surfaces: popups and the panel, on layer-shell, drawn in software
//! (#12 stack: SCTK, tiny-skia, cosmic-text). They only draw the daemon's
//! view and report presses; every rule lives in the state machine.
//!
//! Popups never take the keyboard (`keyboard_interactivity: none`); the
//! panel takes it only on demand, after the user clicks it, and handles Esc
//! only. No output is named: until `tetsu-comp` publishes the focused
//! output, the compositor places the surfaces (on the focused output for
//! niri and most wlroots compositors).

pub mod draw;
pub mod layout;
mod wayland;

use std::io;
use std::time::Instant;

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::reexports::calloop::channel::{self, Channel};
use smithay_client_toolkit::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle};
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
};
use smithay_client_toolkit::shm::Shm;
use smithay_client_toolkit::shm::slot::SlotPool;
use tokio::sync::mpsc;

use crate::core::View;
use crate::daemon::{Command, UiMsg};
use draw::Painter;
use layout::{CARD_WIDTH, Card, Hit, MARGIN, PANEL_WIDTH, PanelLayout};
use wayland::Globals;

const NAMESPACE: &str = "tetsu-notifyd";

/// One layer surface and what is known about it.
#[derive(Debug)]
struct Surface {
    layer: LayerSurface,
    /// Logical size from the last configure; zero until then.
    size: (u32, u32),
    /// Logical size last requested.
    requested: (u32, u32),
    scale: i32,
}

impl Surface {
    fn is(&self, surface: &wl_surface::WlSurface) -> bool {
        self.layer.wl_surface() == surface
    }

    fn configured(&self) -> bool {
        self.size.0 > 0 && self.size.1 > 0
    }
}

/// The UI state on its own thread.
pub struct Ui {
    globals: Globals,
    pool: SlotPool,
    painter: Painter,
    commands: mpsc::UnboundedSender<Command>,
    handle: LoopHandle<'static, Ui>,
    view: View,
    popup: Option<Surface>,
    panel: Option<Surface>,
    panel_wanted: bool,
    panel_scroll: f32,
    cards: Vec<Card>,
    /// The cards on screen: presses are tested against these, never against
    /// a layout whose buffer is not committed yet.
    drawn_cards: Vec<Card>,
    panel_layout: Option<PanelLayout>,
    keyboard_focus: Option<ObjectId>,
    /// A redraw is scheduled for when the next buttons arm.
    arming_timer: Option<Instant>,
    failed: Option<io::Error>,
}

impl std::fmt::Debug for Ui {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ui")
            .field("view", &self.view)
            .finish_non_exhaustive()
    }
}

/// The receiving end for the daemon's messages on the UI thread.
pub type UiChannel = Channel<UiMsg>;

/// A channel whose sender the daemon side forwards [`UiMsg`] into.
pub fn channel() -> (channel::Sender<UiMsg>, UiChannel) {
    channel::channel()
}

/// Runs the surfaces until the Wayland connection fails.
pub fn run(messages: UiChannel, commands: mpsc::UnboundedSender<Command>) -> io::Result<()> {
    let conn = Connection::connect_to_env().map_err(io::Error::other)?;
    let (globals, queue) = registry_queue_init::<Ui>(&conn).map_err(io::Error::other)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, Ui> = EventLoop::try_new()?;
    let handle = event_loop.handle();
    WaylandSource::new(conn, queue)
        .insert(handle.clone())
        .map_err(|e| io::Error::other(e.to_string()))?;
    let shm = Shm::bind(&globals, &qh).map_err(io::Error::other)?;
    let pool = SlotPool::new(CARD_WIDTH as usize * 400 * 4, &shm).map_err(io::Error::other)?;
    let ui_globals = Globals {
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).map_err(io::Error::other)?,
        layer_shell: LayerShell::bind(&globals, &qh)
            .map_err(|e| io::Error::other(format!("no layer-shell: {e}")))?,
        shm,
        pointer: None,
        keyboard: None,
    };
    let message_qh = qh.clone();
    handle
        .insert_source(messages, move |event, _, ui: &mut Ui| {
            if let channel::Event::Msg(msg) = event {
                ui.message(msg, &message_qh);
            }
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
    let mut ui = Ui {
        globals: ui_globals,
        pool,
        painter: Painter::new(),
        commands,
        handle,
        view: View::default(),
        popup: None,
        panel: None,
        panel_wanted: false,
        panel_scroll: 0.0,
        cards: Vec::new(),
        drawn_cards: Vec::new(),
        panel_layout: None,
        keyboard_focus: None,
        arming_timer: None,
        failed: None,
    };
    loop {
        event_loop.dispatch(None, &mut ui)?;
        if let Some(e) = ui.failed.take() {
            return Err(e);
        }
    }
}

impl Ui {
    fn message(&mut self, msg: UiMsg, qh: &QueueHandle<Self>) {
        match msg {
            UiMsg::View(view) => self.view = view,
            UiMsg::OpenPanel => {
                self.panel_wanted = true;
                self.panel_scroll = 0.0;
            }
        }
        self.refresh(qh);
    }

    /// Creates, resizes or removes surfaces to match the view, and draws
    /// those already configured.
    fn refresh(&mut self, qh: &QueueHandle<Self>) {
        if self.view.locked {
            // Nothing is drawn under the lock; pending approvals pop again
            // on unlock.
            self.popup = None;
            self.drawn_cards.clear();
            if self.panel.take().is_some() {
                let _ = self.commands.send(Command::PanelClosed);
            }
            self.panel_layout = None;
            self.panel_wanted = false;
            return;
        }
        let (cards, height) = layout::stack(&self.view.popups, &mut self.painter);
        self.cards = cards;
        if self.cards.is_empty() {
            self.popup = None;
            self.drawn_cards.clear();
        } else {
            let size = (CARD_WIDTH as u32, height.ceil() as u32);
            if self.popup.is_none() {
                self.popup = Some(self.new_surface(qh, size, KeyboardInteractivity::None, false));
            }
            self.resize_popup_or_draw(qh, size);
        }
        if !self.panel_wanted {
            if self.panel.take().is_some() {
                let _ = self.commands.send(Command::PanelClosed);
            }
            self.panel_layout = None;
            return;
        }
        if self.panel.is_none() {
            let size = (PANEL_WIDTH as u32, 0);
            self.panel = Some(self.new_surface(qh, size, KeyboardInteractivity::OnDemand, true));
        }
        self.draw_panel(qh);
    }

    fn new_surface(
        &self,
        qh: &QueueHandle<Self>,
        size: (u32, u32),
        keyboard: KeyboardInteractivity,
        full_height: bool,
    ) -> Surface {
        let surface = self.globals.compositor.create_surface(qh);
        let layer = self.globals.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some(NAMESPACE),
            None,
        );
        let anchor = if full_height {
            Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM
        } else {
            Anchor::TOP | Anchor::RIGHT
        };
        let margin = MARGIN as i32;
        layer.set_anchor(anchor);
        layer.set_margin(margin, margin, margin, margin);
        layer.set_keyboard_interactivity(keyboard);
        layer.set_size(size.0, size.1);
        layer.commit();
        Surface {
            layer,
            size: (0, 0),
            requested: size,
            scale: 1,
        }
    }

    /// Asks for a new popup size when it changed, else draws.
    fn resize_popup_or_draw(&mut self, qh: &QueueHandle<Self>, size: (u32, u32)) {
        let Some(surface) = self.popup.as_mut() else {
            return;
        };
        if surface.requested != size {
            surface.requested = size;
            surface.layer.set_size(size.0, size.1);
            surface.layer.commit();
            return;
        }
        if surface.configured() {
            self.draw_popup(qh);
        }
    }

    fn configured(&mut self, layer: &LayerSurface, size: (u32, u32), qh: &QueueHandle<Self>) {
        if let Some(popup) = self.popup.as_mut().filter(|s| &s.layer == layer) {
            popup.size = (size.0.max(popup.requested.0), size.1.max(popup.requested.1));
            self.draw_popup(qh);
        } else if let Some(panel) = self.panel.as_mut().filter(|s| &s.layer == layer) {
            let first = !panel.configured();
            panel.size = (size.0.max(1), size.1.max(1));
            if first {
                // What the panel shows arms from now.
                let _ = self.commands.send(Command::PanelOpened);
            }
            self.draw_panel(qh);
        }
    }

    fn surface_closed(&mut self, layer: &LayerSurface, qh: &QueueHandle<Self>) {
        if self.popup.as_ref().is_some_and(|s| &s.layer == layer) {
            self.popup = None;
            self.refresh(qh);
        } else if self.panel.as_ref().is_some_and(|s| &s.layer == layer) {
            self.close_panel(qh);
        }
    }

    fn set_scale(&mut self, surface: &wl_surface::WlSurface, factor: i32, qh: &QueueHandle<Self>) {
        let factor = factor.max(1);
        if let Some(popup) = self.popup.as_mut().filter(|s| s.is(surface)) {
            popup.scale = factor;
            self.draw_popup(qh);
        } else if let Some(panel) = self.panel.as_mut().filter(|s| s.is(surface)) {
            panel.scale = factor;
            self.draw_panel(qh);
        }
    }

    fn draw_popup(&mut self, qh: &QueueHandle<Self>) {
        let Some(popup) = self.popup.as_ref().filter(|s| s.configured()) else {
            return;
        };
        let now = Instant::now();
        let (w, h) = popup.size;
        let scale = popup.scale as u32;
        let pixmap = self
            .painter
            .popups(&self.cards, w as f32, h as f32, scale, now);
        if let Some(pixmap) = pixmap {
            present(&mut self.pool, &popup.layer, &pixmap, popup.scale);
            self.drawn_cards = self.cards.clone();
        }
        self.schedule_arming(qh, now);
    }

    fn draw_panel(&mut self, qh: &QueueHandle<Self>) {
        let Some(panel) = self.panel.as_ref().filter(|s| s.configured()) else {
            return;
        };
        let now = Instant::now();
        let (w, h) = (panel.size.0 as f32, panel.size.1 as f32);
        let layout = layout::panel(
            &self.view.pending,
            &self.view.open,
            self.view.history.len(),
            h,
            self.panel_scroll,
            &mut self.painter,
        );
        let scale = panel.scale as u32;
        if let Some(pixmap) = self
            .painter
            .panel(&layout, &self.view.history, w, h, scale, now)
        {
            present(&mut self.pool, &panel.layer, &pixmap, panel.scale);
        }
        self.panel_layout = Some(layout);
        self.schedule_arming(qh, now);
    }

    /// Redraws when the next drawn buttons arm, so they light up.
    fn schedule_arming(&mut self, qh: &QueueHandle<Self>, now: Instant) {
        let next = self
            .view
            .popups
            .iter()
            .chain(&self.view.pending)
            .map(|n| n.armed_at)
            .filter(|at| *at > now)
            .min();
        let Some(at) = next else { return };
        if self.arming_timer.is_some_and(|t| t <= at) {
            return;
        }
        self.arming_timer = Some(at);
        let qh = qh.clone();
        let inserted =
            self.handle
                .insert_source(Timer::from_deadline(at), move |_, _, ui: &mut Ui| {
                    ui.arming_timer = None;
                    ui.draw_popup(&qh);
                    ui.draw_panel(&qh);
                    TimeoutAction::Drop
                });
        if inserted.is_err() {
            self.arming_timer = None;
        }
    }

    fn press(&mut self, surface: &wl_surface::WlSurface, x: f32, y: f32, qh: &QueueHandle<Self>) {
        let hit = if self.popup.as_ref().is_some_and(|s| s.is(surface)) {
            self.drawn_cards.iter().find_map(|c| c.hit(x, y))
        } else if self.panel.as_ref().is_some_and(|s| s.is(surface)) {
            self.panel_layout.as_ref().and_then(|l| l.hit(x, y))
        } else {
            None
        };
        let command = match hit {
            Some(Hit::Action(note, action)) => Command::Click { note, action },
            Some(Hit::Default(note)) => Command::Click {
                note,
                action: "default".into(),
            },
            Some(Hit::Dismiss(note)) => Command::Dismiss(note),
            Some(Hit::ClosePanel) => return self.close_panel(qh),
            None => return,
        };
        let _ = self.commands.send(command);
    }

    fn scroll(&mut self, surface: &wl_surface::WlSurface, delta: f32, qh: &QueueHandle<Self>) {
        let Some(panel) = self.panel.as_ref().filter(|s| s.is(surface)) else {
            return;
        };
        let content = self.panel_layout.as_ref().map_or(0.0, |l| l.content_height);
        let max = (content - panel.size.1 as f32).max(0.0);
        self.panel_scroll = (self.panel_scroll + delta).clamp(0.0, max);
        self.draw_panel(qh);
    }

    fn escape(&mut self, qh: &QueueHandle<Self>) {
        let focused = self
            .panel
            .as_ref()
            .is_some_and(|p| Some(p.layer.wl_surface().id()) == self.keyboard_focus);
        if focused {
            self.close_panel(qh);
        }
    }

    fn close_panel(&mut self, qh: &QueueHandle<Self>) {
        self.panel_wanted = false;
        self.refresh(qh);
    }
}

/// Copies a pixmap into a fresh shm buffer and commits it.
fn present(pool: &mut SlotPool, layer: &LayerSurface, pixmap: &tiny_skia::Pixmap, scale: i32) {
    let (w, h) = (pixmap.width() as i32, pixmap.height() as i32);
    let Ok((buffer, canvas)) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Argb8888) else {
        eprintln!("tetsu-notifyd: no shm buffer for a {w}x{h} surface");
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
