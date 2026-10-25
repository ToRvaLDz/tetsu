//! Checks run before any question: firmware mode, TPM, Secure Boot and
//! candidate disks. All reads happen under `Env::root`.

use std::path::Path;

use crate::disk::{self, Disk};
use crate::env::Env;
use crate::error::InstallError;

const GLOBAL_VARIABLE_GUID: &str = "8be4df61-93ca-11d2-aa0d-e39b6740b8c4";

/// Secure Boot state as the installer needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecureBoot {
    /// Disabled in firmware.
    Off,
    /// Enabled but with no Platform Key: keys can be enrolled.
    SetupMode,
    /// Enabled with a Platform Key: enrolling our keys is not possible.
    Enforcing,
}

/// Result of the preflight scan.
#[derive(Debug)]
pub struct Preflight {
    pub uefi: bool,
    pub tpm2: bool,
    pub secure_boot: SecureBoot,
    pub disks: Vec<Disk>,
}

impl Preflight {
    /// Scan the machine.
    pub fn detect(env: &Env<'_>) -> Result<Self, InstallError> {
        Ok(Self {
            uefi: is_uefi(&env.root),
            tpm2: has_tpm2(&env.root),
            secure_boot: secure_boot_state(&env.root),
            disks: disk::candidates(env)?,
        })
    }

    /// Hard requirements; the TPM is optional (Paranoid only without it).
    pub fn check(&self) -> Result<(), InstallError> {
        if !self.uefi {
            return Err(InstallError::Preflight(
                "the machine did not boot in UEFI mode".into(),
            ));
        }
        if self.secure_boot == SecureBoot::Enforcing {
            return Err(InstallError::Preflight(
                "Secure Boot is enforcing: put the firmware in Setup Mode or turn Secure Boot off"
                    .into(),
            ));
        }
        if self.disks.is_empty() {
            return Err(InstallError::NoDisk);
        }
        Ok(())
    }
}

pub fn is_uefi(root: &Path) -> bool {
    root.join("sys/firmware/efi").is_dir()
}

pub fn has_tpm2(root: &Path) -> bool {
    std::fs::read_to_string(root.join("sys/class/tpm/tpm0/tpm_version_major"))
        .is_ok_and(|v| v.trim() == "2")
}

/// Value byte of an EFI variable: 4 attribute bytes, then the data.
fn efivar_byte(root: &Path, name: &str) -> Option<u8> {
    let path = root
        .join("sys/firmware/efi/efivars")
        .join(format!("{name}-{GLOBAL_VARIABLE_GUID}"));
    std::fs::read(path).ok()?.get(4).copied()
}

pub fn secure_boot_state(root: &Path) -> SecureBoot {
    if efivar_byte(root, "SetupMode") == Some(1) {
        SecureBoot::SetupMode
    } else if efivar_byte(root, "SecureBoot") == Some(1) {
        SecureBoot::Enforcing
    } else {
        SecureBoot::Off
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn efivar(root: &Path, name: &str, value: u8) {
        let dir = root.join("sys/firmware/efi/efivars");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{name}-{GLOBAL_VARIABLE_GUID}")),
            [6, 0, 0, 0, value],
        )
        .unwrap();
    }

    #[test]
    fn uefi_follows_the_efi_directory() {
        let t = tempfile::tempdir().unwrap();
        assert!(!is_uefi(t.path()));
        fs::create_dir_all(t.path().join("sys/firmware/efi")).unwrap();
        assert!(is_uefi(t.path()));
    }

    #[test]
    fn tpm_needs_version_two() {
        let t = tempfile::tempdir().unwrap();
        assert!(!has_tpm2(t.path()));
        let dir = t.path().join("sys/class/tpm/tpm0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tpm_version_major"), "1\n").unwrap();
        assert!(!has_tpm2(t.path()));
        fs::write(dir.join("tpm_version_major"), "2\n").unwrap();
        assert!(has_tpm2(t.path()));
    }

    #[test]
    fn secure_boot_states() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(secure_boot_state(t.path()), SecureBoot::Off);
        efivar(t.path(), "SecureBoot", 0);
        efivar(t.path(), "SetupMode", 0);
        assert_eq!(secure_boot_state(t.path()), SecureBoot::Off);
        efivar(t.path(), "SecureBoot", 1);
        assert_eq!(secure_boot_state(t.path()), SecureBoot::Enforcing);
        efivar(t.path(), "SetupMode", 1);
        assert_eq!(secure_boot_state(t.path()), SecureBoot::SetupMode);
    }

    #[test]
    fn truncated_variable_counts_as_absent() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("sys/firmware/efi/efivars");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("SetupMode-{GLOBAL_VARIABLE_GUID}")),
            [6, 0, 0],
        )
        .unwrap();
        assert_eq!(secure_boot_state(t.path()), SecureBoot::Off);
    }

    fn pf(uefi: bool, sb: SecureBoot, disks: usize) -> Preflight {
        Preflight {
            uefi,
            tpm2: false,
            secure_boot: sb,
            disks: (0..disks)
                .map(|_| crate::disk::tests_support::disk())
                .collect(),
        }
    }

    #[test]
    fn check_rejects_blocking_conditions() {
        assert!(pf(true, SecureBoot::SetupMode, 1).check().is_ok());
        assert!(pf(true, SecureBoot::Off, 1).check().is_ok());
        assert!(matches!(
            pf(false, SecureBoot::Off, 1).check(),
            Err(InstallError::Preflight(_))
        ));
        assert!(matches!(
            pf(true, SecureBoot::Enforcing, 1).check(),
            Err(InstallError::Preflight(_))
        ));
        assert!(matches!(
            pf(true, SecureBoot::Off, 0).check(),
            Err(InstallError::NoDisk)
        ));
    }
}
