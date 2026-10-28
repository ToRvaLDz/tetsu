//! Opening the cards: DRM, GBM and an EGL probe per card, the render device
//! choice (`gpu::select`) and explicit sync (`gpu::sync`).
//!
//! The probe (render node, software check) and the driver and NVIDIA version
//! reads are ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`.

use std::error::Error;
use std::path::Path;

use smithay::backend::allocator::gbm::GbmDevice;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmNode, NodeType};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay, Error as EglError};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::multigpu::gbm::{Error as FactoryError, GbmGlesBackend};
use smithay::backend::session::Session;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::udev::{all_gpus, primary_gpu};
use smithay::reexports::drm::Device as BasicDevice;
use smithay::reexports::drm::control::{Device as ControlDevice, connector};
use smithay::reexports::rustix::fs::OFlags;
use smithay::utils::DeviceFd;
use smithay::wayland::drm_syncobj::{DrmSyncobjState, supports_syncobj_eventfd};

use super::Gpus;
use crate::gpu::select::{Candidate, Selection, select};
use crate::gpu::sync::{Support, decide, parse_driver_version};
use crate::settings::{DeviceChoice, RenderSettings};
use crate::state::State;

const NVIDIA_VERSION_FILE: &str = "/sys/module/nvidia_drm/version";

/// A card that opened.
pub(super) struct Probed {
    pub node: DrmNode,
    pub drm: DrmDevice,
    pub events: DrmDeviceNotifier,
    pub gbm: GbmDevice<DrmDeviceFd>,
    pub fd: DrmDeviceFd,
    /// The render node EGL reports; `None` when EGL did not come up.
    pub render_node: Option<DrmNode>,
    pub software: bool,
    pub primary: bool,
    pub driver: String,
    pub connected: Vec<String>,
}

/// A card ready to get its outputs.
pub(super) struct Opened {
    pub node: DrmNode,
    /// `None`: display-only, rendered for by the render device.
    pub render_node: Option<DrmNode>,
    pub drm: DrmDevice,
    pub events: DrmDeviceNotifier,
    pub gbm: GbmDevice<DrmDeviceFd>,
}

/// The chosen render device.
pub(super) struct Selected {
    pub node: DrmNode,
    pub fd: DrmDeviceFd,
    pub driver: String,
}

/// `GlesRenderer::new` is unsafe: the renderer must be the only user of the
/// context it is given.
#[expect(unsafe_code)]
fn gles_factory(display: &EGLDisplay) -> Result<GlesRenderer, FactoryError> {
    let context = EGLContext::new_with_priority(display, ContextPriority::High)?;
    // SAFETY: the context was just made for this renderer and nothing else
    // uses it.
    unsafe { Ok(GlesRenderer::new(context)?) }
}

pub(super) fn new_gpu_manager() -> Result<Gpus, Box<dyn Error>> {
    Ok(Gpus::new(GbmGlesBackend::with_factory(gles_factory))?)
}

/// `EGLDisplay::new` is unsafe: the GBM device must outlive the display.
/// Returns the render node EGL reports and whether it is a software device.
#[expect(unsafe_code)]
fn egl_probe(gbm: &GbmDevice<DrmDeviceFd>) -> Result<(Option<DrmNode>, bool), EglError> {
    // SAFETY: the display is made from a GBM device we own and is dropped
    // before this function returns.
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    let device = EGLDevice::device_for_display(&display)?;
    Ok((
        device.try_get_render_node().ok().flatten(),
        device.is_software(),
    ))
}

/// The connected connectors, by name, before any output is lit.
fn connected_names(drm: &DrmDevice) -> Vec<String> {
    let Ok(resources) = drm.resource_handles() else {
        return Vec::new();
    };
    resources
        .connectors()
        .iter()
        .filter_map(|handle| drm.get_connector(*handle, true).ok())
        .filter(|info| info.state() == connector::State::Connected && !info.modes().is_empty())
        .map(|info| connector_name(&info))
        .collect()
}

pub(super) fn connector_name(info: &connector::Info) -> String {
    format!("{}-{}", info.interface().as_str(), info.interface_id())
}

fn probe(
    session: &mut LibSeatSession,
    path: &Path,
    primary: bool,
) -> Result<Probed, Box<dyn Error>> {
    let node = DrmNode::from_path(path)?;
    let fd = session
        .open(
            path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, events) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd.clone())?;
    let (render_node, software) = match egl_probe(&gbm) {
        Ok((render_node, software)) => (Some(render_node.unwrap_or(node)), software),
        Err(e) => {
            eprintln!("tetsu-comp: {}: no EGL, display only: {e}", path.display());
            (None, false)
        }
    };
    let driver = drm
        .get_driver()
        .map(|d| d.name().to_string_lossy().into_owned())
        .unwrap_or_default();
    let connected = connected_names(&drm);
    Ok(Probed {
        node,
        drm,
        events,
        gbm,
        fd,
        render_node,
        software,
        primary,
        driver,
        connected,
    })
}

/// Opens every card on the seat; one that fails is skipped with a message.
pub(super) fn probe_all(session: &mut LibSeatSession, seat: &str) -> Vec<Probed> {
    let primary = primary_gpu(seat)
        .ok()
        .flatten()
        .and_then(|p| DrmNode::from_path(p).ok());
    let paths = all_gpus(seat).unwrap_or_default();
    paths
        .iter()
        .filter_map(|path| {
            let is_primary = DrmNode::from_path(path).ok() == primary;
            match probe(session, path, is_primary) {
                Ok(probed) => Some(probed),
                Err(e) => {
                    eprintln!("tetsu-comp: skipping {}: {e}", path.display());
                    None
                }
            }
        })
        .collect()
}

/// `render.device` as a candidate id: the render node of the path.
fn override_id(choice: &DeviceChoice) -> Option<Option<u64>> {
    let DeviceChoice::Node(path) = choice else {
        return None;
    };
    let node = DrmNode::from_path(path).ok();
    let render = node.map(|n| match n.node_with_type(NodeType::Render) {
        Some(Ok(render)) => render,
        _ => n,
    });
    Some(render.map(|n| n.dev_id()))
}

fn candidate(p: &Probed) -> Option<Candidate> {
    Some(Candidate {
        id: p.render_node?.dev_id(),
        primary: p.primary,
        software: p.software,
        outputs: p.connected.clone(),
    })
}

/// Chooses the render device, registers every device that can render with
/// the GPU manager and returns the cards, the render device first.
pub(super) fn open_devices(
    gpus: &mut Gpus,
    probed: Vec<Probed>,
    settings: &RenderSettings,
    seat: &str,
) -> Result<(Selected, Vec<Opened>), Box<dyn Error>> {
    let candidates: Vec<Candidate> = probed.iter().filter_map(candidate).collect();
    let (id, automatic, reason) = match select(&candidates, override_id(&settings.device)) {
        Selection::Device {
            id,
            automatic,
            reason,
        } => (id, automatic, reason),
        Selection::NoGpu => return Err(format!("no GPU on seat {seat}").into()),
    };
    let render_node = DrmNode::from_dev_id(id)?;
    let hardware_exists = probed
        .iter()
        .any(|p| p.render_node.is_some() && !p.software);

    let mut selected = None;
    let mut opened = Vec::new();
    for p in probed {
        let is_render = p.render_node.map(|n| n.dev_id()) == Some(id);
        // A software device next to real hardware only displays.
        let renders = p.render_node.is_some() && (is_render || !(p.software && hardware_exists));
        let render_node_of = if renders { p.render_node } else { None };
        if let Some(node) = render_node_of {
            gpus.as_mut()
                .add_node(node, p.gbm.clone())
                .map_err(|e| format!("cannot add {} to the GPU manager: {e}", p.node))?;
        }
        if is_render {
            println!(
                "tetsu-comp: render device {} ({}), {}: {reason}",
                render_node,
                p.driver,
                if automatic { "automatic" } else { "set" }
            );
            selected = Some(Selected {
                node: render_node,
                fd: p.fd.clone(),
                driver: p.driver.clone(),
            });
        }
        let entry = Opened {
            node: p.node,
            render_node: render_node_of,
            drm: p.drm,
            events: p.events,
            gbm: p.gbm,
        };
        if is_render {
            opened.insert(0, entry);
        } else {
            opened.push(entry);
        }
    }
    let selected = selected.ok_or("the render device vanished")?;
    Ok((selected, opened))
}

/// Decides explicit sync on the render device and, if on, advertises the
/// syncobj global.
pub(super) fn init_explicit_sync(state: &mut State, render: &Selected) {
    let version = std::fs::read_to_string(NVIDIA_VERSION_FILE)
        .ok()
        .and_then(|text| parse_driver_version(&text));
    let support = Support {
        eventfd: supports_syncobj_eventfd(&render.fd),
        driver: render.driver.clone(),
        version,
    };
    let decision = decide(state.render_settings.explicit_sync, support.clone());
    if decision.enabled {
        state.syncobj_state = Some(DrmSyncobjState::new::<State>(
            &state.display_handle,
            render.fd.clone(),
        ));
        println!("tetsu-comp: explicit sync enabled");
    } else {
        let reason = decision.reason.as_deref().unwrap_or("unknown");
        if decision.mode == crate::settings::SyncMode::On {
            eprintln!("tetsu-comp: render.explicit_sync = \"on\" cannot be honoured: {reason}");
        }
        println!("tetsu-comp: explicit sync disabled ({reason})");
    }
    // The global is advertised once: a new mode applies at the next start.
    state.render.support = support;
    state.render.sync_live = false;
    state.render.sync_running = decision.enabled;
}
