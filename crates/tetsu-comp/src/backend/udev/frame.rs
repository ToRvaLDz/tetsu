//! One frame of one output on the render device, copied across GPUs when the
//! output is on another card.
//!
//! The frame flags and the wait on a frame the driver cannot fence are
//! ported from niri `src/backend/tty.rs` at commit
//! `fcbc971cf49a987b1e8d5ff1d64da6b1f61a1787`.

use std::error::Error;

use smithay::backend::drm::DrmNode;
use smithay::backend::drm::compositor::{FrameFlags, PrimaryPlaneElement};
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Kind, RenderElementStates};
use smithay::backend::renderer::{ImportAll, ImportMem};
use smithay::desktop::space::{SpaceRenderElements, space_render_elements};
use smithay::desktop::{Space, Window};
use smithay::reexports::drm::control::crtc;
use smithay::utils::{Logical, Point, Rectangle, Scale};

use super::{Tty, TtyRenderer};
use crate::render::CLEAR_COLOR;

/// Side of the square drawn as the pointer, in pixels.
const CURSOR_SIZE: i32 = 12;
/// Colour of that square.
const CURSOR_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];
/// No overlay planes: they break on NVIDIA and across GPUs.
const FRAME_FLAGS: FrameFlags = FrameFlags::from_bits_truncate(
    FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY.bits()
        | FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT.bits(),
);

smithay::backend::renderer::element::render_elements! {
    /// What a frame is made of: the windows, then the pointer on top.
    pub TtyElements<R, E> where R: ImportAll + ImportMem;
    Cursor=SolidColorRenderElement,
    Space=SpaceRenderElements<R, E>,
}

/// The frame's elements on the (multi-GPU) renderer.
pub(super) type Elements<'a> =
    TtyElements<TtyRenderer<'a>, WaylandSurfaceRenderElement<TtyRenderer<'a>>>;

impl Tty {
    /// Draws one frame of `crtc` on `node`; the states when a frame was
    /// queued, `None` when there was nothing to draw.
    pub(super) fn render(
        &mut self,
        space: &Space<Window>,
        pointer: Point<f64, Logical>,
        node: DrmNode,
        crtc: crtc::Handle,
    ) -> Result<Option<RenderElementStates>, Box<dyn Error>> {
        let Some(head) = self
            .heads
            .iter_mut()
            .find(|h| h.node == node && h.crtc == crtc)
        else {
            return Ok(None);
        };
        let Some(geometry) = space.output_geometry(&head.output) else {
            return Ok(None);
        };
        let mut renderer = if head.target == self.render_node {
            self.gpus.single_renderer(&self.render_node)
        } else {
            self.gpus
                .renderer(&self.render_node, &head.target, head.drm_output.format())
        }
        .map_err(|e| format!("no renderer: {e}"))?;

        let mut elements: Vec<Elements<'_>> = Vec::new();
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
            space_render_elements::<_, Window, _>(&mut renderer, [space], &head.output, 1.0)?;
        elements.extend(windows.into_iter().map(TtyElements::Space));

        let frame =
            head.drm_output
                .render_frame(&mut renderer, &elements, CLEAR_COLOR, FRAME_FLAGS)?;
        let rendered = !frame.is_empty;
        if (frame.needs_sync() || self.wait_for_frame_completion)
            && let PrimaryPlaneElement::Swapchain(element) = &frame.primary_element
            && let Err(e) = element.sync.wait()
        {
            eprintln!("tetsu-comp: waiting for the frame failed: {e:?}");
        }
        let states = frame.states.clone();
        drop(frame);
        if rendered {
            head.drm_output.queue_frame(())?;
            Ok(Some(states))
        } else {
            Ok(None)
        }
    }
}
