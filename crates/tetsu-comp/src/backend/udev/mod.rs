//! The tty backend: a libseat session, every GPU on the seat through
//! DRM/GBM/GLES behind Smithay's `GpuManager`, libinput.
//!
//! One render device composes every output; an output on another card gets a
//! cross-GPU copy (`MultiRenderer`). Every decision (which device renders,
//! how an output is served, which formats, whether explicit sync is on) lives
//! in the pure `crate::gpu` modules; this module only gathers the facts and
//! acts on the answers.
//!
//! Connectors are hotplugged: a udev `Changed` on a known card, and a session
//! resume, look at its connectors again; a lit one that is no longer
//! connected loses its output, a newly connected one is lit. A card that goes
//! away takes its heads with it. Cards are fixed at start: one added later is
//! only logged, and the render device is not chosen again (#70's hot-switch
//! follow-up).
//!
//! The multi-GPU policy is ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787` (GPL-3.0-or-later); the
//! `MultiRenderer` plumbing follows Smithay's `anvil`.

mod apply;
mod device;
mod feedback;
mod frame;
mod output;

use std::error::Error;
use std::time::Duration;

use smithay::backend::allocator::gbm::GbmAllocator;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager};
use smithay::backend::drm::{DrmDeviceFd, DrmNode};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::Id;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::multigpu::gbm::GbmGlesBackend;
use smithay::backend::renderer::multigpu::{GpuManager, MultiRenderer};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{UdevBackend, UdevEvent};
use smithay::output::Output;
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::drm::control::{connector, crtc};
use smithay::reexports::input::Libinput;
use smithay::utils::Point;

use self::frame::Scenery;
use crate::gpu::assign::OutputPlan;
use crate::lock::glue::View;
use crate::state::State;

use feedback::SurfaceDmabufFeedback;

/// Retry delay when a frame had nothing to draw.
const IDLE_RETRY: Duration = Duration::from_millis(16);

/// The GPU manager over GBM-backed GLES devices.
type Backend = GbmGlesBackend<GlesRenderer, DrmDeviceFd>;
type Gpus = GpuManager<Backend>;
/// The renderer of a frame: the render device, with a copy path to the
/// output's device when that is another one.
type TtyRenderer<'a> = MultiRenderer<'a, 'a, Backend, Backend>;

type Allocator = GbmAllocator<DrmDeviceFd>;
type Exporter = GbmFramebufferExporter<DrmDeviceFd>;
type Manager = DrmOutputManager<Allocator, Exporter, bool, DrmDeviceFd>;
type Lit = DrmOutput<Allocator, Exporter, bool, DrmDeviceFd>;

/// One card.
struct Device {
    node: DrmNode,
    /// Its render node; `None` for a display-only device.
    render_node: Option<DrmNode>,
    plan: OutputPlan,
    manager: Manager,
    /// The VBlank source of the card.
    events: RegistrationToken,
}

/// A lit connector.
struct Head {
    node: DrmNode,
    crtc: crtc::Handle,
    /// The connector it lights.
    connector: connector::Handle,
    output: Output,
    drm_output: Lit,
    /// Where this output's buffers are scanned out from.
    target: DrmNode,
    feedback: Option<SurfaceDmabufFeedback>,
}

/// The tty backend's state.
pub struct Tty {
    gpus: Gpus,
    render_node: DrmNode,
    devices: Vec<Device>,
    heads: Vec<Head>,
    cursor_id: Id,
    /// The id of the lock backdrop.
    lock_id: Id,
    libinput: Libinput,
    wait_for_frame_completion: bool,
    /// The `linux-dmabuf` global, kept alive for the session.
    dmabuf_global: Option<smithay::wayland::dmabuf::DmabufGlobal>,
}

/// Opens the session and every GPU, picks the render device, lights every
/// connected connector, sets up dmabuf and explicit sync and starts reading
/// input.
pub fn init(state: &mut State) -> Result<(), Box<dyn Error>> {
    state.mod_key = crate::input::ModKey::Super;
    let (mut session, notifier) = LibSeatSession::new().map_err(|e| format!("libseat: {e}"))?;
    let seat = session.seat();

    let mut gpus = device::new_gpu_manager()?;
    let probed = device::probe_all(&mut session, &seat);
    let (selected, opened) =
        device::open_devices(&mut gpus, probed, &state.render_settings, &seat)?;

    let mut tty = Tty {
        gpus,
        render_node: selected.node,
        devices: Vec::new(),
        heads: Vec::new(),
        cursor_id: Id::new(),
        lock_id: Id::new(),
        libinput: libinput_for(&session, &seat)?,
        wait_for_frame_completion: state.render_settings.wait_for_frame_completion,
        dmabuf_global: None,
    };
    output::build_devices(state, &mut tty, opened)?;
    output::light_connectors(state, &mut tty)?;
    if tty.heads.is_empty() {
        return Err("no connected output".into());
    }
    feedback::init_dmabuf(state, &mut tty)?;
    device::init_explicit_sync(state, &selected);

    let libinput = tty.libinput.clone();
    state.tty = Some(tty);
    state
        .loop_handle
        .insert_source(LibinputInputBackend::new(libinput), |event, (), state| {
            state.process_input_event(event);
        })
        .map_err(|e| e.to_string())?;
    state
        .loop_handle
        .insert_source(notifier, |event, (), state| state.session_event(&event))
        .map_err(|e| e.to_string())?;

    let udev = UdevBackend::new(&seat).map_err(|e| format!("udev: {e}"))?;
    state
        .loop_handle
        .insert_source(udev, |event, _, state| match event {
            UdevEvent::Changed { device_id } => state.card_changed(device_id),
            UdevEvent::Removed { device_id } => state.card_removed(device_id),
            UdevEvent::Added { path, .. } => eprintln!(
                "tetsu-comp: card {} added; restart tetsu-comp to use it",
                path.display()
            ),
        })
        .map_err(|e| e.to_string())?;

    let mut switcher = session;
    state.vt_switch = Some(Box::new(move |vt| {
        if let Err(e) = switcher.change_vt(vt) {
            eprintln!("tetsu-comp: cannot switch to VT {vt}: {e}");
        }
    }));
    state.loop_handle.insert_idle(|state| state.render_all());
    Ok(())
}

fn libinput_for(session: &LibSeatSession, seat: &str) -> Result<Libinput, Box<dyn Error>> {
    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(seat)
        .map_err(|()| format!("libinput cannot use seat {seat}"))?;
    Ok(libinput)
}

impl State {
    /// A frame reached the screen: draw the next one.
    fn frame_done(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let mut presented = None;
        if let Some(tty) = self.tty.as_mut()
            && let Some(head) = tty
                .heads
                .iter_mut()
                .find(|h| h.node == node && h.crtc == crtc)
        {
            match head.drm_output.frame_submitted() {
                // A frame reached the screen: it counts for the lock only if
                // it was rendered in the locked scene (the flag travels with
                // the frame as its user data).
                Ok(Some(locked)) => presented = Some((head.output.clone(), locked)),
                // Nothing was pending: nothing was presented.
                Ok(None) => {}
                Err(e) => eprintln!("tetsu-comp: frame_submitted: {e}"),
            }
        }
        if let Some((output, locked)) = presented {
            self.output_presented(&output, locked);
        }
        self.render_crtc(node, crtc);
    }

    fn render_all(&mut self) {
        let heads: Vec<_> = self
            .tty
            .as_ref()
            .map(|t| t.heads.iter().map(|h| (h.node, h.crtc)).collect())
            .unwrap_or_default();
        for (node, crtc) in heads {
            self.render_crtc(node, crtc);
        }
    }

    /// Draws a frame; with nothing to draw, tries again next frame time.
    fn render_crtc(&mut self, node: DrmNode, crtc: crtc::Handle) {
        // Decided before the renderer is borrowed: what this head may show.
        let view = self
            .tty
            .as_ref()
            .and_then(|t| t.heads.iter().find(|h| h.node == node && h.crtc == crtc))
            .map(|h| h.output.clone())
            .map_or(View::Normal(Vec::new()), |output| self.view(&output));
        if let View::Normal(frames) = &view {
            self.border_buffers.sync(frames);
        }
        let Some(mut tty) = self.tty.take() else {
            return;
        };
        let pointer = self
            .seat
            .get_pointer()
            .map_or_else(Point::default, |p| p.current_location());
        let rendered = tty.render(
            &Scenery {
                space: &self.space,
                mapped: &self.mapped,
                borders: &self.border_buffers,
            },
            &view,
            pointer,
            node,
            crtc,
        );
        let head = tty.heads.iter().find(|h| h.node == node && h.crtc == crtc);
        let known = head.is_some();
        let output = head.map(|h| h.output.clone());
        let feedback = head.and_then(|h| h.feedback.clone());
        self.tty = Some(tty);
        match rendered {
            Ok(Some(states)) => {
                if let Some(output) = output {
                    self.send_frames(&output);
                    self.send_dmabuf_feedback(&output, feedback.as_ref(), &states);
                }
            }
            // The output was unplugged meanwhile: nothing left to retry.
            Ok(None) if !known => {}
            Ok(None) => self.schedule_retry(node, crtc),
            Err(e) if !known => eprintln!("tetsu-comp: render of an unplugged output: {e}"),
            Err(e) => {
                // No queued frame means no VBlank: without a retry the output stays frozen.
                eprintln!("tetsu-comp: render failed: {e}");
                self.schedule_retry(node, crtc);
            }
        }
    }

    /// Tries the CRTC again at the next frame time.
    fn schedule_retry(&mut self, node: DrmNode, crtc: crtc::Handle) {
        let retry = Timer::from_duration(IDLE_RETRY);
        let _ = self.loop_handle.insert_source(retry, move |_, (), state| {
            state.render_crtc(node, crtc);
            TimeoutAction::Drop
        });
    }

    /// The VT went away or came back.
    fn session_event(&mut self, event: &SessionEvent) {
        let Some(tty) = self.tty.as_mut() else {
            return;
        };
        match event {
            SessionEvent::PauseSession => {
                tty.libinput.suspend();
                for device in &mut tty.devices {
                    device.manager.pause();
                }
            }
            SessionEvent::ActivateSession => {
                if tty.libinput.resume().is_err() {
                    eprintln!("tetsu-comp: cannot resume libinput");
                }
                for device in &mut tty.devices {
                    if let Err(e) = device.manager.lock().activate(false) {
                        eprintln!("tetsu-comp: cannot reactivate {}: {e}", device.node);
                    }
                }
                self.loop_handle.insert_idle(|state| state.render_all());
                // The cabling may have changed while the VT was away.
                self.rescan_all();
            }
        }
    }
}
