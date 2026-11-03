//! Software drawing of the bar into a tiny-skia pixmap, text through
//! cosmic-text. Everything is laid out in logical pixels and drawn at the
//! surface's integer scale.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Wrap};
use tiny_skia::{Paint, Pixmap, Rect as SkRect, Transform};

use super::layout::{self, CELL_PADDING, Measure, Placed};
use crate::model::Bar;

/// Colours, as plain values passed into the draw code (#12).
mod palette {
    pub const BACKGROUND: [u8; 4] = [0x16, 0x17, 0x1b, 0xff];
    pub const TEXT: [u8; 4] = [0xe8, 0xe8, 0xea, 0xff];
    pub const MUTED: [u8; 4] = [0x9a, 0xa0, 0xa6, 0xff];
    /// The active workspace of an output.
    pub const ACTIVE: [u8; 4] = [0x34, 0x37, 0x40, 0xff];
    /// The focused workspace.
    pub const FOCUSED: [u8; 4] = [0x4f, 0xd1, 0xc5, 0xff];
    /// Text on the focused workspace.
    pub const ON_FOCUSED: [u8; 4] = [0x10, 0x12, 0x14, 0xff];
}

const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 18.0;
/// Room for glyph overhang past the measured width.
const SLACK: f32 = 2.0;

/// Draws the bar and measures text.
pub struct Painter {
    fonts: FontSystem,
    cache: SwashCache,
}

impl std::fmt::Debug for Painter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Painter")
    }
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

impl Measure for Painter {
    fn width(&mut self, text: &str) -> f32 {
        let buffer = self.shaped(text, FONT_SIZE, LINE_HEIGHT, None);
        buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0, f32::max)
    }
}

/// One text run to draw.
struct Text<'a> {
    text: &'a str,
    x: f32,
    y: f32,
    w: f32,
    color: [u8; 4],
}

impl Painter {
    /// Loads the system fonts.
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    fn shaped(&mut self, text: &str, size: f32, line: f32, clip: Option<(f32, f32)>) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, line));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(clip.map(|c| c.0), clip.map(|c| c.1));
        let attrs = Attrs::new().family(Family::SansSerif);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }

    /// The bar, `width` by `height` logical pixels.
    pub fn bar(&mut self, bar: &Bar, width: f32, height: f32, scale: u32) -> Option<Pixmap> {
        let s = scale as f32;
        let mut pixmap = Pixmap::new(
            (width.ceil() as u32).max(1) * scale,
            (height.ceil() as u32).max(1) * scale,
        )?;
        fill_rect(
            &mut pixmap,
            (0.0, 0.0, width, height),
            palette::BACKGROUND,
            s,
        );
        let segments = layout::layout(bar, width, self);
        let y = (height - LINE_HEIGHT) / 2.0;

        let cells = bar.workspaces.iter().flatten();
        for (cell, placed) in cells.zip(&segments.workspaces) {
            let color = if cell.focused {
                fill_rect(
                    &mut pixmap,
                    (placed.x, 0.0, placed.w, height),
                    palette::FOCUSED,
                    s,
                );
                palette::ON_FOCUSED
            } else if cell.active {
                fill_rect(
                    &mut pixmap,
                    (placed.x, 0.0, placed.w, height),
                    palette::ACTIVE,
                    s,
                );
                palette::TEXT
            } else {
                palette::MUTED
            };
            let w = placed.w - 2.0 * CELL_PADDING;
            let text = Text {
                text: &cell.label,
                x: placed.x + CELL_PADDING,
                y,
                w,
                color,
            };
            self.text(&mut pixmap, s, text);
        }
        let label = bar.network.label();
        let mut texts: Vec<(String, Placed, [u8; 4])> = Vec::new();
        if let Some(clock) = segments.clock {
            texts.push((bar.clock.clone(), clock, palette::TEXT));
        }
        texts.push((label, segments.network, palette::TEXT));
        if let (Some(battery), Some(placed)) = (&bar.battery, segments.battery) {
            texts.push((battery.label(), placed, palette::TEXT));
        }
        for (text, placed, color) in &texts {
            let run = Text {
                text,
                x: placed.x,
                y,
                w: placed.w,
                color: *color,
            };
            self.text(&mut pixmap, s, run);
        }
        Some(pixmap)
    }

    /// Draws one line of text clipped to its width; the text is never
    /// interpreted.
    fn text(&mut self, pixmap: &mut Pixmap, s: f32, t: Text<'_>) {
        let clip = ((t.w + SLACK) * s, LINE_HEIGHT * s);
        let mut buffer = self.shaped(t.text, FONT_SIZE * s, LINE_HEIGHT * s, Some(clip));
        let [r, g, b, a] = t.color;
        let (ox, oy) = ((t.x * s) as i32, (t.y * s) as i32);
        buffer.draw(
            &mut self.fonts,
            &mut self.cache,
            Color::rgba(r, g, b, a),
            |x, y, w, h, color| {
                if x < 0 || y < 0 || x >= clip.0 as i32 || y >= clip.1 as i32 || color.a() == 0 {
                    return;
                }
                let rect = SkRect::from_xywh((ox + x) as f32, (oy + y) as f32, w as f32, h as f32);
                if let Some(rect) = rect {
                    let mut paint = Paint::default();
                    paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());
                    pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                }
            },
        );
    }
}

/// Fills `(x, y, w, h)` in logical pixels.
fn fill_rect(pixmap: &mut Pixmap, (x, y, w, h): (f32, f32, f32, f32), color: [u8; 4], s: f32) {
    if let Some(rect) = SkRect::from_xywh(x * s, y * s, w * s, h * s) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// Copies a pixmap (premultiplied RGBA) into a `wl_shm` ARGB8888 canvas
/// (premultiplied BGRA in memory).
pub fn to_argb8888(pixmap: &Pixmap, canvas: &mut [u8]) {
    let (pixels, _) = canvas.as_chunks_mut::<4>();
    for (dst, &[r, g, b, a]) in pixels.iter_mut().zip(pixmap.data().as_chunks::<4>().0) {
        *dst = [b, g, r, a];
    }
}
