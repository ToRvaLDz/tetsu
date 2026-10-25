//! The API key the installer staged for the first run.
//!
//! Contract with the installer: `~/.local/share/tetsu/staged-api-key`, mode
//! 0600, owned by the user, TOML with the strings `provider` and `key`
//! (written by `tetsu_installer_core::install::config::staged_api_key`).
//! The importer refuses a file that breaks the contract and leaves it
//! alone; after a successful import the file is overwritten with zeros and
//! removed.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::vault::{Secret, Vault, VaultError};

/// Where the staged key sits below the home directory.
pub const STAGED_PATH: &str = ".local/share/tetsu/staged-api-key";
const REQUIRED_MODE: u32 = 0o600;

/// What was imported; the key itself is not part of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    pub provider: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    /// The file breaks the contract; it was not read and is still there.
    #[error("{path}: refused, {reason}")]
    Refused { path: PathBuf, reason: String },
    /// The file is not the TOML the installer writes.
    #[error("{path}: not a staged API key: {reason}")]
    Invalid { path: PathBuf, reason: String },
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The vault did not take the key; the staged file is kept.
    #[error(transparent)]
    Vault(#[from] VaultError),
}

#[derive(Deserialize)]
struct Staged {
    provider: String,
    key: String,
}

pub fn staged_path(home: &Path) -> PathBuf {
    home.join(STAGED_PATH)
}

/// Import the staged key of `home` into `vault` as the current user.
/// `Ok(None)` means nothing is staged.
pub fn import_staged(home: &Path, vault: &mut dyn Vault) -> Result<Option<Imported>, StagingError> {
    import_staged_as(home, rustix::process::geteuid().as_raw(), vault)
}

/// [`import_staged`] for an explicit user id.
pub fn import_staged_as(
    home: &Path,
    uid: u32,
    vault: &mut dyn Vault,
) -> Result<Option<Imported>, StagingError> {
    let path = staged_path(home);
    let io = |source| StagingError::Io {
        path: path.clone(),
        source,
    };
    let refused = |reason: &str| StagingError::Refused {
        path: path.clone(),
        reason: reason.to_owned(),
    };
    match fs::symlink_metadata(&path) {
        Ok(m) if m.file_type().is_symlink() => return Err(refused("it is a symbolic link")),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    }
    let mut file = fs::File::open(&path).map_err(io)?;
    // The checks use the opened file, so the path cannot be swapped after.
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(refused("it is not a regular file"));
    }
    if meta.uid() != uid {
        return Err(refused("it is not owned by the current user"));
    }
    if meta.permissions().mode() & 0o7777 != REQUIRED_MODE {
        return Err(refused("its mode is not 0600"));
    }
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(io)?;
    let staged: Staged = toml::from_str(&text).map_err(|e| StagingError::Invalid {
        path: path.clone(),
        reason: e.message().to_owned(),
    })?;
    if staged.provider.is_empty() || staged.key.is_empty() {
        return Err(StagingError::Invalid {
            path: path.clone(),
            reason: "provider and key must not be empty".into(),
        });
    }
    vault.store_api_key(&staged.provider, &Secret::new(staged.key))?;
    wipe(&path, &meta).map_err(io)?;
    fs::remove_file(&path).map_err(io)?;
    Ok(Some(Imported {
        provider: staged.provider,
    }))
}

/// Overwrite the content with zeros before the file goes away, when the
/// path still names the file that was read.
fn wipe(path: &Path, read: &fs::Metadata) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).open(path)?;
    let now = file.metadata()?;
    if (now.dev(), now.ino()) != (read.dev(), read.ino()) {
        return Ok(());
    }
    let zeros = vec![0u8; usize::try_from(read.len()).unwrap_or(0)];
    file.write_all(&zeros)?;
    file.sync_all()
}

#[cfg(test)]
mod tests;
