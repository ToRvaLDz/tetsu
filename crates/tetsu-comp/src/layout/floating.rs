//! Floating placement.

use super::Rect;

/// At least this many pixels of a floating window stay on screen.
pub const MIN_VISIBLE: i32 = 32;

/// A window of `size` centred on the viewport.
pub fn centered(viewport: Rect, size: (i32, i32)) -> Rect {
    Rect {
        x: viewport.x + (viewport.w - size.0) / 2,
        y: viewport.y + (viewport.h - size.1) / 2,
        w: size.0,
        h: size.1,
    }
}

/// Moves `rect` so that `MIN_VISIBLE` pixels of it stay inside the
/// viewport. The size is kept.
pub fn clamp_into(viewport: Rect, rect: Rect) -> Rect {
    let axis = |pos: i32, len: i32, origin: i32, span: i32| {
        let low = origin + MIN_VISIBLE.min(len) - len;
        let high = origin + span - MIN_VISIBLE.min(len);
        pos.clamp(low, high.max(low))
    };
    Rect {
        x: axis(rect.x, rect.w, viewport.x, viewport.w),
        y: axis(rect.y, rect.h, viewport.y, viewport.h),
        ..rect
    }
}

/// The floating rect of a window that was a tile: the tile size capped to
/// 3/4 of the viewport, centred.
pub fn from_tile(viewport: Rect, tile: Rect) -> Rect {
    let size = (
        tile.w.min(viewport.w * 3 / 4).max(1),
        tile.h.min(viewport.h * 3 / 4).max(1),
    );
    centered(viewport, size)
}
