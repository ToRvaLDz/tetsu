//! Write one UKI to the ESP: scan, plan GC, check space, GC, stage, commit.

use std::fs;
use std::path::PathBuf;

use super::BootError;
use super::esp::Esp;
use super::retention::split_gc;
use super::space::{SpaceProbe, apply_gc, reserve_space};
use super::tool::Runner;
use super::uki::{UkiSpec, build_uki, preflight};

/// A UKI that is on the ESP under its final name.
#[derive(Debug)]
pub struct Committed {
    pub path: PathBuf,
    /// Post-commit retention failed; the entry itself is in place.
    pub gc_error: Option<BootError>,
}

/// Build `spec` into the ESP. `booted` is the counter of the running
/// entry (never removed); `estimated_size` is the UKI size to reserve.
///
/// Order: scan, stale-counter check, preflight, space check, deletion of
/// Bad entries and orphans (plus aged-out entries only if space is short),
/// stage, commit, then the remaining retention. A failure before the commit
/// never removes a good or pending entry that the space did not require.
pub fn write_uki(
    esp: &Esp,
    probe: &dyn SpaceProbe,
    runner: &dyn Runner,
    spec: &UkiSpec,
    booted: Option<u64>,
    estimated_size: u64,
) -> Result<Committed, BootError> {
    fs::create_dir_all(esp.dir()).map_err(|e| BootError::io(esp.dir(), e))?;
    let state = esp.scan()?;
    // Refuse a stale counter before GC deletes anything.
    if let Some(newest) = state.newest_counter()
        && spec.counter <= newest
    {
        return Err(BootError::StaleCounter {
            counter: spec.counter,
            newest,
        });
    }
    // Cheap preconditions first: a build that cannot start changes nothing.
    preflight(runner, spec)?;
    // Delete only what is safe, and what is needed to fit; the rest waits
    // until the new entry is committed.
    let split = split_gc(&state, booted);
    let remaining = reserve_space(probe, esp.dir(), &split, estimated_size)?;
    esp.stage(spec.counter, |tmp| build_uki(runner, spec, tmp))?;
    let path = esp.commit(spec.counter)?;
    // The entry is committed: a failed cleanup must not look like a failed
    // write. It is reported and the next write retries the same GC.
    let gc_error = apply_gc(&remaining).err();
    Ok(Committed { path, gc_error })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::keys::{KeyRef, SigningKeys};
    use crate::boot::space::{SPACE_MARGIN, StatvfsProbe};
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    /// Fake ukify: writes `--output` with fixed content.
    struct FakeUkify;
    impl Runner for FakeUkify {
        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            self.run(program, args).map(|()| String::new())
        }

        fn run(&self, _: &str, args: &[OsString]) -> Result<(), BootError> {
            let out = args
                .iter()
                .find_map(|a| a.to_str()?.strip_prefix("--output="))
                .unwrap();
            let out = Path::new(out);
            fs::write(out, b"uki").map_err(|e| BootError::io(out, e))
        }
    }

    struct Fixed(u64);
    impl SpaceProbe for Fixed {
        fn available_bytes(&self, _: &Path) -> Result<u64, BootError> {
            Ok(self.0)
        }
    }

    fn fixture(files: &[&str]) -> (tempfile::TempDir, Esp, UkiSpec) {
        let t = tempfile::tempdir().unwrap();
        let esp_root = t.path().join("esp");
        let dir = esp_root.join("EFI/Linux");
        fs::create_dir_all(&dir).unwrap();
        for f in files {
            fs::write(dir.join(f), vec![0u8; 100]).unwrap();
        }
        let snap = t.path().join("snap");
        fs::create_dir_all(snap.join("usr/lib/modules/6.1")).unwrap();
        fs::create_dir_all(snap.join("usr/lib/tetsu")).unwrap();
        fs::write(snap.join("usr/lib/modules/6.1/vmlinuz"), b"k").unwrap();
        fs::write(snap.join("usr/lib/tetsu/initrd.img"), b"i").unwrap();
        fs::write(snap.join("usr/lib/os-release"), "NAME=T\n").unwrap();
        let spec = UkiSpec {
            counter: 10,
            snapshot_root: snap,
            kernel_version: "6.1".into(),
            base_cmdline: "quiet".into(),
            keys: SigningKeys {
                db_key: KeyRef::File("/k".into()),
                db_cert: "/c".into(),
                pcr_key: None,
                pcr_public: None,
            },
        };
        let esp = Esp::open(&esp_root);
        (t, esp, spec)
    }

    fn names(esp: &Esp) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(esp.dir())
            .unwrap()
            .map(|d| d.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn writes_commits_and_gcs() {
        let (_t, esp, spec) = fixture(&[
            "tetsu-1.efi",
            "tetsu-2.efi",
            "tetsu-3.efi",
            "tetsu-4.efi",
            "tetsu-5+0-3.efi",
            "tetsu-6.efi.tmp",
            "other.efi",
        ]);
        let done = write_uki(&esp, &StatvfsProbe, &FakeUkify, &spec, Some(4), 1000).unwrap();
        assert_eq!(done.path, esp.dir().join("tetsu-10+3.efi"));
        assert_eq!(
            names(&esp),
            vec![
                "other.efi",
                "tetsu-10+3.efi",
                "tetsu-2.efi",
                "tetsu-3.efi",
                "tetsu-4.efi"
            ]
        );
    }

    #[test]
    fn no_space_fails_before_deleting_or_building() {
        let (_t, esp, spec) = fixture(&["tetsu-1.efi", "tetsu-2+0-3.efi"]);
        let before = names(&esp);
        let err = write_uki(
            &esp,
            &Fixed(0),
            &FakeUkify,
            &spec,
            Some(1),
            10 * SPACE_MARGIN,
        )
        .unwrap_err();
        assert!(matches!(err, BootError::InsufficientSpace { .. }));
        assert_eq!(names(&esp), before);
    }

    #[test]
    fn stale_counter_is_refused() {
        let (_t, esp, mut spec) = fixture(&["tetsu-20.efi"]);
        spec.counter = 10;
        let err = write_uki(&esp, &StatvfsProbe, &FakeUkify, &spec, Some(20), 10).unwrap_err();
        assert!(matches!(err, BootError::StaleCounter { .. }));
    }

    /// Fake ukify that always fails.
    struct FailingUkify;
    impl Runner for FailingUkify {
        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            self.run(program, args).map(|()| String::new())
        }

        fn run(&self, _: &str, _: &[OsString]) -> Result<(), BootError> {
            Err(BootError::Tool {
                tool: "ukify".into(),
                status: "exit status: 1".into(),
                stderr: "boom".into(),
            })
        }
    }

    /// Runner whose `ukify` is not installed.
    struct NoUkify;
    impl Runner for NoUkify {
        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            self.run(program, args).map(|()| String::new())
        }

        fn run(&self, _: &str, _: &[OsString]) -> Result<(), BootError> {
            panic!("must not run")
        }
        fn check_tool(&self, program: &str) -> Result<(), BootError> {
            Err(BootError::ToolMissing {
                tool: program.into(),
            })
        }
    }

    const BUSY: [&str; 8] = [
        "tetsu-1.efi",
        "tetsu-2.efi",
        "tetsu-3.efi",
        "tetsu-4.efi",
        "tetsu-5+3.efi",
        "tetsu-0+0-3.efi",
        "tetsu-99.efi.tmp",
        "foreign.efi",
    ];

    #[test]
    fn failing_build_keeps_the_pending_default_and_good_entries() {
        let (_t, esp, spec) = fixture(&BUSY);
        let err = write_uki(&esp, &StatvfsProbe, &FailingUkify, &spec, Some(4), 1000).unwrap_err();
        assert!(matches!(err, BootError::Tool { .. }));
        // Only the Bad entry and the orphan went.
        assert_eq!(
            names(&esp),
            vec![
                "foreign.efi",
                "tetsu-1.efi",
                "tetsu-2.efi",
                "tetsu-3.efi",
                "tetsu-4.efi",
                "tetsu-5+3.efi"
            ]
        );
    }

    #[test]
    fn missing_tool_changes_nothing() {
        let (_t, esp, spec) = fixture(&BUSY);
        let before = names(&esp);
        let err = write_uki(&esp, &StatvfsProbe, &NoUkify, &spec, Some(4), 1000).unwrap_err();
        assert!(matches!(err, BootError::ToolMissing { .. }));
        assert_eq!(names(&esp), before);
    }

    #[test]
    fn unreadable_inputs_change_nothing() {
        let (_t, esp, spec) = fixture(&BUSY);
        fs::remove_file(spec.snapshot_root.join("usr/lib/os-release")).unwrap();
        let before = names(&esp);
        let err = write_uki(&esp, &StatvfsProbe, &FakeUkify, &spec, Some(4), 1000).unwrap_err();
        assert!(matches!(err, BootError::Io { .. }));
        assert_eq!(names(&esp), before);
    }

    #[test]
    fn aged_out_entries_go_before_the_build_only_if_space_is_short() {
        let (_t, esp, spec) = fixture(&BUSY);
        // Bad (100) + orphan (100) free 200; 50 more are needed: the oldest
        // aged-out entry (1) goes, the pending default (5) stays.
        let probe = Fixed(SPACE_MARGIN);
        let err = write_uki(&esp, &probe, &FailingUkify, &spec, Some(4), 250).unwrap_err();
        assert!(matches!(err, BootError::Tool { .. }));
        assert_eq!(
            names(&esp),
            vec![
                "foreign.efi",
                "tetsu-2.efi",
                "tetsu-3.efi",
                "tetsu-4.efi",
                "tetsu-5+3.efi"
            ]
        );
    }

    #[test]
    fn successful_commit_converges_to_four_entries() {
        let (_t, esp, spec) = fixture(&BUSY);
        write_uki(&esp, &StatvfsProbe, &FakeUkify, &spec, Some(4), 1000).unwrap();
        assert_eq!(
            names(&esp),
            vec![
                "foreign.efi",
                "tetsu-10+3.efi",
                "tetsu-2.efi",
                "tetsu-3.efi",
                "tetsu-4.efi"
            ]
        );
    }

    #[test]
    fn insufficient_even_with_everything_changes_nothing() {
        let (_t, esp, spec) = fixture(&BUSY);
        let before = names(&esp);
        let err = write_uki(
            &esp,
            &Fixed(0),
            &FailingUkify,
            &spec,
            Some(4),
            10 * SPACE_MARGIN,
        )
        .unwrap_err();
        assert!(matches!(err, BootError::InsufficientSpace { .. }));
        assert_eq!(names(&esp), before);
    }

    /// Fake ukify that writes the output, then turns `tetsu-1.efi` into a
    /// directory so the post-commit removal fails (for any uid).
    struct SabotagingUkify;
    impl Runner for SabotagingUkify {
        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            self.run(program, args).map(|()| String::new())
        }

        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            FakeUkify.run(program, args)?;
            let out = args
                .iter()
                .find_map(|a| a.to_str()?.strip_prefix("--output="))
                .unwrap();
            let one = Path::new(out).parent().unwrap().join("tetsu-1.efi");
            fs::remove_file(&one).unwrap();
            fs::create_dir(&one).unwrap();
            Ok(())
        }
    }

    #[test]
    fn post_commit_gc_failure_is_a_typed_error_and_the_write_succeeds() {
        let (_t, esp, spec) = fixture(&BUSY);
        let done = write_uki(&esp, &StatvfsProbe, &SabotagingUkify, &spec, Some(4), 1000).unwrap();
        assert_eq!(done.path, esp.dir().join("tetsu-10+3.efi"));
        match done.gc_error {
            Some(BootError::GcFailed { failures }) => {
                assert_eq!(failures.len(), 1);
                assert!(
                    matches!(&failures[0], BootError::Io { path, .. } if path.ends_with("tetsu-1.efi")),
                    "{failures:?}"
                );
            }
            other => panic!("{other:?}"),
        }
        // The other superseded entry was still removed.
        assert!(!esp.dir().join("tetsu-5+3.efi").exists());
        assert!(esp.dir().join("tetsu-10+3.efi").is_file());
    }

    #[test]
    fn clean_write_has_no_gc_error() {
        let (_t, esp, spec) = fixture(&BUSY);
        let done = write_uki(&esp, &StatvfsProbe, &FakeUkify, &spec, Some(4), 1000).unwrap();
        assert!(done.gc_error.is_none());
    }
}
