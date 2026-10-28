//! Window decorations without Smithay: the inset a border takes from a
//! tile, the four edge rects around a window, the colours and which frames a
//! scene may show. The renderers only draw what `frames` returns, so a locked
//! scene cannot carry a border by construction.

pub mod glue;

use std::collections::HashSet;
use std::hash::Hash;

use crate::layout::Rect;
use crate::lock::Scene;

/// The widest border, in logical pixels.
pub const MAX_BORDER_WIDTH: i64 = 32;

/// The built-in border width.
pub const DEFAULT_WIDTH: i32 = 2;

/// A colour in 8-bit channels, so the settings stay `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha.
    pub a: u8,
}

impl Color {
    /// The straight channels as 0.0..=1.0 floats; the renderer takes them
    /// premultiplied.
    pub fn to_f32(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a].map(|c| f32::from(c) / 255.0)
    }
}

/// The built-in colour of the focused window's border.
pub const ACTIVE_COLOR: Color = Color {
    r: 0x4c,
    g: 0x8d,
    b: 0xf6,
    a: 0xff,
};

/// The built-in colour of the other windows' borders.
pub const INACTIVE_COLOR: Color = Color {
    r: 0x59,
    g: 0x59,
    b: 0x5e,
    a: 0xff,
};

/// The `[decorations]` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decorations {
    /// Border width in logical pixels; 0 draws none.
    pub width: i32,
    /// The focused window's colour.
    pub active: Color,
    /// Every other window's colour.
    pub inactive: Color,
}

impl Decorations {
    /// No border at all.
    pub const NONE: Self = Self {
        width: 0,
        active: ACTIVE_COLOR,
        inactive: INACTIVE_COLOR,
    };
}

impl Default for Decorations {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            ..Self::NONE
        }
    }
}

/// Parses `#rrggbb` or `#rrggbbaa`, in either case.
pub fn parse_color(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Color {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
        a: if digits.len() == 8 { channel(6)? } else { 255 },
    })
}

/// The content rect of a tile once the border takes `width` from each side;
/// every side stays at least 1.
pub fn inset(tile: Rect, width: i32) -> Rect {
    Rect {
        x: tile.x + width,
        y: tile.y + width,
        w: (tile.w - 2 * width).max(1),
        h: (tile.h - 2 * width).max(1),
    }
}

/// The four edges around `content`: top and bottom span the corners, left
/// and right sit between them, so nothing overlaps.
pub fn frame(content: Rect, width: i32) -> [Rect; 4] {
    let full_w = content.w + 2 * width;
    [
        Rect {
            x: content.x - width,
            y: content.y - width,
            w: full_w,
            h: width,
        },
        Rect {
            x: content.x - width,
            y: content.y + content.h,
            w: full_w,
            h: width,
        },
        Rect {
            x: content.x - width,
            y: content.y,
            w: width,
            h: content.h,
        },
        Rect {
            x: content.x + content.w,
            y: content.y,
            w: width,
            h: content.h,
        },
    ]
}

/// A window as the frames see it: its model id and its content rect, in
/// output-local logical pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowBox {
    /// The model's window id.
    pub id: String,
    /// The content rect.
    pub content: Rect,
}

/// The border of one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The model's window id.
    pub id: String,
    /// Top, bottom, left, right.
    pub rects: [Rect; 4],
    /// The colour of the four edges.
    pub color: Color,
}

/// The frames of `windows` (top first), or none on a locked scene or at
/// width 0.
pub fn frames(
    scene: &Scene,
    windows: &[WindowBox],
    focused: Option<&str>,
    decorations: &Decorations,
) -> Vec<Frame> {
    if matches!(scene, Scene::Locked { .. }) || decorations.width <= 0 {
        return Vec::new();
    }
    windows
        .iter()
        .map(|w| Frame {
            id: w.id.clone(),
            rects: frame(w.content, decorations.width),
            color: if focused == Some(w.id.as_str()) {
                decorations.active
            } else {
                decorations.inactive
            },
        })
        .collect()
}

/// The objects that already got their one answer. An object whose client
/// died never sends a release, so each new answer first drops the dead ones:
/// the set never outgrows the live objects.
#[derive(Debug)]
pub struct AnsweredOnce<K> {
    answered: HashSet<K>,
}

impl<K> Default for AnsweredOnce<K> {
    fn default() -> Self {
        Self {
            answered: HashSet::new(),
        }
    }
}

impl<K: Eq + Hash> AnsweredOnce<K> {
    /// True the first time `key` asks; prunes what `alive` rejects first.
    pub fn first(&mut self, key: K, alive: impl Fn(&K) -> bool) -> bool {
        self.answered.retain(|k| alive(k));
        self.answered.insert(key)
    }

    /// Drops a released object.
    pub fn forget(&mut self, key: &K) {
        self.answered.remove(key);
    }

    /// How many objects are remembered.
    pub fn len(&self) -> usize {
        self.answered.len()
    }

    /// True when no object is remembered.
    pub fn is_empty(&self) -> bool {
        self.answered.is_empty()
    }
}
