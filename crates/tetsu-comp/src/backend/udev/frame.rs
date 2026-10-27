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
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{Kind, RenderElementStates};
use smithay::backend::renderer::{ImportAll, ImportMem};
use smithay::desktop::space::{SpaceRenderElements, space_render_elements};
use smithay::desktop::{Space, Window};
use smithay::reexports::drm::control::crtc;
use smithay::utils::{Logical, Point, Rectangle, Scale};

use super::{Tty, TtyRenderer};
use crate::lock::glue::View;
use crate::render::{CLEAR_COLOR, LOCK_COLOR};

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
    /// What a frame is made of: the pointer on top, then either the windows
    /// or the lock surface and its solid backdrop.
    pub TtyElements<R, E> where R: ImportAll + ImportMem;
    Solid=SolidColorRenderElement,
    Space=SpaceRenderElements<R, E>,
    Surface=WaylandSurfaceRenderElement<R>,
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
        view: &View,
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
            elements.push(TtyElements::Solid(SolidColorRenderElement::new(
                self.cursor_id.clone(),
                rect,
                smithay::backend::renderer::utils::CommitCounter::default(),
                CURSOR_COLOR,
                Kind::Cursor,
            )));
        }
        let locked = matches!(view, View::Locked(_));
        let clear = match view {
            View::Normal => {
                let windows = space_render_elements::<_, Window, _>(
                    &mut renderer,
                    [space],
                    &head.output,
                    1.0,
                )?;
                elements.extend(windows.into_iter().map(TtyElements::Space));
                CLEAR_COLOR
            }
            // Locked: the lock surface and an opaque backdrop; no space, so
            // no window, popup or layer can show.
            View::Locked(surface) => {
                if let Some(surface) = surface {
                    elements.extend(render_elements_from_surface_tree::<_, Elements<'_>>(
                        &mut renderer,
                        surface,
                        (0, 0),
                        Scale::from(1.0),
                        1.0,
                        Kind::Unspecified,
                    ));
                }
                elements.push(TtyElements::Solid(SolidColorRenderElement::new(
                    self.lock_id.clone(),
                    Rectangle::new((0, 0).into(), geometry.size.to_physical(1)),
                    smithay::backend::renderer::utils::CommitCounter::default(),
                    LOCK_COLOR,
                    Kind::Unspecified,
                )));
                LOCK_COLOR
            }
        };

        let frame = head
            .drm_output
            .render_frame(&mut renderer, &elements, clear, FRAME_FLAGS)?;
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
            // The flag rides with the frame: a replaced queued frame takes
            // its flag with it.
            head.drm_output.queue_frame(locked)?;
            Ok(Some(states))
        } else {
            Ok(None)
        }
    }
}
