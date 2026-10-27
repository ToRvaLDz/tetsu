//! The default layout. The viewport equals the output for now (#22.6); the
//! layout engine replaces `tile`.

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

/// Space around and between windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gaps {
    /// Between the viewport edge and the windows.
    pub outer: i32,
    /// Between two windows.
    pub inner: i32,
}

impl Gaps {
    /// No gaps.
    pub const NONE: Self = Self { outer: 0, inner: 0 };
}

/// `n` equal columns over `viewport`. The column edges are rounded from
/// the viewport, so the last column ends exactly at its right edge.
pub fn tile(viewport: Rect, gaps: Gaps, n: usize) -> Vec<Rect> {
    let Ok(count) = i32::try_from(n) else {
        return Vec::new();
    };
    if count == 0 {
        return Vec::new();
    }
    let area = Rect {
        x: viewport.x + gaps.outer,
        y: viewport.y + gaps.outer,
        w: (viewport.w - 2 * gaps.outer).max(count),
        h: (viewport.h - 2 * gaps.outer).max(1),
    };
    let edge = |i: i32| area.x + (i64::from(area.w) * i64::from(i) / i64::from(count)) as i32;
    (0..count)
        .map(|i| {
            let left = edge(i);
            let right = edge(i + 1);
            let gap = if i + 1 < count { gaps.inner } else { 0 };
            Rect {
                x: left,
                y: area.y,
                w: (right - left - gap).max(1),
                h: area.h,
            }
        })
        .collect()
}
