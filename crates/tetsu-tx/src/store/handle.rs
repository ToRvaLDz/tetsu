//! The store facade: lock, orphan GC, source selection and snapshot
//! creation at transaction start; sealing; retention. Snapshot GC and
//! retention need the lock, so they take the guard as proof.

use std::path::Path;

use super::StoreError;
use super::btrfs::Btrfs;
use super::layout::Layout;
use super::lock::TxLock;
use super::record::Records;
use super::select::Source;
use crate::boot::esp::EspState;

/// A transaction that has its new, writable snapshot.
#[derive(Debug)]
pub struct Begun {
    pub lock: TxLock,
    pub counter: u64,
    pub source: Source,
}

/// The snapshot store over one top-level mount.
pub struct Store<'a> {
    layout: Layout,
    records: Records,
    btrfs: &'a dyn Btrfs,
}

impl<'a> Store<'a> {
    pub fn new(layout: Layout, records: Records, btrfs: &'a dyn Btrfs) -> Self {
        Self {
            layout,
            records,
            btrfs,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn records(&self) -> &Records {
        &self.records
    }

    /// Delete the orphans of an interrupted transaction. Returns the deleted
    /// counters; failures are collected and the rest still attempted.
    pub fn gc_orphans(
        &self,
        _lock: &TxLock,
        esp: &EspState,
        booted: Option<u64>,
    ) -> Result<Vec<u64>, StoreError> {
        let _ = (esp, booted);
        todo!()
    }

    /// Start a transaction: take the lock, collect orphans, pick the source,
    /// allocate the next counter and create its writable snapshot. A failed
    /// create leaves no partial snapshot behind.
    pub fn begin(
        &self,
        lock_path: &Path,
        esp: &EspState,
        booted: u64,
    ) -> Result<Begun, StoreError> {
        let _ = (lock_path, esp, booted);
        todo!()
    }

    /// Seal the snapshot of a begun transaction.
    pub fn seal(&self, begun: &Begun) -> Result<(), StoreError> {
        let _ = begun;
        todo!()
    }

    /// Delete the snapshots retention lets go: outside the newest 10 and
    /// neither booted, default nor newest good. Returns the deleted
    /// counters; failures are collected and the rest still attempted.
    pub fn apply_retention(
        &self,
        _lock: &TxLock,
        esp: &EspState,
        booted: u64,
    ) -> Result<Vec<u64>, StoreError> {
        let _ = (esp, booted);
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::{EspEntry, OrphanTmp};
    use crate::store::btrfs::tests::FakeBtrfs;
    use crate::store::record::{BootVerdict, Record};
    use std::fs;
    use std::path::PathBuf;

    struct Env {
        _top: tempfile::TempDir,
        layout: Layout,
        records: Records,
        btrfs: FakeBtrfs,
        lock_path: PathBuf,
    }

    impl Env {
        fn new() -> Self {
            let top = tempfile::tempdir().unwrap();
            let layout = Layout::new(&top.path().join("top"));
            fs::create_dir(layout.top()).unwrap();
            let records = Records::new(&top.path().join("records"));
            let btrfs = FakeBtrfs::new(layout.top());
            let lock_path = top.path().join("run/tx.lock");
            Self {
                _top: top,
                layout,
                records,
                btrfs,
                lock_path,
            }
        }

        fn store(&self) -> Store<'_> {
            Store::new(self.layout.clone(), self.records.clone(), &self.btrfs)
        }

        /// Sealed snapshot `n` with a record carrying `boot`.
        fn committed(&self, n: u64, boot: Option<BootVerdict>) {
            self.partial(n);
            self.btrfs.adopt(&self.layout.etc(n));
            self.btrfs.adopt(&self.layout.etc_base(n));
            self.btrfs.set_readonly(&self.layout.root(n), true).unwrap();
            self.records
                .write(&Record {
                    counter: n,
                    kind: None,
                    source: None,
                    boot,
                    reconstructed: false,
                })
                .unwrap();
            self.btrfs.take_ops();
        }

        /// Snapshot `n` interrupted after the root snapshot, no record.
        fn partial(&self, n: u64) {
            fs::create_dir_all(self.layout.snapshot_dir(n)).unwrap();
            self.btrfs.adopt(&self.layout.root(n));
            self.btrfs.take_ops();
        }

        fn snapshots(&self) -> Vec<u64> {
            self.layout.list_snapshots().unwrap()
        }
    }

    fn esp(names: &[&str], tmps: &[u64]) -> EspState {
        let mut st = EspState::default();
        for n in names {
            let Some(FileKind::Entry(name)) = parse_file_name(n) else {
                panic!("{n}")
            };
            st.entries.push(EspEntry {
                name,
                path: PathBuf::from(n),
                size: 1,
            });
        }
        for &counter in tmps {
            st.orphan_tmps.push(OrphanTmp {
                counter,
                path: PathBuf::new(),
                size: 1,
            });
        }
        st
    }

    const G: Option<BootVerdict> = Some(BootVerdict::Good);
    const B: Option<BootVerdict> = Some(BootVerdict::Bad);

    #[test]
    fn begin_collects_orphans_then_snapshots_the_booted_one() {
        let env = Env::new();
        env.committed(2, G);
        env.committed(3, G);
        env.partial(4);
        let st = esp(&["tetsu-2.efi", "tetsu-3.efi"], &[4]);
        let begun = env.store().begin(&env.lock_path, &st, 3).unwrap();
        assert_eq!(begun.counter, 5);
        assert_eq!(
            begun.source,
            Source {
                counter: 3,
                inherit_etc_base: false
            }
        );
        assert_eq!(env.snapshots(), vec![2, 3, 5]);
        assert_eq!(env.btrfs.readonly(&env.layout.root(5)), Some(false));
        assert_eq!(env.btrfs.readonly(&env.layout.etc_base(5)), Some(true));
    }

    #[test]
    fn begin_from_a_pending_default_inherits_etc_base() {
        let env = Env::new();
        env.committed(3, G);
        env.committed(4, None);
        let st = esp(&["tetsu-3.efi", "tetsu-4+3.efi"], &[]);
        let begun = env.store().begin(&env.lock_path, &st, 3).unwrap();
        assert_eq!(
            begun.source,
            Source {
                counter: 4,
                inherit_etc_base: true
            }
        );
        let ops = env.btrfs.take_ops();
        assert_eq!(
            ops.last().unwrap(),
            "snapshot -r @snapshots/4/etc-base @snapshots/5/etc-base"
        );
    }

    #[test]
    fn begin_rebuilds_a_missing_record_instead_of_collecting() {
        let env = Env::new();
        env.committed(3, G);
        // Cut between the commit and the record: 4 is on the ESP only.
        env.committed(4, None);
        fs::remove_file(env.records.path(4)).unwrap();
        let st = esp(&["tetsu-3.efi", "tetsu-4+3.efi"], &[]);
        env.store().begin(&env.lock_path, &st, 3).unwrap();
        assert_eq!(env.snapshots(), vec![3, 4, 5]);
        assert!(env.records.load().unwrap()[&4].reconstructed);
    }

    #[test]
    fn begin_is_busy_while_another_transaction_runs() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        let _held = TxLock::acquire(&env.lock_path).unwrap();
        assert!(matches!(
            env.store().begin(&env.lock_path, &st, 1),
            Err(StoreError::Busy { .. })
        ));
        assert_eq!(env.snapshots(), vec![1]);
    }

    #[test]
    fn failed_create_leaves_nothing_and_releases_the_lock() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        *env.btrfs.fail.borrow_mut() = Some("snapshot");
        assert!(env.store().begin(&env.lock_path, &st, 1).is_err());
        assert_eq!(env.snapshots(), vec![1]);
        TxLock::acquire(&env.lock_path).unwrap();
    }

    #[test]
    fn seal_makes_the_new_root_read_only() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        let store = env.store();
        let begun = store.begin(&env.lock_path, &st, 1).unwrap();
        store.seal(&begun).unwrap();
        assert_eq!(env.btrfs.readonly(&env.layout.root(2)), Some(true));
        assert_eq!(env.btrfs.readonly(&env.layout.etc(2)), Some(false));
    }

    #[test]
    fn retention_keeps_ten_and_the_protected_ones() {
        let env = Env::new();
        env.committed(1, G);
        for n in 2..=13 {
            env.committed(n, B);
        }
        env.committed(14, None);
        let st = esp(&["tetsu-14+2-1.efi"], &[]);
        let lock = TxLock::acquire(&env.lock_path).unwrap();
        let deleted = env.store().apply_retention(&lock, &st, 14).unwrap();
        // 1 is the newest good one; 5..=14 are the newest ten.
        assert_eq!(deleted, vec![2, 3, 4]);
        let mut expected = vec![1];
        expected.extend(5..=14);
        assert_eq!(env.snapshots(), expected);
    }

    #[test]
    fn gc_failures_are_collected() {
        let env = Env::new();
        env.committed(1, G);
        env.partial(2);
        env.partial(3);
        let st = esp(&["tetsu-1.efi"], &[]);
        *env.btrfs.fail.borrow_mut() = Some("delete");
        let lock = TxLock::acquire(&env.lock_path).unwrap();
        match env.store().gc_orphans(&lock, &st, Some(1)) {
            Err(StoreError::Cleanup { failures }) => assert_eq!(failures.len(), 2),
            other => panic!("{other:?}"),
        }
    }
}
