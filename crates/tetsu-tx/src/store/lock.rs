//! The single-transaction lock: an exclusive `flock` on a lock file, held
//! for the life of the guard and released by the kernel if the process dies.

use std::fs::File;
use std::path::{Path, PathBuf};

use super::StoreError;

/// Held while a transaction runs.
#[derive(Debug)]
pub struct TxLock {
    _file: File,
    path: PathBuf,
}

impl TxLock {
    /// Take the lock without waiting; `Busy` when another holder has it.
    /// Creates the lock file and its directory when missing.
    pub fn acquire(path: &Path) -> Result<Self, StoreError> {
        let _ = path;
        todo!()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_busy_until_the_first_drops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run/tetsu/tx.lock");
        let first = TxLock::acquire(&path).unwrap();
        assert_eq!(first.path(), path);
        match TxLock::acquire(&path) {
            Err(StoreError::Busy { path: p }) => assert_eq!(p, path),
            other => panic!("{other:?}"),
        }
        drop(first);
        TxLock::acquire(&path).unwrap();
    }

    #[test]
    fn unwritable_location_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"").unwrap();
        assert!(matches!(
            TxLock::acquire(&blocker.join("tx.lock")),
            Err(StoreError::Io { .. })
        ));
    }
}
