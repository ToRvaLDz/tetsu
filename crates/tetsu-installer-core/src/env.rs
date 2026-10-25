//! Everything the steps read from the machine, injectable for tests.

use std::path::PathBuf;

use tetsu_tx::boot::tool::Runner;

use crate::disk::os::OsProbe;
use crate::entropy::Entropy;

/// Machine access: sysfs root, external commands, OS probing, randomness.
pub struct Env<'a> {
    /// Root under which `/sys` and `/dev/disk` are read (`/` in production).
    pub root: PathBuf,
    pub runner: &'a dyn Runner,
    pub probe: &'a dyn OsProbe,
    pub entropy: &'a dyn Entropy,
    /// Device node of the partition the installer booted from (for example
    /// `/dev/sdb1`); the disk holding it is never offered.
    pub boot_partition: Option<PathBuf>,
}
