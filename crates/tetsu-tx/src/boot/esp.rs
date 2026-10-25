//! The ESP as the state store: `<esp>/EFI/Linux` is read, never cached.

use std::path::{Path, PathBuf};

use super::BootError;
use super::entry::{BootState, EntryName};

/// Boot attempts granted to a freshly committed entry.
pub const BOOT_TRIES: u32 = 3;

/// A committed Tetsu UKI found on the ESP.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EspEntry {
    pub name: EntryName,
    pub path: PathBuf,
    pub size: u64,
}

impl EspEntry {
    pub fn state(&self) -> BootState {
        self.name.state()
    }
}

/// A staged `*.efi.tmp` left behind by an interrupted write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanTmp {
    pub counter: u64,
    pub path: PathBuf,
    pub size: u64,
}

/// Snapshot of the Tetsu files in `EFI/Linux`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EspState {
    /// Committed entries, sorted by counter ascending.
    pub entries: Vec<EspEntry>,
    pub orphan_tmps: Vec<OrphanTmp>,
}

impl EspState {
    /// The entry systemd-boot picks: the newest one that is not Bad.
    pub fn default_entry(&self) -> Option<&EspEntry> {
        todo!()
    }

    pub fn newest_counter(&self) -> Option<u64> {
        self.entries.last().map(|e| e.name.counter)
    }
}

/// Handle on the `EFI/Linux` directory of an ESP.
#[derive(Clone, Debug)]
pub struct Esp {
    dir: PathBuf,
}

impl Esp {
    pub fn open(esp_root: &Path) -> Self {
        todo!()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn scan(&self) -> Result<EspState, BootError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn esp_with(files: &[(&str, usize)]) -> (tempfile::TempDir, Esp) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("EFI/Linux");
        fs::create_dir_all(&dir).unwrap();
        for (name, size) in files {
            fs::write(dir.join(name), vec![0u8; *size]).unwrap();
        }
        let esp = Esp::open(root.path());
        (root, esp)
    }

    fn counters(state: &EspState) -> Vec<u64> {
        state.entries.iter().map(|e| e.name.counter).collect()
    }

    #[test]
    fn scan_sorts_entries_and_reports_sizes() {
        let (_r, esp) = esp_with(&[
            ("tetsu-10+3.efi", 30),
            ("tetsu-2.efi", 10),
            ("tetsu-9+2-1.efi", 20),
        ]);
        let st = esp.scan().unwrap();
        assert_eq!(counters(&st), vec![2, 9, 10]);
        assert_eq!(st.entries[0].size, 10);
        assert_eq!(st.entries[2].path, esp.dir().join("tetsu-10+3.efi"));
        assert_eq!(st.entries[1].state(), BootState::Trying);
    }

    #[test]
    fn foreign_files_are_ignored() {
        let (_r, esp) = esp_with(&[
            ("fedora-6.1.efi", 5),
            ("tetsu-1.efi", 5),
            ("notes.txt", 1),
            ("tetsu-3.efi.bak", 1),
        ]);
        let st = esp.scan().unwrap();
        assert_eq!(counters(&st), vec![1]);
        assert!(st.orphan_tmps.is_empty());
    }

    #[test]
    fn orphan_tmp_is_listed_but_not_an_entry() {
        let (_r, esp) = esp_with(&[("tetsu-1.efi", 5), ("tetsu-2.efi.tmp", 7)]);
        let st = esp.scan().unwrap();
        assert_eq!(counters(&st), vec![1]);
        assert_eq!(st.orphan_tmps.len(), 1);
        assert_eq!(st.orphan_tmps[0].counter, 2);
        assert_eq!(st.orphan_tmps[0].size, 7);
        assert_eq!(st.default_entry().unwrap().name.counter, 1);
    }

    #[test]
    fn default_is_newest_non_bad() {
        let (_r, esp) = esp_with(&[
            ("tetsu-1.efi", 1),
            ("tetsu-2.efi", 1),
            ("tetsu-3+0-3.efi", 1),
            ("tetsu-4+0-3.efi", 1),
        ]);
        let st = esp.scan().unwrap();
        assert_eq!(st.default_entry().unwrap().name.counter, 2);
    }

    #[test]
    fn default_prefers_newer_untried_over_older_good() {
        let (_r, esp) = esp_with(&[("tetsu-1.efi", 1), ("tetsu-2+3.efi", 1)]);
        let st = esp.scan().unwrap();
        assert_eq!(st.default_entry().unwrap().name.counter, 2);
    }

    #[test]
    fn all_bad_or_empty_has_no_default() {
        let (_r, esp) = esp_with(&[("tetsu-1+0-3.efi", 1)]);
        assert!(esp.scan().unwrap().default_entry().is_none());
        let (_r, esp) = esp_with(&[]);
        assert!(esp.scan().unwrap().default_entry().is_none());
    }

    #[test]
    fn missing_efi_linux_scans_empty() {
        let root = tempfile::tempdir().unwrap();
        let st = Esp::open(root.path()).scan().unwrap();
        assert_eq!(st, EspState::default());
    }
}
