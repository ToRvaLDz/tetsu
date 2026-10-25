//! The transaction engine: begin (lock, GC, snapshot), the package phases in
//! the container, DKMS, initrd, seal, UKI write and commit, record and
//! retention. Any failure before the commit deletes the new snapshot and its
//! scratch `/var`, so the system is left as it was.

use std::path::PathBuf;

use super::{PackageChange, TxError};
use crate::boot::esp::Esp;
use crate::boot::keys::SigningKeys;
use crate::boot::space::SpaceProbe;
use crate::boot::tool::Runner;
use crate::store::btrfs::Btrfs;
use crate::store::layout::Layout;
use crate::store::record::{Records, TxKind};

#[cfg(test)]
mod tests;

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

impl Engine<'_> {
    /// Run one transaction to its commit.
    pub fn run(&self, request: &Request) -> Result<Report, TxError> {
        let _ = request;
        todo!()
    }
}
