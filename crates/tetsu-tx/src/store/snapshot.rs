//! Snapshot lifecycle: layout creation for transaction zero, snapshot of a
//! source with its nested `etc`, sealing and deletion. Only `tetsu-tx`
//! creates and deletes snapshots, so the nested `etc` is handled here
//! explicitly: Btrfs snapshots are not recursive.

use std::fs;
use std::io::ErrorKind;

use super::StoreError;
use super::btrfs::Btrfs;
use super::layout::{Layout, SHARED};

/// Counter of the first snapshot, created by the installer.
pub const FIRST_COUNTER: u64 = 1;

/// Where `/var/lib/pacman` points: the database lives in the snapshot.
pub const PACMAN_DB: &str = "/usr/lib/pacman";

/// Create the layout of transaction zero on an empty top level:
/// `@snapshots`, the shared subvolumes, `@snapshots/1/root` with its nested
/// `etc`, and the `/var/lib/pacman` compatibility symlink in `@var`. The
/// payload is extracted afterwards, then [`capture_etc_base`] runs.
pub fn init_layout(layout: &Layout, btrfs: &dyn Btrfs) -> Result<u64, StoreError> {
    let _ = (layout, btrfs);
    todo!()
}

/// Take the read-only `etc-base` of `counter` from its own `etc`.
pub fn capture_etc_base(
    layout: &Layout,
    btrfs: &dyn Btrfs,
    counter: u64,
) -> Result<(), StoreError> {
    let _ = (layout, btrfs, counter);
    todo!()
}

/// Create snapshot `counter` from `source`: a writable root, a writable
/// clone of the source `etc` in place of the empty directory the root
/// snapshot leaves, and a read-only `etc-base`, inherited from the source
/// when `inherit_etc_base` (the source is a pending default), else taken
/// from the source `etc`.
pub fn create_snapshot(
    layout: &Layout,
    btrfs: &dyn Btrfs,
    counter: u64,
    source: u64,
    inherit_etc_base: bool,
) -> Result<(), StoreError> {
    let _ = (layout, btrfs, counter, source, inherit_etc_base);
    todo!()
}

/// Seal snapshot `counter`: `ro` on the root (the nested `etc` stays
/// writable), then commit the filesystem.
pub fn seal(layout: &Layout, btrfs: &dyn Btrfs, counter: u64) -> Result<(), StoreError> {
    let _ = (layout, btrfs, counter);
    todo!()
}

/// Delete snapshot `counter`, sealed or partial (an interrupted create):
/// unseal the root, delete the nested `etc`, the root and `etc-base`, then
/// the directory.
pub fn delete_snapshot(layout: &Layout, btrfs: &dyn Btrfs, counter: u64) -> Result<(), StoreError> {
    let _ = (layout, btrfs, counter);
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::btrfs::tests::FakeBtrfs;
    use std::path::Path;

    fn setup() -> (tempfile::TempDir, Layout, FakeBtrfs) {
        let top = tempfile::tempdir().unwrap();
        let layout = Layout::new(top.path());
        let btrfs = FakeBtrfs::new(top.path());
        (top, layout, btrfs)
    }

    /// A sealed snapshot `n` as transaction zero would leave it.
    fn sealed(layout: &Layout, b: &FakeBtrfs, n: u64) {
        fs::create_dir_all(layout.snapshot_dir(n)).unwrap();
        b.adopt(&layout.root(n));
        b.adopt(&layout.etc(n));
        b.adopt(&layout.etc_base(n));
        b.set_readonly(&layout.root(n), true).unwrap();
        b.set_readonly(&layout.etc_base(n), true).unwrap();
        b.take_ops();
    }

    #[test]
    fn init_layout_creates_every_subvolume() {
        let (_t, l, b) = setup();
        assert_eq!(init_layout(&l, &b).unwrap(), 1);
        assert_eq!(
            b.take_ops(),
            [
                "create @snapshots",
                "create @var",
                "create @home",
                "create @root",
                "create @swap",
                "create @snapshots/1/root",
                "create @snapshots/1/root/etc",
            ]
        );
        let link = fs::read_link(l.shared("@var").join("lib/pacman")).unwrap();
        assert_eq!(link, Path::new(PACMAN_DB));
        assert_eq!(l.list_snapshots().unwrap(), vec![1]);
    }

    #[test]
    fn init_layout_refuses_an_existing_layout() {
        let (_t, l, b) = setup();
        fs::create_dir(l.shared("@home")).unwrap();
        assert!(matches!(
            init_layout(&l, &b),
            Err(StoreError::LayoutExists { .. })
        ));
        assert!(b.take_ops().is_empty());
    }

    #[test]
    fn capture_etc_base_is_read_only() {
        let (_t, l, b) = setup();
        init_layout(&l, &b).unwrap();
        b.take_ops();
        capture_etc_base(&l, &b, 1).unwrap();
        assert_eq!(
            b.take_ops(),
            ["snapshot -r @snapshots/1/root/etc @snapshots/1/etc-base"]
        );
    }

    #[test]
    fn create_from_booted_takes_etc_base_from_its_etc() {
        let (_t, l, b) = setup();
        sealed(&l, &b, 4);
        create_snapshot(&l, &b, 5, 4, false).unwrap();
        assert_eq!(
            b.take_ops(),
            [
                "snapshot @snapshots/4/root @snapshots/5/root",
                "snapshot @snapshots/4/root/etc @snapshots/5/root/etc",
                "snapshot -r @snapshots/4/root/etc @snapshots/5/etc-base",
            ]
        );
        assert_eq!(b.readonly(&l.root(5)), Some(false));
        assert_eq!(b.readonly(&l.etc(5)), Some(false));
        assert_eq!(b.readonly(&l.etc_base(5)), Some(true));
    }

    #[test]
    fn create_from_pending_default_inherits_its_etc_base() {
        let (_t, l, b) = setup();
        sealed(&l, &b, 6);
        create_snapshot(&l, &b, 7, 6, true).unwrap();
        assert_eq!(
            b.take_ops().last().unwrap(),
            "snapshot -r @snapshots/6/etc-base @snapshots/7/etc-base"
        );
    }

    #[test]
    fn create_refuses_an_existing_counter() {
        let (_t, l, b) = setup();
        sealed(&l, &b, 1);
        sealed(&l, &b, 2);
        assert!(create_snapshot(&l, &b, 2, 1, false).is_err());
        assert!(b.take_ops().is_empty());
    }

    #[test]
    fn create_from_missing_source_fails() {
        let (_t, l, b) = setup();
        assert!(matches!(
            create_snapshot(&l, &b, 2, 1, false),
            Err(StoreError::NotFound(1))
        ));
        assert!(!l.snapshot_dir(2).exists());
    }

    #[test]
    fn seal_sets_root_ro_and_syncs() {
        let (_t, l, b) = setup();
        sealed(&l, &b, 1);
        create_snapshot(&l, &b, 2, 1, false).unwrap();
        b.take_ops();
        seal(&l, &b, 2).unwrap();
        assert_eq!(b.take_ops(), ["ro @snapshots/2/root true", "sync "]);
        assert_eq!(b.readonly(&l.root(2)), Some(true));
        assert_eq!(b.readonly(&l.etc(2)), Some(false));
    }

    #[test]
    fn delete_handles_the_nested_etc() {
        let (_t, l, b) = setup();
        sealed(&l, &b, 3);
        delete_snapshot(&l, &b, 3).unwrap();
        assert_eq!(
            b.take_ops(),
            [
                "ro @snapshots/3/root false",
                "delete @snapshots/3/root/etc",
                "delete @snapshots/3/root",
                "delete @snapshots/3/etc-base",
            ]
        );
        assert!(!l.snapshot_dir(3).exists());
    }

    #[test]
    fn delete_a_partial_snapshot() {
        let (_t, l, b) = setup();
        sealed(&l, &b, 1);
        // Interrupted right after the root snapshot: `etc` is still the
        // empty placeholder directory, no `etc-base`.
        fs::create_dir(l.snapshot_dir(2)).unwrap();
        b.snapshot(&l.root(1), &l.root(2), false).unwrap();
        b.take_ops();
        delete_snapshot(&l, &b, 2).unwrap();
        assert_eq!(
            b.take_ops(),
            ["ro @snapshots/2/root false", "delete @snapshots/2/root"]
        );
        assert!(!l.snapshot_dir(2).exists());
    }

    #[test]
    fn delete_an_empty_snapshot_dir() {
        let (_t, l, b) = setup();
        fs::create_dir_all(l.snapshot_dir(9)).unwrap();
        delete_snapshot(&l, &b, 9).unwrap();
        assert!(b.take_ops().is_empty());
        assert!(!l.snapshot_dir(9).exists());
    }

    #[test]
    fn delete_missing_is_not_found() {
        let (_t, l, b) = setup();
        assert!(matches!(
            delete_snapshot(&l, &b, 9),
            Err(StoreError::NotFound(9))
        ));
    }
}
