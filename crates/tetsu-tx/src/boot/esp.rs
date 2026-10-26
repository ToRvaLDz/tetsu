//! The ESP as the state store: `<esp>/EFI/Linux` is read, never cached.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::BootError;
use super::entry::{BootState, EntryName, FileKind, parse_file_name, tmp_file_name};
use crate::fault::{self, Point};

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
        self.entries
            .iter()
            .rev()
            .find(|e| e.state() != BootState::Bad)
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
        Self {
            dir: esp_root.join("EFI").join("Linux"),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write `tetsu-<counter>.efi.tmp` through `write` (which receives the
    /// path to create) and fsync it. Refuses a counter that is not newer
    /// than every committed entry.
    pub fn stage<F>(&self, counter: u64, write: F) -> Result<PathBuf, BootError>
    where
        F: FnOnce(&Path) -> Result<(), BootError>,
    {
        self.stage_with(counter, write, &mut |p| sync_file(p))
    }

    /// [`Esp::stage`] with the fsync injected, so tests can see which paths
    /// are synced.
    pub(crate) fn stage_with<F>(
        &self,
        counter: u64,
        write: F,
        sync: &mut dyn FnMut(&Path) -> Result<(), BootError>,
    ) -> Result<PathBuf, BootError>
    where
        F: FnOnce(&Path) -> Result<(), BootError>,
    {
        let state = self.scan()?;
        if let Some(newest) = state.newest_counter()
            && counter <= newest
        {
            return Err(BootError::StaleCounter { counter, newest });
        }
        fs::create_dir_all(&self.dir).map_err(|e| BootError::io(&self.dir, e))?;
        // Make the new directory entries durable: `EFI` in the root, `Linux` in `EFI`.
        for parent in self.dir.ancestors().skip(1).take(2) {
            sync(parent)?;
        }
        let tmp = self.dir.join(tmp_file_name(counter));
        let written = write(&tmp).and_then(|()| sync(&tmp));
        if let Err(e) = written {
            // Best effort: a half-written stage must not linger as an orphan.
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(tmp)
    }

    /// Rename the staged file to `tetsu-<counter>+3.efi` and fsync the
    /// directory. Returns the committed path.
    pub fn commit(&self, counter: u64) -> Result<PathBuf, BootError> {
        self.commit_with(counter, &mut |p| sync_file(p))
    }

    pub(crate) fn commit_with(
        &self,
        counter: u64,
        sync: &mut dyn FnMut(&Path) -> Result<(), BootError>,
    ) -> Result<PathBuf, BootError> {
        let tmp = self.dir.join(tmp_file_name(counter));
        let dest = self
            .dir
            .join(EntryName::untried(counter, BOOT_TRIES).file_name());
        fs::rename(&tmp, &dest).map_err(|e| BootError::io(&tmp, e))?;
        fault::reach(Point::Renamed);
        // On FAT the renamed entry gets its size only when the file itself is
        // synced; the directory fsync alone can commit an entry of size 0.
        sync(&dest)?;
        sync(&self.dir)?;
        fault::reach(Point::Committed);
        Ok(dest)
    }

    /// Read the directory afresh. A missing `EFI/Linux` is an empty state.
    pub fn scan(&self) -> Result<EspState, BootError> {
        let mut state = EspState::default();
        let read = match fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(state),
            Err(e) => return Err(BootError::io(&self.dir, e)),
        };
        for item in read {
            let item = item.map_err(|e| BootError::io(&self.dir, e))?;
            // Non-UTF-8 names cannot be ours.
            let Some(file_name) = item.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(kind) = parse_file_name(&file_name) else {
                continue;
            };
            let path = item.path();
            let meta = item.metadata().map_err(|e| BootError::io(&path, e))?;
            if !meta.is_file() {
                continue;
            }
            let size = meta.len();
            match kind {
                FileKind::Entry(name) => state.entries.push(EspEntry { name, path, size }),
                FileKind::Tmp(counter) => state.orphan_tmps.push(OrphanTmp {
                    counter,
                    path,
                    size,
                }),
            }
        }
        state
            .entries
            .sort_by(|a, b| (a.name.counter, &a.path).cmp(&(b.name.counter, &b.path)));
        state.orphan_tmps.sort_by_key(|t| t.counter);
        Ok(state)
    }
}

/// fsync a file or a directory.
pub(crate) fn sync_file(path: &Path) -> Result<(), BootError> {
    fs::File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| BootError::io(path, e))
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

    #[test]
    fn stage_leaves_only_the_tmp() {
        let (_r, esp) = esp_with(&[("tetsu-1.efi", 1)]);
        let path = esp
            .stage(2, |p| fs::write(p, b"uki").map_err(|e| BootError::io(p, e)))
            .unwrap();
        assert_eq!(path, esp.dir().join("tetsu-2.efi.tmp"));
        assert_eq!(fs::read(&path).unwrap(), b"uki");
        let st = esp.scan().unwrap();
        assert_eq!(counters(&st), vec![1]);
        assert_eq!(st.orphan_tmps.len(), 1);
        assert_eq!(st.orphan_tmps[0].counter, 2);
    }

    #[test]
    fn stage_creates_the_directory() {
        let root = tempfile::tempdir().unwrap();
        let esp = Esp::open(root.path());
        esp.stage(1, |p| fs::write(p, b"x").map_err(|e| BootError::io(p, e)))
            .unwrap();
        assert!(esp.dir().join("tetsu-1.efi.tmp").is_file());
    }

    #[test]
    fn commit_yields_untried_with_three_tries() {
        let (_r, esp) = esp_with(&[("tetsu-1.efi", 1)]);
        esp.stage(2, |p| fs::write(p, b"uki").map_err(|e| BootError::io(p, e)))
            .unwrap();
        let path = esp.commit(2).unwrap();
        assert_eq!(path, esp.dir().join("tetsu-2+3.efi"));
        assert_eq!(fs::read(&path).unwrap(), b"uki");
        let st = esp.scan().unwrap();
        assert!(st.orphan_tmps.is_empty());
        let e = st.entries.last().unwrap();
        assert_eq!(e.name, EntryName::untried(2, BOOT_TRIES));
        assert_eq!(e.state(), BootState::Untried);
        assert_eq!(st.default_entry().unwrap().name.counter, 2);
    }

    #[test]
    fn stale_counter_is_refused_and_nothing_is_written() {
        let (_r, esp) = esp_with(&[("tetsu-5+0-3.efi", 1), ("tetsu-3.efi", 1)]);
        for counter in [3, 5] {
            let err = esp
                .stage(counter, |_| panic!("must not write"))
                .unwrap_err();
            assert!(matches!(err, BootError::StaleCounter { newest: 5, .. }));
        }
        assert!(esp.scan().unwrap().orphan_tmps.is_empty());
    }

    #[test]
    fn stage_syncs_the_file_and_the_parents_of_a_created_directory() {
        let root = tempfile::tempdir().unwrap();
        let esp = Esp::open(root.path());
        let mut calls: Vec<PathBuf> = Vec::new();
        let tmp = esp
            .stage_with(
                1,
                |p| fs::write(p, b"x").map_err(|e| BootError::io(p, e)),
                &mut |p| {
                    calls.push(p.to_path_buf());
                    Ok(())
                },
            )
            .unwrap();
        assert!(calls.contains(&root.path().to_path_buf()), "{calls:?}");
        assert!(calls.contains(&root.path().join("EFI")), "{calls:?}");
        assert!(calls.contains(&tmp), "{calls:?}");
        // Directory entries are made durable before the file is written.
        let file_at = calls.iter().position(|c| *c == tmp).unwrap();
        let root_at = calls.iter().position(|c| c == root.path()).unwrap();
        assert!(root_at < file_at);
    }

    #[test]
    fn stage_surfaces_a_parent_sync_error_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let esp = Esp::open(root.path());
        let res = esp.stage_with(1, |_| panic!("must not write"), &mut |p| {
            if p == root.path() {
                Err(BootError::io(p, std::io::Error::other("sync failed")))
            } else {
                Ok(())
            }
        });
        assert!(matches!(res, Err(BootError::Io { .. })), "{res:?}");
    }

    #[test]
    fn commit_syncs_the_committed_file_then_the_directory() {
        // On FAT the size of a renamed file reaches its new directory entry
        // only when the file itself is synced: the directory fsync alone can
        // leave a committed entry of size 0.
        let (_r, esp) = esp_with(&[]);
        esp.stage(1, |p| fs::write(p, b"x").map_err(|e| BootError::io(p, e)))
            .unwrap();
        let mut calls: Vec<PathBuf> = Vec::new();
        let dest = esp
            .commit_with(1, &mut |p| {
                calls.push(p.to_path_buf());
                Ok(())
            })
            .unwrap();
        assert_eq!(calls, vec![dest, esp.dir().to_path_buf()]);
    }

    #[test]
    fn failed_write_leaves_no_tmp() {
        let (_r, esp) = esp_with(&[]);
        let err = esp
            .stage(1, |p| {
                fs::write(p, b"partial").unwrap();
                Err(BootError::ToolMissing {
                    tool: "ukify".into(),
                })
            })
            .unwrap_err();
        assert!(matches!(err, BootError::ToolMissing { .. }));
        assert!(esp.scan().unwrap().orphan_tmps.is_empty());
    }

    #[test]
    fn commit_without_stage_fails() {
        let (_r, esp) = esp_with(&[]);
        assert!(matches!(esp.commit(1), Err(BootError::Io { .. })));
    }
}
