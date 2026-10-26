//! The sync as the initrd runs it: find the device behind `/sysroot`, mount
//! the Btrfs top level next to it, sync, unmount.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use super::SyncError;
use super::firstboot::{Outcome, Skip};
use crate::boot::tool::Runner;

/// Where the initrd keeps what the sync reads.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `/proc`.
    pub proc: PathBuf,
    /// The new root, `/sysroot`.
    pub sysroot: PathBuf,
    /// Scratch directory for the top-level mount.
    pub work: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
            sysroot: PathBuf::from("/sysroot"),
            work: PathBuf::from("/run/tetsu-etc-sync"),
        }
    }
}

/// Sync the booted snapshot's `/etc` when it is due. The top level is
/// unmounted again whatever the sync did.
pub fn run(paths: &Paths, runner: &dyn Runner) -> Result<Outcome, SyncError> {
    let _ = (paths, runner, OsString::new(), fs::read_dir("/"));
    Ok(Outcome::Skipped(Skip::NotTetsu))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::boot::BootError;
    use crate::store::layout::Layout;
    use crate::store::record::{Record, Records, TxKind};
    use crate::tx::fake::{Reply, ScriptedRunner};

    const MOUNTINFO: &str = "30 27 0:31 / /sysroot rw - btrfs /dev/mapper/root rw\n";
    const CMDLINE: &str = "root=/dev/mapper/root rw rootflags=subvol=@snapshots/2/root quiet\n";

    struct Env {
        dir: tempfile::TempDir,
        paths: Paths,
    }

    impl Env {
        fn new(cmdline: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let proc = dir.path().join("proc");
            fs::create_dir_all(proc.join("self")).unwrap();
            fs::write(proc.join("cmdline"), cmdline).unwrap();
            fs::write(proc.join("self/mountinfo"), MOUNTINFO).unwrap();
            let paths = Paths {
                proc,
                sysroot: PathBuf::from("/sysroot"),
                work: dir.path().join("run/tetsu-etc-sync"),
            };
            Self { dir, paths }
        }

        fn top(&self) -> PathBuf {
            self.paths.work.join("top")
        }

        /// A runner whose `mount` builds a layout under its target: the
        /// running snapshot 1 with a user file and snapshot 2, its update.
        fn runner(&self) -> ScriptedRunner {
            let runner = ScriptedRunner::new();
            runner.on_effect(&["mount"], |line| build(Path::new(line.last().unwrap())));
            runner
        }
    }

    fn build(top: &Path) {
        let layout = Layout::new(top);
        for n in [1, 2] {
            fs::create_dir_all(layout.etc(n)).unwrap();
            fs::create_dir_all(layout.etc_base(n)).unwrap();
        }
        fs::write(layout.etc(1).join("mine.conf"), "MINE\n").unwrap();
        Records::new(&top.join("@var/lib/tetsu/transactions"))
            .write(&Record {
                counter: 2,
                kind: Some(TxKind::Update),
                source: Some(1),
                running: Some(1),
                ..Record::default()
            })
            .unwrap();
    }

    #[test]
    fn it_mounts_the_top_level_syncs_and_unmounts() {
        let env = Env::new(CMDLINE);
        let runner = env.runner();
        let Outcome::Synced(report) = run(&env.paths, &runner).unwrap() else {
            panic!("skipped")
        };
        assert_eq!(report.taken, ["mine.conf"]);
        let top = env.top().display().to_string();
        assert_eq!(
            runner.lines(),
            [
                format!("mount -t btrfs -o subvolid=5,noatime /dev/mapper/root {top}"),
                format!("umount {top}"),
            ]
        );
        assert!(!env.paths.work.exists());
    }

    #[test]
    fn a_command_line_without_a_tetsu_snapshot_mounts_nothing() {
        let env = Env::new("root=/dev/sda1 rw quiet\n");
        let runner = env.runner();
        assert_eq!(
            run(&env.paths, &runner).unwrap(),
            Outcome::Skipped(Skip::NotTetsu)
        );
        assert!(runner.lines().is_empty());
        assert!(!env.paths.work.exists());
    }

    #[test]
    fn it_unmounts_even_when_the_sync_fails() {
        let env = Env::new(CMDLINE);
        let runner = ScriptedRunner::new();
        runner.on_effect(&["mount"], |line| {
            let top = Path::new(line.last().unwrap());
            build(top);
            // A file where the output directory has to go.
            fs::write(top.join("@var/lib/tetsu/etc-sync"), "in the way").unwrap();
        });
        assert!(run(&env.paths, &runner).is_err());
        let calls = runner.lines();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[1].starts_with("umount "));
        assert!(!env.paths.work.exists());
    }

    #[test]
    fn a_failed_mount_is_an_error_and_leaves_nothing_behind() {
        let env = Env::new(CMDLINE);
        let runner = ScriptedRunner::new();
        runner.on(&["mount"], Reply::Fail("wrong fs type".into()));
        let err = run(&env.paths, &runner).unwrap_err();
        assert!(
            matches!(err, SyncError::Tool(BootError::Tool { .. })),
            "{err}"
        );
        assert_eq!(runner.lines().len(), 1);
        assert!(!env.paths.work.exists());
        let _ = &env.dir;
    }

    #[test]
    fn a_sysroot_that_is_not_btrfs_is_an_error_before_any_mount() {
        let env = Env::new(CMDLINE);
        fs::write(
            env.paths.proc.join("self/mountinfo"),
            "30 27 0:31 / /sysroot rw - ext4 /dev/sda1 rw\n",
        )
        .unwrap();
        let runner = env.runner();
        assert!(matches!(
            run(&env.paths, &runner),
            Err(SyncError::Mountinfo(_))
        ));
        assert!(runner.lines().is_empty());
    }
}
