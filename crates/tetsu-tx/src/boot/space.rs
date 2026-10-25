//! Free-space check on the ESP, done before anything is deleted.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use super::BootError;
use super::retention::GcPlan;

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

/// Delete everything in `plan`. Call only after [`check_space`] passed.
pub fn apply_gc(plan: &GcPlan) -> Result<(), BootError> {
    for path in &plan.remove {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(BootError::io(path, e)),
        }
    }
    Ok(())
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
