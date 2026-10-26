//! The sync as the initrd runs it: find the device behind `/sysroot`, mount
//! the Btrfs top level next to it, sync, unmount.

use std::fs;
use std::path::{Path, PathBuf};

use super::SyncError;
use super::firstboot::{Outcome, Skip, run as run_firstboot};
use super::mountinfo::source_of;
use crate::boot::tool::Runner;
use crate::store::layout::{Layout, booted_counter};
use crate::store::record::{RECORDS_DIR, Records};

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

/// What to print for a result: one summary line, then one per conflict. The
/// sync never blocks the boot, so a failure is a line too.
pub fn lines(result: &Result<Outcome, SyncError>) -> Vec<String> {
    const NAME: &str = "tetsu-etc-sync";
    match result {
        Ok(Outcome::Skipped(why)) => vec![format!("{NAME}: skipped: {why}")],
        Ok(Outcome::Synced(r)) => {
            let mut out = vec![format!(
                "{NAME}: synced: {} taken, {} deleted, {} merged, {} conflicts, {} skipped",
                r.taken.len(),
                r.deleted.len(),
                r.merged.len(),
                r.conflicts.len(),
                r.skipped.len()
            )];
            for c in &r.conflicts {
                let kept = if c.kept { ", your copy was kept" } else { "" };
                out.push(format!("{NAME}: conflict: {} ({:?}{kept})", c.path, c.kind));
            }
            out
        }
        Err(e) => vec![format!(
            "{NAME}: failed: {e}; it runs again at the next boot"
        )],
    }
}

/// The sync against the mounted top level.
fn sync_mounted(top: &Path, booted: u64) -> Result<Outcome, SyncError> {
    let layout = Layout::new(top);
    let var = layout.shared("@var");
    let records = Records::new(&var.join(RECORDS_DIR.trim_start_matches("/var/")));
    run_firstboot(&layout, &records, &var, booted)
}

/// Sync the booted snapshot's `/etc` when it is due. The top level is
/// unmounted again whatever the sync did.
pub fn run(paths: &Paths, runner: &dyn Runner) -> Result<Outcome, SyncError> {
    let read = |path: PathBuf| fs::read_to_string(&path).map_err(|e| SyncError::io(&path, e));
    let cmdline = read(paths.proc.join("cmdline"))?;
    let Some(booted) = booted_counter(&cmdline) else {
        return Ok(Outcome::Skipped(Skip::NotTetsu));
    };
    let mountinfo = read(paths.proc.join("self/mountinfo"))?;
    let device = source_of(&mountinfo, &paths.sysroot.to_string_lossy())?;
    let top = paths.work.join("top");
    fs::create_dir_all(&top).map_err(|e| SyncError::io(&top, e))?;
    let mounted = runner.run(
        "mount",
        &[
            "-t".into(),
            "btrfs".into(),
            "-o".into(),
            "subvolid=5,noatime".into(),
            device.into(),
            top.clone().into_os_string(),
        ],
    );
    let synced = match mounted {
        Ok(()) => {
            let outcome = sync_mounted(&top, booted);
            // Unmount whatever the sync did; its own failure comes first.
            if let Err(e) = runner.run("umount", &[top.clone().into_os_string()]) {
                // The top level is still there: never clean up under it.
                return outcome.and(Err(SyncError::StillMounted { top, source: e }));
            }
            outcome
        }
        Err(e) => Err(SyncError::from(e)),
    };
    remove_work(paths);
    synced
}

/// Remove the empty scratch directories, never recursively: a mount point
/// that is not empty may still be a filesystem. Best effort, a leftover only
/// wastes a tmpfs entry.
fn remove_work(paths: &Paths) {
    let _ = fs::remove_dir(paths.work.join("top"));
    let _ = fs::remove_dir(&paths.work);
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
        _dir: tempfile::TempDir,
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
            Self { _dir: dir, paths }
        }

        fn top(&self) -> PathBuf {
            self.paths.work.join("top")
        }

        /// A runner whose `mount` builds a layout under its target: the
        /// running snapshot 1 with a user file and snapshot 2, its update.
        fn runner(&self) -> ScriptedRunner {
            let runner = ScriptedRunner::new();
            runner.on_effect(&["mount"], |line| build(Path::new(line.last().unwrap())));
            runner.on_effect(&["umount"], |line| unmount(Path::new(line.last().unwrap())));
            runner
        }
    }

    /// What a real unmount leaves: the mount point, empty.
    fn unmount(top: &Path) {
        for entry in fs::read_dir(top).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                fs::remove_dir_all(&path).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
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
    fn the_lines_say_what_happened() {
        use crate::etcsync::apply::{Conflict, SyncReport};
        use crate::etcsync::plan::ConflictKind;
        let skipped = Ok(Outcome::Skipped(Skip::AlreadySynced));
        assert_eq!(
            lines(&skipped),
            ["tetsu-etc-sync: skipped: the marker is present"]
        );
        let report = SyncReport {
            taken: vec!["a".into()],
            conflicts: vec![Conflict {
                path: "pacman.conf".into(),
                kind: ConflictKind::BothChanged,
                kept: true,
            }],
            ..SyncReport::default()
        };
        let got = lines(&Ok(Outcome::Synced(report)));
        assert_eq!(
            got,
            [
                "tetsu-etc-sync: synced: 1 taken, 0 deleted, 0 merged, 1 conflicts, 0 skipped",
                "tetsu-etc-sync: conflict: pacman.conf (BothChanged, your copy was kept)",
            ]
        );
        let failed = lines(&Err(SyncError::Mountinfo("boom".into())));
        assert_eq!(
            failed,
            ["tetsu-etc-sync: failed: boom; it runs again at the next boot"]
        );
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
        runner.on_effect(&["umount"], |line| unmount(Path::new(line.last().unwrap())));
        assert!(run(&env.paths, &runner).is_err());
        let calls = runner.lines();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(calls[1].starts_with("umount "));
        assert!(!env.paths.work.exists());
    }

    #[test]
    fn a_failed_unmount_leaves_the_mounted_top_level_alone() {
        let env = Env::new(CMDLINE);
        let runner = ScriptedRunner::new();
        runner.on_effect(&["mount"], |line| build(Path::new(line.last().unwrap())));
        runner.on(&["umount"], Reply::Fail("target is busy".into()));
        let err = run(&env.paths, &runner).unwrap_err();
        assert!(err.to_string().contains("still mounted"), "{err}");
        // Still mounted: what is under it is the real filesystem.
        let layout = Layout::new(&env.top());
        assert!(layout.etc(1).join("mine.conf").is_file());
        assert!(layout.etc_synced_marker(2).is_file());
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
