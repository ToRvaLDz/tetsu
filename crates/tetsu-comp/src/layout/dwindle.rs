//! Dwindle: each window takes half of what is left.

use super::{Gaps, Rect, split};

/// `n` rects over `viewport`. Window i takes one half of the remaining
/// area, split along its longer side (left/right when it is at least as
/// wide as tall); the last window takes the rest.
pub fn dwindle(viewport: Rect, gaps: Gaps, n: usize) -> Vec<Rect> {
    let mut rest = viewport.inset(gaps);
    let mut rects = Vec::with_capacity(n);
    for i in 0..n {
        if i + 1 == n {
            rects.push(rest);
            break;
        }
        let (first, second);
        if rest.w >= rest.h {
            let halves = split(rest.w, 2, gaps.inner);
            first = Rect {
                w: halves[0].1,
                ..rest
            };
            second = Rect {
                x: rest.x + halves[1].0,
                w: halves[1].1,
                ..rest
            };
        } else {
            let halves = split(rest.h, 2, gaps.inner);
            first = Rect {
                h: halves[0].1,
                ..rest
            };
            second = Rect {
                y: rest.y + halves[1].0,
                h: halves[1].1,
                ..rest
            };
        }
        rects.push(first);
        rest = second;
    }
    rects
}
