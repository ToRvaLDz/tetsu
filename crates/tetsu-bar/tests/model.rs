//! The redraw decision: `apply` returns a bar only when what is shown changed.

use tetsu_bar::model::workspaces::WsCell;
use tetsu_bar::model::{Bar, Change};
use tetsu_bar::sources::battery::{Battery, Status};
use tetsu_bar::sources::network::Network;

fn cell(label: &str, active: bool, focused: bool) -> WsCell {
    WsCell {
        label: label.into(),
        active,
        focused,
    }
}

fn strip() -> Vec<WsCell> {
    vec![cell("1", true, true), cell("2", false, false)]
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
        workspaces: Some(strip()),
    }
}

#[test]
fn a_tick_inside_the_same_minute_is_not_a_redraw() {
    assert_eq!(bar().apply(Change::Tick("09:05".into())), None);
}

#[test]
fn a_minute_rollover_redraws() {
    let next = bar().apply(Change::Tick("09:06".into())).unwrap();
    assert_eq!(next.clock, "09:06");
    assert_eq!(next.battery, bar().battery);
}

#[test]
fn an_identical_battery_sample_is_not_a_redraw() {
    assert_eq!(bar().apply(Change::Battery(bar().battery)), None);
}

#[test]
fn a_capacity_or_status_change_redraws() {
    let lower = Some(Battery {
        percent: 84,
        status: Status::Discharging,
    });
    assert_eq!(bar().apply(Change::Battery(lower)).unwrap().battery, lower);
    let plugged = Some(Battery {
        percent: 85,
        status: Status::Charging,
    });
    assert!(bar().apply(Change::Battery(plugged)).is_some());
    assert_eq!(bar().apply(Change::Battery(None)).unwrap().battery, None);
}

#[test]
fn an_identical_network_sample_is_not_a_redraw() {
    assert_eq!(bar().apply(Change::Network(bar().network)), None);
}

#[test]
fn a_link_change_redraws() {
    let wired = Network::Wired {
        iface: "enp3s0".into(),
    };
    assert_eq!(
        bar().apply(Change::Network(wired.clone())).unwrap().network,
        wired
    );
    assert!(bar().apply(Change::Network(Network::Offline)).is_some());
}

#[test]
fn a_strip_equal_to_the_current_one_is_not_a_redraw() {
    assert_eq!(bar().apply(Change::Workspaces(Some(strip()))), None);
}

#[test]
fn a_focus_change_redraws() {
    let moved = vec![cell("1", true, false), cell("2", false, true)];
    let next = bar()
        .apply(Change::Workspaces(Some(moved.clone())))
        .unwrap();
    assert_eq!(next.workspaces, Some(moved));
}

#[test]
fn a_disconnect_redraws_and_a_second_one_does_not() {
    let gone = bar().apply(Change::Workspaces(None)).unwrap();
    assert_eq!(gone.workspaces, None);
    assert_eq!(gone.apply(Change::Workspaces(None)), None);
}

#[test]
fn an_empty_strip_is_not_the_same_as_no_strip() {
    let gone = bar().apply(Change::Workspaces(None)).unwrap();
    assert!(gone.apply(Change::Workspaces(Some(vec![]))).is_some());
}

#[test]
fn apply_leaves_the_original_untouched() {
    let before = bar();
    let _ = before.apply(Change::Tick("10:00".into()));
    assert_eq!(before, bar());
}
