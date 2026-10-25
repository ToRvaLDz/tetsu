//! Recognise an operating system already present on a disk.
//!
//! Looking inside a partition needs a mount, so it sits behind [`OsProbe`];
//! the lsblk metadata (filesystem type, label) is handled here.

use std::path::Path;

use super::Partition;

/// Looks inside a partition. Production code mounts it read-only; tests
/// fake it.
pub trait OsProbe {
    /// Directory names under `EFI/` of a FAT partition.
    fn efi_vendors(&self, partition: &Path) -> Vec<String>;

    /// `PRETTY_NAME` (or `NAME`) from the partition's `/etc/os-release`.
    fn os_release(&self, partition: &Path) -> Option<String>;
}

/// Probe that never looks inside a partition: only lsblk metadata counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullProbe;

impl OsProbe for NullProbe {
    fn efi_vendors(&self, _partition: &Path) -> Vec<String> {
        Vec::new()
    }

    fn os_release(&self, _partition: &Path) -> Option<String> {
        None
    }
}

/// Label fragments that name an operating system.
const OS_LABELS: &[&str] = &[
    "arch",
    "ubuntu",
    "debian",
    "fedora",
    "nixos",
    "opensuse",
    "mint",
    "manjaro",
    "windows",
    "macintosh hd",
    "macos",
];

/// Directories of `EFI/` that belong to no particular system.
const GENERIC_EFI_DIRS: &[&str] = &["boot"];

fn vendor_name(dir: &str) -> String {
    match dir.to_ascii_lowercase().as_str() {
        "microsoft" => "Windows".to_owned(),
        _ => dir.to_owned(),
    }
}

/// Name found in one partition and how specific it is (lower wins).
fn recognise(part: &Partition, probe: &dyn OsProbe) -> Option<(u8, String)> {
    let fstype = part.fstype.as_deref().unwrap_or("");
    match fstype {
        "vfat" => probe
            .efi_vendors(&part.path)
            .into_iter()
            .find(|d| !GENERIC_EFI_DIRS.contains(&d.to_ascii_lowercase().as_str()))
            .map(|d| (0, vendor_name(&d))),
        "ext4" | "btrfs" | "xfs" | "f2fs" => probe
            .os_release(&part.path)
            .map(|n| (0, n))
            .or_else(|| label_hint(part).map(|l| (1, l))),
        "ntfs" => Some((2, "Windows (NTFS)".to_owned())),
        "apfs" | "hfsplus" => Some((2, "macOS".to_owned())),
        _ => None,
    }
}

fn label_hint(part: &Partition) -> Option<String> {
    let label = part.label.as_deref()?;
    let lower = label.to_ascii_lowercase();
    OS_LABELS
        .iter()
        .any(|k| lower.contains(k))
        .then(|| label.to_owned())
}

/// Name of the operating system held by `partitions`, if one is recognised.
pub fn detect(partitions: &[Partition], probe: &dyn OsProbe) -> Option<String> {
    partitions
        .iter()
        .filter_map(|p| recognise(p, probe))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, name)| name)
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Probe answering from fixed tables keyed by partition path.
    #[derive(Default)]
    pub struct FakeProbe {
        pub efi: HashMap<PathBuf, Vec<String>>,
        pub release: HashMap<PathBuf, String>,
    }

    impl OsProbe for FakeProbe {
        fn efi_vendors(&self, p: &Path) -> Vec<String> {
            self.efi.get(p).cloned().unwrap_or_default()
        }

        fn os_release(&self, p: &Path) -> Option<String> {
            self.release.get(p).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeProbe;
    use super::*;

    fn part(path: &str, fstype: Option<&str>, label: Option<&str>) -> Partition {
        Partition {
            path: path.into(),
            size: 1 << 30,
            fstype: fstype.map(str::to_owned),
            label: label.map(str::to_owned),
        }
    }

    #[test]
    fn esp_with_microsoft_is_windows() {
        let mut probe = FakeProbe::default();
        probe
            .efi
            .insert("/dev/sda1".into(), vec!["BOOT".into(), "Microsoft".into()]);
        let parts = [part("/dev/sda1", Some("vfat"), None)];
        assert_eq!(detect(&parts, &probe).as_deref(), Some("Windows"));
    }

    #[test]
    fn esp_with_other_vendor_uses_its_directory_name() {
        let mut probe = FakeProbe::default();
        probe.efi.insert("/dev/sda1".into(), vec!["ubuntu".into()]);
        let parts = [part("/dev/sda1", Some("vfat"), None)];
        assert_eq!(detect(&parts, &probe).as_deref(), Some("ubuntu"));
    }

    #[test]
    fn esp_with_only_the_fallback_loader_is_not_an_os() {
        let mut probe = FakeProbe::default();
        probe.efi.insert("/dev/sda1".into(), vec!["BOOT".into()]);
        let parts = [part("/dev/sda1", Some("vfat"), None)];
        assert_eq!(detect(&parts, &probe), None);
    }

    #[test]
    fn ntfs_is_windows_but_the_esp_name_wins() {
        let parts = [part("/dev/sda2", Some("ntfs"), None)];
        assert_eq!(
            detect(&parts, &NullProbe).as_deref(),
            Some("Windows (NTFS)")
        );
        let mut probe = FakeProbe::default();
        probe
            .efi
            .insert("/dev/sda1".into(), vec!["Microsoft".into()]);
        let both = [parts[0].clone(), part("/dev/sda1", Some("vfat"), None)];
        assert_eq!(detect(&both, &probe).as_deref(), Some("Windows"));
    }

    #[test]
    fn linux_root_found_by_os_release_or_label() {
        let mut probe = FakeProbe::default();
        probe
            .release
            .insert("/dev/sda3".into(), "Fedora Linux 42".into());
        let parts = [part("/dev/sda3", Some("btrfs"), None)];
        assert_eq!(detect(&parts, &probe).as_deref(), Some("Fedora Linux 42"));
        let labelled = [part("/dev/sda3", Some("ext4"), Some("Arch root"))];
        assert_eq!(detect(&labelled, &NullProbe).as_deref(), Some("Arch root"));
    }

    #[test]
    fn plain_data_is_not_an_os() {
        let parts = [
            part("/dev/sda1", Some("ext4"), Some("backup")),
            part("/dev/sda2", Some("crypto_LUKS"), None),
            part("/dev/sda3", None, None),
        ];
        assert_eq!(detect(&parts, &NullProbe), None);
    }
}
