//! The compositor state: Smithay's protocol states, the space, and the
//! model the bus reports.

use std::ffi::OsString;
use std::io;
use std::time::{Duration, Instant};

use smithay::desktop::{PopupManager, Space, Window, layer_map_for_output};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::{EventLoop, LoopHandle, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode as KdeDefaultMode;
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, GlobalId,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Size, Transform as SmithayTransform};
use smithay::wayland::compositor::{CompositorClientState, CompositorState, with_states};
use smithay::wayland::dmabuf::DmabufState;
use smithay::wayland::drm_syncobj::DrmSyncobjState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::wlr_layer::{
    KeyboardInteractivity, Layer as WlrLayer, WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::virtual_pointer::VirtualPointerManagerState;

use crate::bus::{Publisher, Snapshot};
use crate::config::glue::config_reply;
use crate::config::store::Store;
use crate::decor::AnsweredOnce;
use crate::decor::glue::{BorderBuffers, DecorStates};
use crate::engine::{Arrangement, Engine};
use crate::gpu::control::RenderCtl;
use crate::idle::glue::IdleGlue;
use crate::input::ModKey;
use crate::layout::{Rect, usable};
use crate::lock::LockMachine;
use crate::lock::glue::LockGlue;
use crate::model::{OutputInfo, WindowInfo};
use crate::outputs::geometry::to_smithay_transform;
use crate::outputs::glue::{close_layers, smithay_mode, smithay_scale};
use crate::settings::{RenderSettings, Settings};
use crate::trust::{Trust, trust_of};

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
    /// Whether the output is lit. A dark one is listed but has no
    /// `wl_output` global, no layers and no workspaces.
    pub enabled: bool,
    /// Its `wl_output` global; none while it is dark.
    pub(crate) global: Option<GlobalId>,
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
    /// How much the client is trusted; fixed when it connects.
    pub trust: Trust,
}

impl ClientState {
    /// The data of a client of the given trust.
    pub fn with_trust(trust: Trust) -> Self {
        Self {
            compositor_state: CompositorClientState::default(),
            trust,
        }
    }
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
    pub(crate) layer_shell_state: WlrLayerShellState,
    /// The two decoration protocols (always server-side).
    pub(crate) decorations: DecorStates,
    /// The border solids of each window, stable across frames.
    pub(crate) border_buffers: BorderBuffers,
    pub(crate) shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub(crate) seat_state: SeatState<State>,
    pub(crate) data_device_state: DataDeviceState,
    /// `linux-dmabuf` bookkeeping; the global exists only on the tty backend.
    pub(crate) dmabuf_state: DmabufState,
    /// `wp_linux_drm_syncobj_manager_v1`, when explicit sync is enabled.
    pub(crate) syncobj_state: Option<DrmSyncobjState>,
    /// The render devices and explicit sync, as `render.v1` reports them.
    pub render: RenderCtl,
    /// The `[render]` settings.
    pub render_settings: RenderSettings,
    /// Popups of the toplevels.
    pub popups: PopupManager,
    /// The one seat.
    pub seat: Seat<Self>,

    /// The layout engine and the model the bus reports.
    pub engine: Engine,
    /// The outputs, in the order they were added.
    pub outputs: Vec<OutputEntry>,
    /// The transform of the backend's framebuffer, composed under every
    /// output's own (winit draws upside down).
    pub base_transform: SmithayTransform,
    /// Toplevels that have not committed a buffer yet.
    pub unmapped: Vec<Window>,
    /// Mapped windows.
    pub mapped: Vec<Mapped>,
    /// The bus thread, once bound.
    pub publisher: Option<Publisher>,
    /// The config layers, watched for changes.
    pub config: Option<Store>,
    /// The session lock decisions.
    pub lock: LockMachine,
    pub(crate) lock_glue: LockGlue,
    pub(crate) session_lock_state: SessionLockManagerState,
    /// The idle timer and `ext-idle-notify-v1`.
    pub(crate) idle: IdleGlue,
    /// The bus socket, passed to the clients the compositor starts.
    pub comp_socket: Option<std::path::PathBuf>,
    /// The `[trust] input` commands, started once the bus is up.
    pub trusted_input: Vec<Vec<String>>,
    /// The virtual input globals; kept for the life of the display.
    #[allow(dead_code)]
    pub(crate) virtual_keyboard_state: VirtualKeyboardManagerState,
    #[allow(dead_code)]
    pub(crate) virtual_pointer_state: VirtualPointerManagerState,
    /// `wp_viewporter`, which xwayland-satellite needs; kept for the life
    /// of the display.
    #[allow(dead_code)]
    pub(crate) viewporter_state: ViewporterState,
    /// The modifier of the layout binds.
    pub mod_key: ModKey,
    /// The tty backend, once started.
    pub tty: Option<crate::backend::udev::Tty>,
    /// Output changes owed to the tty while it is taken out of the state.
    pub(crate) tty_queue: crate::outputs::queue::ChangeQueue,
    /// Whether the tty is taken out of the state right now.
    pub(crate) tty_taken: bool,
    /// The X11 display and its satellite, when X11 is on.
    pub xwayland: Option<crate::xwayland::Xwayland>,
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
        let socket_name = crate::socket::listen(display, event_loop)?;
        // Only helpers the compositor starts with `Trust::Input` see these.
        let virtual_keyboard_state = VirtualKeyboardManagerState::new::<Self, _>(&dh, |client| {
            trust_of(client) == Trust::Input
        });
        let virtual_pointer_state = VirtualPointerManagerState::new::<Self, _>(&dh, |client| {
            trust_of(client) == Trust::Input
        });
        let trusted_input = settings.trust.input.clone();
        let lock_command = settings.lock.command.clone();
        // Any client may ask for the lock; the machine decides.
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, |_| true);
        let idle = IdleGlue::new(&dh, event_loop.handle(), settings.lock.idle_timeout_s);
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        Ok(Self {
            start_time: Instant::now(),
            loop_handle: event_loop.handle(),
            loop_signal: event_loop.get_signal(),
            socket_name,
            space: Space::default(),
            compositor_state: CompositorState::new::<Self>(&dh),
            xdg_shell_state: XdgShellState::new::<Self>(&dh),
            layer_shell_state: WlrLayerShellState::new::<Self>(&dh),
            border_buffers: BorderBuffers::default(),
            decorations: DecorStates {
                xdg: XdgDecorationState::new::<Self>(&dh),
                kde: KdeDecorationState::new::<Self>(&dh, KdeDefaultMode::Server),
                answered: AnsweredOnce::default(),
            },
            shm_state: ShmState::new::<Self>(&dh, vec![]),
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(&dh),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(&dh),
            dmabuf_state: DmabufState::new(),
            syncobj_state: None,
            render: RenderCtl::default(),
            render_settings: settings.render.clone(),
            popups: PopupManager::default(),
            seat,
            display_handle: dh,
            engine: Engine::new(settings),
            outputs: Vec::new(),
            base_transform: SmithayTransform::Normal,
            unmapped: Vec::new(),
            mapped: Vec::new(),
            publisher: None,
            config: None,
            lock: LockMachine::new(),
            lock_glue: LockGlue::new(lock_command),
            session_lock_state,
            idle,
            comp_socket: None,
            trusted_input,
            virtual_keyboard_state,
            virtual_pointer_state,
            viewporter_state,
            mod_key: ModKey::Super,
            tty: None,
            tty_queue: crate::outputs::queue::ChangeQueue::default(),
            tty_taken: false,
            xwayland: None,
            vt_switch: None,
        })
    }

    /// Adds an output of `size` at `refresh` mHz with one mode.
    pub fn add_virtual_output(
        &mut self,
        name: &str,
        size: Size<i32, smithay::utils::Physical>,
        refresh: i32,
    ) -> Output {
        let mut info = OutputInfo::named(name);
        info.modes = vec![tetsu_bus::output::Mode {
            width: i64::from(size.w),
            height: i64::from(size.h),
            refresh_mhz: i64::from(refresh),
            preferred: true,
        }];
        self.add_output(info)
    }

    /// Adds an output: the config's say on it is merged in, the model
    /// places it (and brings back the workspaces it had), the `wl_output`
    /// global appears if it is lit, the windows are laid out again.
    pub fn add_output(&mut self, mut info: OutputInfo) -> Output {
        self.merge_output_prefs(&mut info);
        let output = Output::new(
            info.name.clone(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: info.make.clone().unwrap_or_else(|| "Tetsu".into()),
                model: info.model.clone().unwrap_or_else(|| info.name.clone()),
                serial_number: info.serial.clone().unwrap_or_else(|| "Unknown".into()),
            },
        );
        for mode in &info.modes {
            output.add_mode(smithay_mode(mode));
            if mode.preferred {
                output.set_preferred(smithay_mode(mode));
            }
        }
        let current = info
            .current_mode
            .and_then(|i| info.modes.get(i))
            .map(smithay_mode);
        let enabled = info.enabled;
        let global = enabled.then(|| output.create_global::<State>(&self.display_handle));
        output.change_current_state(
            current,
            Some(self.base_transform + to_smithay_transform(info.transform)),
            Some(smithay_scale(info.scale)),
            None,
        );
        let id = self.engine.add_output_info(info);
        if enabled {
            self.lock.output_added(&id);
        }
        self.outputs.push(OutputEntry {
            id,
            output: output.clone(),
            origin: Point::from((0, 0)),
            enabled,
            global,
        });
        // The config decides about the others too (a pin may have moved).
        if let Err(refused) = self.reapply_outputs() {
            eprintln!("tetsu-comp: {refused}");
        }
        output
    }

    /// Unplugs an output: its layer surfaces are closed, its workspaces
    /// move to another output (see the model), the `wl_output` global goes
    /// away and the windows are laid out again.
    pub fn remove_output(&mut self, output: &Output) {
        let Some(at) = self.outputs.iter().position(|o| &o.output == output) else {
            return;
        };
        let entry = self.outputs.remove(at);
        close_layers(&entry.output);
        self.space.unmap_output(&entry.output);
        self.engine.remove_output(&entry.id);
        self.lock_output_removed(&entry.id);
        if let Some(global) = entry.global {
            self.retire_global(global);
        }
        eprintln!("tetsu-comp: output {} removed", entry.output.name());
        // A dark output may have been lit meanwhile (the last lit one went).
        if let Err(refused) = self.reapply_outputs() {
            eprintln!("tetsu-comp: {refused}");
        }
    }

    /// Puts every output where the model's automatic layout says. Nothing
    /// else computes an origin.
    pub fn apply_output_layout(&mut self) {
        let layout = self.engine.model().output_layout();
        for entry in &mut self.outputs {
            let Some((_, rect)) = layout.iter().find(|(id, _)| *id == entry.id) else {
                continue;
            };
            entry.origin = Point::from((rect.x, rect.y));
            entry
                .output
                .change_current_state(None, None, None, Some(entry.origin));
            self.space.map_output(&entry.output, entry.origin);
        }
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
        layer_map_for_output(output).arrange();
        if let Some(entry) = self.outputs.iter().find(|o| &o.output == output) {
            let modes = vec![tetsu_bus::output::Mode {
                width: i64::from(size.w),
                height: i64::from(size.h),
                refresh_mhz: i64::from(refresh),
                preferred: true,
            }];
            self.engine.set_output_mode(&entry.id, modes, Some(0));
        }
        self.apply_output_layout();
        self.relayout();
        self.lock_output_resized(output);
    }

    /// The rect each output lays its windows out in: the output narrowed
    /// by the exclusive zones of its layer surfaces.
    pub fn viewports(&self) -> Vec<(String, Rect)> {
        self.engine
            .model()
            .output_layout()
            .into_iter()
            .map(|(id, rect)| {
                let view = self
                    .outputs
                    .iter()
                    .find(|o| o.id == id)
                    .map_or(rect, |entry| {
                        let zone = layer_map_for_output(&entry.output).non_exclusive_zone();
                        let zone = Rect {
                            x: zone.loc.x,
                            y: zone.loc.y,
                            w: zone.size.w,
                            h: zone.size.h,
                        };
                        usable(rect, zone)
                    });
                (id, view)
            })
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
        let arrangements: Vec<Arrangement> = self
            .viewports()
            .into_iter()
            .map(|(id, view)| self.engine.arrange(&id, view))
            .collect();
        for arrangement in &arrangements {
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
        for (id, view) in self.viewports() {
            let arrangement = self.engine.arrange(&id, view);
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
    pub fn map_window(&mut self, window: Window, pid: Option<i64>, xwayland: bool) {
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let (app_id, title) = toplevel_text(toplevel.wl_surface());
        let size = window.geometry().size;
        let committed = (size.w > 0 && size.h > 0).then_some((size.w, size.h));
        let id = self.engine.open(
            WindowInfo {
                app_id,
                title,
                pid,
                xwayland,
            },
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
        self.border_buffers.forget(&mapped.id);
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
        // Locked: the keyboard is the lock surface's and nothing else's.
        if self.lock.is_locked() {
            let target = self.lock_keyboard_target();
            if let Some(keyboard) = self.seat.get_keyboard() {
                keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
            }
            return;
        }
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
        // A mapped layer that asks for exclusive keyboard focus wins.
        let surface = self.exclusive_layer().or_else(|| {
            target
                .as_ref()
                .and_then(Window::toplevel)
                .map(|t| t.wl_surface().clone())
        });
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
        }
    }

    /// The surface of a mapped `overlay` or `top` layer that asked for
    /// exclusive keyboard interactivity; `overlay` first, the focused output
    /// first. `on_demand` layers get no focus yet.
    fn exclusive_layer(&self) -> Option<WlSurface> {
        let focused = self.engine.model().focused_output();
        let mut outputs: Vec<&OutputEntry> = self.outputs.iter().collect();
        outputs.sort_by_key(|o| Some(o.id.as_str()) != focused);
        [WlrLayer::Overlay, WlrLayer::Top]
            .into_iter()
            .find_map(|wanted| {
                outputs.iter().find_map(|entry| {
                    let map = layer_map_for_output(&entry.output);
                    map.layers_on(wanted)
                        .find(|layer| {
                            layer.cached_state().keyboard_interactivity
                                == KeyboardInteractivity::Exclusive
                                && crate::handlers::has_buffer(layer.wl_surface())
                        })
                        .map(|layer| layer.wl_surface().clone())
                })
            })
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
        self.idle_tick();
        self.lock_tick();
        let mut events = self.engine.flush();
        events.session.extend(self.lock.flush());
        if let Some(publisher) = self.publisher.as_mut() {
            publisher.publish(events);
            publisher.supervise(|| {
                Snapshot::of(self.engine.model())
                    .with_locked(self.lock.is_locked())
                    .with_config(config_reply(self.config.as_ref()))
            });
        }
    }

    /// Frame callbacks, popup cleanup and a flush to the clients.
    pub fn send_frames(&mut self, output: &Output) {
        if self.lock.is_locked() {
            // Locked: only the lock surface is drawn, so only it is told.
            self.send_lock_frames(output);
            let _ = self.display_handle.flush_clients();
            return;
        }
        let elapsed = self.start_time.elapsed();
        for window in self.space.elements() {
            window.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                Some(output.clone())
            });
        }
        let zone_changed = {
            let mut map = layer_map_for_output(output);
            for layer in map.layers() {
                layer.send_frame(output, elapsed, Some(Duration::ZERO), |_, _| {
                    Some(output.clone())
                });
            }
            let before = map.non_exclusive_zone();
            map.cleanup();
            map.non_exclusive_zone() != before
        };
        if zone_changed {
            self.relayout();
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
