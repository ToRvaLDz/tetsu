//! The LUKS root as the kernel command line names it:
//! `rd.luks.name=<uuid>=<mapper> root=/dev/mapper/<mapper>`.

use std::path::PathBuf;

use crate::error::InstallError;

/// The encrypted root volume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LuksRoot {
    /// The LUKS partition, the argument of `systemd-cryptenroll`.
    pub device: PathBuf,
    /// The opened mapping, mounted at `/sysroot`.
    pub mapper: PathBuf,
}

/// Find the root volume in `cmdline`. When `root=` names a mapper, that
/// volume is taken; otherwise a single `rd.luks.name` is enough.
pub fn luks_root(cmdline: &str) -> Result<LuksRoot, InstallError> {
    let mut names: Vec<(&str, &str)> = Vec::new();
    let mut root = None;
    for word in cmdline.split_whitespace() {
        if let Some(rest) = word.strip_prefix("rd.luks.name=") {
            if let Some((uuid, name)) = rest.split_once('=').filter(|(u, n)| {
                !u.is_empty() && !n.is_empty() && !u.contains('/') && !n.contains('/')
            }) {
                names.push((uuid, name));
            }
        } else if let Some(rest) = word.strip_prefix("root=/dev/mapper/") {
            root = Some(rest);
        }
    }
    let found = match root {
        Some(name) => names.iter().find(|(_, n)| *n == name),
        None if names.len() == 1 => names.first(),
        None => None,
    };
    let (uuid, name) = found.ok_or_else(|| {
        InstallError::Failed("the kernel command line names no LUKS root volume".into())
    })?;
    Ok(LuksRoot {
        device: PathBuf::from(format!("/dev/disk/by-uuid/{uuid}")),
        mapper: PathBuf::from(format!("/dev/mapper/{name}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_installer_command_line_is_understood() {
        let r = luks_root("BOOT_IMAGE=x rd.luks.name=ab-12=root root=/dev/mapper/root rw quiet\n")
            .unwrap();
        assert_eq!(r.device, PathBuf::from("/dev/disk/by-uuid/ab-12"));
        assert_eq!(r.mapper, PathBuf::from("/dev/mapper/root"));
    }

    #[test]
    fn root_picks_among_several_volumes() {
        let r =
            luks_root("rd.luks.name=u1=data rd.luks.name=u2=root root=/dev/mapper/root").unwrap();
        assert_eq!(r.device, PathBuf::from("/dev/disk/by-uuid/u2"));
        assert!(luks_root("rd.luks.name=u1=data rd.luks.name=u2=root").is_err());
        assert!(luks_root("rd.luks.name=u1=data root=/dev/mapper/root").is_err());
    }

    #[test]
    fn no_volume_or_a_path_trick_is_an_error() {
        assert!(luks_root("quiet rw").is_err());
        assert!(luks_root("rd.luks.name=../x=root root=/dev/mapper/root").is_err());
        assert!(luks_root("rd.luks.name=u=a/b root=/dev/mapper/a/b").is_err());
        assert!(luks_root("rd.luks.name=u root=/dev/mapper/u").is_err());
    }
}
