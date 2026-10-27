//! The render device choice: pure candidates in, one selection out.

use tetsu_comp::gpu::select::{Candidate, Selection, is_internal, select};

fn gpu(id: u64, primary: bool, software: bool, outputs: &[&str]) -> Candidate {
    Candidate {
        id,
        primary,
        software,
        outputs: outputs.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn picked(selection: Selection) -> (u64, bool, &'static str) {
    match selection {
        Selection::Device {
            id,
            automatic,
            reason,
        } => (id, automatic, reason),
        Selection::NoGpu => panic!("no gpu"),
    }
}

const IGPU: u64 = 1;
const DGPU: u64 = 2;

#[test]
fn internal_panels_are_edp_lvds_and_dsi() {
    assert!(is_internal("eDP-1"));
    assert!(is_internal("LVDS-1"));
    assert!(is_internal("DSI-2"));
    assert!(!is_internal("HDMI-A-1"));
    assert!(!is_internal("DP-3"));
}

#[test]
fn gs66_with_only_the_panel_picks_the_igpu() {
    let c = [
        gpu(IGPU, true, false, &["eDP-1"]),
        gpu(DGPU, false, false, &[]),
    ];
    let (id, automatic, _) = picked(select(&c, None));
    assert_eq!(id, IGPU);
    assert!(automatic);
}

#[test]
fn gs66_with_an_external_monitor_on_the_dgpu_picks_the_dgpu() {
    let c = [
        gpu(IGPU, true, false, &["eDP-1"]),
        gpu(DGPU, false, false, &["HDMI-A-1"]),
    ];
    let (id, automatic, reason) = picked(select(&c, None));
    assert_eq!(id, DGPU);
    assert!(automatic);
    assert_eq!(reason, "drives an external output");
}

#[test]
fn a_desktop_with_one_dgpu_picks_it() {
    let c = [gpu(DGPU, true, false, &["DP-1", "DP-2"])];
    assert_eq!(picked(select(&c, None)).0, DGPU);
}

#[test]
fn no_outputs_anywhere_picks_the_primary() {
    let c = [gpu(DGPU, false, false, &[]), gpu(IGPU, true, false, &[])];
    assert_eq!(picked(select(&c, None)).0, IGPU);
}

#[test]
fn the_most_connected_outputs_win_when_both_drive_externals() {
    let c = [
        gpu(IGPU, true, false, &["HDMI-A-1"]),
        gpu(DGPU, false, false, &["DP-1", "DP-2"]),
    ];
    assert_eq!(picked(select(&c, None)).0, DGPU);
}

#[test]
fn a_tie_picks_the_primary_then_the_lowest_id() {
    let tie = [
        gpu(5, false, false, &["DP-1"]),
        gpu(3, true, false, &["DP-2"]),
    ];
    assert_eq!(picked(select(&tie, None)).0, 3);
    let flat = [
        gpu(7, false, false, &["DP-1"]),
        gpu(4, false, false, &["DP-2"]),
    ];
    assert_eq!(picked(select(&flat, None)).0, 4);
}

#[test]
fn a_software_device_loses_to_hardware() {
    let c = [
        gpu(1, true, true, &["Virtual-1", "Virtual-2"]),
        gpu(2, false, false, &[]),
    ];
    assert_eq!(picked(select(&c, None)).0, 2);
}

#[test]
fn a_software_only_system_picks_the_software_device() {
    let c = [gpu(9, true, true, &["Virtual-1"])];
    assert_eq!(picked(select(&c, None)).0, 9);
}

#[test]
fn an_override_that_is_present_is_honoured() {
    let c = [
        gpu(IGPU, true, false, &["eDP-1"]),
        gpu(DGPU, false, false, &["HDMI-A-1"]),
    ];
    let (id, automatic, _) = picked(select(&c, Some(Some(IGPU))));
    assert_eq!(id, IGPU);
    assert!(!automatic);
}

#[test]
fn an_override_that_is_absent_falls_back_to_auto() {
    let c = [
        gpu(IGPU, true, false, &["eDP-1"]),
        gpu(DGPU, false, false, &["HDMI-A-1"]),
    ];
    let (id, automatic, reason) = picked(select(&c, Some(None)));
    assert_eq!(id, DGPU);
    assert!(automatic);
    assert_eq!(reason, "override not found");
}

#[test]
fn no_candidates_gives_nogpu() {
    assert_eq!(select(&[], None), Selection::NoGpu);
    assert_eq!(select(&[], Some(Some(1))), Selection::NoGpu);
}

#[test]
fn an_override_naming_a_device_that_is_not_there_falls_back_to_auto() {
    let c = [gpu(IGPU, true, false, &["eDP-1"])];
    let (id, automatic, reason) = picked(select(&c, Some(Some(99))));
    assert_eq!(id, IGPU);
    assert!(automatic);
    assert_eq!(reason, "override not found");
}
