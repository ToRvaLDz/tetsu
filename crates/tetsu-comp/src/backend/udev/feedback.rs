//! `linux-dmabuf`: the global with default feedback from the render node, a
//! per-output render and scanout feedback, and the import path.
//!
//! The per-output feedback is ported from niri `src/backend/tty.rs`
//! (`surface_dmabuf_feedback`) at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`; the tranche rules are
//! `gpu::formats::scanout_tranches`.

use std::error::Error;

use smithay::backend::allocator::Format;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::drm::DrmNode;
use smithay::backend::renderer::element::RenderElementStates;
use smithay::backend::renderer::element::utils::select_dmabuf_feedback;
use smithay::backend::renderer::{ImportDma, ImportMemWl};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::dmabuf::{DmabufFeedback, DmabufFeedbackBuilder};

use super::{Lit, Tty};
use crate::gpu::assign::OutputPlan;
use crate::gpu::formats::scanout_tranches;
use crate::state::State;

/// The two feedbacks of an output: what to send a surface that is composited
/// and what to send one that can be scanned out directly.
#[derive(Clone)]
pub(super) struct SurfaceDmabufFeedback {
    render: DmabufFeedback,
    scanout: DmabufFeedback,
}

/// Creates the `linux-dmabuf` global from the render device and tells
/// `wl_shm` what that device can import.
pub(super) fn init_dmabuf(state: &mut State, tty: &mut Tty) -> Result<(), Box<dyn Error>> {
    let renderer = tty
        .gpus
        .single_renderer(&tty.render_node)
        .map_err(|e| format!("no renderer on the render device: {e}"))?;
    let formats = renderer.dmabuf_formats();
    state.shm_state.update_formats(renderer.shm_formats());
    drop(renderer);
    let default_feedback = DmabufFeedbackBuilder::new(tty.render_node.dev_id(), formats).build()?;
    let global = state
        .dmabuf_state
        .create_global_with_default_feedback::<State>(&state.display_handle, &default_feedback);
    tty.dmabuf_global = Some(global);
    Ok(())
}

/// The feedback of one lit output.
pub(super) fn for_head(
    tty: &mut Tty,
    plan: &OutputPlan,
    scanout_node: DrmNode,
    drm_output: &Lit,
) -> Option<SurfaceDmabufFeedback> {
    let render_formats = tty
        .gpus
        .single_renderer(&tty.render_node)
        .ok()?
        .dmabuf_formats();
    let (primary, either) = drm_output.with_compositor(|compositor| {
        let surface = compositor.surface();
        let primary_plane: Vec<Format> = surface.plane_info().formats.iter().copied().collect();
        let overlay: Vec<Format> = surface
            .planes()
            .overlay
            .iter()
            .flat_map(|p| p.formats.iter().copied())
            .collect();
        let render: Vec<Format> = render_formats.iter().copied().collect();
        scanout_tranches(&primary_plane, &overlay, &render, !plan.cross_gpu)
    });
    let builder = DmabufFeedbackBuilder::new(tty.render_node.dev_id(), render_formats);
    let scanout = builder
        .clone()
        .add_preference_tranche(scanout_node.dev_id(), TrancheFlags::Scanout, primary, 4..=6)
        .add_preference_tranche(scanout_node.dev_id(), TrancheFlags::Scanout, either, 4..=6)
        .build()
        .ok()?;
    // On the render device's own outputs both tranches carry the scanout
    // formats, to avoid sending them twice.
    let render = if plan.cross_gpu {
        builder.build().ok()?
    } else {
        scanout.clone()
    };
    Some(SurfaceDmabufFeedback { render, scanout })
}

impl Tty {
    /// Whether the render device can import `dmabuf`.
    pub(crate) fn import_dmabuf(&mut self, dmabuf: &Dmabuf) -> bool {
        let imported = self
            .gpus
            .single_renderer(&self.render_node)
            .and_then(|mut renderer| renderer.import_dmabuf(dmabuf, None))
            .is_ok();
        if imported && dmabuf.node().is_none() {
            dmabuf.set_node(self.render_node);
        }
        imported
    }

    /// Imports a committed buffer on the render device ahead of the frame.
    pub(crate) fn early_import(&mut self, surface: &WlSurface) {
        if let Err(e) = self.gpus.early_import(self.render_node, surface) {
            eprintln!("tetsu-comp: early import failed: {e}");
        }
    }
}

impl State {
    /// Tells each window which dmabuf feedback applies after a frame.
    pub(super) fn send_dmabuf_feedback(
        &self,
        output: &Output,
        feedback: Option<&SurfaceDmabufFeedback>,
        states: &RenderElementStates,
    ) {
        let Some(feedback) = feedback else {
            return;
        };
        for window in self.space.elements() {
            window.send_dmabuf_feedback(
                output,
                |_, _| Some(output.clone()),
                |surface, _| {
                    select_dmabuf_feedback(surface, states, &feedback.render, &feedback.scanout)
                },
            );
        }
    }
}
