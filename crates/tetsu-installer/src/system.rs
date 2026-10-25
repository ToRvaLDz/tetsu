//! What the installer reads from the live system around it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use tetsu_installer_core::secureboot::MicrosoftCerts;
use tetsu_tx::boot::tool::Runner;

/// File name of the system image on the installation medium.
pub const PAYLOAD_FILE: &str = "tetsu-payload.tar";
/// Where the image carries the Microsoft certificates, in `kek/` and `db/`.
pub const MICROSOFT_CERTS_DIR: &str = "usr/share/tetsu/secureboot/microsoft";

pub fn payload_path(medium: &Path) -> PathBuf {
    medium.join(PAYLOAD_FILE)
}

/// The Microsoft certificates found under `root`; none is fine, the keys
/// enrolled are then our own only.
pub fn microsoft_certs(root: &Path) -> MicrosoftCerts {
    MicrosoftCerts::from_dir(&root.join(MICROSOFT_CERTS_DIR))
}

/// The `console=` parameters of a kernel command line, in order. They are
/// the only part of the installer's command line the installed system
/// inherits; each is checked to hold nothing but plain characters.
pub fn console_args(cmdline: &str) -> Vec<String> {
    cmdline
        .split_whitespace()
        .filter(|w| {
            w.strip_prefix("console=").is_some_and(|v| {
                !v.is_empty()
                    && v.chars()
                        .all(|c| c.is_ascii_alphanumeric() || ",.-_/".contains(c))
            })
        })
        .map(str::to_owned)
        .collect()
}

/// The partition the installer booted from: the source of the mount of the
/// medium. `None` when it is not a device (an ISO served from RAM), in
/// which case no disk is excluded.
pub fn boot_partition(runner: &dyn Runner, medium: &Path) -> Option<PathBuf> {
    let args: [OsString; 4] = ["-n".into(), "-o".into(), "SOURCE".into(), medium.into()];
    let out = runner.output("findmnt", &args).ok()?;
    // A bind or subvolume mount reads `/dev/sdb1[/subdir]`.
    let source = out.lines().next()?.split('[').next()?.trim();
    source.starts_with("/dev/").then(|| PathBuf::from(source))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tetsu_tx::boot::BootError;

    use super::*;

    #[test]
    fn only_plain_console_parameters_are_inherited() {
        assert_eq!(
            console_args("BOOT_IMAGE=/x quiet console=tty0 console=ttyS0,115200n8 root=/dev/x"),
            ["console=tty0", "console=ttyS0,115200n8"]
        );
        assert!(console_args("console= console=a;b console=$x consolefoo=1").is_empty());
    }

    struct Findmnt(Result<&'static str, ()>);

    impl Runner for Findmnt {
        fn run(&self, _: &str, _: &[OsString]) -> Result<(), BootError> {
            unreachable!()
        }

        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            assert_eq!(program, "findmnt");
            assert_eq!(args[..3], ["-n", "-o", "SOURCE"]);
            assert_eq!(args[3], "/run/tetsu/medium");
            self.0.map(str::to_owned).map_err(|()| BootError::Tool {
                tool: "findmnt".into(),
                status: "exit status: 1".into(),
                stderr: String::new(),
            })
        }
    }

    fn boot(out: Result<&'static str, ()>) -> Option<PathBuf> {
        boot_partition(&Findmnt(out), Path::new("/run/tetsu/medium"))
    }

    #[test]
    fn the_boot_partition_is_the_source_of_the_medium_mount() {
        assert_eq!(boot(Ok("/dev/sdb1\n")), Some("/dev/sdb1".into()));
        assert_eq!(boot(Ok("/dev/sdb1[/iso]\n")), Some("/dev/sdb1".into()));
        assert_eq!(
            boot(Ok("/dev/mapper/ventoy\n")),
            Some("/dev/mapper/ventoy".into())
        );
    }

    #[test]
    fn a_medium_that_is_not_a_device_excludes_nothing() {
        assert_eq!(boot(Ok("overlay\n")), None);
        assert_eq!(boot(Ok("")), None);
        assert_eq!(boot(Err(())), None);
    }

    #[test]
    fn certificates_are_the_sorted_der_files_of_kek_and_db() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(microsoft_certs(t.path()), MicrosoftCerts::default());
        let base = t.path().join(MICROSOFT_CERTS_DIR);
        for rel in ["kek/b.der", "kek/a.der", "kek/readme.txt", "db/uefi.der"] {
            let p = base.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "").unwrap();
        }
        let certs = microsoft_certs(t.path());
        assert_eq!(certs.kek, [base.join("kek/a.der"), base.join("kek/b.der")]);
        assert_eq!(certs.db, [base.join("db/uefi.der")]);
    }

    #[test]
    fn the_payload_sits_at_the_top_of_the_medium() {
        assert_eq!(
            payload_path(Path::new("/m")),
            Path::new("/m/tetsu-payload.tar")
        );
    }
}
