//! The pure side of the decorations: the inset, the border rects, the
//! colours and the frames a scene allows, with no compositor around them.

use tetsu_comp::decor::{
    ACTIVE_COLOR, Color, Decorations, INACTIVE_COLOR, WindowBox, frame, frames, inset, parse_color,
};
use tetsu_comp::layout::Rect;
use tetsu_comp::lock::{Decision, LockId, LockMachine, Scene};
use tetsu_comp::trust::Trust;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

fn decorations(width: i32) -> Decorations {
    Decorations {
        width,
        active: ACTIVE_COLOR,
        inactive: INACTIVE_COLOR,
    }
}

fn boxes() -> Vec<WindowBox> {
    vec![
        WindowBox {
            id: "w1".to_owned(),
            content: rect(10, 10, 100, 80),
        },
        WindowBox {
            id: "w2".to_owned(),
            content: rect(200, 10, 100, 80),
        },
    ]
}

#[test]
fn width_zero_leaves_the_tile_alone() {
    let tile = rect(0, 0, 1920, 1080);
    assert_eq!(inset(tile, 0), tile);
}

#[test]
fn a_border_shrinks_the_tile_on_every_side() {
    assert_eq!(
        inset(rect(0, 0, 1920, 1080), 3),
        rect(3, 3, 1914, 1074)
    );
}

#[test]
fn a_thin_tile_keeps_sides_of_at_least_one() {
    let thin = inset(rect(5, 5, 4, 2), 3);
    assert_eq!((thin.x, thin.y), (8, 8));
    assert!(thin.w >= 1 && thin.h >= 1);
}

fn touches(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

#[test]
fn the_four_edges_wrap_the_content_without_overlap() {
    let content = rect(10, 20, 100, 60);
    let w = 3;
    let edges = frame(content, w);
    for (i, a) in edges.iter().enumerate() {
        assert!(!touches(a, &content), "edge {i} covers the content");
        for b in &edges[i + 1..] {
            assert!(!touches(a, b), "two edges overlap");
        }
    }
    let area: i32 = edges.iter().map(|e| e.w * e.h).sum();
    assert_eq!(area, (100 + 2 * w) * (60 + 2 * w) - 100 * 60);
    let left = edges.iter().map(|e| e.x).min().unwrap();
    let top = edges.iter().map(|e| e.y).min().unwrap();
    let right = edges.iter().map(|e| e.x + e.w).max().unwrap();
    let bottom = edges.iter().map(|e| e.y + e.h).max().unwrap();
    assert_eq!((left, top), (content.x - w, content.y - w));
    assert_eq!(
        (right, bottom),
        (content.x + content.w + w, content.y + content.h + w)
    );
}

#[test]
fn colours_parse_in_six_or_eight_digits() {
    assert_eq!(
        parse_color("#3366CC"),
        Some(Color {
            r: 0x33,
            g: 0x66,
            b: 0xcc,
            a: 255
        })
    );
    assert_eq!(parse_color("#3366cc80").map(|c| c.a), Some(0x80));
}

#[test]
fn bad_colours_are_refused() {
    for bad in ["red", "#12345", "#gggggg", "", "3366cc", "#3366cc8"] {
        assert_eq!(parse_color(bad), None, "{bad:?}");
    }
}

#[test]
fn frames_keep_the_order_and_colour_the_focused_window() {
    let d = decorations(2);
    let out = frames(&Scene::Normal, &boxes(), Some("w2"), &d);
    let ids: Vec<&str> = out.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["w1", "w2"]);
    assert_eq!(out[0].color, d.inactive);
    assert_eq!(out[1].color, d.active);
    assert_eq!(out[0].rects, frame(rect(10, 10, 100, 80), 2));
}

#[test]
fn width_zero_draws_nothing() {
    assert!(frames(&Scene::Normal, &boxes(), Some("w1"), &decorations(0)).is_empty());
}

#[test]
fn a_locked_scene_never_gets_a_border() {
    let mut machine = LockMachine::new();
    machine.output_added("out-1");
    assert_eq!(
        machine.request(LockId(1), Trust::Untrusted),
        Decision::Accept
    );
    let scene = machine.scene("out-1");
    assert!(matches!(scene, Scene::Locked { .. }));
    assert!(frames(&scene, &boxes(), Some("w1"), &decorations(4)).is_empty());
}
