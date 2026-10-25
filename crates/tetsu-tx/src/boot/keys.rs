//! Signing key references.

use std::path::{Path, PathBuf};

use super::BootError;

/// Default key directory; always overridable.
pub const DEFAULT_KEYS_DIR: &str = "/var/lib/tetsu/keys";

/// Where a private key lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyRef {
    /// A key file.
    File(PathBuf),
}

impl KeyRef {
    /// Parse a bare path or a `file:` URI. `pkcs11:` (and any other scheme)
    /// is recognised but not supported yet.
    pub fn parse(uri: &str) -> Result<Self, BootError> {
        let unsupported = || BootError::UnsupportedKeyUri(uri.to_owned());
        let path = if let Some(rest) = uri.strip_prefix("file:") {
            // `file:///abs` carries an empty authority; `file:/abs`, `file:rel` do not.
            match rest.strip_prefix("//") {
                Some(abs) if abs.starts_with('/') => abs,
                Some(_) => return Err(unsupported()),
                None => rest,
            }
        } else if has_scheme(uri) {
            return Err(unsupported());
        } else {
            uri
        };
        if path.is_empty() {
            return Err(unsupported());
        }
        Ok(Self::File(PathBuf::from(path)))
    }

    pub fn as_path(&self) -> &Path {
        match self {
            Self::File(p) => p,
        }
    }
}

/// `scheme:` prefix of a URI. Paths starting with `/` or `.` never count.
fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once(':') else {
        return false;
    };
    scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Everything needed to sign a UKI and systemd-boot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningKeys {
    /// Secure Boot db private key.
    pub db_key: KeyRef,
    /// Secure Boot db certificate.
    pub db_cert: PathBuf,
    /// PCR policy private key; none means no `.pcrsig`.
    pub pcr_key: Option<KeyRef>,
    /// PCR policy public key (`.pcrpkey`).
    pub pcr_public: Option<PathBuf>,
}

impl SigningKeys {
    /// Keys laid out as `db.key`, `db.crt` and, when `with_pcr`,
    /// `pcr.key` and `pcr.pub` inside `dir`.
    pub fn in_dir(dir: &Path, with_pcr: bool) -> Self {
        Self {
            db_key: KeyRef::File(dir.join("db.key")),
            db_cert: dir.join("db.crt"),
            pcr_key: with_pcr.then(|| KeyRef::File(dir.join("pcr.key"))),
            pcr_public: with_pcr.then(|| dir.join("pcr.pub")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_path() {
        assert_eq!(
            KeyRef::parse("/var/keys/db.key").unwrap(),
            KeyRef::File("/var/keys/db.key".into())
        );
        assert_eq!(
            KeyRef::parse("keys/db.key").unwrap(),
            KeyRef::File("keys/db.key".into())
        );
    }

    #[test]
    fn file_uris() {
        for uri in ["file:///k/db.key", "file:/k/db.key"] {
            assert_eq!(
                KeyRef::parse(uri).unwrap().as_path(),
                Path::new("/k/db.key")
            );
        }
        assert_eq!(
            KeyRef::parse("file:rel/db.key").unwrap().as_path(),
            Path::new("rel/db.key")
        );
    }

    #[test]
    fn pkcs11_is_unsupported() {
        let uri = "pkcs11:token=tetsu;object=db";
        match KeyRef::parse(uri).unwrap_err() {
            BootError::UnsupportedKeyUri(u) => assert_eq!(u, uri),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn other_schemes_and_empty_are_rejected() {
        assert!(KeyRef::parse("https://x/key").is_err());
        assert!(KeyRef::parse("").is_err());
        assert!(KeyRef::parse("file:").is_err());
    }

    #[test]
    fn in_dir_layout() {
        let k = SigningKeys::in_dir(Path::new("/k"), false);
        assert_eq!(k.db_key.as_path(), Path::new("/k/db.key"));
        assert_eq!(k.db_cert, Path::new("/k/db.crt"));
        assert!(k.pcr_key.is_none() && k.pcr_public.is_none());
        let k = SigningKeys::in_dir(Path::new("/k"), true);
        assert_eq!(k.pcr_key.unwrap().as_path(), Path::new("/k/pcr.key"));
        assert_eq!(k.pcr_public.unwrap(), Path::new("/k/pcr.pub"));
    }

    #[test]
    fn default_dir_constant() {
        assert_eq!(DEFAULT_KEYS_DIR, "/var/lib/tetsu/keys");
    }
}
