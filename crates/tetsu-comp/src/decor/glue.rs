//! The Smithay side of the decorations: the protocol states.

use std::collections::HashSet;

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;

/// The two decoration protocols and what the KDE one has answered.
pub struct DecorStates {
    /// `zxdg_decoration_manager_v1`; held to keep the global alive.
    pub xdg: XdgDecorationState,
    /// `org_kde_kwin_server_decoration_manager`, default mode Server.
    pub kde: KdeDecorationState,
    /// The KDE decoration objects that already got their one answer.
    pub answered: HashSet<ObjectId>,
}

use std::collections::HashMap;

use smithay::backend::renderer::element::solid::SolidColorBuffer;
use smithay::output::Output;

use super::{Color, Frame, WindowBox, frames};
use crate::layout::Rect;
use crate::lock::Scene;
use crate::state::State;

/// The four solid buffers of each window, kept between frames so the damage
/// ids stay stable and only a change of size or colour damages.
#[derive(Default)]
pub struct BorderBuffers {
    buffers: HashMap<String, [SolidColorBuffer; 4]>,
}

/// A straight colour as the premultiplied floats the renderer takes.
fn premultiplied(color: Color) -> [f32; 4] {
    let [r, g, b, a] = color.to_f32();
    [r * a, g * a, b * a, a]
}

impl BorderBuffers {
    /// Brings the buffers up to date with this frame's `frames`.
    pub fn sync(&mut self, frames: &[Frame]) {
        for frame in frames {
            let color = premultiplied(frame.color);
            let buffers = self
                .buffers
                .entry(frame.id.clone())
                .or_insert_with(|| std::array::from_fn(|_| SolidColorBuffer::new((0, 0), color)));
            for (buffer, rect) in buffers.iter_mut().zip(&frame.rects) {
                buffer.update((rect.w, rect.h), color);
            }
        }
    }

    /// The buffers of a window, if it has had a frame.
    pub fn of(&self, id: &str) -> Option<&[SolidColorBuffer; 4]> {
        self.buffers.get(id)
    }

    /// Drops the buffers of a window that is gone.
    pub fn forget(&mut self, id: &str) {
        self.buffers.remove(id);
    }
}

impl State {
    /// The frames of the windows that show on `output`, top first. Only the
    /// normal scene asks for them: `View::Locked` has no field for frames.
    pub(crate) fn frames_on(&self, output: &Output) -> Vec<Frame> {
        let Some(geometry) = self.space.output_geometry(output) else {
            return Vec::new();
        };
        let windows: Vec<WindowBox> = self
            .space
            .elements()
            .rev()
            .filter_map(|window| {
                let id = self.mapped.iter().find(|m| &m.window == window)?.id.clone();
                let bbox = self.space.element_bbox(window)?;
                if !geometry.overlaps(bbox) {
                    return None;
                }
                let content = self.space.element_geometry(window)?;
                Some(WindowBox {
                    id,
                    content: Rect {
                        x: content.loc.x - geometry.loc.x,
                        y: content.loc.y - geometry.loc.y,
                        w: content.size.w,
                        h: content.size.h,
                    },
                })
            })
            .collect();
        frames(
            &Scene::Normal,
            &windows,
            self.engine.model().focused_window(),
            &self.engine.settings().decorations,
        )
    }
}
