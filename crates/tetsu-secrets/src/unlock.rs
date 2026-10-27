//! The vault key, from the credential systemd decrypted at login.
//!
//! The user manager decrypts `tetsu-secrets.vault-key` (sealed by
//! `systemd-creds` to the host and, at best, the TPM) into the private
//! `$CREDENTIALS_DIRECTORY` of the broker's unit through
//! `LoadCredentialEncrypted=`. Nothing else holds the key.

use std::path::{Path, PathBuf};

use crate::vault::Secret;

/// The credential name, in the unit file and in `systemd-creds`.
pub const CREDENTIAL_NAME: &str = "tetsu-secrets.vault-key";
/// The shortest key accepted, in bytes.
pub const MIN_KEY_LEN: usize = 32;

/// Why the key could not be read. No variant carries key material.
#[derive(Debug, thiserror::Error)]
pub enum UnlockError {
    /// `$CREDENTIALS_DIRECTORY` is not set: not started by the unit.
    #[error("CREDENTIALS_DIRECTORY is not set")]
    NoDirectory,
    /// The credential is not in the directory.
    #[error("{0}: credential not found")]
    Missing(PathBuf),
    /// The credential is empty.
    #[error("{0}: credential is empty")]
    Empty(PathBuf),
    /// The credential is shorter than [`MIN_KEY_LEN`] bytes.
    #[error("{0}: credential is shorter than {MIN_KEY_LEN} bytes")]
    TooShort(PathBuf),
    /// The credential is not text.
    #[error("{0}: credential is not valid UTF-8 text")]
    NotText(PathBuf),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Reads the vault key from `dir` (`$CREDENTIALS_DIRECTORY`).
pub fn vault_key(dir: Option<&Path>) -> Result<Secret, UnlockError> {
    todo!("{dir:?}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const KEY: &str = "S3NT1NEL-SENTINEL-0123456789abcdef0123456789";

    fn with_key(content: &[u8]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(CREDENTIAL_NAME), content).unwrap();
        dir
    }

    #[test]
    fn a_good_key_is_read_exactly() {
        let dir = with_key(KEY.as_bytes());
        let key = vault_key(Some(dir.path())).unwrap();
        assert_eq!(key.expose(), KEY);
    }

    #[test]
    fn no_directory_is_refused() {
        assert!(matches!(vault_key(None), Err(UnlockError::NoDirectory)));
    }

    #[test]
    fn a_missing_credential_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let e = vault_key(Some(dir.path())).unwrap_err();
        assert!(matches!(e, UnlockError::Missing(_)), "{e}");
        assert!(e.to_string().contains(CREDENTIAL_NAME));
    }

    #[test]
    fn an_empty_credential_is_refused() {
        let dir = with_key(b"");
        assert!(matches!(
            vault_key(Some(dir.path())),
            Err(UnlockError::Empty(_))
        ));
    }

    #[test]
    fn a_short_credential_is_refused_without_its_content() {
        let dir = with_key(b"S3NT1NEL-short");
        let e = vault_key(Some(dir.path())).unwrap_err();
        assert!(matches!(e, UnlockError::TooShort(_)), "{e}");
        assert!(!e.to_string().contains("S3NT1NEL"));
    }

    #[test]
    fn a_binary_credential_is_refused() {
        let dir = with_key(&[0xff; 40]);
        assert!(matches!(
            vault_key(Some(dir.path())),
            Err(UnlockError::NotText(_))
        ));
    }
}
