//! Disk step: whole disk only, wiped. Disks show model, size and the
//! operating system found on them; the installer's own boot disk is never
//! offered, and a disk holding an OS requires retyping its name.

pub mod lsblk;
pub mod os;

use std::path::PathBuf;

use crate::env::Env;
use crate::error::InstallError;
use crate::frontend::{Frontend, ask_valid};

/// A partition as lsblk reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub path: PathBuf,
    pub size: u64,
    pub fstype: Option<String>,
    pub label: Option<String>,
}

/// A whole disk that can receive the installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    /// Kernel name, e.g. `nvme0n1`.
    pub name: String,
    pub path: PathBuf,
    /// Stable `/dev/disk/by-id` link, when one exists.
    pub by_id: Option<PathBuf>,
    pub size: u64,
    pub model: Option<String>,
    pub removable: bool,
    pub partitions: Vec<Partition>,
    /// Operating system recognised on the disk.
    pub os: Option<String>,
}

/// The disk the user chose to wipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskChoice {
    pub name: String,
    pub path: PathBuf,
    pub by_id: Option<PathBuf>,
    pub size: u64,
    pub model: Option<String>,
    pub os: Option<String>,
}

impl From<&Disk> for DiskChoice {
    fn from(d: &Disk) -> Self {
        Self {
            name: d.name.clone(),
            path: d.path.clone(),
            by_id: d.by_id.clone(),
            size: d.size,
            model: d.model.clone(),
            os: d.os.clone(),
        }
    }
}

/// Size as GiB with one decimal.
pub fn human_size(bytes: u64) -> String {
    const GIB: u64 = 1 << 30;
    let tenths = bytes.saturating_mul(10) / GIB;
    format!("{}.{} GiB", tenths / 10, tenths % 10)
}

/// Disks the installer may wipe: whole disks, without the boot medium,
/// with `by_id` links and detected systems filled in.
pub fn candidates(env: &Env<'_>) -> Result<Vec<Disk>, InstallError> {
    let json = env.runner.output("lsblk", &lsblk::lsblk_args())?;
    let links = lsblk::by_id_links(&env.root);
    Ok(lsblk::parse(&json)?
        .into_iter()
        .filter(|(d, _)| d.size > 0)
        .filter(|(d, below)| !holds_boot_medium(d, below, env.boot_partition.as_ref()))
        .map(|(mut d, _)| {
            d.by_id = links.get(&d.name).cloned();
            d.os = os::detect(&d.partitions, env.probe);
            d
        })
        .collect())
}

fn holds_boot_medium(disk: &Disk, below: &[PathBuf], boot: Option<&PathBuf>) -> bool {
    boot.is_some_and(|b| &disk.path == b || below.contains(b))
}

fn describe(d: &Disk) -> String {
    let model = d.model.as_deref().unwrap_or("unknown model");
    let removable = if d.removable { ", removable" } else { "" };
    let os =
        d.os.as_deref()
            .map_or_else(String::new, |o| format!(" [{o} detected]"));
    format!("{}  {model}  {}{removable}{os}", d.name, human_size(d.size))
}

/// Make the user retype the name of a disk that holds an OS. An empty line
/// cancels.
fn confirm_by_name(fe: &mut dyn Frontend, disk: &Disk) -> Result<bool, InstallError> {
    let os = disk.os.as_deref().unwrap_or("an operating system");
    fe.message(&format!(
        "{} holds {os}; everything on it will be lost.",
        disk.name
    ));
    let name = disk.name.clone();
    let check = move |s: &str| {
        if s.is_empty() || s == name {
            Ok(())
        } else {
            Err(format!("type {name} exactly, or leave empty to go back"))
        }
    };
    let typed = ask_valid(fe, &format!("Type {} to erase it", disk.name), &check)?;
    Ok(!typed.is_empty())
}

/// Ask which disk to wipe, looping until one is confirmed.
pub fn run(fe: &mut dyn Frontend, disks: &[Disk]) -> Result<DiskChoice, InstallError> {
    if disks.is_empty() {
        return Err(InstallError::NoDisk);
    }
    let options: Vec<String> = disks.iter().map(describe).collect();
    loop {
        let i = fe.choose("Disk to wipe (the whole disk is erased)", &options)?;
        let Some(disk) = disks.get(i) else {
            fe.message("invalid choice");
            continue;
        };
        let confirmed = if disk.os.is_some() {
            confirm_by_name(fe, disk)?
        } else {
            fe.confirm(&format!("Erase all data on {}?", disk.name))?
        };
        if confirmed {
            return Ok(disk.into());
        }
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub fn disk() -> Disk {
        named("sda", None)
    }

    pub fn named(name: &str, os: Option<&str>) -> Disk {
        Disk {
            name: name.to_owned(),
            path: PathBuf::from("/dev").join(name),
            by_id: None,
            size: 512 << 30,
            model: Some("Test Disk".to_owned()),
            removable: false,
            partitions: Vec::new(),
            os: os.map(str::to_owned),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::os::NullProbe;
    use super::os::fake::FakeProbe;
    use super::tests_support::named;
    use super::*;
    use crate::entropy::fake::Counter;
    use crate::testing::Script;
    use std::ffi::OsString;
    use tetsu_tx::boot::BootError;
    use tetsu_tx::boot::tool::Runner;

    struct LsblkFake(&'static str);

    impl Runner for LsblkFake {
        fn run(&self, _: &str, _: &[OsString]) -> Result<(), BootError> {
            unreachable!()
        }

        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            assert_eq!(program, "lsblk");
            assert_eq!(args, lsblk::lsblk_args());
            Ok(self.0.to_owned())
        }
    }

    fn list(boot: Option<&str>, probe: &dyn os::OsProbe) -> Vec<Disk> {
        let t = tempfile::tempdir().unwrap();
        let counter = Counter::default();
        let runner = LsblkFake(lsblk::tests::SAMPLE);
        let env = Env {
            root: t.path().to_owned(),
            runner: &runner,
            probe,
            entropy: &counter,
            boot_partition: boot.map(PathBuf::from),
        };
        candidates(&env).unwrap()
    }

    #[test]
    fn candidates_are_whole_disks_with_detected_os() {
        let mut probe = FakeProbe::default();
        probe
            .efi
            .insert("/dev/sda1".into(), vec!["Microsoft".into()]);
        let disks = list(None, &probe);
        assert_eq!(
            disks.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            ["sda", "sdb"]
        );
        assert_eq!(disks[0].os.as_deref(), Some("Windows"));
        assert_eq!(disks[1].os, None);
    }

    #[test]
    fn boot_disk_is_excluded_by_its_partition_or_itself() {
        let by_part = list(Some("/dev/sda1"), &NullProbe);
        assert_eq!(by_part.len(), 1);
        assert_eq!(by_part[0].name, "sdb");
        let by_disk = list(Some("/dev/sdb"), &NullProbe);
        assert_eq!(by_disk[0].name, "sda");
        let nested = list(Some("/dev/mapper/root"), &NullProbe);
        assert_eq!(nested[0].name, "sdb");
    }

    #[test]
    fn sizes_are_human_readable() {
        assert_eq!(human_size(512 << 30), "512.0 GiB");
        assert_eq!(human_size(15_609_618_432), "14.5 GiB");
    }

    #[test]
    fn plain_disk_needs_a_yes() {
        let disks = [named("sda", None)];
        let mut fe = Script::new().pick(0).yes(false).pick(0).yes(true);
        let c = run(&mut fe, &disks).unwrap();
        assert_eq!(c.name, "sda");
        fe.assert_done();
    }

    #[test]
    fn disk_with_os_requires_the_exact_name() {
        let disks = [named("sda", Some("Windows")), named("sdb", None)];
        // Typing the wrong name is re-asked; an empty line goes back to the list.
        let mut fe = Script::new()
            .pick(0)
            .text("sdb")
            .text("")
            .pick(0)
            .text("sda");
        let c = run(&mut fe, &disks).unwrap();
        assert_eq!(c.name, "sda");
        assert_eq!(c.os.as_deref(), Some("Windows"));
        assert!(fe.messages.iter().any(|m| m.contains("type sda exactly")));
        fe.assert_done();
    }

    #[test]
    fn options_show_model_size_and_os() {
        let disks = [named("sda", Some("Windows"))];
        let mut fe = Script::new().pick(0).text("sda");
        run(&mut fe, &disks).unwrap();
        let line = &fe.options[0][0];
        assert!(line.contains("Test Disk") && line.contains("512.0 GiB"));
        assert!(line.contains("Windows detected"));
    }

    #[test]
    fn no_disk_is_an_error() {
        let mut fe = Script::new();
        assert!(matches!(run(&mut fe, &[]), Err(InstallError::NoDisk)));
    }
}
