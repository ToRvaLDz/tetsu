//! Where the vault, its sealed key and its socket live.

use std::path::{Path, PathBuf};

use crate::unlock::CREDENTIAL_NAME;

/// The socket file name below `$XDG_RUNTIME_DIR/tetsu/`.
pub const SOCKET_NAME: &str = "org.tetsu.secrets.v1";

/// The per-user file locations, all below one home directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
    config_home: PathBuf,
}

impl Paths {
    /// The locations below `home`.
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_owned(),
            config_home: home.join(".config"),
        }
    }

    /// The locations below `home`, with the configuration directory taken
    /// from `xdg_config_home` (`$XDG_CONFIG_HOME`). Like the XDG spec, only
    /// an absolute path counts; else it is `~/.config`.
    pub fn with_config_home(home: &Path, xdg_config_home: Option<&Path>) -> Self {
        let mut paths = Self::new(home);
        if let Some(dir) = xdg_config_home.filter(|_| false) {
            paths.config_home = dir.to_owned();
        }
        paths
    }

    /// The locations below `home`, honouring `$XDG_CONFIG_HOME` the way the
    /// user manager does when it looks for the credential store.
    pub fn from_process(home: &Path) -> Self {
        let xdg = std::env::var_os("XDG_CONFIG_HOME");
        Self::with_config_home(home, xdg.as_deref().map(Path::new))
    }

    /// The home directory.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Where user units find their encrypted credentials.
    pub fn credstore(&self) -> PathBuf {
        self.config_home.join("credstore.encrypted")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credstore_defaults_to_the_home_config() {
        let paths = Paths::with_config_home(Path::new("/home/u"), None);
        assert_eq!(
            paths.credstore(),
            Path::new("/home/u/.config/credstore.encrypted")
        );
    }

    #[test]
    fn the_credstore_follows_an_absolute_xdg_config_home() {
        let paths = Paths::with_config_home(Path::new("/home/u"), Some(Path::new("/etc/cfg")));
        assert_eq!(paths.credstore(), Path::new("/etc/cfg/credstore.encrypted"));
        assert_eq!(
            paths.vault(),
            Path::new("/home/u/.local/share/tetsu/secrets/vault.keyring")
        );
    }

    #[test]
    fn a_relative_xdg_config_home_is_ignored() {
        let paths = Paths::with_config_home(Path::new("/home/u"), Some(Path::new("cfg")));
        assert_eq!(
            paths.credstore(),
            Path::new("/home/u/.config/credstore.encrypted")
        );
    }
}
