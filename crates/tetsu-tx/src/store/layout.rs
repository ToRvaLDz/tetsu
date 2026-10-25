//! Paths of the Btrfs layout, relative to the top-level (subvolid=5) mount.
//!
//! * `@snapshots/<n>/root`       root snapshot, sealed `ro`
//! * `@snapshots/<n>/root/etc`   nested writable subvolume
//! * `@snapshots/<n>/etc-base`   read-only base of the `/etc` merge
//! * `@snapshots/<n>/committed`  marker: the UKI is committed on the ESP
//! * `@var`, `@home`, `@root`, `@swap`   shared, outside every snapshot

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::StoreError;
use crate::boot::entry::canonical;

/// Subvolume holding every snapshot directory.
pub const SNAPSHOTS: &str = "@snapshots";

/// File name of the commit marker in a snapshot directory.
pub const COMMITTED: &str = "committed";

/// Shared subvolumes, outside every snapshot.
pub const SHARED: [&str; 4] = ["@var", "@home", "@root", "@swap"];

/// The layout under a mounted top level.
#[derive(Clone, Debug)]
pub struct Layout {
    top: PathBuf,
}

impl Layout {
    pub fn new(top: &Path) -> Self {
        Self {
            top: top.to_path_buf(),
        }
    }

    pub fn top(&self) -> &Path {
        &self.top
    }

    pub fn snapshots_dir(&self) -> PathBuf {
        self.top.join(SNAPSHOTS)
    }

    pub fn snapshot_dir(&self, counter: u64) -> PathBuf {
        self.snapshots_dir().join(counter.to_string())
    }

    pub fn root(&self, counter: u64) -> PathBuf {
        self.snapshot_dir(counter).join("root")
    }

    pub fn etc(&self, counter: u64) -> PathBuf {
        self.root(counter).join("etc")
    }

    pub fn etc_base(&self, counter: u64) -> PathBuf {
        self.snapshot_dir(counter).join("etc-base")
    }

    /// Marker written once the snapshot's UKI is committed on the ESP.
    pub fn committed_marker(&self, counter: u64) -> PathBuf {
        self.snapshot_dir(counter).join(COMMITTED)
    }

    pub fn is_committed(&self, counter: u64) -> bool {
        self.committed_marker(counter).is_file()
    }

    pub fn shared(&self, name: &str) -> PathBuf {
        self.top.join(name)
    }

    /// Counters of the snapshot directories, ascending. Anything that is not
    /// a canonical number is ignored; a missing `@snapshots` is empty.
    pub fn list_snapshots(&self) -> Result<Vec<u64>, StoreError> {
        let dir = self.snapshots_dir();
        let read = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(StoreError::io(&dir, e)),
        };
        let mut counters = Vec::new();
        for item in read {
            let item = item.map_err(|e| StoreError::io(&dir, e))?;
            let Some(counter) = item.file_name().to_str().and_then(canonical) else {
                continue;
            };
            let kind = item
                .file_type()
                .map_err(|e| StoreError::io(item.path(), e))?;
            if kind.is_dir() {
                counters.push(counter);
            }
        }
        counters.sort_unstable();
        Ok(counters)
    }
}

/// The `subvol=` value baked into the UKI cmdline for `counter`.
pub fn rootflags_subvol(counter: u64) -> String {
    format!("{SNAPSHOTS}/{counter}/root")
}

/// The snapshot counter the kernel cmdline booted, from
/// `rootflags=subvol=@snapshots/<n>/root`.
pub fn booted_counter(cmdline: &str) -> Option<u64> {
    // The kernel keeps the last occurrence of a parameter.
    let flags = cmdline
        .split_whitespace()
        .filter_map(|p| p.strip_prefix("rootflags="))
        .next_back()?;
    let subvol = flags
        .split(',')
        .filter_map(|o| o.strip_prefix("subvol="))
        .next_back()?;
    let rest = subvol.trim_start_matches('/').strip_prefix(SNAPSHOTS)?;
    let counter = rest.strip_prefix('/')?.strip_suffix("/root")?;
    canonical(counter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_the_layout() {
        let l = Layout::new(Path::new("/mnt/top"));
        assert_eq!(l.root(7), Path::new("/mnt/top/@snapshots/7/root"));
        assert_eq!(l.etc(7), Path::new("/mnt/top/@snapshots/7/root/etc"));
        assert_eq!(l.etc_base(7), Path::new("/mnt/top/@snapshots/7/etc-base"));
        assert_eq!(l.shared("@var"), Path::new("/mnt/top/@var"));
    }

    #[test]
    fn rootflags_names_the_root_subvolume() {
        assert_eq!(rootflags_subvol(12), "@snapshots/12/root");
    }

    #[test]
    fn booted_counter_from_cmdline() {
        let c = "quiet rw rootflags=subvol=@snapshots/12/root root=UUID=x";
        assert_eq!(booted_counter(c), Some(12));
        let c = "rootflags=compress=zstd,subvol=/@snapshots/3/root,noatime";
        assert_eq!(booted_counter(c), Some(3));
    }

    #[test]
    fn booted_counter_last_rootflags_wins() {
        let c = "rootflags=subvol=@snapshots/1/root rootflags=subvol=@snapshots/2/root";
        assert_eq!(booted_counter(c), Some(2));
    }

    #[test]
    fn booted_counter_rejects_foreign_roots() {
        for c in [
            "",
            "root=/dev/sda1",
            "rootflags=subvol=@",
            "rootflags=subvol=@snapshots/1/etc",
            "rootflags=subvol=@snapshots/01/root",
            "rootflags=subvol=@snapshots/x/root",
            "rootflags=subvol=@snapshots/1/root/extra",
            "rootflags=subvolid=5",
        ] {
            assert_eq!(booted_counter(c), None, "{c:?}");
        }
    }

    #[test]
    fn list_snapshots_sorted_and_filtered() {
        let top = tempfile::tempdir().unwrap();
        let l = Layout::new(top.path());
        assert_eq!(l.list_snapshots().unwrap(), Vec::<u64>::new());
        for d in ["10", "2", "9", "x", "03"] {
            fs::create_dir_all(l.snapshots_dir().join(d)).unwrap();
        }
        fs::write(l.snapshots_dir().join("4"), b"").unwrap();
        assert_eq!(l.list_snapshots().unwrap(), vec![2, 9, 10]);
    }
}
