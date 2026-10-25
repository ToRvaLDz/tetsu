//! Transaction engine pieces on top of the snapshot store and the boot
//! library: the package model, the container argv, the steps run inside a
//! snapshot, the base guard and the account merge, and the engine that runs
//! them in order.

use serde::{Deserialize, Serialize};

use crate::boot::BootError;
use crate::store::StoreError;

pub mod accounts;
pub mod dkms;
pub mod engine;
#[cfg(test)]
pub(crate) mod fake;
pub mod guard;
pub mod nspawn;
pub mod pkg;
pub mod steps;
pub mod varreport;

/// Errors a transaction step returns, shaped for the engine.
#[derive(Debug, thiserror::Error)]
pub enum TxError {
    /// Another transaction holds the lock.
    #[error("another transaction is in progress")]
    InProgress,
    /// The package does not exist in the sync databases.
    #[error("package not found: {0}")]
    PackageNotFound(String),
    /// The package is not in the user layer (it is base, or not installed).
    #[error("not a user package: {0}")]
    NotUserPackage(String),
    /// No snapshot with this counter.
    #[error("snapshot {0} not found")]
    SnapshotNotFound(u64),
    /// The transaction failed and was discarded.
    #[error("transaction failed: {reason}")]
    Failed {
        reason: String,
        /// What the user could do about it, when something is known.
        proposal: Option<String>,
        /// Lines worth keeping in the transaction log.
        log: Vec<String>,
    },
}

impl TxError {
    /// A failure with a reason only.
    pub fn failed(reason: impl Into<String>) -> Self {
        Self::Failed {
            reason: reason.into(),
            proposal: None,
            log: Vec::new(),
        }
    }
}

impl From<BootError> for TxError {
    fn from(e: BootError) -> Self {
        Self::failed(e.to_string())
    }
}

impl From<StoreError> for TxError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Busy { .. } => Self::InProgress,
            StoreError::NotFound(n) => Self::SnapshotNotFound(n),
            other => Self::failed(other.to_string()),
        }
    }
}

/// One package whose version differs between two states of a snapshot.
/// `from` is `None` for a new package, `to` is `None` for a removed one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageChange {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn busy_store_is_in_progress() {
        let e = StoreError::Busy {
            path: PathBuf::from("/run/tetsu/tx.lock"),
        };
        assert!(matches!(TxError::from(e), TxError::InProgress));
    }

    #[test]
    fn missing_snapshot_keeps_its_counter() {
        assert!(matches!(
            TxError::from(StoreError::NotFound(7)),
            TxError::SnapshotNotFound(7)
        ));
    }

    #[test]
    fn other_errors_become_failures_with_the_cause() {
        let e = BootError::ToolMissing {
            tool: "ukify".into(),
        };
        match TxError::from(e) {
            TxError::Failed {
                reason,
                proposal,
                log,
            } => {
                assert!(reason.contains("ukify"), "{reason}");
                assert!(proposal.is_none() && log.is_empty());
            }
            other => panic!("{other:?}"),
        }
        let s = StoreError::LayoutExists {
            path: PathBuf::from("/top"),
        };
        assert!(matches!(TxError::from(s), TxError::Failed { .. }));
    }

    #[test]
    fn package_change_round_trips_through_json() {
        let c = PackageChange {
            name: "vim".into(),
            from: None,
            to: Some("9.1-1".into()),
        };
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<PackageChange>(&json).unwrap(), c);
    }
}
