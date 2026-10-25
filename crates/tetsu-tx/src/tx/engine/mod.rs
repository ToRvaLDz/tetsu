//! The transaction engine: begin (lock, GC, snapshot), the package phases in
//! the container, DKMS, initrd, seal, UKI write and commit, record and
//! retention. Any failure before the commit deletes the new snapshot and its
//! scratch `/var`, so the system is left as it was.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::dkms::{self, Dkms};
use super::nspawn::Nspawn;
use super::{PackageChange, TxError, pkg, steps, varreport};
use crate::boot::BootError;
use crate::boot::esp::Esp;
use crate::boot::install::write_uki;
use crate::boot::keys::SigningKeys;
use crate::boot::space::SpaceProbe;
use crate::boot::tool::Runner;
use crate::boot::uki::{UkiSpec, read_no_follow};
use crate::store::btrfs::Btrfs;
use crate::store::layout::Layout;
use crate::store::record::{Record, Records, TxKind};
use crate::store::{Begun, Store, snapshot};

mod packages;
mod rollback;
#[cfg(test)]
mod tests;

/// Name prefix of the throwaway `/var` directories in the run directory.
const SCRATCH_NAME: &str = "tx-var-";
/// The base kernel command line of a snapshot, relative to its root.
const CMDLINE: &str = "etc/kernel/cmdline";
/// Room added to the kernel and initrd sizes for the UKI's own sections.
const ESTIMATE_SLACK: u64 = 16 * 1024 * 1024;

/// The throwaway `/var` of transaction `counter`.
fn scratch_dir(run_dir: &Path, counter: u64) -> PathBuf {
    run_dir.join(format!("{SCRATCH_NAME}{counter}"))
}

/// Default directory of the per-transaction runtime state.
pub const DEFAULT_RUN_DIR: &str = "/run/tetsu";
/// Default transaction lock.
pub const DEFAULT_LOCK: &str = "/run/tetsu/tx.lock";

/// What the user asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Update,
    Install(Vec<String>),
    Remove(Vec<String>),
    /// Back to a snapshot: the given counter, or the last good one.
    Rollback(Option<u64>),
}

impl Request {
    pub fn kind(&self) -> TxKind {
        match self {
            Self::Update => TxKind::Update,
            Self::Install(_) => TxKind::Install,
            Self::Remove(_) => TxKind::Remove,
            Self::Rollback(_) => TxKind::Rollback,
        }
    }

    /// The packages the user named.
    fn packages(&self) -> &[String] {
        match self {
            Self::Install(p) | Self::Remove(p) => p,
            Self::Update | Self::Rollback(_) => &[],
        }
    }

    /// An install or a remove needs at least one package.
    fn check(&self) -> Result<(), TxError> {
        let needs_packages = matches!(self, Self::Install(_) | Self::Remove(_));
        if needs_packages && self.packages().is_empty() {
            return Err(TxError::failed("no package given"));
        }
        Ok(())
    }
}

/// Runtime locations of the engine.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The transaction lock file.
    pub lock: PathBuf,
    /// Holds the throwaway `/var` of each transaction.
    pub run_dir: PathBuf,
    /// The host package cache.
    pub cache: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            lock: PathBuf::from(DEFAULT_LOCK),
            run_dir: PathBuf::from(DEFAULT_RUN_DIR),
            cache: PathBuf::from(super::nspawn::DEFAULT_CACHE),
        }
    }
}

/// A committed transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The snapshot the transaction produced.
    pub counter: u64,
    pub kind: TxKind,
    /// The packages the user asked to install or remove.
    pub packages: Vec<String>,
    pub changes: Vec<PackageChange>,
    /// What the record keeps: `/var` report, GC failures, DKMS notes.
    pub log: Vec<String>,
    /// What went wrong after the commit; the transaction still succeeded.
    pub warnings: Vec<String>,
}

/// Everything a transaction needs, injected.
pub struct Engine<'a> {
    pub layout: Layout,
    pub records: Records,
    pub esp: Esp,
    pub keys: SigningKeys,
    pub paths: Paths,
    /// The snapshot the system booted.
    pub booted: u64,
    /// Kernel command line for a snapshot with no `etc/kernel/cmdline`.
    pub default_cmdline: String,
    pub runner: &'a dyn Runner,
    pub btrfs: &'a dyn Btrfs,
    pub probe: &'a dyn SpaceProbe,
    /// Unix seconds.
    pub clock: &'a dyn Fn() -> u64,
}

/// What the build up to and including the commit produced.
struct Built {
    log: Vec<String>,
    changes: Vec<PackageChange>,
    /// The ESP cleanup after the commit failed; the entry is in place.
    esp_cleanup: Option<BootError>,
}

impl Engine<'_> {
    /// Run one transaction to its commit. Before the commit any failure
    /// deletes the new snapshot and the scratch `/var`; after it nothing can
    /// fail the transaction any more, problems are warnings in the report.
    pub fn run(&self, request: &Request) -> Result<Report, TxError> {
        request.check()?;
        let started = (self.clock)();
        let store = Store::new(self.layout.clone(), self.records.clone(), self.btrfs);
        let begun = self.begin(&store, request)?;
        let scratch = scratch_dir(&self.paths.run_dir, begun.counter);
        let built = self.build(&store, &begun, request, &scratch);
        let leftover = fs::remove_dir_all(&scratch)
            .err()
            .filter(|e| e.kind() != ErrorKind::NotFound);
        match built {
            Ok(built) => Ok(self.finish(&store, &begun, request, built, started, leftover)),
            Err(e) => Err(self.discard(&begun, e)),
        }
    }

    fn begin(&self, store: &Store<'_>, request: &Request) -> Result<Begun, TxError> {
        let read = || self.esp.scan();
        let lock = &self.paths.lock;
        let begun = match request {
            Request::Rollback(target) => store.begin_picking(lock, read, self.booted, |esp| {
                rollback::pick(&self.layout, &self.records, esp, *target, self.booted)
            }),
            _ => store.begin(lock, read, self.booted),
        };
        Ok(begun?)
    }

    /// Everything up to the commit.
    fn build(
        &self,
        store: &Store<'_>,
        begun: &Begun,
        request: &Request,
        scratch: &Path,
    ) -> Result<Built, TxError> {
        let root = self.layout.root(begun.counter);
        let mut log: Vec<String> = begun
            .gc_failures
            .iter()
            .map(|e| format!("a leftover snapshot could not be deleted: {e}"))
            .collect();
        self.prepare_scratch(scratch, &mut log)?;
        let ns = Nspawn::new(&root, scratch, &self.paths.cache);
        let (changes, user_layer) = self.prepare(request, &ns, begun, scratch, &mut log)?;
        let ver = self.kernel_and_initrd(request, &ns, scratch, &user_layer, &mut log)?;
        let spec = UkiSpec {
            counter: begun.counter,
            snapshot_root: root.clone(),
            kernel_version: ver,
            base_cmdline: self.base_cmdline(&root)?,
            keys: self.keys.clone(),
        };
        store.seal(begun)?;
        let estimate = estimated_size(&spec)?;
        let esp_cleanup = match write_uki(
            &self.esp,
            self.probe,
            self.runner,
            &spec,
            Some(self.booted),
            estimate,
        ) {
            Ok(committed) => committed.gc_error,
            // Renamed into place and a later sync failed: it is committed.
            Err(e) if self.is_on_esp(begun.counter) => Some(e),
            Err(e) => return Err(e.into()),
        };
        Ok(Built {
            log,
            changes,
            esp_cleanup,
        })
    }

    /// Sweep the `/var` scratch dirs of dead transactions (under the lock)
    /// and create this transaction's.
    fn prepare_scratch(&self, scratch: &Path, log: &mut Vec<String>) -> Result<(), TxError> {
        let dir = &self.paths.run_dir;
        match fs::read_dir(dir) {
            Ok(read) => {
                for item in read {
                    let note = match item {
                        Ok(item) => remove_stale(&item.path()),
                        Err(e) => Some(format!("{}: {e}", dir.display())),
                    };
                    log.extend(note);
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(BootError::io(dir, e).into()),
        }
        if scratch.exists() {
            return Err(TxError::failed(format!(
                "a stale {} could not be removed",
                scratch.display()
            )));
        }
        for dir in [scratch, &self.paths.cache] {
            fs::create_dir_all(dir).map_err(|e| BootError::io(dir, e))?;
        }
        Ok(())
    }

    /// The package phase, or for a rollback the account merge. Returns the
    /// package changes and the user layer.
    fn prepare(
        &self,
        request: &Request,
        ns: &Nspawn,
        begun: &Begun,
        scratch: &Path,
        log: &mut Vec<String>,
    ) -> Result<(Vec<PackageChange>, Vec<String>), TxError> {
        if matches!(request, Request::Rollback(_)) {
            rollback::check_initrd(&ns.root)?;
            rollback::merge_accounts(&self.layout, begun.counter, self.booted)?;
            return Ok((Vec::new(), Vec::new()));
        }
        let phase = packages::run(self.runner, ns, &ns.root, request)?;
        for path in varreport::var_report(scratch, &ns.root)? {
            log.push(format!(
                "{path} is not covered by a tmpfiles.d entry and is lost with the scratch /var"
            ));
        }
        Ok((pkg::diff(&phase.before, &phase.after), phase.user_layer))
    }

    /// The kernel version of the new root; for everything but a rollback
    /// also the DKMS build and the initrd.
    fn kernel_and_initrd(
        &self,
        request: &Request,
        ns: &Nspawn,
        scratch: &Path,
        user_layer: &[String],
        log: &mut Vec<String>,
    ) -> Result<String, TxError> {
        let ver = steps::kernel_version(&ns.root)?;
        if matches!(request, Request::Rollback(_)) {
            return Ok(ver);
        }
        match dkms::build_modules(self.runner, ns, &ver, user_layer) {
            Ok(Dkms::Built) => log.push(format!("DKMS modules built for kernel {ver}")),
            Ok(Dkms::Skipped) => {}
            Err(e) => return Err(packages::dkms_blocked(e, request, user_layer)),
        }
        steps::build_initrd(self.runner, &ns.root, scratch, &ver)?;
        Ok(ver)
    }

    /// The command line of the snapshot's `etc/kernel/cmdline` (a symlink is
    /// refused), the configured default when it has none.
    fn base_cmdline(&self, root: &Path) -> Result<String, TxError> {
        match read_no_follow(&root.join(CMDLINE)) {
            Ok(text) => Ok(text.split_whitespace().collect::<Vec<_>>().join(" ")),
            Err(BootError::Io { source, .. }) if source.kind() == ErrorKind::NotFound => {
                Ok(self.default_cmdline.clone())
            }
            Err(e) => Err(e.into()),
        }
    }

    fn is_on_esp(&self, counter: u64) -> bool {
        self.esp
            .scan()
            .is_ok_and(|s| s.entries.iter().any(|e| e.name.counter == counter))
    }

    /// The UKI is committed: marker, record, retention. Nothing here fails
    /// the transaction.
    fn finish(
        &self,
        store: &Store<'_>,
        begun: &Begun,
        request: &Request,
        built: Built,
        started: u64,
        leftover: Option<std::io::Error>,
    ) -> Report {
        let mut warnings = Vec::new();
        warnings.extend(
            built
                .esp_cleanup
                .map(|e| format!("ESP cleanup after the commit failed: {e}")),
        );
        warnings.extend(leftover.map(|e| format!("the scratch /var could not be removed: {e}")));
        if let Err(e) = store.mark_committed(begun) {
            warnings.push(format!(
                "the commit marker was not written ({e}); the next transaction repairs it"
            ));
        }
        let record = Record {
            counter: begun.counter,
            kind: Some(request.kind()),
            source: Some(begun.source.counter),
            packages: request.packages().to_vec(),
            changes: built.changes.clone(),
            started_at: Some(started),
            finished_at: Some((self.clock)()),
            rolled_back_from: matches!(request, Request::Rollback(_)).then_some(self.booted),
            log: built.log.clone(),
            ..Record::default()
        };
        if let Err(e) = self.records.write(&record) {
            warnings.push(format!(
                "the record was not written ({e}); it is rebuilt from the ESP"
            ));
        }
        warnings.extend(self.retention(store, begun));
        Report {
            counter: begun.counter,
            kind: request.kind(),
            packages: request.packages().to_vec(),
            changes: built.changes,
            log: built.log,
            warnings,
        }
    }

    /// Apply retention; the problem, if any, as a warning.
    fn retention(&self, store: &Store<'_>, begun: &Begun) -> Option<String> {
        let state = match self.esp.scan() {
            Ok(state) => state,
            Err(e) => return Some(format!("retention skipped, the ESP is unreadable: {e}")),
        };
        store
            .apply_retention(&begun.lock, &state, self.booted)
            .err()
            .map(|e| format!("retention left snapshots behind: {e}"))
    }

    /// Delete the new snapshot of a transaction that failed before the
    /// commit; the lock is released when `begun` drops.
    fn discard(&self, begun: &Begun, e: TxError) -> TxError {
        let Err(failed) = snapshot::delete_snapshot(&self.layout, self.btrfs, begun.counter) else {
            return e;
        };
        let note = format!(
            "snapshot {} could not be deleted ({failed}); the next transaction collects it",
            begun.counter
        );
        match e {
            TxError::Failed {
                reason,
                proposal,
                mut log,
            } => {
                log.push(note);
                TxError::Failed {
                    reason,
                    proposal,
                    log,
                }
            }
            other => other,
        }
    }
}

/// Remove `path` when it is a scratch `/var` of a dead transaction; the
/// problem, if any, as a log line.
fn remove_stale(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if !name.starts_with(SCRATCH_NAME) {
        return None;
    }
    fs::remove_dir_all(path)
        .err()
        .map(|e| format!("a stale {} could not be removed: {e}", path.display()))
}

/// The UKI size to reserve: kernel plus initrd plus slack.
fn estimated_size(spec: &UkiSpec) -> Result<u64, TxError> {
    let size = |path: PathBuf| match fs::symlink_metadata(&path) {
        Ok(m) => Ok(m.len()),
        Err(e) => Err(TxError::from(BootError::io(path, e))),
    };
    Ok(size(spec.kernel_path())? + size(spec.initrd_path())? + ESTIMATE_SLACK)
}
