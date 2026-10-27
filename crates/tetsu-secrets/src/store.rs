//! The vault file: an oo7 v1 keyring, opened only by the broker.
//!
//! One item per secret. Its label is `tetsu-secret:<id>`, its attributes
//! hold the binding (`tetsu.id`, `tetsu.kind`, `tetsu.header`,
//! `tetsu.host`, `tetsu.allow`, `tetsu.created_at`, `tetsu.rotated_at`)
//! and its secret is the value. The file is 0600 in a 0700 directory.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::model::{SecretId, SecretMeta};
use crate::vault::Secret;

/// Why the vault refused.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// No secret has this id.
    #[error("no secret `{0}`")]
    NotFound(SecretId),
    /// A secret has this id already.
    #[error("a secret `{0}` exists already")]
    Exists(SecretId),
    /// The file cannot be opened with this key, or is not a keyring.
    #[error("{path}: cannot open the vault: {reason}")]
    Open { path: PathBuf, reason: String },
    /// The file or an item in it is damaged.
    #[error("the vault is damaged: {0}")]
    Corrupt(String),
    /// Reading or writing the file failed.
    #[error("vault i/o: {0}")]
    Io(String),
}

/// An open vault.
pub struct Store {
    path: PathBuf,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store").field("path", &self.path).finish()
    }
}

impl Store {
    /// Opens the vault at `path` with `key`, creating its directory 0700.
    /// A missing file is an empty vault, written on the first change.
    pub async fn open(path: &Path, key: &Secret) -> Result<Self, StoreError> {
        todo!("{path:?} {}", key.expose().len())
    }

    /// Every secret's binding; values stay in the file.
    pub async fn list(&self) -> Result<Vec<SecretMeta>, StoreError> {
        todo!()
    }

    /// The value of one secret.
    pub async fn value(&self, id: &SecretId) -> Result<Secret, StoreError> {
        todo!("{id}")
    }

    /// Stores a new secret; the id must be free.
    pub async fn add(&self, meta: &SecretMeta, value: &Secret) -> Result<(), StoreError> {
        todo!("{meta:?} {}", value.expose().len())
    }

    /// Replaces the value; the binding stays, `rotated_at` becomes `at`.
    pub async fn rotate(
        &self,
        id: &SecretId,
        value: &Secret,
        at: u64,
    ) -> Result<SecretMeta, StoreError> {
        todo!("{id} {} {at}", value.expose().len())
    }

    /// Deletes a secret and its value.
    pub async fn remove(&self, id: &SecretId) -> Result<(), StoreError> {
        todo!("{id}")
    }
}

#[cfg(test)]
mod tests;
