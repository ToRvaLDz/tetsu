//! Leaving the machine tidy after an install that did not finish: release
//! the mounts and the encrypted volume, then wipe the signatures of the
//! partitions and of the disk, so nothing half-installed can be mistaken
//! for a working system or unlocked. The disk is not restored: it was
//! wiped when the install began.

use tetsu_installer_core::install::InstallTarget;
use tetsu_installer_core::install::target::MAPPER_NAME;
use tetsu_tx::boot::tool::Runner;

use std::ffi::OsString;

fn arg(s: impl Into<OsString>) -> OsString {
    s.into()
}

/// Best-effort cleanup; returns one line per step that failed. Unmounting
/// something that is not mounted is not a failure worth reporting.
pub fn cleanup(runner: &dyn Runner, target: &InstallTarget) -> Vec<String> {
    let mut problems = Vec::new();
    for dir in [target.esp(), target.top()] {
        let _ = runner.run("umount", &[arg(dir)]);
    }
    let mut step = |program: &str, args: Vec<OsString>| {
        if let Err(e) = runner.run(program, &args) {
            problems.push(format!("{program}: {e}"));
        }
    };
    // The volume is only open if the install got that far.
    if target.mapper().exists() {
        step("cryptsetup", vec![arg("close"), arg(MAPPER_NAME)]);
    }
    for device in [
        target.root_partition(),
        target.esp_partition(),
        target.disk.clone(),
    ] {
        if device.exists() {
            step("wipefs", vec![arg("-a"), arg(device)]);
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::path::Path;

    use tetsu_installer_core::preflight::SecureBoot;
    use tetsu_tx::boot::BootError;

    use super::*;

    /// Records the calls. `cryptsetup` is left out: whether it runs depends
    /// on `/dev/mapper/root` of the host running the tests.
    #[derive(Default)]
    struct Recorder {
        calls: RefCell<Vec<String>>,
        fail: Option<&'static str>,
    }

    impl Runner for Recorder {
        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            let a: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            self.calls
                .borrow_mut()
                .push(format!("{program} {}", a.join(" ")));
            if Some(program) == self.fail {
                return Err(BootError::Tool {
                    tool: program.into(),
                    status: "exit status: 1".into(),
                    stderr: "busy".into(),
                });
            }
            Ok(())
        }

        fn output(&self, _: &str, _: &[OsString]) -> Result<String, BootError> {
            unreachable!()
        }
    }

    /// A target whose device nodes are files in `root`, so they "exist".
    fn target(root: &Path, nodes: &[&str]) -> InstallTarget {
        for n in nodes {
            fs::write(root.join(n), "").unwrap();
        }
        InstallTarget::new(
            &root.join("sda"),
            Path::new("/medium/payload.tar"),
            &root.join("work"),
            SecureBoot::Off,
        )
    }

    #[test]
    fn mounts_go_first_then_the_volume_then_the_signatures() {
        let t = tempfile::tempdir().unwrap();
        let tg = target(t.path(), &["sda", "sda1", "sda2"]);
        let rec = Recorder::default();
        assert!(cleanup(&rec, &tg).is_empty());
        let r = t.path().display();
        assert_eq!(
            *rec.calls.borrow(),
            [
                format!("umount {r}/work/esp"),
                format!("umount {r}/work/top"),
                format!("wipefs -a {r}/sda2"),
                format!("wipefs -a {r}/sda1"),
                format!("wipefs -a {r}/sda"),
            ]
        );
    }

    #[test]
    fn devices_that_never_appeared_are_left_alone() {
        let t = tempfile::tempdir().unwrap();
        let tg = target(t.path(), &["sda"]);
        let rec = Recorder::default();
        cleanup(&rec, &tg);
        let calls = rec.calls.borrow();
        assert_eq!(calls.iter().filter(|c| c.starts_with("wipefs")).count(), 1);
    }

    #[test]
    fn a_failing_step_is_reported_and_the_rest_still_runs() {
        let t = tempfile::tempdir().unwrap();
        let tg = target(t.path(), &["sda", "sda1", "sda2"]);
        let rec = Recorder {
            fail: Some("wipefs"),
            ..Recorder::default()
        };
        let problems = cleanup(&rec, &tg);
        assert_eq!(problems.len(), 3);
        assert!(problems[0].contains("wipefs"));
        assert_eq!(rec.calls.borrow().len(), 5);
    }
}
