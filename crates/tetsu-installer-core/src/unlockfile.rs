//! `etc/tetsu/unlock-level.toml`: the unlock level, whether the TPM seal
//! is done, and whether our Secure Boot keys are enrolled.
//!
//! ```toml
//! level = "balanced"      # paranoid | comfort | balanced
//! state = "pending"       # pending | sealed
//! secure_boot = "enrolled" # only after `tetsu secureboot enroll`
//! ```
//!
//! `secure_boot` is informational: the first-boot seal checks the live
//! firmware state, not this line.

use std::fs;
use std::io::Write;
use std::path::Path;

use serde::Deserialize;

use crate::error::InstallError;
use crate::unlock::UnlockLevel;

/// Value of `secure_boot` once our keys are enrolled after the install.
pub const ENROLLED: &str = "enrolled";

/// Whether the TPM seal is done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealState {
    /// Waiting for the first boot with Secure Boot on.
    Pending,
    Sealed,
}

impl SealState {
    pub fn id(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Sealed => "sealed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnlockFile {
    pub level: UnlockLevel,
    pub state: SealState,
    pub secure_boot_enrolled: bool,
}

#[derive(Deserialize)]
struct Raw {
    level: String,
    state: String,
    secure_boot: Option<String>,
}

impl UnlockFile {
    /// The file the installer writes: Paranoid needs no seal, the others
    /// wait for the first boot.
    pub fn initial(level: UnlockLevel) -> Self {
        Self {
            level,
            state: if level.needs_tpm() {
                SealState::Pending
            } else {
                SealState::Sealed
            },
            secure_boot_enrolled: false,
        }
    }

    pub fn parse(text: &str) -> Result<Self, InstallError> {
        let bad = |what: String| InstallError::Failed(format!("unlock-level.toml: {what}"));
        let raw: Raw = toml::from_str(text).map_err(|e| bad(e.message().to_owned()))?;
        let level = UnlockLevel::ALL
            .into_iter()
            .find(|l| l.id() == raw.level)
            .ok_or_else(|| bad(format!("unknown level \"{}\"", raw.level)))?;
        let state = match raw.state.as_str() {
            "pending" => SealState::Pending,
            "sealed" => SealState::Sealed,
            other => return Err(bad(format!("unknown state \"{other}\""))),
        };
        Ok(Self {
            level,
            state,
            secure_boot_enrolled: raw.secure_boot.as_deref() == Some(ENROLLED),
        })
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "level = \"{}\"\nstate = \"{}\"\n",
            self.level.id(),
            self.state.id()
        );
        if self.secure_boot_enrolled {
            out.push_str(&format!("secure_boot = \"{ENROLLED}\"\n"));
        }
        out
    }

    pub fn read(path: &Path) -> Result<Self, InstallError> {
        let text = fs::read_to_string(path).map_err(|e| InstallError::io(path, e))?;
        Self::parse(&text)
    }

    /// Replace the file in one step (a temporary file next to it, then a
    /// rename), so a crash never leaves half a file.
    pub fn write(&self, path: &Path) -> Result<(), InstallError> {
        let tmp = path.with_extension("toml.new");
        let result = fs::File::create(&tmp)
            .and_then(|mut f| {
                f.write_all(self.render().as_bytes())
                    .and_then(|()| f.sync_all())
            })
            .and_then(|()| fs::rename(&tmp, path));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map_err(|e| InstallError::io(path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_output_parses_back() {
        for level in UnlockLevel::ALL {
            let file = UnlockFile::initial(level);
            assert_eq!(UnlockFile::parse(&file.render()).unwrap(), file);
        }
        assert_eq!(
            UnlockFile::initial(UnlockLevel::Comfort).render(),
            "level = \"comfort\"\nstate = \"pending\"\n"
        );
        assert_eq!(
            UnlockFile::initial(UnlockLevel::Paranoid).state,
            SealState::Sealed
        );
    }

    #[test]
    fn the_enrolled_marker_round_trips() {
        let file = UnlockFile {
            secure_boot_enrolled: true,
            ..UnlockFile::initial(UnlockLevel::Balanced)
        };
        assert_eq!(
            file.render(),
            "level = \"balanced\"\nstate = \"pending\"\nsecure_boot = \"enrolled\"\n"
        );
        assert_eq!(UnlockFile::parse(&file.render()).unwrap(), file);
    }

    #[test]
    fn bad_files_are_refused() {
        for text in [
            "",
            "level = \"comfort\"\n",
            "level = \"turbo\"\nstate = \"pending\"\n",
            "level = \"comfort\"\nstate = \"half\"\n",
            "level = [",
        ] {
            assert!(UnlockFile::parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn write_replaces_the_file_and_leaves_no_temporary() {
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("unlock-level.toml");
        fs::write(&path, "old").unwrap();
        let file = UnlockFile::initial(UnlockLevel::Comfort);
        file.write(&path).unwrap();
        assert_eq!(UnlockFile::read(&path).unwrap(), file);
        assert_eq!(fs::read_dir(t.path()).unwrap().count(), 1);
        assert!(file.write(&t.path().join("no/such/dir/f.toml")).is_err());
    }
}
