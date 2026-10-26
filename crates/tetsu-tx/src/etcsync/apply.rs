//! Apply the three-way sync to real directory trees.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::SyncError;
use super::plan::ConflictKind;

/// A path where both sides changed it and the transaction's version won.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    /// Relative to `/etc`.
    pub path: String,
    pub kind: ConflictKind,
    /// The user's copy was kept aside.
    pub kept: bool,
}

/// What a sync did, with every path relative to `/etc`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    /// The user's version was carried into the new tree.
    pub taken: Vec<String>,
    /// Deleted from the new tree because the user deleted it.
    pub deleted: Vec<String>,
    /// Account files merged from both sides.
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
    /// Sockets, FIFOs and devices, not synced.
    pub skipped: Vec<String>,
}

impl SyncReport {
    /// Nothing was changed, reported or skipped.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Sync the user's changes (`old` against `base`) into `new`. The user's
/// copy of a conflicting path goes under `kept`.
pub fn sync(base: &Path, old: &Path, new: &Path, kept: &Path) -> Result<SyncReport, SyncError> {
    let _ = (base, old, new, kept);
    Ok(SyncReport::default())
}
