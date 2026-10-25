//! Looking for other operating systems on the candidate disks. A partition
//! is mounted read-only (no exec, no devices, no setuid, no journal
//! replay) under the run directory, inspected and unmounted again.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use tetsu_installer_core::disk::os::OsProbe;
use tetsu_installer_core::install::rundir::RunDir;
use tetsu_tx::boot::tool::Runner;

const MOUNT_OPTIONS: &str = "ro,nodev,noexec,nosuid";

/// The mount options for a filesystem type, or `None` when the type is not
/// one we know how to mount without writing. A read-only mount of a dirty
/// ext4, XFS, Btrfs or F2FS still replays the journal onto a disk the user
/// may never pick, so each gets its option against that.
fn mount_options(fstype: &str) -> Option<String> {
    let extra = match fstype {
        "ext2" | "ext3" | "ext4" => "noload",
        "btrfs" => "rescue=nologreplay",
        "xfs" => "norecovery",
        "f2fs" => "norecovery",
        "vfat" | "exfat" | "ntfs" | "ntfs3" | "iso9660" | "udf" => "",
        _ => return None,
    };
    Some(if extra.is_empty() {
        MOUNT_OPTIONS.to_owned()
    } else {
        format!("{MOUNT_OPTIONS},{extra}")
    })
}
const OS_RELEASE_FILES: [&str; 2] = ["etc/os-release", "usr/lib/os-release"];

/// [`OsProbe`] backed by real (or, in tests, faked) mounts.
pub struct MountProbe<'a> {
    runner: &'a dyn Runner,
    run_dir: PathBuf,
}

impl<'a> MountProbe<'a> {
    pub fn new(runner: &'a dyn Runner, run_dir: &Path) -> Self {
        Self {
            runner,
            run_dir: run_dir.to_path_buf(),
        }
    }

    /// The filesystem type of `partition`, from its signature.
    fn fstype(&self, partition: &Path) -> Option<String> {
        let args: [OsString; 3] = ["-n".into(), "-o".into(), "FSTYPE".into()];
        let mut args = args.to_vec();
        args.push(partition.into());
        let out = self.runner.output("lsblk", &args).ok()?;
        let fstype = out.lines().next()?.trim();
        (!fstype.is_empty()).then(|| fstype.to_owned())
    }

    /// Run `look` on the mounted partition; `None` when it cannot be
    /// mounted (unknown filesystem, encrypted, damaged).
    fn mounted<T>(&self, partition: &Path, look: impl FnOnce(&Path) -> T) -> Option<T> {
        let options = mount_options(&self.fstype(partition)?)?;
        let dir = RunDir::create(&self.run_dir).ok()?.subdir("probe").ok()?;
        let args: [OsString; 4] = [
            "-o".into(),
            options.into(),
            partition.into(),
            dir.clone().into(),
        ];
        self.runner.run("mount", &args).ok()?;
        let found = look(&dir);
        // A stuck unmount only leaves a read-only mount behind; the next
        // probe reports it as a failed mount and the disk is still offered.
        let _ = self.runner.run("umount", &[dir.into()]);
        Some(found)
    }
}

impl OsProbe for MountProbe<'_> {
    fn efi_vendors(&self, partition: &Path) -> Vec<String> {
        self.mounted(partition, |root| {
            let Ok(entries) = fs::read_dir(root.join("EFI")) else {
                return Vec::new();
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        })
        .unwrap_or_default()
    }

    fn os_release(&self, partition: &Path) -> Option<String> {
        self.mounted(partition, |root| {
            OS_RELEASE_FILES
                .iter()
                .find_map(|f| fs::read_to_string(root.join(f)).ok())
                .and_then(|text| pretty_name(&text))
        })
        .flatten()
    }
}

/// `PRETTY_NAME`, else `NAME`, from os-release text.
fn pretty_name(text: &str) -> Option<String> {
    let value = |key: &str| {
        text.lines().find_map(|l| {
            let v = l.trim().strip_prefix(key)?.strip_prefix('=')?;
            let v = v.trim().trim_matches(['"', '\'']).trim();
            (!v.is_empty()).then(|| v.to_owned())
        })
    };
    value("PRETTY_NAME").or_else(|| value("NAME"))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use tetsu_tx::boot::BootError;

    use super::*;

    /// Mounting a device fills the mount point with that device's files.
    #[derive(Default)]
    struct Devices {
        files: HashMap<&'static str, Vec<(&'static str, &'static str)>>,
        /// Filesystem type per device; ext4 when absent.
        fstypes: HashMap<&'static str, &'static str>,
        calls: RefCell<Vec<String>>,
    }

    impl Runner for Devices {
        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            let a: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            self.calls
                .borrow_mut()
                .push(format!("{program} {}", a.join(" ")));
            if program == "mount" {
                let Some(files) = self.files.get(a[2].as_str()) else {
                    return Err(BootError::Tool {
                        tool: "mount".into(),
                        status: "exit status: 32".into(),
                        stderr: "unknown filesystem type".into(),
                    });
                };
                for (rel, text) in files {
                    let path = Path::new(&a[3]).join(rel);
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, text).unwrap();
                }
            }
            if program == "umount" {
                // The files belonged to the device, not to the directory.
                fs::remove_dir_all(&a[0]).unwrap();
                fs::create_dir(&a[0]).unwrap();
            }
            Ok(())
        }

        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            assert_eq!(program, "lsblk");
            assert_eq!(args[..3], ["-n", "-o", "FSTYPE"]);
            let device = args[3].to_string_lossy();
            let fstype = self.fstypes.get(device.as_ref()).copied();
            Ok(format!("{}\n", fstype.unwrap_or("ext4")))
        }
    }

    fn probe_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn efi_vendors_are_the_directories_under_efi() {
        let t = probe_dir();
        let mut dev = Devices::default();
        dev.files.insert(
            "/dev/sda1",
            vec![
                ("EFI/Microsoft/Boot/bootmgfw.efi", ""),
                ("EFI/BOOT/BOOTX64.EFI", ""),
                ("EFI/stray-file", ""),
            ],
        );
        let probe = MountProbe::new(&dev, t.path());
        assert_eq!(
            probe.efi_vendors(Path::new("/dev/sda1")),
            ["BOOT", "Microsoft"]
        );
        let calls = dev.calls.borrow();
        let mnt = t.path().join("probe");
        assert_eq!(
            *calls,
            [
                format!(
                    "mount -o ro,nodev,noexec,nosuid,noload /dev/sda1 {}",
                    mnt.display()
                ),
                format!("umount {}", mnt.display()),
            ]
        );
    }

    #[test]
    fn os_release_reads_pretty_name_from_etc_or_usr_lib() {
        let t = probe_dir();
        let mut dev = Devices::default();
        dev.files.insert(
            "/dev/sda3",
            vec![(
                "etc/os-release",
                "NAME=\"Fedora Linux\"\nPRETTY_NAME=\"Fedora Linux 42 (Workstation)\"\n",
            )],
        );
        dev.files.insert(
            "/dev/sda4",
            vec![("usr/lib/os-release", "NAME=Arch Linux\nID=arch\n")],
        );
        dev.files.insert("/dev/sda5", vec![("data/file", "x")]);
        let probe = MountProbe::new(&dev, t.path());
        assert_eq!(
            probe.os_release(Path::new("/dev/sda3")).as_deref(),
            Some("Fedora Linux 42 (Workstation)")
        );
        assert_eq!(
            probe.os_release(Path::new("/dev/sda4")).as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(probe.os_release(Path::new("/dev/sda5")), None);
    }

    #[test]
    fn a_partition_that_does_not_mount_is_simply_unknown() {
        let t = probe_dir();
        let dev = Devices::default();
        let probe = MountProbe::new(&dev, t.path());
        assert!(probe.efi_vendors(Path::new("/dev/sda9")).is_empty());
        assert_eq!(probe.os_release(Path::new("/dev/sda9")), None);
        // Nothing was mounted, so nothing is unmounted.
        assert_eq!(dev.calls.borrow().len(), 2);
        assert!(dev.calls.borrow().iter().all(|c| c.starts_with("mount ")));
    }

    #[test]
    fn no_filesystem_replays_its_journal() {
        let with = |t: &str| mount_options(t).unwrap();
        assert_eq!(with("ext4"), "ro,nodev,noexec,nosuid,noload");
        assert_eq!(with("ext3"), "ro,nodev,noexec,nosuid,noload");
        assert_eq!(with("btrfs"), "ro,nodev,noexec,nosuid,rescue=nologreplay");
        assert_eq!(with("xfs"), "ro,nodev,noexec,nosuid,norecovery");
        assert_eq!(with("vfat"), "ro,nodev,noexec,nosuid");
        // Encrypted, swap, or something we cannot mount safely.
        for other in ["crypto_LUKS", "swap", "reiserfs", ""] {
            assert_eq!(mount_options(other), None, "{other}");
        }
    }

    #[test]
    fn each_partition_is_mounted_with_the_options_of_its_filesystem() {
        let t = probe_dir();
        let mut dev = Devices::default();
        dev.files
            .insert("/dev/sda2", vec![("etc/os-release", "NAME=Fedora\n")]);
        dev.fstypes.insert("/dev/sda2", "btrfs");
        dev.files.insert("/dev/sda3", vec![]);
        dev.fstypes.insert("/dev/sda3", "crypto_LUKS");
        let probe = MountProbe::new(&dev, t.path());
        assert_eq!(
            probe.os_release(Path::new("/dev/sda2")).as_deref(),
            Some("Fedora")
        );
        assert_eq!(probe.os_release(Path::new("/dev/sda3")), None);
        let calls = dev.calls.borrow();
        assert!(calls[0].starts_with("mount -o ro,nodev,noexec,nosuid,rescue=nologreplay "));
        // The encrypted partition is never mounted.
        assert_eq!(calls.len(), 2, "{calls:?}");
    }

    #[test]
    fn os_release_parsing_handles_quotes_and_missing_keys() {
        assert_eq!(pretty_name("NAME='Void'\n").as_deref(), Some("Void"));
        assert_eq!(pretty_name("ID=x\n"), None);
        assert_eq!(pretty_name("PRETTY_NAME=\n"), None);
    }
}
