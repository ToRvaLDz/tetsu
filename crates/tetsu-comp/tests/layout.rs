//! The default layout: equal columns over the viewport.

use tetsu_comp::layout::{Gaps, Rect, tile};

const VIEW: Rect = Rect {
    x: 0,
    y: 0,
    w: 1920,
    h: 1080,
};

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn no_windows_no_rects() {
    assert_eq!(tile(VIEW, Gaps::NONE, 0), []);
}

#[test]
fn one_window_takes_the_viewport() {
    assert_eq!(tile(VIEW, Gaps::NONE, 1), [VIEW]);
}

#[test]
fn three_windows_share_equal_columns() {
    assert_eq!(
        tile(VIEW, Gaps::NONE, 3),
        [
            rect(0, 0, 640, 1080),
            rect(640, 0, 640, 1080),
            rect(1280, 0, 640, 1080)
        ]
    );
}

#[test]
fn rounding_leaves_no_gap_and_no_overlap() {
    let view = rect(10, 20, 1000, 500);
    for n in 1..=9 {
        let rects = tile(view, Gaps::NONE, n);
        assert_eq!(rects.len(), n);
        assert_eq!(rects[0].x, view.x);
        for pair in rects.windows(2) {
            assert_eq!(pair[0].x + pair[0].w, pair[1].x, "n={n}");
        }
        let last = rects.last().unwrap();
        assert_eq!(last.x + last.w, view.x + view.w, "n={n}");
        assert!(rects.iter().all(|r| r.y == 20 && r.h == 500 && r.w > 0));
    }
}

#[test]
fn gaps_separate_columns_and_inset_the_edges() {
    let gaps = Gaps {
        outer: 10,
        inner: 20,
    };
    let rects = tile(VIEW, gaps, 2);
    assert_eq!(rects[0].x, 10);
    assert_eq!(rects[0].y, 10);
    assert_eq!(rects[0].h, 1060);
    assert_eq!(rects[1].x - (rects[0].x + rects[0].w), 20);
    assert_eq!(rects[1].x + rects[1].w, 1920 - 10);
}

#[test]
fn a_viewport_smaller_than_its_gaps_still_gives_positive_sizes() {
    let rects = tile(
        rect(0, 0, 30, 30),
        Gaps {
            outer: 40,
            inner: 40,
        },
        3,
    );
    assert_eq!(rects.len(), 3);
    assert!(rects.iter().all(|r| r.w >= 1 && r.h >= 1));
}
