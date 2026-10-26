//! The first boot of a transaction's snapshot: find out whether the `/etc`
//! sync is due, run it and leave the evidence.

use std::fmt;
use std::fs::{self, File};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::SyncError;
use super::apply::{SyncReport, private_dir, sync};
use crate::store::layout::Layout;
use crate::store::record::{Records, TxKind};

/// Where the reports and kept copies go, relative to the shared `@var`.
pub const OUT_DIR: &str = "lib/tetsu/etc-sync";
const KEPT: &str = "kept";
const REPORT: &str = "report.json";

/// Why the sync did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// The kernel command line names no Tetsu snapshot.
    NotTetsu,
    NoRecord,
    /// The record was rebuilt from the ESP: nothing says what to sync.
    Reconstructed,
    /// A rollback deployment gets the accounts merge of the transaction
    /// itself, not a general sync.
    Rollback,
    NoSource,
    /// The marker of an earlier, complete sync is there.
    AlreadySynced,
    /// A tree the sync needs is missing.
    MissingTree(String),
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotTetsu => f.write_str("not booted from a Tetsu snapshot"),
            Self::NoRecord => f.write_str("no transaction record"),
            Self::Reconstructed => f.write_str("the record was rebuilt from the ESP"),
            Self::Rollback => f.write_str("a rollback"),
            Self::NoSource => f.write_str("the snapshot has no source snapshot"),
            Self::AlreadySynced => f.write_str("the marker is present"),
            Self::MissingTree(p) => write!(f, "{p} is missing"),
        }
    }
}

/// What a first-boot run came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Synced(SyncReport),
    Skipped(Skip),
}

/// `report.json`: the sync report with the snapshots it is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredReport {
    pub counter: u64,
    /// The snapshot whose `/etc` was the user's side.
    pub from: u64,
    #[serde(flatten)]
    pub report: SyncReport,
}

/// Run the sync for the booted snapshot `booted` when it is due. `layout`
/// is the Btrfs top level, `var_root` the shared `@var` (the kept copies and
/// the report go to `<var_root>/lib/tetsu/etc-sync/<booted>`). Writes the
/// kept copies, flushes the filesystem, then the report and the marker last.
pub fn run(
    layout: &Layout,
    records: &Records,
    var_root: &Path,
    booted: u64,
) -> Result<Outcome, SyncError> {
    let Some(record) = records.load()?.remove(&booted) else {
        return Ok(Outcome::Skipped(Skip::NoRecord));
    };
    if record.reconstructed {
        return Ok(Outcome::Skipped(Skip::Reconstructed));
    }
    if record.kind == Some(TxKind::Rollback) {
        return Ok(Outcome::Skipped(Skip::Rollback));
    }
    let Some(source) = record.source else {
        return Ok(Outcome::Skipped(Skip::NoSource));
    };
    if layout.etc_synced_marker(booted).is_file() {
        return Ok(Outcome::Skipped(Skip::AlreadySynced));
    }
    // Records written before `running` existed: the source is the best guess.
    let from = record.running.unwrap_or(source);
    let (base, old, new) = (
        layout.etc_base(booted),
        layout.etc(from),
        layout.etc(booted),
    );
    if let Some(missing) = [&base, &old, &new].into_iter().find(|p| !p.is_dir()) {
        let shown = missing.display().to_string();
        return Ok(Outcome::Skipped(Skip::MissingTree(shown)));
    }
    let out = var_root.join(OUT_DIR).join(booted.to_string());
    private_dir(&out)?;
    let report = sync(&base, &old, &new, &out.join(KEPT))?;
    // The sync's unlinks, renames and directory changes are not fsynced one
    // by one: flush the whole filesystem, so the marker never outlives them.
    flush_fs(layout.top())?;
    let stored = StoredReport {
        counter: booted,
        from,
        report,
    };
    let json = serde_json::to_vec_pretty(&stored).expect("a report always serialises");
    write_durably(&out.join(REPORT), &json)?;
    write_durably(&layout.etc_synced_marker(booted), b"")?;
    Ok(Outcome::Synced(stored.report))
}

/// Write `path` through a temporary name, fsync it and its directory.
fn write_durably(path: &Path, data: &[u8]) -> Result<(), SyncError> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).map_err(|e| SyncError::io(&tmp, e))?;
    sync_path(&tmp)?;
    fs::rename(&tmp, path).map_err(|e| SyncError::io(path, e))?;
    match path.parent() {
        Some(dir) => sync_path(dir),
        None => Ok(()),
    }
}

/// `syncfs` on the filesystem holding `path`.
fn flush_fs(path: &Path) -> Result<(), SyncError> {
    File::open(path)
        .and_then(|f| rustix::fs::syncfs(&f).map_err(Into::into))
        .map_err(|e| SyncError::io(path, e))
}

fn sync_path(path: &Path) -> Result<(), SyncError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| SyncError::io(path, e))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::store::record::{Record, TxKind};

    struct Env {
        dir: tempfile::TempDir,
        layout: Layout,
        records: Records,
    }

    impl Env {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let layout = Layout::new(&dir.path().join("top"));
            let records = Records::new(&dir.path().join("top/@var/lib/tetsu/transactions"));
            Self {
                dir,
                layout,
                records,
            }
        }

        fn var(&self) -> std::path::PathBuf {
            self.dir.path().join("top/@var")
        }

        fn out(&self, n: u64) -> std::path::PathBuf {
            self.var().join(format!("lib/tetsu/etc-sync/{n}"))
        }

        /// A snapshot with an `etc` and an `etc-base`, `file` in both.
        fn snapshot(&self, n: u64, file: &str, text: &str) {
            for dir in [self.layout.etc(n), self.layout.etc_base(n)] {
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join(file), text).unwrap();
            }
        }

        fn put(&self, dir: std::path::PathBuf, file: &str, text: &str) {
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(file), text).unwrap();
        }

        fn record(&self, record: Record) {
            self.records.write(&record).unwrap();
        }

        fn run(&self, booted: u64) -> Result<Outcome, SyncError> {
            run(&self.layout, &self.records, &self.var(), booted)
        }
    }

    fn tx(counter: u64, source: Option<u64>, running: Option<u64>) -> Record {
        Record {
            counter,
            kind: Some(TxKind::Update),
            source,
            running,
            ..Record::default()
        }
    }

    /// Snapshot 1 is running with a user file, snapshot 2 is its update.
    fn updated() -> Env {
        let env = Env::new();
        env.snapshot(1, "hosts", "a\n");
        env.put(env.layout.etc(1), "mine.conf", "MINE\n");
        env.snapshot(2, "hosts", "a\n");
        // The transaction copied etc as it was at its start.
        fs::write(env.layout.etc_base(2).join("hosts"), "a\n").unwrap();
        env.record(tx(2, Some(1), Some(1)));
        env
    }

    #[test]
    fn the_users_changes_reach_the_booted_snapshot() {
        let env = updated();
        let Outcome::Synced(report) = env.run(2).unwrap() else {
            panic!("skipped")
        };
        assert_eq!(report.taken, ["mine.conf"]);
        let got = fs::read_to_string(env.layout.etc(2).join("mine.conf")).unwrap();
        assert_eq!(got, "MINE\n");
    }

    #[test]
    fn the_report_and_the_marker_are_written_and_the_directory_is_private() {
        use std::os::unix::fs::MetadataExt;
        let env = updated();
        env.put(env.layout.etc(1), "hosts", "user\n");
        env.put(env.layout.etc(2), "hosts", "tx\n");
        env.run(2).unwrap();
        assert!(env.layout.etc_synced_marker(2).is_file());
        let text = fs::read_to_string(env.out(2).join("report.json")).unwrap();
        let stored: StoredReport = serde_json::from_str(&text).unwrap();
        assert_eq!((stored.counter, stored.from), (2, 1));
        assert_eq!(stored.report.conflicts.len(), 1);
        assert_eq!(stored.report.conflicts[0].path, "hosts");
        let kept = fs::read_to_string(env.out(2).join("kept/hosts")).unwrap();
        assert_eq!(kept, "user\n");
        assert_eq!(fs::metadata(env.out(2)).unwrap().mode() & 0o7777, 0o700);
    }

    #[test]
    fn a_second_run_is_skipped_by_the_marker() {
        let env = updated();
        env.run(2).unwrap();
        assert_eq!(env.run(2).unwrap(), Outcome::Skipped(Skip::AlreadySynced));
    }

    #[test]
    fn a_failed_sync_leaves_no_marker() {
        let env = updated();
        // A file where the output directory has to go.
        env.put(env.var().join("lib/tetsu"), "etc-sync", "in the way");
        assert!(env.run(2).is_err());
        assert!(!env.layout.etc_synced_marker(2).exists());
    }

    #[test]
    fn nothing_runs_without_a_usable_record() {
        let env = updated();
        assert_eq!(env.run(7).unwrap(), Outcome::Skipped(Skip::NoRecord));
        env.record(Record {
            reconstructed: true,
            ..tx(2, Some(1), Some(1))
        });
        assert_eq!(env.run(2).unwrap(), Outcome::Skipped(Skip::Reconstructed));
        env.record(Record {
            kind: Some(TxKind::Rollback),
            ..tx(2, Some(1), Some(1))
        });
        assert_eq!(env.run(2).unwrap(), Outcome::Skipped(Skip::Rollback));
        env.record(tx(2, None, None));
        assert_eq!(env.run(2).unwrap(), Outcome::Skipped(Skip::NoSource));
        assert!(!env.layout.etc_synced_marker(2).exists());
        assert!(!env.out(2).exists());
    }

    #[test]
    fn a_missing_tree_is_a_skip_not_an_error() {
        let env = updated();
        fs::remove_dir_all(env.layout.etc(1)).unwrap();
        assert!(matches!(
            env.run(2).unwrap(),
            Outcome::Skipped(Skip::MissingTree(p)) if p.ends_with("@snapshots/1/root/etc")
        ));
        let env = updated();
        fs::remove_dir_all(env.layout.etc_base(2)).unwrap();
        assert!(matches!(
            env.run(2).unwrap(),
            Outcome::Skipped(Skip::MissingTree(p)) if p.ends_with("@snapshots/2/etc-base")
        ));
        assert!(!env.layout.etc_synced_marker(2).exists());
    }

    #[test]
    fn on_a_pending_chain_the_running_etc_is_the_users_side() {
        let env = updated();
        // Snapshot 3 was built from the pending snapshot 2 while 1 ran.
        env.snapshot(3, "hosts", "a\n");
        env.record(tx(3, Some(2), Some(1)));
        let Outcome::Synced(report) = env.run(3).unwrap() else {
            panic!("skipped")
        };
        assert_eq!(report.taken, ["mine.conf"]);
        assert!(env.layout.etc(3).join("mine.conf").is_file());
    }

    #[test]
    fn an_old_record_without_running_falls_back_to_the_source() {
        let env = updated();
        env.record(tx(2, Some(1), None));
        let Outcome::Synced(report) = env.run(2).unwrap() else {
            panic!("skipped")
        };
        assert_eq!(report.taken, ["mine.conf"]);
    }
}
