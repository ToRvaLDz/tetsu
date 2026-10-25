//! Snapshot store (decision #20): the Btrfs layout, snapshot creation and
//! sealing, the transaction lock, transaction records and retention.
//! No pacman, no ESP writes, no orchestration.

use std::path::PathBuf;

use crate::boot::BootError;

/// Errors raised by the snapshot store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Another transaction holds the lock.
    #[error("{path}: another transaction is running", path = path.display())]
    Busy { path: PathBuf },
    /// Filesystem error, with the path involved.
    #[error("{path}: {source}", path = path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// An external tool (btrfs) failed or is missing.
    #[error(transparent)]
    Tool(#[from] BootError),
    /// A transaction record cannot be parsed.
    #[error("{path}: corrupt transaction record: {reason}", path = path.display())]
    CorruptRecord { path: PathBuf, reason: String },
    /// No snapshot with this counter.
    #[error("snapshot {0} not found")]
    NotFound(u64),
    /// The top level already holds a Tetsu layout.
    #[error("{path}: layout already exists", path = path.display())]
    LayoutExists { path: PathBuf },
    /// Some deletions failed; the rest were attempted.
    #[error("cleanup left {} failure(s), first: {}", failures.len(), failures.first().map_or_else(String::new, ToString::to_string))]
    Cleanup { failures: Vec<StoreError> },
}

impl StoreError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub mod btrfs;
pub mod layout;
pub mod lock;
pub mod record;
pub mod select;
pub mod snapshot;
