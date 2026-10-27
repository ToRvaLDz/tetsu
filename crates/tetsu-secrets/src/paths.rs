//! Where the vault, its sealed key and its socket live.

use std::path::{Path, PathBuf};

use crate::unlock::CREDENTIAL_NAME;

/// The socket file name below `$XDG_RUNTIME_DIR/tetsu/`.
pub const SOCKET_NAME: &str = "org.tetsu.secrets.v1";

/// The per-user file locations, all below one home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
}

impl Paths {
    /// The locations below `home`.
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
        }
    }

    /// The home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Where user units find their encrypted credentials.
    pub fn credstore(&self) -> PathBuf {
        self.home.join(".config/credstore.encrypted")
    }

    /// The sealed vault key.
    pub fn credential(&self) -> PathBuf {
        self.credstore().join(CREDENTIAL_NAME)
    }

    /// The vault file; only the broker opens it.
    pub fn vault(&self) -> PathBuf {
        self.home.join(".local/share/tetsu/secrets/vault.keyring")
    }
}

/// The broker socket for a runtime directory.
pub fn socket(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("tetsu").join(SOCKET_NAME)
}
