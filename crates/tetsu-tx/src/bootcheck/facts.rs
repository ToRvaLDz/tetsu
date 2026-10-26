//! What the boot check looks at, and the parsers that read it off the
//! output of the system.

use std::collections::BTreeSet;

use crate::etcsync::mountinfo::unescape;

/// State of `greetd.service`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Greetd {
    /// Not installed (the payload has no greetd yet): not checked.
    NotInstalled,
    Active,
    /// Installed but in this `ActiveState`.
    NotActive(String),
}

/// The facts the checks are evaluated on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Mount points of the running system.
    pub mount_points: BTreeSet<String>,
    /// Units in the `failed` state.
    pub failed_units: Vec<String>,
    pub greetd: Greetd,
}

/// The mount points in `/proc/self/mountinfo`.
pub fn mount_points(mountinfo: &str) -> BTreeSet<String> {
    mountinfo
        .lines()
        .filter_map(|line| line.split_whitespace().nth(4))
        .map(unescape)
        .collect()
}

/// The unit names in the output of
/// `systemctl list-units --state=failed --plain --no-legend`.
pub fn failed_units(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            line.split_whitespace()
                .find(|w| !matches!(*w, "●" | "*" | "×"))
        })
        .map(str::to_owned)
        .collect()
}

/// The state in the output of
/// `systemctl show -p LoadState,ActiveState greetd.service`.
pub fn greetd(show: &str) -> Greetd {
    let field = |key: &str| {
        show.lines()
            .find_map(|l| l.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
    };
    match (field("LoadState"), field("ActiveState")) {
        (Some("loaded"), Some("active")) => Greetd::Active,
        (Some("loaded"), Some(state)) => Greetd::NotActive(state.to_owned()),
        // Not installed, masked, or output we cannot read.
        _ => Greetd::NotInstalled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_points_come_from_the_fifth_field() {
        let info = "\
21 27 0:20 / /proc rw - proc proc rw
30 27 0:31 /@snapshots/2/root / rw shared:1 - btrfs /dev/vda2 rw
31 30 0:32 /@var /var rw shared:2 - btrfs /dev/vda2 rw
32 30 0:33 / /mnt/my\\040disk rw - ext4 /dev/vdb rw
";
        let points = mount_points(info);
        for p in ["/", "/var", "/proc", "/mnt/my disk"] {
            assert!(points.contains(p), "{p}");
        }
        assert_eq!(points.len(), 4);
        assert!(mount_points("").is_empty());
    }

    #[test]
    fn failed_units_are_the_first_column() {
        let listing = "\
tetsu-keyring-init.service loaded failed failed Initialise the pacman keyring
  systemd-timesyncd.service loaded failed failed Network Time Synchronization
● foo.service loaded failed failed Foo
";
        assert_eq!(
            failed_units(listing),
            [
                "tetsu-keyring-init.service",
                "systemd-timesyncd.service",
                "foo.service"
            ]
        );
        assert!(failed_units("").is_empty());
    }

    #[test]
    fn greetd_is_read_from_the_load_and_active_state() {
        let show = |load: &str, active: &str| format!("LoadState={load}\nActiveState={active}\n");
        assert_eq!(greetd(&show("loaded", "active")), Greetd::Active);
        assert_eq!(
            greetd(&show("loaded", "failed")),
            Greetd::NotActive("failed".into())
        );
        assert_eq!(greetd(&show("not-found", "inactive")), Greetd::NotInstalled);
        assert_eq!(greetd(&show("masked", "inactive")), Greetd::NotInstalled);
        // Output we cannot read is not a reason to fail the boot.
        assert_eq!(greetd(""), Greetd::NotInstalled);
    }
}
