//! Segment placement with a fixed-width text measurer: ten pixels a char.

use tetsu_bar::model::Bar;
use tetsu_bar::model::workspaces::WsCell;
use tetsu_bar::sources::battery::{Battery, Status};
use tetsu_bar::sources::network::Network;
use tetsu_bar::ui::layout::{GAP, Measure, PADDING, Placed, Segments, layout};

struct Fixed;

impl Measure for Fixed {
    fn width(&mut self, text: &str) -> f32 {
        text.chars().count() as f32 * 10.0
    }
}

fn cell(label: &str) -> WsCell {
    WsCell {
        label: label.into(),
        active: false,
        focused: false,
    }
}

fn bar() -> Bar {
    Bar {
        clock: "09:05".into(),
        battery: Some(Battery {
            percent: 85,
            status: Status::Discharging,
        }),
        network: Network::Wifi {
            iface: "wlan0".into(),
            quality: Some(60),
        },
        workspaces: Some(vec![cell("1"), cell("2"), cell("web")]),
    }
}

/// Every placed item, left to right.
fn items(s: &Segments) -> Vec<Placed> {
    let mut all: Vec<Placed> = s.workspaces.clone();
    all.extend(s.clock);
    all.push(s.network);
    all.extend(s.battery);
    all.sort_by(|a, b| a.x.total_cmp(&b.x));
    all
}

fn assert_no_overlap(s: &Segments) {
    for pair in items(s).windows(2) {
        assert!(
            pair[0].x + pair[0].w <= pair[1].x,
            "{:?} overlaps {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn nothing_overlaps_at_any_width() {
    let mut width = 0.0;
    while width < 900.0 {
        assert_no_overlap(&layout(&bar(), width, &mut Fixed));
        width += 7.0;
    }
}

#[test]
fn the_right_group_is_flush_right() {
    let s = layout(&bar(), 1000.0, &mut Fixed);
    let battery = s.battery.unwrap();
    assert_eq!(battery.x + battery.w, 1000.0 - PADDING);
    assert_eq!(s.network.x + s.network.w + GAP, battery.x);
}

#[test]
fn without_a_battery_the_network_is_flush_right() {
    let mut no_battery = bar();
    no_battery.battery = None;
    let s = layout(&no_battery, 1000.0, &mut Fixed);
    assert_eq!(s.battery, None);
    assert_eq!(s.network.x + s.network.w, 1000.0 - PADDING);
}

#[test]
fn the_clock_is_centred() {
    let clock = layout(&bar(), 1000.0, &mut Fixed).clock.unwrap();
    assert_eq!(clock.x + clock.w / 2.0, 500.0);
}

#[test]
fn the_workspaces_start_at_the_left_padding() {
    let s = layout(&bar(), 1000.0, &mut Fixed);
    assert_eq!(s.workspaces.len(), 3);
    assert_eq!(s.workspaces[0].x, PADDING);
}

#[test]
fn on_a_narrow_bar_the_clock_goes_first() {
    let s = layout(&bar(), 400.0, &mut Fixed);
    assert_eq!(s.clock, None);
    assert_eq!(s.workspaces.len(), 3);
    assert_no_overlap(&s);
}

#[test]
fn hidden_segments_take_no_room() {
    let mut hidden = bar();
    hidden.workspaces = None;
    hidden.battery = None;
    let s = layout(&hidden, 1000.0, &mut Fixed);
    assert_eq!(s.workspaces, vec![]);
    assert_eq!(s.battery, None);
    // Only the clock and the network remain.
    assert_eq!(items(&s).len(), 2);
}
