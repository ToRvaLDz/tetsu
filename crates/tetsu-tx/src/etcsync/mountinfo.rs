//! The device behind a mount point, from `/proc/self/mountinfo`.

use super::SyncError;

/// The source device of the btrfs filesystem mounted at `mount_point`; the
/// last mount there wins. Fails when nothing is mounted there or it is not
/// btrfs.
pub fn source_of(mountinfo: &str, mount_point: &str) -> Result<String, SyncError> {
    let _ = (mountinfo, mount_point);
    Err(SyncError::Mountinfo("stub".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = "\
21 27 0:20 / /proc rw,nosuid shared:5 - proc proc rw
30 27 0:31 /@snapshots/2/root /sysroot rw,relatime shared:1 - btrfs /dev/mapper/root rw,ssd,subvolid=257,subvol=/@snapshots/2/root
31 30 0:32 / /sysroot/efi rw - vfat /dev/vda1 rw
";

    #[test]
    fn the_device_of_a_btrfs_mount_is_found() {
        assert_eq!(source_of(INFO, "/sysroot").unwrap(), "/dev/mapper/root");
    }

    #[test]
    fn the_last_mount_at_a_point_wins() {
        let info = format!("{INFO}40 27 0:40 / /sysroot rw - btrfs /dev/sdb1 rw\n");
        assert_eq!(source_of(&info, "/sysroot").unwrap(), "/dev/sdb1");
    }

    #[test]
    fn a_missing_mount_or_another_filesystem_is_an_error() {
        assert!(source_of(INFO, "/nowhere").is_err());
        assert!(source_of(INFO, "/sysroot/efi").is_err());
        assert!(source_of("", "/sysroot").is_err());
        assert!(source_of("garbage line\n", "/sysroot").is_err());
    }

    #[test]
    fn escaped_characters_are_decoded() {
        let info = "30 27 0:31 / /sys\\040root rw - btrfs /dev/disk/by-label/my\\040disk rw\n";
        assert_eq!(
            source_of(info, "/sys root").unwrap(),
            "/dev/disk/by-label/my disk"
        );
    }
}
