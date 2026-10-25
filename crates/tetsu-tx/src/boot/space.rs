//! Free-space check on the ESP, done before anything is deleted.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use super::BootError;
use super::esp::sync_file;
use super::retention::{GcPlan, GcSplit};

/// Slack kept free on top of the incoming entry (FAT metadata, rounding).
pub const SPACE_MARGIN: u64 = 1024 * 1024;

/// Source of the free-space figure; faked in tests.
pub trait SpaceProbe {
    fn available_bytes(&self, path: &Path) -> Result<u64, BootError>;
}

/// `statvfs`-backed probe.
#[derive(Clone, Copy, Debug, Default)]
pub struct StatvfsProbe;

impl SpaceProbe for StatvfsProbe {
    fn available_bytes(&self, path: &Path) -> Result<u64, BootError> {
        let st = rustix::fs::statvfs(path).map_err(|e| BootError::io(path, e.into()))?;
        Ok(st.f_bavail.saturating_mul(st.f_frsize))
    }
}

/// Pass when the free space plus what `plan` frees covers `needed` plus the
/// margin. Deletes nothing.
pub fn check_space(
    probe: &dyn SpaceProbe,
    dir: &Path,
    plan: &GcPlan,
    needed: u64,
) -> Result<(), BootError> {
    let available = probe.available_bytes(dir)?.saturating_add(plan.freed_bytes);
    let required = needed.saturating_add(SPACE_MARGIN);
    if available >= required {
        Ok(())
    } else {
        Err(BootError::InsufficientSpace {
            needed: required,
            available,
        })
    }
}

pub(crate) fn apply_gc_with(
    plan: &GcPlan,
    sync: &mut dyn FnMut(&Path) -> Result<(), BootError>,
) -> Result<(), BootError> {
    let _ = sync;
    apply_gc(plan)
}

/// Delete everything in `plan`. Call only after [`check_space`] passed.
pub fn apply_gc(plan: &GcPlan) -> Result<(), BootError> {
    let mut dirs = BTreeSet::new();
    for path in &plan.remove {
        match fs::remove_file(path) {
            Ok(()) => {
                if let Some(parent) = path.parent() {
                    dirs.insert(parent.to_path_buf());
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(BootError::io(path, e)),
        }
    }
    // Make the deletions durable before the caller relies on the space.
    for dir in dirs {
        sync_file(&dir)?;
    }
    Ok(())
}

/// Like [`make_room`], but deletes as little as possible before the build:
/// `split.now` always, then entries of `split.later` (oldest first) only
/// while the space is still short. The space check counts everything GC
/// could free and runs before any deletion. Returns the part of `later`
/// left undeleted, for the caller to apply once the new entry is committed.
pub fn reserve_space(
    probe: &dyn SpaceProbe,
    dir: &Path,
    split: &GcSplit,
    needed: u64,
) -> Result<GcPlan, BootError> {
    let free_now = probe.available_bytes(dir)?;
    let later_total: u64 = split.later.iter().map(|i| i.bytes).sum();
    let everything = GcPlan {
        remove: Vec::new(),
        freed_bytes: split.now.freed_bytes.saturating_add(later_total),
    };
    check_space(probe, dir, &everything, needed)?;

    let required = needed.saturating_add(SPACE_MARGIN);
    let mut short = required.saturating_sub(free_now.saturating_add(split.now.freed_bytes));
    let mut early = split.now.clone();
    let mut rest = GcPlan::default();
    for item in &split.later {
        if short > 0 {
            short = short.saturating_sub(item.bytes);
            early.remove.push(item.path.clone());
            early.freed_bytes += item.bytes;
        } else {
            rest.remove.push(item.path.clone());
            rest.freed_bytes += item.bytes;
        }
    }
    apply_gc(&early)?;
    Ok(rest)
}

/// [`check_space`], then [`apply_gc`]: nothing is deleted on failure.
pub fn make_room(
    probe: &dyn SpaceProbe,
    dir: &Path,
    plan: &GcPlan,
    needed: u64,
) -> Result<(), BootError> {
    check_space(probe, dir, plan, needed)?;
    apply_gc(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::esp::Esp;
    use crate::boot::retention::plan_gc;
    use std::fs;
    use std::path::PathBuf;

    struct Fake(u64);
    impl SpaceProbe for Fake {
        fn available_bytes(&self, _: &Path) -> Result<u64, BootError> {
            Ok(self.0)
        }
    }

    fn esp(files: &[(&str, usize)]) -> (tempfile::TempDir, Esp) {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("EFI/Linux");
        fs::create_dir_all(&dir).unwrap();
        for (n, s) in files {
            fs::write(dir.join(n), vec![0u8; *s]).unwrap();
        }
        let e = Esp::open(root.path());
        (root, e)
    }

    fn listing(e: &Esp) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(e.dir())
            .unwrap()
            .map(|d| d.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn passes_when_free_space_alone_suffices() {
        let plan = GcPlan::default();
        let need = 500;
        assert!(check_space(&Fake(need + SPACE_MARGIN), Path::new("/"), &plan, need).is_ok());
    }

    #[test]
    fn gc_freed_bytes_count() {
        let plan = GcPlan {
            remove: vec![],
            freed_bytes: 400,
        };
        assert!(check_space(&Fake(SPACE_MARGIN + 100), Path::new("/"), &plan, 500).is_ok());
        assert!(check_space(&Fake(SPACE_MARGIN + 99), Path::new("/"), &plan, 500).is_err());
    }

    #[test]
    fn insufficient_reports_and_deletes_nothing() {
        let (_r, e) = esp(&[
            ("tetsu-1+0-3.efi", 100),
            ("tetsu-2.efi", 100),
            ("tetsu-3.efi.tmp", 50),
        ]);
        let before = listing(&e);
        let plan = plan_gc(&e.scan().unwrap(), Some(2));
        let err = make_room(&Fake(10), e.dir(), &plan, 10 * SPACE_MARGIN).unwrap_err();
        match err {
            BootError::InsufficientSpace { needed, available } => {
                assert_eq!(needed, 11 * SPACE_MARGIN);
                assert_eq!(available, 10 + 150);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(listing(&e), before);
    }

    #[test]
    fn enough_after_gc_deletes_the_plan() {
        let (_r, e) = esp(&[
            ("tetsu-1+0-3.efi", 100),
            ("tetsu-2.efi", 100),
            ("tetsu-3.efi.tmp", 50),
        ]);
        let plan = plan_gc(&e.scan().unwrap(), Some(2));
        make_room(&Fake(SPACE_MARGIN), e.dir(), &plan, 150).unwrap();
        assert_eq!(listing(&e), vec!["tetsu-2.efi"]);
    }

    /// Records the directories it is asked to sync; fails on `fail_on`.
    struct SyncLog {
        calls: Vec<PathBuf>,
        fail_on: Option<PathBuf>,
    }

    impl SyncLog {
        fn new(fail_on: Option<PathBuf>) -> Self {
            Self {
                calls: Vec::new(),
                fail_on,
            }
        }

        fn sync(&mut self, dir: &Path) -> Result<(), BootError> {
            self.calls.push(dir.to_path_buf());
            if self.fail_on.as_deref() == Some(dir) {
                return Err(BootError::io(dir, std::io::Error::other("sync failed")));
            }
            Ok(())
        }
    }

    #[test]
    fn apply_gc_syncs_each_touched_directory_once() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a"), d.path().join("b"));
        let other = d.path().join("sub");
        fs::create_dir(&other).unwrap();
        let c = other.join("c");
        for f in [&a, &b, &c] {
            fs::write(f, b"x").unwrap();
        }
        let plan = GcPlan {
            remove: vec![a.clone(), b.clone(), c.clone()],
            freed_bytes: 3,
        };
        let mut log = SyncLog::new(None);
        apply_gc_with(&plan, &mut |p| log.sync(p)).unwrap();
        assert!(!a.exists() && !b.exists() && !c.exists());
        let mut calls = log.calls.clone();
        calls.sort();
        assert_eq!(calls, vec![d.path().to_path_buf(), other]);
    }

    #[test]
    fn apply_gc_skips_the_sync_when_nothing_was_removed() {
        let plan = GcPlan {
            remove: vec!["/nonexistent-tetsu-test/x.efi".into()],
            freed_bytes: 0,
        };
        let mut log = SyncLog::new(None);
        apply_gc_with(&plan, &mut |p| log.sync(p)).unwrap();
        assert!(log.calls.is_empty());
    }

    #[test]
    fn apply_gc_surfaces_a_sync_error() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("a");
        fs::write(&f, b"x").unwrap();
        let plan = GcPlan {
            remove: vec![f],
            freed_bytes: 1,
        };
        let mut log = SyncLog::new(Some(d.path().to_path_buf()));
        match apply_gc_with(&plan, &mut |p| log.sync(p)).unwrap_err() {
            BootError::GcFailed { failures } => {
                assert_eq!(failures.len(), 1);
                assert!(matches!(failures[0], BootError::Io { .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn one_failing_removal_does_not_stop_the_others() {
        let d = tempfile::tempdir().unwrap();
        let (first, last) = (d.path().join("1"), d.path().join("3"));
        // unlink of a directory fails for every uid, root included.
        let stuck = d.path().join("2");
        fs::write(&first, b"x").unwrap();
        fs::create_dir(&stuck).unwrap();
        fs::write(&last, b"x").unwrap();
        let plan = GcPlan {
            remove: vec![first.clone(), stuck.clone(), last.clone()],
            freed_bytes: 2,
        };
        let mut log = SyncLog::new(None);
        match apply_gc_with(&plan, &mut |p| log.sync(p)).unwrap_err() {
            BootError::GcFailed { failures } => {
                assert_eq!(failures.len(), 1);
                assert!(
                    matches!(&failures[0], BootError::Io { path, .. } if *path == stuck),
                    "{failures:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(!first.exists() && !last.exists() && stuck.exists());
        // The deletions that did happen are still made durable.
        assert_eq!(log.calls, vec![d.path().to_path_buf()]);
    }

    #[test]
    fn apply_gc_tolerates_already_missing_files() {
        let plan = GcPlan {
            remove: vec!["/nonexistent-tetsu-test/x.efi".into()],
            freed_bytes: 0,
        };
        assert!(apply_gc(&plan).is_ok());
    }

    #[test]
    fn statvfs_probe_reads_a_real_directory() {
        let d = tempfile::tempdir().unwrap();
        assert!(StatvfsProbe.available_bytes(d.path()).unwrap() > 0);
        assert!(
            StatvfsProbe
                .available_bytes(&d.path().join("nope"))
                .is_err()
        );
    }
}
