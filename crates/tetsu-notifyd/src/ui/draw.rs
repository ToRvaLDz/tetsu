//! Software drawing of the cards into tiny-skia pixmaps, text through
//! cosmic-text. Everything is laid out in logical pixels and drawn at the
//! surface's integer scale.

use std::time::Instant;

use cosmic_text::{
    Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight, Wrap,
};
use tiny_skia::{
    FillRule, IntSize, Paint, PathBuilder, Pixmap, PixmapPaint, Rect as SkRect, Stroke, Transform,
};

use super::layout::{
    BODY_LINE, CLOSE_SIZE, Card, HEADER, HISTORY_ROW, MARGIN, Measure, PADDING, PanelLayout, Rect,
    SENDER_LINE, TITLE_LINE,
};
use crate::core::{HistoryView, Icon, Outcome, Source};
use tetsu_bus::notify::Urgency;

/// Colours, as plain values passed into the draw code (#12).
mod palette {
    pub const CARD: [u8; 4] = [0x1e, 0x1f, 0x24, 0xf5];
    pub const PANEL: [u8; 4] = [0x16, 0x17, 0x1b, 0xff];
    pub const TEXT: [u8; 4] = [0xe8, 0xe8, 0xea, 0xff];
    pub const MUTED: [u8; 4] = [0x9a, 0xa0, 0xa6, 0xff];
    /// The Tetsu frame: only bus callers, named from their credentials.
    pub const TETSU: [u8; 4] = [0x4f, 0xd1, 0xc5, 0xff];
    pub const CRITICAL: [u8; 4] = [0xe5, 0x48, 0x4d, 0xff];
    pub const BUTTON: [u8; 4] = [0x34, 0x37, 0x40, 0xff];
    pub const BUTTON_DISARMED: [u8; 4] = [0x26, 0x28, 0x2e, 0xff];
}

const CORNER: f32 = 8.0;
const FRAME_WIDTH: f32 = 2.0;
const ICON_SIZE: f32 = 16.0;

/// Draws cards and measures text.
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
    fn lines(&mut self, text: &str, width: f32) -> usize {
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(13.0, BODY_LINE));
        buffer.set_wrap(Wrap::WordOrGlyph);
        buffer.set_size(Some(width), None);
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer.layout_runs().count().max(1)
    }
}

/// One text run to draw.
struct Text<'a> {
    text: &'a str,
    rect: Rect,
    size: f32,
    line: f32,
    color: [u8; 4],
    bold: bool,
    wrap: bool,
}

impl Painter {
    /// Loads the system fonts.
    pub fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    /// The popup surface: `cards` stacked, `height` logical pixels tall.
    pub fn popups(
        &mut self,
        cards: &[Card],
        width: f32,
        height: f32,
        scale: u32,
        now: Instant,
    ) -> Option<Pixmap> {
        let mut pixmap = new_pixmap(width, height, scale)?;
        for card in cards {
            self.card(&mut pixmap, card, scale as f32, now);
        }
        Some(pixmap)
    }

    /// The panel surface.
    #[allow(clippy::too_many_arguments)]
    pub fn panel(
        &mut self,
        layout: &PanelLayout,
        history: &[HistoryView],
        width: f32,
        height: f32,
        scale: u32,
        now: Instant,
    ) -> Option<Pixmap> {
        let s = scale as f32;
        let mut pixmap = new_pixmap(width, height, scale)?;
        fill_rect(
            &mut pixmap,
            Rect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: height,
            },
            palette::PANEL,
            s,
        );
        for card in &layout.cards {
            if card.rect.y + card.rect.h > HEADER && card.rect.y < height {
                self.card(&mut pixmap, card, s, now);
            }
        }
        for (i, entry) in history.iter().enumerate() {
            let y = layout.history_top + i as f32 * HISTORY_ROW;
            if y + HISTORY_ROW > HEADER && y < height {
                self.history_row(&mut pixmap, entry, y, width, s);
            }
        }
        // The header covers content scrolled under it.
        fill_rect(
            &mut pixmap,
            Rect {
                x: 0.0,
                y: 0.0,
                w: width,
                h: HEADER,
            },
            palette::PANEL,
            s,
        );
        self.text(
            &mut pixmap,
            s,
            Text {
                text: "Notifications",
                rect: Rect {
                    x: MARGIN + PADDING,
                    y: (HEADER - TITLE_LINE) / 2.0,
                    w: width - 80.0,
                    h: TITLE_LINE,
                },
                size: 15.0,
                line: TITLE_LINE,
                color: palette::TEXT,
                bold: true,
                wrap: false,
            },
        );
        draw_cross(&mut pixmap, layout.close, palette::MUTED, s);
        Some(pixmap)
    }

    fn card(&mut self, pixmap: &mut Pixmap, card: &Card, s: f32, now: Instant) {
        let framed = card.framed();
        rounded(pixmap, card.rect, palette::CARD, s, None);
        if framed {
            rounded(pixmap, card.rect, palette::TETSU, s, Some(FRAME_WIDTH));
        } else if card.note.urgency == Urgency::Critical {
            fill_rect(
                pixmap,
                Rect {
                    w: 3.0,
                    ..card.rect
                },
                palette::CRITICAL,
                s,
            );
        }
        self.card_header(pixmap, card, s, framed);
        self.card_text(pixmap, card, s);
        self.card_buttons(pixmap, card, s, now);
        draw_cross(pixmap, card.close, palette::MUTED, s);
    }

    /// The sender line: the Tetsu frame's credential-derived name, or the
    /// application name with its category.
    fn card_header(&mut self, pixmap: &mut Pixmap, card: &Card, s: f32, framed: bool) {
        let mut sender_x = card.body.x;
        if let Some(icon) = &card.note.icon {
            draw_icon(pixmap, icon, card.body.x, card.body.y, s);
            sender_x += ICON_SIZE + 4.0;
        }
        let sender = match (&card.note.source, &card.note.category) {
            (Source::Tetsu(name), _) => format!("Tetsu · {name}"),
            (Source::App(name), Some(category)) => format!("{name} · {category}"),
            (Source::App(name), None) => name.clone(),
        };
        let sender_color = if framed {
            palette::TETSU
        } else {
            palette::MUTED
        };
        let text_right = card.body.x + card.body.w;
        self.text(
            pixmap,
            s,
            Text {
                text: &sender,
                rect: Rect {
                    x: sender_x,
                    y: card.body.y,
                    w: text_right - sender_x,
                    h: SENDER_LINE,
                },
                size: 11.0,
                line: SENDER_LINE,
                color: sender_color,
                bold: framed,
                wrap: false,
            },
        );
    }

    /// Title and body.
    fn card_text(&mut self, pixmap: &mut Pixmap, card: &Card, s: f32) {
        self.text(
            pixmap,
            s,
            Text {
                text: &card.note.title,
                rect: Rect {
                    x: card.body.x,
                    y: card.body.y + SENDER_LINE,
                    w: card.body.w,
                    h: TITLE_LINE,
                },
                size: 14.0,
                line: TITLE_LINE,
                color: palette::TEXT,
                bold: true,
                wrap: false,
            },
        );
        if card.body_lines > 0 {
            self.text(
                pixmap,
                s,
                Text {
                    text: &card.note.body,
                    rect: Rect {
                        x: card.body.x,
                        y: card.body.y + SENDER_LINE + TITLE_LINE,
                        w: card.body.w,
                        h: card.body_lines as f32 * BODY_LINE,
                    },
                    size: 13.0,
                    line: BODY_LINE,
                    color: palette::TEXT,
                    bold: false,
                    wrap: true,
                },
            );
        }
    }

    /// The buttons, drawn disarmed until they accept clicks.
    fn card_buttons(&mut self, pixmap: &mut Pixmap, card: &Card, s: f32, now: Instant) {
        let armed = now >= card.note.armed_at;
        for (rect, action) in &card.buttons {
            let (fill, ink) = if armed {
                (palette::BUTTON, palette::TEXT)
            } else {
                (palette::BUTTON_DISARMED, palette::MUTED)
            };
            rounded(pixmap, *rect, fill, s, None);
            self.text(
                pixmap,
                s,
                Text {
                    text: &action.label,
                    rect: Rect {
                        x: rect.x + 8.0,
                        y: rect.y + (rect.h - BODY_LINE) / 2.0,
                        w: rect.w - 16.0,
                        h: BODY_LINE,
                    },
                    size: 13.0,
                    line: BODY_LINE,
                    color: ink,
                    bold: false,
                    wrap: false,
                },
            );
        }
    }

    fn history_row(
        &mut self,
        pixmap: &mut Pixmap,
        entry: &HistoryView,
        y: f32,
        width: f32,
        s: f32,
    ) {
        let x = MARGIN + PADDING;
        let w = width - 2.0 * x;
        let who = match &entry.source {
            Source::Tetsu(name) => format!("Tetsu · {name}"),
            Source::App(name) => name.clone(),
        };
        let outcome = match &entry.outcome {
            Outcome::Invoked(label) => label.clone(),
            Outcome::Dismissed => "dismissed".into(),
            Outcome::Expired => "expired".into(),
            Outcome::Closed => "closed".into(),
            Outcome::Withdrawn => "withdrawn".into(),
        };
        self.text(
            pixmap,
            s,
            Text {
                text: &entry.title,
                rect: Rect {
                    x,
                    y,
                    w,
                    h: BODY_LINE,
                },
                size: 13.0,
                line: BODY_LINE,
                color: palette::TEXT,
                bold: false,
                wrap: false,
            },
        );
        self.text(
            pixmap,
            s,
            Text {
                text: &format!("{who} · {outcome}"),
                rect: Rect {
                    x,
                    y: y + BODY_LINE,
                    w,
                    h: SENDER_LINE,
                },
                size: 11.0,
                line: SENDER_LINE,
                color: palette::MUTED,
                bold: false,
                wrap: false,
            },
        );
    }

    /// Draws text clipped to its rectangle; the text is never interpreted.
    fn text(&mut self, pixmap: &mut Pixmap, s: f32, t: Text<'_>) {
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(t.size * s, t.line * s));
        buffer.set_wrap(if t.wrap {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        buffer.set_size(Some(t.rect.w * s), Some(t.rect.h * s));
        let attrs = Attrs::new().family(Family::SansSerif).weight(if t.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        });
        buffer.set_text(t.text, &attrs, Shaping::Advanced, None);
        let [r, g, b, a] = t.color;
        let (ox, oy) = ((t.rect.x * s) as i32, (t.rect.y * s) as i32);
        let (cw, ch) = ((t.rect.w * s) as i32, (t.rect.h * s) as i32);
        buffer.draw(
            &mut self.fonts,
            &mut self.cache,
            Color::rgba(r, g, b, a),
            |x, y, w, h, color| {
                if x < 0 || y < 0 || x >= cw || y >= ch || color.a() == 0 {
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

fn new_pixmap(width: f32, height: f32, scale: u32) -> Option<Pixmap> {
    Pixmap::new(
        (width.ceil() as u32).max(1) * scale,
        (height.ceil() as u32).max(1) * scale,
    )
}

fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    paint
}

fn fill_rect(pixmap: &mut Pixmap, r: Rect, color: [u8; 4], s: f32) {
    if let Some(rect) = SkRect::from_xywh(r.x * s, r.y * s, r.w * s, r.h * s) {
        pixmap.fill_rect(rect, &paint(color), Transform::identity(), None);
    }
}

/// A rounded rectangle, filled, or stroked `stroke` wide.
fn rounded(pixmap: &mut Pixmap, r: Rect, color: [u8; 4], s: f32, stroke: Option<f32>) {
    let inset = stroke.map_or(0.0, |w| w / 2.0);
    let (x, y, w, h) = (
        (r.x + inset) * s,
        (r.y + inset) * s,
        (r.w - 2.0 * inset) * s,
        (r.h - 2.0 * inset) * s,
    );
    let c = (CORNER * s).min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + c, y);
    pb.line_to(x + w - c, y);
    pb.quad_to(x + w, y, x + w, y + c);
    pb.line_to(x + w, y + h - c);
    pb.quad_to(x + w, y + h, x + w - c, y + h);
    pb.line_to(x + c, y + h);
    pb.quad_to(x, y + h, x, y + h - c);
    pb.line_to(x, y + c);
    pb.quad_to(x, y, x + c, y);
    pb.close();
    let Some(path) = pb.finish() else { return };
    match stroke {
        Some(width) => {
            let stroke = Stroke {
                width: width * s,
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
        }
        None => pixmap.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        ),
    }
}

fn draw_cross(pixmap: &mut Pixmap, r: Rect, color: [u8; 4], s: f32) {
    let pad = CLOSE_SIZE * 0.3;
    let mut pb = PathBuilder::new();
    pb.move_to((r.x + pad) * s, (r.y + pad) * s);
    pb.line_to((r.x + r.w - pad) * s, (r.y + r.h - pad) * s);
    pb.move_to((r.x + r.w - pad) * s, (r.y + pad) * s);
    pb.line_to((r.x + pad) * s, (r.y + r.h - pad) * s);
    if let Some(path) = pb.finish() {
        let stroke = Stroke {
            width: 1.5 * s,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
    }
}

fn draw_icon(pixmap: &mut Pixmap, icon: &Icon, x: f32, y: f32, s: f32) {
    let Some(size) = IntSize::from_wh(icon.width, icon.height) else {
        return;
    };
    let premultiplied: Vec<u8> = icon
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|&[r, g, b, a]| {
            let m = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
            [m(r), m(g), m(b), a]
        })
        .collect();
    let Some(source) = Pixmap::from_vec(premultiplied, size) else {
        return;
    };
    let sx = ICON_SIZE * s / icon.width as f32;
    let sy = ICON_SIZE * s / icon.height as f32;
    let transform = Transform::from_row(sx, 0.0, 0.0, sy, x * s, y * s);
    let paint = PixmapPaint {
        quality: tiny_skia::FilterQuality::Bilinear,
        ..PixmapPaint::default()
    };
    pixmap.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
}

/// Copies a pixmap (premultiplied RGBA) into a `wl_shm` ARGB8888 canvas
/// (premultiplied BGRA in memory).
pub fn to_argb8888(pixmap: &Pixmap, canvas: &mut [u8]) {
    let (pixels, _) = canvas.as_chunks_mut::<4>();
    for (dst, &[r, g, b, a]) in pixels.iter_mut().zip(pixmap.data().as_chunks::<4>().0) {
        *dst = [b, g, r, a];
    }
}
