//! Battery and network readers on tempdir trees shaped like `/sys` and `/proc`.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use tempfile::TempDir;
use tetsu_bar::sources::battery::{self, Battery, Status};
use tetsu_bar::sources::network::{self, Network};

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn supply(root: &Path, name: &str, kind: &str, capacity: &str, status: &str) {
    let dir = format!("sys/class/power_supply/{name}");
    write(root, &format!("{dir}/type"), &format!("{kind}\n"));
    write(root, &format!("{dir}/capacity"), &format!("{capacity}\n"));
    write(root, &format!("{dir}/status"), &format!("{status}\n"));
}

#[test]
fn one_battery() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "BAT0", "Battery", "85", "Discharging");
    supply(root.path(), "AC", "Mains", "1", "Unknown");
    let read = battery::read(root.path()).unwrap();
    assert_eq!(
        read,
        Battery {
            percent: 85,
            status: Status::Discharging
        }
    );
    assert_eq!(read.label(), "BAT 85%");
}

#[test]
fn charging_shows_a_plus() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "BAT0", "Battery", "40", "Charging");
    assert_eq!(battery::read(root.path()).unwrap().label(), "BAT 40%+");
}

#[test]
fn two_batteries_average_and_charging_wins() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "BAT0", "Battery", "80", "Discharging");
    supply(root.path(), "BAT1", "Battery", "41", "Charging");
    let read = battery::read(root.path()).unwrap();
    assert_eq!(read.percent, 61);
    assert_eq!(read.status, Status::Charging);
}

#[test]
fn discharging_beats_full() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "BAT0", "Battery", "100", "Full");
    supply(root.path(), "BAT1", "Battery", "90", "Discharging");
    assert_eq!(
        battery::read(root.path()).unwrap().status,
        Status::Discharging
    );
}

#[test]
fn other_statuses_count_as_full() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "BAT0", "Battery", "100", "Not charging");
    assert_eq!(battery::read(root.path()).unwrap().status, Status::Full);
}

#[test]
fn no_battery_gives_none() {
    let root = TempDir::new().unwrap();
    assert_eq!(battery::read(root.path()), None);
    supply(root.path(), "AC", "Mains", "1", "Unknown");
    assert_eq!(battery::read(root.path()), None);
}

#[test]
fn a_malformed_capacity_skips_that_supply() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "BAT0", "Battery", "n/a", "Discharging");
    supply(root.path(), "BAT1", "Battery", "50", "Discharging");
    assert_eq!(battery::read(root.path()).unwrap().percent, 50);
    let only_bad = TempDir::new().unwrap();
    supply(only_bad.path(), "BAT0", "Battery", "", "Discharging");
    assert_eq!(battery::read(only_bad.path()), None);
}

#[test]
fn device_batteries_are_not_the_laptop() {
    let root = TempDir::new().unwrap();
    supply(root.path(), "hid-mouse", "Battery", "10", "Discharging");
    write(
        root.path(),
        "sys/class/power_supply/hid-mouse/scope",
        "Device\n",
    );
    assert_eq!(battery::read(root.path()), None);
}

fn link(root: &Path, name: &str, state: &str, wifi: bool, physical: bool) {
    let dir = root.join("sys/class/net").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("operstate"), format!("{state}\n")).unwrap();
    if physical {
        // The real `device` is a symlink; a dangling one still counts.
        symlink("../../../devices/pci", dir.join("device")).unwrap();
    }
    if wifi {
        fs::create_dir_all(dir.join("wireless")).unwrap();
    }
}

const WIRELESS: &str = "\
Inter-| sta-|   Quality        |   Discarded packets               | Missed | WE
 face | tus | link level noise |  nwid  crypt   frag  retry   misc | beacon | 22
wlan0: 0000   35.  -75.  -256        0      0      0      0      0        0
";

#[test]
fn wired_up_beats_wifi_up() {
    let root = TempDir::new().unwrap();
    link(root.path(), "wlan0", "up", true, true);
    link(root.path(), "enp3s0", "up", false, true);
    assert_eq!(
        network::read(root.path()),
        Network::Wired {
            iface: "enp3s0".into()
        }
    );
    assert_eq!(network::read(root.path()).label(), "eth enp3s0");
}

#[test]
fn wifi_quality_comes_from_proc_net_wireless() {
    let root = TempDir::new().unwrap();
    link(root.path(), "wlan0", "up", true, true);
    write(root.path(), "proc/net/wireless", WIRELESS);
    let read = network::read(root.path());
    assert_eq!(
        read,
        Network::Wifi {
            iface: "wlan0".into(),
            quality: Some(50)
        }
    );
    assert_eq!(read.label(), "wifi 50%");
}

#[test]
fn wifi_without_a_quality_line_shows_the_interface() {
    let root = TempDir::new().unwrap();
    link(root.path(), "wlan0", "up", true, true);
    assert_eq!(network::read(root.path()).label(), "wifi wlan0");
}

#[test]
fn wifi_quality_is_capped_at_a_hundred() {
    let root = TempDir::new().unwrap();
    link(root.path(), "wlan0", "up", true, true);
    write(
        root.path(),
        "proc/net/wireless",
        "wlan0: 0000   90.  -40.  -256 0 0 0 0 0 0\n",
    );
    let Network::Wifi { quality, .. } = network::read(root.path()) else {
        panic!("expected wifi");
    };
    assert_eq!(quality, Some(100));
}

#[test]
fn phy80211_marks_wifi_too() {
    let root = TempDir::new().unwrap();
    link(root.path(), "wlan0", "up", false, true);
    fs::create_dir_all(root.path().join("sys/class/net/wlan0/phy80211")).unwrap();
    assert!(matches!(network::read(root.path()), Network::Wifi { .. }));
}

#[test]
fn loopback_and_virtual_interfaces_are_ignored() {
    let root = TempDir::new().unwrap();
    link(root.path(), "lo", "unknown", false, false);
    link(root.path(), "docker0", "up", false, false);
    link(root.path(), "veth1", "up", false, false);
    link(root.path(), "tun0", "up", false, false);
    assert_eq!(network::read(root.path()), Network::Offline);
}

#[test]
fn a_physical_lo_is_still_loopback() {
    let root = TempDir::new().unwrap();
    link(root.path(), "lo", "up", false, true);
    assert_eq!(network::read(root.path()), Network::Offline);
}

#[test]
fn all_down_is_offline() {
    let root = TempDir::new().unwrap();
    link(root.path(), "enp3s0", "down", false, true);
    link(root.path(), "wlan0", "dormant", true, true);
    let read = network::read(root.path());
    assert_eq!(read, Network::Offline);
    assert_eq!(read.label(), "offline");
}

#[test]
fn a_missing_class_net_is_offline() {
    let root = TempDir::new().unwrap();
    assert_eq!(network::read(root.path()), Network::Offline);
}
