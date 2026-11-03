//! What is drawn: the normal scene (layers, windows and their borders) over
//! a clear colour, or the locked scene.

use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, Kind};
use smithay::backend::renderer::{ImportAll, ImportMem, Renderer, Texture};
use smithay::desktop::{LayerSurface, Space, Window, layer_map_for_output};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Scale};
use smithay::wayland::shell::wlr_layer::Layer;

use crate::decor::Frame;
use crate::decor::glue::BorderBuffers;
use crate::lock::{LockMachine, Scene};
use crate::state::Mapped;

/// The background behind the windows.
pub const CLEAR_COLOR: [f32; 4] = [0.08, 0.08, 0.10, 1.0];
/// The opaque colour of a locked output.
pub const LOCK_COLOR: [f32; 4] = [0.02, 0.02, 0.02, 1.0];

smithay::backend::renderer::element::render_elements! {
    /// What a frame is made of: surfaces and solid colours (borders, the
    /// lock backdrop, the pointer).
    pub SceneElements<R> where R: ImportAll + ImportMem;
    Surface=WaylandSurfaceRenderElement<R>,
    Solid=SolidColorRenderElement,
}

/// What `output` may show: the one place that decides it.
pub fn scene(lock: &LockMachine, output: &str) -> Scene {
    lock.scene(output)
}

fn layer_elements<R, E>(renderer: &mut R, output: &Output, scale: f64, lower: bool) -> Vec<E>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + Texture + 'static,
    E: From<WaylandSurfaceRenderElement<R>>,
{
    let map = layer_map_for_output(output);
    let wanted: Vec<&LayerSurface> = map
        .layers()
        .rev()
        .filter(|s| matches!(s.layer(), Layer::Background | Layer::Bottom) == lower)
        .collect();
    wanted
        .into_iter()
        .filter_map(|surface| map.layer_geometry(surface).map(|geo| (geo.loc, surface)))
        .flat_map(|(loc, surface)| {
            AsRenderElements::<R>::render_elements::<E>(
                surface,
                renderer,
                loc.to_physical_precise_round(scale),
                Scale::from(scale),
                1.0,
            )
        })
        .collect()
}

/// The one builder of the normal scene, shared by winit and tty. In order,
/// top to bottom: the `Overlay` and `Top` layers; each window with its
/// popups, followed by the four edges of its border; the `Bottom` and
/// `Background` layers. It follows Smithay's `space_render_elements`.
pub fn normal_elements<R, E>(
    renderer: &mut R,
    space: &Space<Window>,
    mapped: &[Mapped],
    output: &Output,
    frames: &[Frame],
    borders: &BorderBuffers,
) -> Vec<E>
where
    R: Renderer + ImportAll,
    R::TextureId: Clone + Texture + 'static,
    E: From<WaylandSurfaceRenderElement<R>> + From<SolidColorRenderElement>,
{
    let scale = output.current_scale().fractional_scale();
    let mut elements = layer_elements::<R, E>(renderer, output, scale, false);
    if let Some(output_geo) = space.output_geometry(output) {
        for window in space.elements().rev() {
            let Some(bbox) = space.element_bbox(window) else {
                continue;
            };
            if !output_geo.overlaps(bbox) {
                continue;
            }
            let Some(at) = space.element_location(window) else {
                continue;
            };
            let location: Point<i32, Logical> = at - window.geometry().loc - output_geo.loc;
            elements.extend(AsRenderElements::<R>::render_elements::<E>(
                window,
                renderer,
                location.to_physical_precise_round(scale),
                Scale::from(scale),
                1.0,
            ));
            let id = mapped.iter().find(|m| &m.window == window).map(|m| &m.id);
            let frame = id.and_then(|id| frames.iter().find(|f| &f.id == id));
            let buffers = frame.and_then(|f| borders.of(&f.id));
            if let (Some(frame), Some(buffers)) = (frame, buffers) {
                for (rect, buffer) in frame.rects.iter().zip(buffers) {
                    let corner: Point<i32, Logical> = (rect.x, rect.y).into();
                    elements.push(E::from(SolidColorRenderElement::from_buffer(
                        buffer,
                        corner.to_physical_precise_round(scale),
                        Scale::from(scale),
                        1.0,
                        Kind::Unspecified,
                    )));
                }
            }
        }
    }
    elements.extend(layer_elements::<R, E>(renderer, output, scale, true));
    elements
}
