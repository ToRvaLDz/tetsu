//! Master and stack, as in dwm.

use super::{Gaps, Rect, split};

/// Smallest master share.
pub const MIN_RATIO: f64 = 0.1;
/// Largest master share.
pub const MAX_RATIO: f64 = 0.9;
/// Default master share.
pub const DEFAULT_RATIO: f64 = 0.55;

/// Clamps a ratio into `MIN_RATIO..=MAX_RATIO`; NaN gives the default.
pub fn clamp_ratio(ratio: f64) -> f64 {
    if ratio.is_nan() {
        DEFAULT_RATIO
    } else {
        ratio.clamp(MIN_RATIO, MAX_RATIO)
    }
}

/// `n` rects over `viewport`: the first is the master on the left with
/// `ratio` of the width, the rest stack on the right in equal rows. A lone
/// window fills the viewport.
pub fn master(viewport: Rect, gaps: Gaps, n: usize, ratio: f64) -> Vec<Rect> {
    let area = viewport.inset(gaps);
    match n {
        0 => return Vec::new(),
        1 => return vec![area],
        _ => {}
    }
    let usable = (area.w - gaps.inner).max(2);
    let master_w = ((f64::from(usable) * clamp_ratio(ratio)).round() as i32).clamp(1, usable - 1);
    let stack_x = area.x + master_w + gaps.inner;
    let stack_w = usable - master_w;
    let mut rects = vec![Rect {
        w: master_w,
        ..area
    }];
    rects.extend(
        split(area.h, n - 1, gaps.inner)
            .into_iter()
            .map(|(y, h)| Rect {
                x: stack_x,
                y: area.y + y,
                w: stack_w,
                h,
            }),
    );
    rects
}
