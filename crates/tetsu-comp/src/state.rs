//! The compositor state: Smithay's protocol states, the space, and the
//! model the bus reports.

use std::ffi::OsString;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::desktop::{PopupManager, Space, Window};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{
    EventLoop, Interest, LoopHandle, LoopSignal, Mode as PollMode, PostAction,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Size, Transform};
use smithay::wayland::compositor::{CompositorClientState, CompositorState, with_states};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;

use crate::bus::{Publisher, Snapshot};
use crate::engine::{Arrangement, Engine};
use crate::input::ModKey;
use crate::layout::Rect;
use crate::model::WindowInfo;
use crate::settings::Settings;

/// Key repeat delay and rate of the seat keyboard.
const REPEAT_DELAY_MS: i32 = 200;
const REPEAT_RATE: i32 = 25;

/// An output the model knows.
pub struct OutputEntry {
    /// The model's id (`out-<n>`).
    pub id: String,
    /// The Wayland output.
    pub output: Output,
    /// Where it sits in the space.
    pub origin: Point<i32, Logical>,
}

/// A mapped window and its id in the model.
pub struct Mapped {
    /// The model's id (`win-<n>`).
    pub id: String,
    /// The window.
    pub window: Window,
    /// Whether the last layout pass placed it as floating.
    pub floating: bool,
}

/// Data attached to each Wayland client.
#[derive(Default)]
pub struct ClientState {
    /// Smithay's per-client compositor state.
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

/// Everything the compositor owns.
pub struct State {
    /// When the compositor started, for frame callback times.
    pub start_time: Instant,
    /// The Wayland display.
    pub display_handle: DisplayHandle,
    /// The calloop handle, to add event sources.
    pub loop_handle: LoopHandle<'static, State>,
    /// Stops the loop.
    pub loop_signal: LoopSignal,
    /// The Wayland socket name (`wayland-1`).
    pub socket_name: OsString,

    /// Mapped windows and outputs.
    pub space: Space<Window>,
    pub(crate) compositor_state: CompositorState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub(crate) seat_state: SeatState<State>,
    pub(crate) data_device_state: DataDeviceState,
    /// Popups of the toplevels.
    pub popups: PopupManager,
    /// The one seat.
    pub seat: Seat<Self>,

    /// The layout engine and the model the bus reports.
    pub engine: Engine,
    /// The outputs, in the order they were added.
    pub outputs: Vec<OutputEntry>,
    /// Toplevels that have not committed a buffer yet.
    pub unmapped: Vec<Window>,
    /// Mapped windows.
    pub mapped: Vec<Mapped>,
    /// The bus thread, once bound.
    pub publisher: Option<Publisher>,
    /// The modifier of the layout binds.
    pub mod_key: ModKey,
    /// The tty backend, once started.
    pub tty: Option<crate::backend::udev::Tty>,
    /// Switches to a VT (tty only).
    pub vt_switch: Option<Box<dyn FnMut(i32)>>,
}

impl State {
    /// Creates the state and starts listening for Wayland clients.
    pub fn new(
        event_loop: &mut EventLoop<'static, State>,
        display: Display<State>,
        settings: Settings,
    ) -> io::Result<Self> {
        let dh = display.handle();
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "tetsu");
        seat.add_keyboard(Default::default(), REPEAT_DELAY_MS, REPEAT_RATE)
            .map_err(|e| io::Error::other(format!("keyboard: {e}")))?;
        seat.add_pointer();
        let socket_name = Self::listen(display, event_loop)?;
        Ok(Self {
            start_time: Instant::now(),
            loop_handle: event_loop.handle(),
            loop_signal: event_loop.get_signal(),
            socket_name,
            space: Space::default(),
            compositor_state: CompositorState::new::<Self>(&dh),
            xdg_shell_state: XdgShellState::new::<Self>(&dh),
            shm_state: ShmState::new::<Self>(&dh, vec![]),
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&dh),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(&dh),
            popups: PopupManager::default(),
            seat,
            display_handle: dh,
            engine: Engine::new(settings),
            outputs: Vec::new(),
            unmapped: Vec::new(),
            mapped: Vec::new(),
            publisher: None,
            mod_key: ModKey::Super,
            tty: None,
            vt_switch: None,
        })
    }

    fn listen(
        display: Display<State>,
        event_loop: &mut EventLoop<'static, State>,
    ) -> io::Result<OsString> {
        let listening = ListeningSocketSource::new_auto().map_err(io::Error::other)?;
        let socket_name = listening.socket_name().to_os_string();
        let handle = event_loop.handle();
        handle
            .insert_source(listening, |stream, (), state| {
                let data = Arc::new(ClientState::default());
                if let Err(e) = state.display_handle.insert_client(stream, data) {
                    eprintln!("tetsu-comp: cannot accept a client: {e}");
                }
            })
            .map_err(|e| io::Error::other(e.to_string()))?;
        handle
            .insert_source(
                Generic::new(display, Interest::READ, PollMode::Level),
                |_, display, state| {
                    // SAFETY: the display is never dropped while the source lives.
                    #[expect(unsafe_code)]
                    let result = unsafe { display.get_mut().dispatch_clients(state) };
                    match result {
                        Ok(_) => Ok(PostAction::Continue),
                        Err(e) => {
                            eprintln!("tetsu-comp: wayland dispatch failed: {e}");
                            Ok(PostAction::Remove)
                        }
                    }
                },
            )
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(socket_name)
    }

    /// Adds an output of `size` at `refresh` mHz to the right of the others.
    pub fn add_output(
        &mut self,
        name: &str,
        size: Size<i32, smithay::utils::Physical>,
        refresh: i32,
    ) -> Output {
        let output = Output::new(
            name.to_owned(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tetsu".into(),
                model: name.to_owned(),
                serial_number: "Unknown".into(),
            },
        );
        let mode = Mode { size, refresh };
        let x: i32 = self
            .outputs
            .iter()
            .filter_map(|o| o.output.current_mode())
            .map(|m| m.size.w)
            .sum();
        let origin = Point::from((x, 0));
        let _global = output.create_global::<State>(&self.display_handle);
        output.change_current_state(Some(mode), Some(Transform::Normal), None, Some(origin));
        output.set_preferred(mode);
        self.space.map_output(&output, origin);
        let id = self.engine.add_output(name);
        self.outputs.push(OutputEntry {
            id,
            output: output.clone(),
            origin,
        });
        output
    }

    /// Resizes an output's mode and tiles its windows again.
    pub fn resize_output(
        &mut self,
        output: &Output,
        size: Size<i32, smithay::utils::Physical>,
        refresh: i32,
    ) {
        let mode = Mode { size, refresh };
        output.change_current_state(Some(mode), None, None, None);
        output.set_preferred(mode);
        self.relayout();
    }

    fn viewport(entry: &OutputEntry) -> Rect {
        let size = entry
            .output
            .current_mode()
            .map_or((0, 0).into(), |m| m.size);
        Rect {
            x: entry.origin.x,
            y: entry.origin.y,
            w: size.w,
            h: size.h,
        }
    }

    /// The rect each output lays its windows out in.
    pub fn viewports(&self) -> Vec<(String, Rect)> {
        self.outputs
            .iter()
            .map(|o| (o.id.clone(), Self::viewport(o)))
            .collect()
    }

    /// The window of a toplevel's `wl_surface`, mapped or not.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        let wants = |w: &Window| w.toplevel().is_some_and(|t| t.wl_surface() == surface);
        self.mapped
            .iter()
            .map(|m| &m.window)
            .chain(self.unmapped.iter())
            .find(|w| wants(w))
            .cloned()
    }

    /// Answers the first commit of a toplevel with its tile, or with no
    /// size when it floats.
    pub(crate) fn send_initial_configure(&self, toplevel: &ToplevelSurface) {
        let (app_id, title) = toplevel_text(toplevel.wl_surface());
        let plan = self
            .engine
            .plan(app_id.as_deref(), title.as_deref(), &self.viewports());
        if plan.floating {
            apply_floating(toplevel, plan.configure_size);
        } else if let Some((w, h)) = plan.configure_size {
            apply_tile(toplevel, (w, h));
        }
        toplevel.send_configure();
    }

    /// Lays the windows of every output's active workspace out again:
    /// tiles, then floating windows above them; windows of other
    /// workspaces are unmapped.
    pub fn relayout(&mut self) {
        let arrangements: Vec<(Rect, Arrangement)> = self
            .outputs
            .iter()
            .map(|o| {
                let view = Self::viewport(o);
                (view, self.engine.arrange(&o.id, view))
            })
            .collect();
        for (_, arrangement) in &arrangements {
            for id in &arrangement.hidden {
                if let Some(mapped) = self.mapped.iter().find(|m| &m.id == id) {
                    self.space.unmap_elem(&mapped.window);
                }
            }
            for (id, rect) in &arrangement.tiled {
                let Some(mapped) = self.mapped.iter_mut().find(|m| &m.id == id) else {
                    continue;
                };
                mapped.floating = false;
                self.space
                    .map_element(mapped.window.clone(), (rect.x, rect.y), false);
                if let Some(toplevel) = mapped.window.toplevel() {
                    apply_tile(toplevel, (rect.w, rect.h));
                    toplevel.send_pending_configure();
                }
            }
            for (id, rect) in &arrangement.floating {
                let Some(mapped) = self.mapped.iter_mut().find(|m| &m.id == id) else {
                    continue;
                };
                // A window that just left the tiles takes its floating size;
                // one that already floated keeps the size its client chose.
                let size = (!mapped.floating).then_some((rect.w, rect.h));
                mapped.floating = true;
                self.space
                    .map_element(mapped.window.clone(), (rect.x, rect.y), false);
                if let Some(toplevel) = mapped.window.toplevel() {
                    apply_floating_pending(toplevel, size);
                    toplevel.send_pending_configure();
                }
            }
        }
        self.raise_floating();
    }

    /// Floating windows above the tiles, the focused one last.
    fn raise_floating(&mut self) {
        let focused = self.engine.model().focused_window().map(str::to_owned);
        let mut order: Vec<String> = Vec::new();
        for o in &self.outputs {
            let arrangement = self.engine.arrange(&o.id, Self::viewport(o));
            order.extend(arrangement.floating.into_iter().map(|(id, _)| id));
        }
        order.sort_by_key(|id| focused.as_ref() == Some(id));
        for id in order {
            if let Some(mapped) = self.mapped.iter().find(|m| m.id == id) {
                self.space.raise_element(&mapped.window, false);
            }
        }
    }

    /// Maps a toplevel that committed its first buffer: it is opened in the
    /// engine, which places it by the rules, tiled or floating.
    pub fn map_window(&mut self, window: Window, pid: Option<i64>) {
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let (app_id, title) = toplevel_text(toplevel.wl_surface());
        let size = window.geometry().size;
        let committed = (size.w > 0 && size.h > 0).then_some((size.w, size.h));
        let id = self.engine.open(
            WindowInfo { app_id, title, pid },
            committed,
            &self.viewports(),
        );
        let floating = self.engine.is_floating(&id);
        self.mapped.push(Mapped {
            id,
            window,
            floating,
        });
        self.relayout();
        self.sync_focus();
    }

    /// Forgets a toplevel that went away.
    pub fn unmap_window(&mut self, surface: &WlSurface) {
        self.unmapped
            .retain(|w| w.toplevel().is_none_or(|t| t.wl_surface() != surface));
        let Some(at) = self.mapped.iter().position(|m| {
            m.window
                .toplevel()
                .is_some_and(|t| t.wl_surface() == surface)
        }) else {
            return;
        };
        let mapped = self.mapped.remove(at);
        self.engine.close(&mapped.id);
        self.space.unmap_elem(&mapped.window);
        self.relayout();
        self.sync_focus();
    }

    /// Focuses a window (by model id) or nothing, and follows it.
    pub fn focus_window(&mut self, id: Option<&str>) {
        self.engine.focus(id);
        self.sync_focus();
    }

    /// Makes the keyboard focus and the activated state follow the model.
    pub fn sync_focus(&mut self) {
        let focused = self.engine.model().focused_window().map(str::to_owned);
        let target = focused
            .as_ref()
            .and_then(|id| self.mapped.iter().find(|m| &m.id == id))
            .map(|m| m.window.clone());
        self.raise_floating();
        for mapped in &self.mapped {
            let active = focused.as_ref() == Some(&mapped.id);
            mapped.window.set_activated(active);
            if let Some(toplevel) = mapped.window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
        let surface = target
            .as_ref()
            .and_then(Window::toplevel)
            .map(|t| t.wl_surface().clone());
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// The process id of the client owning `surface`.
    pub fn client_pid(&self, surface: &WlSurface) -> Option<i64> {
        let client = self.display_handle.get_client(surface.id()).ok()?;
        let credentials = client.get_credentials(&self.display_handle).ok()?;
        Some(i64::from(credentials.pid))
    }

    /// Flushes the model's events to the bus thread and restarts it if it
    /// died. Runs on the publish tick.
    pub fn publish_tick(&mut self) {
        let events = self.engine.flush();
        if let Some(publisher) = self.publisher.as_mut() {
            publisher.publish(events);
            publisher.supervise(|| Snapshot::of(self.engine.model()));
        }
    }

    /// Frame callbacks, popup cleanup and a flush to the clients.
    pub fn send_frames(&mut self, output: &Output) {
        let elapsed = self.start_time.elapsed();
        for window in self.space.elements() {
            window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        self.space.refresh();
        self.popups.cleanup();
        let _ = self.display_handle.flush_clients();
    }
}

/// A toplevel's app_id and title.
pub(crate) fn toplevel_text(surface: &WlSurface) -> (Option<String>, Option<String>) {
    with_states(surface, |states| {
        let data = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|d| d.lock().unwrap_or_else(|e| e.into_inner()));
        data.map_or((None, None), |d| (d.app_id.clone(), d.title.clone()))
    })
}

const TILED_EDGES: [xdg_toplevel::State; 4] = [
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

/// Puts a tile into a toplevel's pending state.
fn apply_tile(toplevel: &ToplevelSurface, size: (i32, i32)) {
    toplevel.with_pending_state(|state| {
        state.size = Some(size.into());
        for edge in TILED_EDGES {
            state.states.set(edge);
        }
    });
}

/// A floating window: no tiled states; `size` when it has one to take,
/// else none (the client chooses).
fn apply_floating(toplevel: &ToplevelSurface, size: Option<(i32, i32)>) {
    toplevel.with_pending_state(|state| {
        state.size = size.map(Into::into);
        for edge in TILED_EDGES {
            state.states.unset(edge);
        }
    });
}

/// Like `apply_floating`, but leaves the size alone when `size` is `None`.
fn apply_floating_pending(toplevel: &ToplevelSurface, size: Option<(i32, i32)>) {
    toplevel.with_pending_state(|state| {
        if size.is_some() {
            state.size = size.map(Into::into);
        }
        for edge in TILED_EDGES {
            state.states.unset(edge);
        }
    });
}
