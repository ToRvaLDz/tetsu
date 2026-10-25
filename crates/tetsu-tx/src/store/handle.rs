//! The store facade: lock, orphan GC, source selection and snapshot
//! creation at transaction start; sealing; retention. Snapshot GC and
//! retention need the lock, so they take the guard as proof.

use std::path::Path;

use super::StoreError;
use super::btrfs::Btrfs;
use super::layout::Layout;
use super::lock::TxLock;
use super::record::Records;
use super::select::{self, Source};
use super::snapshot;
use crate::boot::BootError;
use crate::boot::esp::EspState;

/// A transaction that has its new, writable snapshot.
#[derive(Debug)]
pub struct Begun {
    pub lock: TxLock,
    pub counter: u64,
    pub source: Source,
    /// The ESP as read under the lock: the base of every later decision.
    pub esp: EspState,
    /// Orphans that could not be deleted; the transaction went on anyway,
    /// since the next counter skips them.
    pub gc_failures: Vec<StoreError>,
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
        let snapshots = self.layout.list_snapshots()?;
        // A cut between the ESP rename and the marker leaves an entry with
        // no marker: repair it, best effort, like a missing record.
        for e in &esp.entries {
            let n = e.name.counter;
            if snapshots.contains(&n) && !self.layout.is_committed(n) {
                let _repaired = snapshot::mark_committed(&self.layout, n);
            }
        }
        let committed: Vec<u64> = snapshots
            .iter()
            .copied()
            .filter(|&n| self.layout.is_committed(n))
            .collect();
        let records = self.records.load_or_reconstruct(esp)?;
        let doomed = select::orphans(&snapshots, &committed, &records, esp, booted);
        self.delete_all(doomed)
    }

    /// Start a transaction: take the lock, read the ESP through `read_esp`
    /// (only now, so no other transaction can change it underneath), collect
    /// orphans, pick the source, allocate the next counter and create its
    /// writable snapshot. A failed create leaves no partial snapshot behind.
    pub fn begin<F>(&self, lock_path: &Path, read_esp: F, booted: u64) -> Result<Begun, StoreError>
    where
        F: FnOnce() -> Result<EspState, BootError>,
    {
        self.start(lock_path, read_esp, booted, |esp| {
            Ok(select::select_source(esp, booted))
        })
    }

    /// Like [`Store::begin`], with the source chosen by `pick` from the ESP
    /// read under the lock, so a rollback target is judged on state no other
    /// transaction can change. A failed `pick` leaves nothing behind.
    pub fn begin_picking<F, P>(
        &self,
        lock_path: &Path,
        read_esp: F,
        booted: u64,
        pick: P,
    ) -> Result<Begun, StoreError>
    where
        F: FnOnce() -> Result<EspState, BootError>,
        P: FnOnce(&EspState) -> Result<Source, StoreError>,
    {
        self.start(lock_path, read_esp, booted, pick)
    }

    /// The body of the `begin` family; `pick` chooses the source from the ESP
    /// read under the lock.
    fn start<F, P>(
        &self,
        lock_path: &Path,
        read_esp: F,
        booted: u64,
        pick: P,
    ) -> Result<Begun, StoreError>
    where
        F: FnOnce() -> Result<EspState, BootError>,
        P: FnOnce(&EspState) -> Result<Source, StoreError>,
    {
        let lock = TxLock::acquire(lock_path)?;
        let esp = read_esp()?;
        let gc_failures = match self.gc_orphans(&lock, &esp, Some(booted)) {
            Ok(_) => Vec::new(),
            Err(StoreError::Cleanup { failures }) => failures,
            Err(e) => return Err(e),
        };
        let source = pick(&esp)?;
        let snapshots = self.layout.list_snapshots()?;
        let records = self.records.load()?;
        let counter = select::next_counter(&snapshots, &records, &esp);
        let created = snapshot::create_snapshot(
            &self.layout,
            self.btrfs,
            counter,
            source.counter,
            source.inherit_etc_base,
        );
        if let Err(e) = created {
            // Best effort: the next GC would collect it anyway.
            let _cleanup = snapshot::delete_snapshot(&self.layout, self.btrfs, counter);
            return Err(e);
        }
        Ok(Begun {
            lock,
            counter,
            source,
            esp,
            gc_failures,
        })
    }

    /// Mark the snapshot of a begun transaction committed: call it right
    /// after the ESP rename, before writing the record.
    pub fn mark_committed(&self, begun: &Begun) -> Result<(), StoreError> {
        snapshot::mark_committed(&self.layout, begun.counter)
    }

    /// Seal the snapshot of a begun transaction.
    pub fn seal(&self, begun: &Begun) -> Result<(), StoreError> {
        snapshot::seal(&self.layout, self.btrfs, begun.counter)
    }

    /// Delete the snapshots retention lets go: outside the newest 10 and
    /// neither booted, default, newest good, last good (the implicit
    /// rollback target) nor still on the ESP. Returns the deleted
    /// counters; failures are collected and the rest still attempted.
    pub fn apply_retention(
        &self,
        _lock: &TxLock,
        esp: &EspState,
        booted: u64,
    ) -> Result<Vec<u64>, StoreError> {
        let snapshots = self.layout.list_snapshots()?;
        let records = self.records.load_or_reconstruct(esp)?;
        let on_esp = esp.entries.iter().map(|e| e.name.counter);
        let protected: Vec<u64> = [
            Some(booted),
            select::default_counter(esp),
            select::newest_good(&snapshots, &records),
            select::last_good(&snapshots, &records, booted),
        ]
        .into_iter()
        .flatten()
        .chain(on_esp)
        .collect();
        let doomed = select::plan_retention(&snapshots, &protected, select::KEEP_SNAPSHOTS);
        self.delete_all(doomed)
    }

    /// Delete every counter in `doomed`, collecting failures.
    fn delete_all(&self, doomed: Vec<u64>) -> Result<Vec<u64>, StoreError> {
        let mut deleted = Vec::new();
        let mut failures = Vec::new();
        for n in doomed {
            match snapshot::delete_snapshot(&self.layout, self.btrfs, n) {
                Ok(()) => deleted.push(n),
                Err(e) => failures.push(e),
            }
        }
        if failures.is_empty() {
            Ok(deleted)
        } else {
            Err(StoreError::Cleanup { failures })
        }
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
                    ..Record::default()
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
        let begun = env
            .store()
            .begin(&env.lock_path, || Ok(st.clone()), 3)
            .unwrap();
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
    fn begin_a_pending_default_inherits_etc_base() {
        let env = Env::new();
        env.committed(3, G);
        env.committed(4, None);
        let st = esp(&["tetsu-3.efi", "tetsu-4+3.efi"], &[]);
        let begun = env
            .store()
            .begin(&env.lock_path, || Ok(st.clone()), 3)
            .unwrap();
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
        env.store()
            .begin(&env.lock_path, || Ok(st.clone()), 3)
            .unwrap();
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
            env.store().begin(&env.lock_path, || Ok(st.clone()), 1),
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
        assert!(
            env.store()
                .begin(&env.lock_path, || Ok(st.clone()), 1)
                .is_err()
        );
        assert_eq!(env.snapshots(), vec![1]);
        TxLock::acquire(&env.lock_path).unwrap();
    }

    fn pick(counter: u64, inherit_etc_base: bool) -> Source {
        Source {
            counter,
            inherit_etc_base,
        }
    }

    #[test]
    fn begin_picking_snapshots_the_explicit_source() {
        let env = Env::new();
        env.committed(1, G);
        env.committed(2, B);
        env.committed(3, G);
        let st = esp(&["tetsu-1.efi", "tetsu-3.efi"], &[]);
        let begun = env
            .store()
            .begin_picking(&env.lock_path, || Ok(st.clone()), 3, |_| Ok(pick(1, false)))
            .unwrap();
        assert_eq!(begun.counter, 4);
        assert_eq!(begun.source, pick(1, false));
        assert_eq!(begun.esp, st);
        assert!(
            env.btrfs
                .take_ops()
                .contains(&"snapshot @snapshots/1/root @snapshots/4/root".to_owned())
        );
        assert_eq!(env.snapshots(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn begin_picking_collects_orphans_and_reads_the_esp_under_the_lock() {
        let env = Env::new();
        env.committed(1, G);
        env.partial(2);
        let st = esp(&["tetsu-1.efi"], &[2]);
        let lock_path = env.lock_path.clone();
        let read = || {
            assert!(matches!(
                TxLock::acquire(&lock_path),
                Err(StoreError::Busy { .. })
            ));
            Ok(st.clone())
        };
        let begun = env
            .store()
            .begin_picking(&env.lock_path, read, 1, |_| Ok(pick(1, false)))
            .unwrap();
        assert_eq!(begun.counter, 3);
        assert_eq!(env.snapshots(), vec![1, 3]);
    }

    #[test]
    fn begin_picking_an_unknown_snapshot_is_not_found_and_leaves_nothing() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        assert!(matches!(
            env.store()
                .begin_picking(&env.lock_path, || Ok(st.clone()), 1, |_| Ok(pick(9, false))),
            Err(StoreError::NotFound(9))
        ));
        assert_eq!(env.snapshots(), vec![1]);
        TxLock::acquire(&env.lock_path).unwrap();
    }

    #[test]
    fn begin_picking_is_busy_while_another_transaction_runs() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        let _held = TxLock::acquire(&env.lock_path).unwrap();
        assert!(matches!(
            env.store()
                .begin_picking(&env.lock_path, || Ok(st.clone()), 1, |_| Ok(pick(1, false))),
            Err(StoreError::Busy { .. })
        ));
    }

    #[test]
    fn seal_makes_the_new_root_read_only() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        let store = env.store();
        let begun = store.begin(&env.lock_path, || Ok(st.clone()), 1).unwrap();
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

    #[test]
    fn begin_reads_the_esp_under_the_lock() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        let lock_path = env.lock_path.clone();
        let read = || {
            assert!(
                matches!(TxLock::acquire(&lock_path), Err(StoreError::Busy { .. })),
                "the ESP must be read while the lock is held"
            );
            Ok(st.clone())
        };
        let begun = env.store().begin(&env.lock_path, read, 1).unwrap();
        assert_eq!(begun.esp, st);
    }

    #[test]
    fn an_undeletable_orphan_does_not_block_the_transaction() {
        let env = Env::new();
        env.committed(1, G);
        env.partial(2);
        // A stray file keeps the directory from going away.
        fs::write(env.layout.snapshot_dir(2).join("stray"), b"x").unwrap();
        let st = esp(&["tetsu-1.efi"], &[]);
        let begun = env
            .store()
            .begin(&env.lock_path, || Ok(st.clone()), 1)
            .unwrap();
        assert_eq!(begun.gc_failures.len(), 1);
        assert_eq!(begun.counter, 3);
    }

    #[test]
    fn a_committed_snapshot_survives_lost_records_and_a_collected_uki() {
        let env = Env::new();
        env.committed(1, G);
        env.committed(2, B);
        snapshot::mark_committed(&env.layout, 2).unwrap();
        fs::remove_file(env.records.path(2)).unwrap();
        // The Bad entry 2 already left the ESP.
        let st = esp(&["tetsu-1.efi"], &[]);
        let begun = env
            .store()
            .begin(&env.lock_path, || Ok(st.clone()), 1)
            .unwrap();
        assert_eq!(begun.counter, 3);
        assert_eq!(env.snapshots(), vec![1, 2, 3]);
    }

    #[test]
    fn retention_keeps_last_good_and_every_esp_entry() {
        let env = Env::new();
        env.committed(1, G);
        env.committed(2, G);
        for n in 3..=12 {
            env.committed(n, B);
        }
        env.committed(13, G);
        // Booted 13 is good, so newest good is 13; last good is 2. The ESP
        // still holds 1 as a previous good entry.
        let st = esp(&["tetsu-1.efi", "tetsu-2.efi", "tetsu-13.efi"], &[]);
        let lock = TxLock::acquire(&env.lock_path).unwrap();
        let deleted = env.store().apply_retention(&lock, &st, 13).unwrap();
        assert_eq!(deleted, vec![3]);
    }

    #[test]
    fn gc_repairs_the_marker_of_snapshots_on_the_esp() {
        let env = Env::new();
        env.committed(1, G);
        // Cut between the ESP rename and the marker.
        env.committed(2, None);
        let st = esp(&["tetsu-1.efi", "tetsu-2+3.efi"], &[]);
        let lock = TxLock::acquire(&env.lock_path).unwrap();
        env.store().gc_orphans(&lock, &st, Some(1)).unwrap();
        assert!(env.layout.is_committed(2));
    }

    #[test]
    fn begin_picking_uses_the_source_picked_under_the_lock() {
        let env = Env::new();
        env.committed(1, G);
        env.committed(2, G);
        let st = esp(&["tetsu-1.efi", "tetsu-2.efi"], &[]);
        let lock_path = env.lock_path.clone();
        let chosen = |seen: &EspState| {
            assert!(matches!(
                TxLock::acquire(&lock_path),
                Err(StoreError::Busy { .. })
            ));
            assert_eq!(seen.entries.len(), 2);
            Ok(pick(1, false))
        };
        let begun = env
            .store()
            .begin_picking(&env.lock_path, || Ok(st.clone()), 2, chosen)
            .unwrap();
        assert_eq!(begun.counter, 3);
        assert_eq!(begun.source, pick(1, false));
    }

    #[test]
    fn a_failed_pick_leaves_nothing_and_releases_the_lock() {
        let env = Env::new();
        env.committed(1, G);
        let st = esp(&["tetsu-1.efi"], &[]);
        let result = env.store().begin_picking(
            &env.lock_path,
            || Ok(st.clone()),
            1,
            |_| Err(StoreError::NotFound(7)),
        );
        assert!(matches!(result, Err(StoreError::NotFound(7))));
        assert_eq!(env.snapshots(), vec![1]);
        assert!(env.btrfs.take_ops().is_empty());
        TxLock::acquire(&env.lock_path).unwrap();
    }
}
