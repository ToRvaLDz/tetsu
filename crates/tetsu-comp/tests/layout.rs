//! The layout algorithms: dwindle, master and floating placement.

use std::str::FromStr;

use tetsu_comp::layout::floating::{MIN_VISIBLE, centered, clamp_into, from_tile};
use tetsu_comp::layout::{Gaps, LayoutKind, Rect, dwindle::dwindle, master::master, split, usable};

const VIEW: Rect = Rect {
    x: 0,
    y: 0,
    w: 1920,
    h: 1080,
};

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

const GAPS: Gaps = Gaps {
    outer: 10,
    inner: 20,
};

fn right(r: &Rect) -> i32 {
    r.x + r.w
}

fn bottom(r: &Rect) -> i32 {
    r.y + r.h
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < right(b) && b.x < right(a) && a.y < bottom(b) && b.y < bottom(a)
}

/// Both layouts as `(name, rects)` for n windows.
fn both(view: Rect, gaps: Gaps, n: usize) -> [(&'static str, Vec<Rect>); 2] {
    [
        ("dwindle", dwindle(view, gaps, n)),
        ("master", master(view, gaps, n, 0.55)),
    ]
}

#[test]
fn split_leaves_no_hole_and_no_overlap() {
    for parts in 1..=9 {
        let pieces = split(1001, parts, 0);
        assert_eq!(pieces.len(), parts);
        assert_eq!(pieces[0].0, 0);
        for pair in pieces.windows(2) {
            assert_eq!(pair[0].0 + pair[0].1, pair[1].0, "parts={parts}");
        }
        let last = pieces.last().unwrap();
        assert_eq!(last.0 + last.1, 1001);
    }
}

#[test]
fn split_separates_pieces_by_the_inner_gap() {
    let pieces = split(1000, 3, 20);
    for pair in pieces.windows(2) {
        assert_eq!(pair[1].0 - (pair[0].0 + pair[0].1), 20);
    }
    let last = pieces.last().unwrap();
    assert_eq!(last.0 + last.1, 1000);
}

#[test]
fn dwindle_known_cases() {
    assert_eq!(dwindle(VIEW, Gaps::NONE, 0), []);
    assert_eq!(dwindle(VIEW, Gaps::NONE, 1), [VIEW]);
    assert_eq!(
        dwindle(VIEW, Gaps::NONE, 2),
        [rect(0, 0, 960, 1080), rect(960, 0, 960, 1080)]
    );
    assert_eq!(
        dwindle(VIEW, Gaps::NONE, 3),
        [
            rect(0, 0, 960, 1080),
            rect(960, 0, 960, 540),
            rect(960, 540, 960, 540)
        ]
    );
    // 960x540 splits left/right again (width >= height).
    assert_eq!(
        dwindle(VIEW, Gaps::NONE, 5),
        [
            rect(0, 0, 960, 1080),
            rect(960, 0, 960, 540),
            rect(960, 540, 480, 540),
            rect(1440, 540, 480, 270),
            rect(1440, 810, 480, 270),
        ]
    );
}

#[test]
fn master_known_cases() {
    assert_eq!(master(VIEW, Gaps::NONE, 0, 0.55), []);
    assert_eq!(master(VIEW, Gaps::NONE, 1, 0.55), [VIEW]);
    assert_eq!(
        master(VIEW, Gaps::NONE, 2, 0.55),
        [rect(0, 0, 1056, 1080), rect(1056, 0, 864, 1080)]
    );
    assert_eq!(
        master(VIEW, Gaps::NONE, 4, 0.55),
        [
            rect(0, 0, 1056, 1080),
            rect(1056, 0, 864, 360),
            rect(1056, 360, 864, 360),
            rect(1056, 720, 864, 360),
        ]
    );
}

#[test]
fn the_master_ratio_is_clamped() {
    assert_eq!(
        master(VIEW, Gaps::NONE, 2, 0.0),
        master(VIEW, Gaps::NONE, 2, 0.1)
    );
    assert_eq!(master(VIEW, Gaps::NONE, 2, -3.0)[0].w, 192);
    assert_eq!(master(VIEW, Gaps::NONE, 2, 7.0)[0].w, 1728);
    assert_eq!(master(VIEW, Gaps::NONE, 2, f64::NAN)[0].w, 1056);
}

#[test]
fn rects_stay_inside_the_inset_viewport_and_never_overlap() {
    let views = [VIEW, rect(1920, 30, 1366, 768)];
    for view in views {
        for gaps in [Gaps::NONE, GAPS] {
            let area = rect(
                view.x + gaps.outer,
                view.y + gaps.outer,
                view.w - 2 * gaps.outer,
                view.h - 2 * gaps.outer,
            );
            for n in 1..=9 {
                for (name, rects) in both(view, gaps, n) {
                    assert_eq!(rects.len(), n, "{name} n={n}");
                    for r in &rects {
                        assert!(r.w > 0 && r.h > 0, "{name} n={n}: {r:?}");
                        assert!(
                            r.x >= area.x
                                && r.y >= area.y
                                && right(r) <= right(&area)
                                && bottom(r) <= bottom(&area),
                            "{name} n={n}: {r:?} outside {area:?}"
                        );
                    }
                    for (i, a) in rects.iter().enumerate() {
                        for b in &rects[i + 1..] {
                            assert!(!overlaps(a, b), "{name} n={n}: {a:?} {b:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn without_gaps_the_areas_cover_the_viewport_exactly() {
    let view = rect(10, 20, 1001, 777);
    for n in 1..=9 {
        for (name, rects) in both(view, Gaps::NONE, n) {
            let covered: i64 = rects.iter().map(|r| i64::from(r.w) * i64::from(r.h)).sum();
            assert_eq!(
                covered,
                i64::from(view.w) * i64::from(view.h),
                "{name} n={n}"
            );
        }
    }
}

#[test]
fn gaps_are_honoured_between_every_pair() {
    for n in 2..=9 {
        for (name, rects) in both(VIEW, GAPS, n) {
            for (i, a) in rects.iter().enumerate() {
                for b in &rects[i + 1..] {
                    let across_x = (b.x - right(a)).max(a.x - right(b));
                    let across_y = (b.y - bottom(a)).max(a.y - bottom(b));
                    assert!(
                        across_x.max(across_y) >= GAPS.inner,
                        "{name} n={n}: {a:?} {b:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn gaps_inset_a_single_window() {
    for (_, rects) in both(VIEW, GAPS, 1) {
        assert_eq!(rects, [rect(10, 10, 1900, 1060)]);
    }
}

#[test]
fn a_viewport_smaller_than_its_gaps_still_gives_positive_sizes() {
    let gaps = Gaps {
        outer: 40,
        inner: 40,
    };
    for n in 1..=4 {
        for (name, rects) in both(rect(0, 0, 30, 30), gaps, n) {
            assert_eq!(rects.len(), n, "{name}");
            assert!(rects.iter().all(|r| r.w >= 1 && r.h >= 1), "{name} n={n}");
        }
    }
}

#[test]
fn layout_kinds_cycle_and_parse() {
    assert_eq!(LayoutKind::Dwindle.next(), LayoutKind::Master);
    assert_eq!(LayoutKind::Master.next(), LayoutKind::Floating);
    assert_eq!(LayoutKind::Floating.next(), LayoutKind::Dwindle);
    assert_eq!(LayoutKind::from_str("dwindle"), Ok(LayoutKind::Dwindle));
    assert_eq!(LayoutKind::from_str("master"), Ok(LayoutKind::Master));
    assert_eq!(LayoutKind::from_str("floating"), Ok(LayoutKind::Floating));
    assert!(LayoutKind::from_str("spiral").is_err());
}

#[test]
fn floating_windows_are_centred() {
    assert_eq!(centered(VIEW, (640, 480)), rect(640, 300, 640, 480));
    let view = rect(1920, 0, 1000, 800);
    assert_eq!(centered(view, (200, 100)), rect(2320, 350, 200, 100));
}

#[test]
fn clamping_keeps_a_strip_of_the_window_on_screen() {
    // Far off to the right and below.
    let r = clamp_into(VIEW, rect(5000, 5000, 400, 300));
    assert_eq!((r.x, r.y), (1920 - MIN_VISIBLE, 1080 - MIN_VISIBLE));
    // Far off to the left and above.
    let r = clamp_into(VIEW, rect(-5000, -5000, 400, 300));
    assert_eq!((r.x, r.y), (MIN_VISIBLE - 400, MIN_VISIBLE - 300));
    // Inside stays put, and the size never changes.
    assert_eq!(
        clamp_into(VIEW, rect(100, 100, 400, 300)),
        rect(100, 100, 400, 300)
    );
}

#[test]
fn a_tile_that_floats_is_capped_to_three_quarters_and_centred() {
    assert_eq!(from_tile(VIEW, VIEW), rect(240, 135, 1440, 810));
    assert_eq!(
        from_tile(VIEW, rect(0, 0, 400, 300)),
        rect(760, 390, 400, 300)
    );
}

// ---- the viewport left by layer-shell zones ----

#[test]
fn an_empty_layer_map_leaves_the_whole_output() {
    let output = rect(1920, 0, 1920, 1080);
    assert_eq!(usable(output, rect(0, 0, 1920, 1080)), output);
}

#[test]
fn a_top_bar_on_an_output_at_x_1920_shrinks_it_from_the_top() {
    let output = rect(1920, 0, 1920, 1080);
    assert_eq!(
        usable(output, rect(0, 28, 1920, 1052)),
        rect(1920, 28, 1920, 1052)
    );
}

#[test]
fn left_and_bottom_reservations_combine() {
    let output = rect(0, 0, 1920, 1080);
    assert_eq!(
        usable(output, rect(40, 0, 1880, 1050)),
        rect(40, 0, 1880, 1050)
    );
}

#[test]
fn a_degenerate_zone_falls_back_to_the_whole_output() {
    let output = rect(100, 50, 1920, 1080);
    for zone in [
        rect(0, 0, 0, 1080),
        rect(0, 0, 1920, 0),
        rect(0, 1080, -5, 20),
        rect(0, 0, 1920, -1),
    ] {
        assert_eq!(usable(output, zone), output, "{zone:?}");
    }
}
