//! Where the installation happens: the disk, the payload on the medium and
//! every directory the executor works in.

use std::path::{Path, PathBuf};

use tetsu_tx::store::layout::Layout;
use tetsu_tx::store::record::RECORDS_DIR;
use tetsu_tx::tx::nspawn::DEFAULT_CACHE;

use crate::preflight::SecureBoot;
use crate::secureboot::MicrosoftCerts;

/// Default private directory for secret files and scratch data.
pub const DEFAULT_RUN_DIR: &str = "/run/tetsu-installer";
/// Name of the LUKS mapping the root filesystem is opened under.
pub const MAPPER_NAME: &str = "root";
/// Filesystem label of the ESP.
pub const ESP_LABEL: &str = "TETSU-ESP";
/// Partition label of the LUKS2 partition.
pub const ROOT_LABEL: &str = "TETSU-ROOT";
/// Label of the Btrfs filesystem.
pub const BTRFS_LABEL: &str = "tetsu";
/// Where the keys live inside the target, below the `@var` subvolume.
pub const KEYS_SUBDIR: &str = "lib/tetsu/keys";

/// Paths and machine facts the plan needs besides the answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallTarget {
    /// The disk to wipe, for example `/dev/nvme0n1`.
    pub disk: PathBuf,
    /// The payload tarball on the install medium.
    pub payload: PathBuf,
    /// Mount points live below this directory.
    pub work_dir: PathBuf,
    /// Private directory (mode 0700) for secret files and scratch data.
    pub run_dir: PathBuf,
    /// Package cache bound into the initrd build container.
    pub cache_dir: PathBuf,
    pub microsoft_certs: MicrosoftCerts,
    /// Enroll Tetsu's keys alone even when `microsoft_certs` is empty. The
    /// installer never sets it: without the certificates the enrollment
    /// is refused and the install takes the off path.
    pub without_microsoft: bool,
    /// Firmware state found by the preflight.
    pub secure_boot: SecureBoot,
    /// `console=` parameters of the installer's own kernel command line,
    /// carried over so a machine driven over a serial line stays so.
    pub console_args: Vec<String>,
}

impl InstallTarget {
    pub fn new(disk: &Path, payload: &Path, work_dir: &Path, secure_boot: SecureBoot) -> Self {
        Self {
            disk: disk.to_path_buf(),
            payload: payload.to_path_buf(),
            work_dir: work_dir.to_path_buf(),
            run_dir: PathBuf::from(DEFAULT_RUN_DIR),
            cache_dir: PathBuf::from(DEFAULT_CACHE),
            microsoft_certs: MicrosoftCerts::default(),
            without_microsoft: false,
            secure_boot,
            console_args: Vec::new(),
        }
    }

    /// Mount point of the Btrfs top level (subvolid 5).
    pub fn top(&self) -> PathBuf {
        self.work_dir.join("top")
    }

    /// Mount point of the ESP.
    pub fn esp(&self) -> PathBuf {
        self.work_dir.join("esp")
    }

    pub fn layout(&self) -> Layout {
        Layout::new(&self.top())
    }

    /// The key directory inside the target (`/var/lib/tetsu/keys`).
    pub fn keys_dir(&self) -> PathBuf {
        self.layout().shared("@var").join(KEYS_SUBDIR)
    }

    /// The transaction records directory inside the target.
    pub fn records_dir(&self) -> PathBuf {
        self.layout().shared("@var").join(
            RECORDS_DIR
                .trim_start_matches('/')
                .trim_start_matches("var/"),
        )
    }

    /// Device node of partition `n` (1-based): `p` is inserted when the
    /// disk name ends in a digit (`nvme0n1p1`, `mmcblk0p1`, `sda1`).
    pub fn partition(&self, n: u32) -> PathBuf {
        let name = self.disk.to_string_lossy();
        let sep = if name.ends_with(|c: char| c.is_ascii_digit()) {
            "p"
        } else {
            ""
        };
        PathBuf::from(format!("{name}{sep}{n}"))
    }

    pub fn esp_partition(&self) -> PathBuf {
        self.partition(1)
    }

    pub fn root_partition(&self) -> PathBuf {
        self.partition(2)
    }

    /// The opened LUKS mapping.
    pub fn mapper(&self) -> PathBuf {
        Path::new("/dev/mapper").join(MAPPER_NAME)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(disk: &str) -> InstallTarget {
        InstallTarget::new(
            Path::new(disk),
            Path::new("/run/medium/payload.tar"),
            Path::new("/mnt/work"),
            SecureBoot::Off,
        )
    }

    #[test]
    fn partition_nodes_follow_the_disk_name() {
        assert_eq!(target("/dev/sda").esp_partition(), Path::new("/dev/sda1"));
        assert_eq!(
            target("/dev/nvme0n1").root_partition(),
            Path::new("/dev/nvme0n1p2")
        );
        assert_eq!(
            target("/dev/mmcblk0").esp_partition(),
            Path::new("/dev/mmcblk0p1")
        );
    }

    #[test]
    fn directories_hang_off_the_work_dir_and_the_layout() {
        let t = target("/dev/sda");
        assert_eq!(t.top(), Path::new("/mnt/work/top"));
        assert_eq!(t.esp(), Path::new("/mnt/work/esp"));
        assert_eq!(t.keys_dir(), Path::new("/mnt/work/top/@var/lib/tetsu/keys"));
        assert_eq!(
            t.records_dir(),
            Path::new("/mnt/work/top/@var/lib/tetsu/transactions")
        );
        assert_eq!(t.mapper(), Path::new("/dev/mapper/root"));
        assert_eq!(t.run_dir, Path::new(DEFAULT_RUN_DIR));
    }
}
