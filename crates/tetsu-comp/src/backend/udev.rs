//! The tty backend: libseat session, one GPU through DRM/GBM/GLES, libinput.
//! Anvil-style and minimal: no hotplug, no multi-GPU, no explicit sync, no
//! dmabuf (clients use wl_shm). The multi-GPU work ports a fuller backend
//! over this one.

use std::error::Error;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::exporter::gbm::{GbmFramebufferExporter, NodeFilter};
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::{ImportAll, ImportMem, ImportMemWl};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{all_gpus, primary_gpu};
use smithay::desktop::space::{SpaceRenderElements, space_render_elements};
use smithay::desktop::{Space, Window};
use smithay::output::Mode;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc};
use smithay::reexports::input::Libinput;
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::{DeviceFd, Logical, Point, Rectangle, Scale};

use crate::render::CLEAR_COLOR;
use crate::state::State;

/// Pixel formats tried for the scanout swapchain, best first.
const COLOR_FORMATS: [Fourcc; 4] = [
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];
/// Side of the square drawn as the pointer, in pixels.
const CURSOR_SIZE: i32 = 12;
/// Colour of that square.
const CURSOR_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
/// Retry delay when a frame had nothing to draw.
const IDLE_RETRY: Duration = Duration::from_millis(16);

smithay::backend::renderer::element::render_elements! {
    /// What a frame is made of: the windows, then the pointer on top.
    pub TtyElements<R, E> where R: ImportAll + ImportMem;
    Cursor=SolidColorRenderElement,
    Space=SpaceRenderElements<R, E>,
}

/// The frame's elements on the GLES renderer.
type Elements = TtyElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>;

type Output0 =
    DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

type Manager = DrmOutputManager<
    GbmAllocator<DrmDeviceFd>,
    GbmFramebufferExporter<DrmDeviceFd>,
    (),
    DrmDeviceFd,
>;

/// A lit connector.
struct Head {
    crtc: crtc::Handle,
    output: smithay::output::Output,
    drm_output: Output0,
}

/// The tty backend's state.
pub struct Tty {
    renderer: GlesRenderer,
    manager: Manager,
    heads: Vec<Head>,
    cursor_id: smithay::backend::renderer::element::Id,
    libinput: Libinput,
}

impl Tty {
    /// Draws one frame of `crtc`; returns whether anything was queued.
    fn render(
        &mut self,
        space: &Space<Window>,
        pointer: Point<f64, Logical>,
        crtc: crtc::Handle,
    ) -> Result<bool, Box<dyn Error>> {
        let Some(head) = self.heads.iter_mut().find(|h| h.crtc == crtc) else {
            return Ok(false);
        };
        let Some(geometry) = space.output_geometry(&head.output) else {
            return Ok(false);
        };
        let mut elements: Vec<Elements> = Vec::new();
        if geometry.to_f64().contains(pointer) {
            let at = (pointer - geometry.loc.to_f64())
                .to_physical(Scale::from(1.0))
                .to_i32_round();
            let rect = Rectangle::new(at, (CURSOR_SIZE, CURSOR_SIZE).into());
            elements.push(TtyElements::Cursor(SolidColorRenderElement::new(
                self.cursor_id.clone(),
                rect,
                smithay::backend::renderer::utils::CommitCounter::default(),
                CURSOR_COLOR,
                Kind::Cursor,
            )));
        }
        let windows =
            space_render_elements::<_, Window, _>(&mut self.renderer, [space], &head.output, 1.0)?;
        elements.extend(windows.into_iter().map(TtyElements::Space));
        let frame = head.drm_output.render_frame(
            &mut self.renderer,
            &elements,
            CLEAR_COLOR,
            FrameFlags::DEFAULT,
        )?;
        let rendered = !frame.is_empty;
        drop(frame);
        if rendered {
            head.drm_output.queue_frame(())?;
        }
        Ok(rendered)
    }
}

/// Opens the session and the GPU, lights every connected connector and
/// starts reading input.
pub fn init(state: &mut State) -> Result<(), Box<dyn Error>> {
    let (mut session, notifier) = LibSeatSession::new().map_err(|e| format!("libseat: {e}"))?;
    let seat = session.seat();
    let path = match primary_gpu(&seat)? {
        Some(path) => path,
        None => all_gpus(&seat)?
            .into_iter()
            .next()
            .ok_or("no GPU on this seat")?,
    };
    let fd = session
        .open(
            &path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, drm_events) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;
    let renderer = create_renderer(&gbm)?;
    state.shm_state.update_formats(renderer.shm_formats());

    let render_formats: FormatSet = renderer
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect();
    let manager: Manager = DrmOutputManager::new(
        drm,
        GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        ),
        GbmFramebufferExporter::new(gbm.clone(), NodeFilter::All),
        Some(gbm),
        COLOR_FORMATS,
        render_formats,
    );

    let mut libinput =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput
        .udev_assign_seat(&seat)
        .map_err(|()| format!("libinput cannot use seat {seat}"))?;

    let mut tty = Tty {
        renderer,
        manager,
        heads: Vec::new(),
        cursor_id: smithay::backend::renderer::element::Id::new(),
        libinput: libinput.clone(),
    };
    light_connectors(state, &mut tty)?;
    if tty.heads.is_empty() {
        return Err("no connected output".into());
    }
    state.tty = Some(tty);

    state
        .loop_handle
        .insert_source(drm_events, |event, _, state| match event {
            DrmEvent::VBlank(crtc) => state.frame_done(crtc),
            DrmEvent::Error(e) => eprintln!("tetsu-comp: drm: {e}"),
        })
        .map_err(|e| e.to_string())?;
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

    let mut switcher = session;
    state.vt_switch = Some(Box::new(move |vt| {
        if let Err(e) = switcher.change_vt(vt) {
            eprintln!("tetsu-comp: cannot switch to VT {vt}: {e}");
        }
    }));
    state.loop_handle.insert_idle(|state| state.render_all());
    Ok(())
}

/// `EGLDisplay::new` and `GlesRenderer::new` are unsafe: the renderer must
/// be the only user of the context it is given.
#[expect(unsafe_code)]
fn create_renderer(gbm: &GbmDevice<DrmDeviceFd>) -> Result<GlesRenderer, Box<dyn Error>> {
    // SAFETY: the display is made from a GBM device we own and keep alive
    // for as long as the renderer, and nothing else uses this context.
    unsafe {
        let display = EGLDisplay::new(gbm.clone())?;
        let context = EGLContext::new_with_priority(&display, ContextPriority::High)?;
        Ok(GlesRenderer::new(context)?)
    }
}

/// Lights each connected connector on a free CRTC, at its preferred mode.
fn light_connectors(state: &mut State, tty: &mut Tty) -> Result<(), Box<dyn Error>> {
    let resources = tty.manager.device().resource_handles()?;
    let mut used: Vec<crtc::Handle> = Vec::new();
    for handle in resources.connectors() {
        let info = tty.manager.device().get_connector(*handle, true)?;
        if info.state() != connector::State::Connected || info.modes().is_empty() {
            continue;
        }
        let Some(crtc) = free_crtc(tty.manager.device(), &resources, &info, &used) else {
            eprintln!("tetsu-comp: no free CRTC for {info}");
            continue;
        };
        let drm_mode = *info
            .modes()
            .iter()
            .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
            .unwrap_or(&info.modes()[0]);
        let mode = Mode::from(drm_mode);
        let name = format!("{}-{}", info.interface().as_str(), info.interface_id());
        let output = state.add_output(&name, mode.size, mode.refresh);
        let drm_output = tty
            .manager
            .lock()
            .initialize_output::<_, Elements>(
                crtc,
                drm_mode,
                &[*handle],
                &output,
                None,
                &mut tty.renderer,
                &DrmOutputRenderElements::default(),
            )
            .map_err(|e| format!("cannot light {name}: {e}"))?;
        used.push(crtc);
        tty.heads.push(Head {
            crtc,
            output,
            drm_output,
        });
    }
    Ok(())
}

/// The CRTC a connector already drives, else any free one it can use.
fn free_crtc(
    device: &DrmDevice,
    resources: &smithay::reexports::drm::control::ResourceHandles,
    info: &connector::Info,
    used: &[crtc::Handle],
) -> Option<crtc::Handle> {
    let encoders: Vec<_> = info
        .encoders()
        .iter()
        .filter_map(|e| device.get_encoder(*e).ok())
        .collect();
    let current = encoders
        .iter()
        .filter_map(|e| e.crtc())
        .find(|c| !used.contains(c));
    current.or_else(|| {
        encoders
            .iter()
            .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
            .find(|c| !used.contains(c))
    })
}

impl State {
    /// A frame reached the screen: draw the next one.
    fn frame_done(&mut self, crtc: crtc::Handle) {
        if let Some(tty) = self.tty.as_mut()
            && let Some(head) = tty.heads.iter().find(|h| h.crtc == crtc)
            && let Err(e) = head.drm_output.frame_submitted()
        {
            eprintln!("tetsu-comp: frame_submitted: {e}");
        }
        self.render_crtc(crtc);
    }

    fn render_all(&mut self) {
        let crtcs: Vec<_> = self
            .tty
            .as_ref()
            .map(|t| t.heads.iter().map(|h| h.crtc).collect())
            .unwrap_or_default();
        for crtc in crtcs {
            self.render_crtc(crtc);
        }
    }

    /// Draws a frame; with nothing to draw, tries again next frame time.
    fn render_crtc(&mut self, crtc: crtc::Handle) {
        let Some(mut tty) = self.tty.take() else {
            return;
        };
        let pointer = self
            .seat
            .get_pointer()
            .map_or_else(Point::default, |p| p.current_location());
        let rendered = tty.render(&self.space, pointer, crtc);
        let output = tty
            .heads
            .iter()
            .find(|h| h.crtc == crtc)
            .map(|h| h.output.clone());
        self.tty = Some(tty);
        match rendered {
            Ok(true) => {
                if let Some(output) = output {
                    self.send_frames(&output);
                }
            }
            Ok(false) => {
                let retry = Timer::from_duration(IDLE_RETRY);
                let _ = self.loop_handle.insert_source(retry, move |_, (), state| {
                    state.render_crtc(crtc);
                    TimeoutAction::Drop
                });
            }
            Err(e) => eprintln!("tetsu-comp: render failed: {e}"),
        }
    }

    /// The VT went away or came back.
    fn session_event(&mut self, event: &SessionEvent) {
        let Some(tty) = self.tty.as_mut() else {
            return;
        };
        match event {
            SessionEvent::PauseSession => {
                tty.libinput.suspend();
                tty.manager.pause();
            }
            SessionEvent::ActivateSession => {
                if tty.libinput.resume().is_err() {
                    eprintln!("tetsu-comp: cannot resume libinput");
                }
                if let Err(e) = tty.manager.lock().activate(false) {
                    eprintln!("tetsu-comp: cannot reactivate the GPU: {e}");
                }
                self.loop_handle.insert_idle(|state| state.render_all());
            }
        }
    }
}
