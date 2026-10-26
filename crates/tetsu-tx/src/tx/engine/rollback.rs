//! What a rollback does instead of the package phase: pick the target,
//! carry the running system's accounts over and check the stored initrd.

use std::fs;
use std::path::Path;

use crate::boot::esp::EspState;
use crate::store::StoreError;
use crate::store::layout::Layout;
use crate::store::record::Records;
use crate::store::select::{self, Source};
use crate::tx::TxError;
use crate::tx::accounts::{ACCOUNT_FILES, merge_file};
use crate::tx::steps::INITRD;

/// The snapshot to roll back to: `target` when it exists and is committed,
/// else the last good one older than the booted snapshot. `NotFound(0)`
/// when there is none. Call it under the lock.
pub(super) fn pick(
    layout: &Layout,
    records: &Records,
    esp: &EspState,
    target: Option<u64>,
    booted: u64,
) -> Result<Source, StoreError> {
    let snapshots = layout.list_snapshots()?;
    let counter = match target {
        Some(n) if snapshots.contains(&n) && layout.is_committed(n) => n,
        Some(n) => return Err(StoreError::NotFound(n)),
        None => {
            let known = records.load_or_reconstruct(esp)?;
            select::last_good(&snapshots, &known, booted).ok_or(StoreError::NotFound(0))?
        }
    };
    Ok(Source {
        counter,
        inherit_etc_base: false,
    })
}

/// Merge the account files of the booted snapshot's `etc` into the `etc` of
/// the new snapshot `counter`: the running system's accounts win.
pub(super) fn merge_accounts(layout: &Layout, counter: u64, booted: u64) -> Result<(), TxError> {
    let (running, target) = (layout.etc(booted), layout.etc(counter));
    for name in ACCOUNT_FILES {
        merge_file(&running.join(name), &target.join(name))?;
    }
    Ok(())
}

/// The stored initrd must be there: a rollback never rebuilds it.
pub(super) fn check_initrd(root: &Path) -> Result<(), TxError> {
    let initrd = root.join(INITRD.trim_start_matches('/'));
    if fs::symlink_metadata(&initrd).is_ok_and(|m| m.file_type().is_file()) {
        return Ok(());
    }
    Err(TxError::failed(format!(
        "the snapshot has no stored initrd at {}, and a rollback does not rebuild it",
        initrd.display()
    )))
}
