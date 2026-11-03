//! The enrollment as a write sequence that rolls back: db and KEK are
//! appended before the PK, and the PK write is the one that ends Setup Mode.
//! When a write fails before it, the variables already written are put back.

use std::path::Path;

use tetsu_tx::boot::tool::Runner;

use crate::error::InstallError;

use super::Enrollment;

/// Enroll the keys, rolling db and KEK back when a write fails before the PK
/// is in. Best effort: the error says exactly what is left.
pub fn enroll(_runner: &dyn Runner, _e: &Enrollment, _efi_root: &Path) -> Result<(), InstallError> {
    todo!("enroll")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    use tetsu_tx::boot::BootError;

    use super::super::{MicrosoftCerts, efi_time};
    use super::*;
    use crate::efi::tests_support::{esl, write_pk};
    use crate::error::Rollback;
    use crate::preflight::GLOBAL_VARIABLE_GUID;

    const T: u64 = 1_793_734_685;

    /// Records every call; a call whose line contains one of `fail` fails.
    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        fail: Vec<String>,
    }

    impl Fake {
        fn failing(parts: &[&str]) -> Self {
            Self {
                fail: parts.iter().map(|p| (*p).to_owned()).collect(),
                ..Self::default()
            }
        }

        fn lines(&self) -> Vec<String> {
            self.calls.borrow().clone()
        }
    }

    impl Runner for Fake {
        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            let args: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let line = format!("{program} {}", args.join(" "));
            self.calls.borrow_mut().push(line.clone());
            if self.fail.iter().any(|f| line.contains(f.as_str())) {
                return Err(BootError::Tool {
                    tool: program.to_owned(),
                    status: "exit status: 1".to_owned(),
                    stderr: "boom".to_owned(),
                });
            }
            if program == "cert-to-efi-sig-list" {
                fs::write(args.last().unwrap(), b"ESL").unwrap();
            }
            Ok(())
        }

        fn output(&self, p: &str, a: &[OsString]) -> Result<String, BootError> {
            self.run(p, a).map(|()| String::new())
        }
    }

    struct Setup {
        dir: tempfile::TempDir,
    }

    impl Setup {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir_all(dir.path().join("w")).unwrap();
            Self { dir }
        }

        fn root(&self) -> PathBuf {
            self.dir.path().join("efi")
        }

        fn work(&self, name: &str) -> PathBuf {
            self.dir.path().join("w").join(name)
        }

        fn var(&self, name: &str, guid: &str, data: &[u8]) {
            let dir = self.root().join("sys/firmware/efi/efivars");
            fs::create_dir_all(&dir).unwrap();
            let mut file = vec![7, 0, 0, 0];
            file.extend(data);
            fs::write(dir.join(format!("{name}-{guid}")), file).unwrap();
        }

        fn enrollment(&self) -> Enrollment {
            Enrollment {
                keys_dir: "/k".into(),
                work_dir: self.dir.path().join("w"),
                owner_guid: "11111111-2222-8333-8444-555555555555".into(),
                microsoft: MicrosoftCerts::default(),
                without_microsoft: true,
                timestamp: T,
            }
        }

        fn enroll(&self, fake: &Fake) -> Result<(), InstallError> {
            enroll(fake, &self.enrollment(), &self.root())
        }

        /// The restore lines of `var` as they must read.
        fn restore(&self, var: &str, signer: &str) -> [String; 2] {
            let w = self.work(var).display().to_string();
            [
                format!(
                    "sign-efi-sig-list -t {} -k /k/{signer}.key -c /k/{signer}.crt {var} {w}.orig.esl {w}.restore.auth",
                    efi_time(T + 1)
                ),
                format!("efi-updatevar -f {w}.restore.auth {var}"),
            ]
        }
    }

    fn restore_lines(fake: &Fake) -> Vec<String> {
        fake.lines()
            .into_iter()
            .skip_while(|l| !l.contains("restore.auth") || !l.starts_with("sign-efi-sig-list"))
            .collect()
    }

    fn outcome(err: InstallError) -> (&'static str, Rollback) {
        match err {
            InstallError::EnrollmentFailed { var, outcome, .. } => (var, outcome),
            other => panic!("not an enrollment failure: {other}"),
        }
    }

    #[test]
    fn a_complete_enrollment_restores_nothing_and_writes_the_pk_last() {
        let s = Setup::new();
        let fake = Fake::default();
        s.enroll(&fake).unwrap();
        let lines = fake.lines();
        assert!(lines.iter().all(|l| !l.contains("restore")), "{lines:#?}");
        let writes: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with("efi-updatevar"))
            .collect();
        assert_eq!(writes.len(), 3);
        assert!(writes[0].ends_with(" db") && writes[1].ends_with(" KEK"));
        assert_eq!(lines.last(), Some(writes[2]));
        assert!(writes[2].ends_with(" PK"));
    }

    #[test]
    fn the_snapshot_is_taken_before_the_first_call() {
        let s = Setup::new();
        s.var("db", "d719b2cb-3d3a-4596-a3bc-dad00e67656f", &[1, 2, 3]);
        let fake = Fake::default();
        s.enroll(&fake).unwrap();
        assert_eq!(fs::read(s.work("db.orig.esl")).unwrap(), [1, 2, 3]);
        // KEK is absent: a zero-length file.
        assert_eq!(fs::read(s.work("KEK.orig.esl")).unwrap(), b"");
    }

    #[test]
    fn a_db_write_failure_restores_nothing() {
        let s = Setup::new();
        let fake = Fake::failing(&["efi-updatevar -a -f"]);
        let err = s.enroll(&fake).unwrap_err();
        assert_eq!(outcome(err), ("db", Rollback::NothingWritten));
        let lines = fake.lines();
        assert!(lines.iter().all(|l| !l.contains("restore")), "{lines:#?}");
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.starts_with("efi-updatevar"))
                .count(),
            1
        );
    }

    #[test]
    fn a_kek_write_failure_restores_the_db() {
        let s = Setup::new();
        let fake = Fake::failing(&["KEK.auth KEK"]);
        let err = s.enroll(&fake).unwrap_err();
        assert_eq!(outcome(err), ("KEK", Rollback::Restored(vec!["db"])));
        assert_eq!(restore_lines(&fake), s.restore("db", "KEK"));
    }

    #[test]
    fn a_pk_write_failure_restores_the_db_then_the_kek() {
        let s = Setup::new();
        let fake = Fake::failing(&["PK.auth PK"]);
        let err = s.enroll(&fake).unwrap_err();
        assert_eq!(outcome(err), ("PK", Rollback::Restored(vec!["db", "KEK"])));
        let mut want = s.restore("db", "KEK").to_vec();
        want.extend(s.restore("KEK", "PK"));
        assert_eq!(restore_lines(&fake), want);
        // The restores are full replacements, never appends.
        assert!(want.iter().all(|l| !l.contains(" -a ")));
    }

    #[test]
    fn an_empty_snapshot_gives_an_empty_list_and_the_same_restore_lines() {
        let s = Setup::new();
        let fake = Fake::failing(&["PK.auth PK"]);
        s.enroll(&fake).unwrap_err();
        assert_eq!(fs::read(s.work("db.orig.esl")).unwrap(), b"");
        assert_eq!(fs::read(s.work("KEK.orig.esl")).unwrap(), b"");
        assert_eq!(restore_lines(&fake).len(), 4);
    }

    #[test]
    fn a_non_empty_snapshot_is_what_the_restore_signs() {
        let s = Setup::new();
        s.var("db", "d719b2cb-3d3a-4596-a3bc-dad00e67656f", b"OLD-DB");
        s.var("KEK", GLOBAL_VARIABLE_GUID, b"OLD-KEK");
        let fake = Fake::failing(&["PK.auth PK"]);
        s.enroll(&fake).unwrap_err();
        assert_eq!(fs::read(s.work("db.orig.esl")).unwrap(), b"OLD-DB");
        assert_eq!(fs::read(s.work("KEK.orig.esl")).unwrap(), b"OLD-KEK");
    }

    #[test]
    fn a_failing_restore_is_listed_and_the_next_one_is_still_tried() {
        let s = Setup::new();
        let fake = Fake::failing(&["PK.auth PK", "db.restore.auth db"]);
        let err = s.enroll(&fake).unwrap_err();
        let msg = err.to_string();
        assert_eq!(
            outcome(err),
            (
                "PK",
                Rollback::Left {
                    restored: vec!["KEK"],
                    left: vec!["db"]
                }
            )
        );
        assert!(msg.contains("KEK was restored"), "{msg}");
        assert!(msg.contains("Tetsu's entries are left in db"), "{msg}");
        assert!(
            msg.contains("reset the Secure Boot keys in the firmware setup"),
            "{msg}"
        );
        assert!(
            fake.lines()
                .last()
                .unwrap()
                .ends_with("KEK.restore.auth KEK")
        );
    }

    #[test]
    fn a_rejected_restore_goes_through_the_same_path() {
        let s = Setup::new();
        // The signature itself is refused, for the db and the KEK.
        let fake = Fake::failing(&["PK.auth PK", "restore.auth"]);
        let err = s.enroll(&fake).unwrap_err();
        let msg = err.to_string();
        assert_eq!(
            outcome(err),
            (
                "PK",
                Rollback::Left {
                    restored: vec![],
                    left: vec!["db", "KEK"]
                }
            )
        );
        assert!(msg.contains("left in db and KEK"), "{msg}");
    }

    #[test]
    fn a_pk_failure_with_the_pk_in_the_firmware_rolls_nothing_back() {
        let s = Setup::new();
        write_pk(&s.root(), &esl(&[&[1, 2, 3]]));
        let fake = Fake::failing(&["PK.auth PK"]);
        let err = s.enroll(&fake).unwrap_err();
        let msg = err.to_string();
        assert_eq!(outcome(err), ("PK", Rollback::PkPresent));
        assert!(msg.contains("the PK is in the firmware"), "{msg}");
        assert!(msg.contains("keys are enrolled"), "{msg}");
        assert!(fake.lines().iter().all(|l| !l.contains("restore")));
    }

    #[test]
    fn a_failure_in_the_preparation_writes_nothing_and_restores_nothing() {
        let s = Setup::new();
        let fake = Fake::failing(&["sign-efi-sig-list"]);
        let err = s.enroll(&fake).unwrap_err();
        assert!(
            !matches!(err, InstallError::EnrollmentFailed { .. }),
            "{err}"
        );
        assert!(fake.lines().iter().all(|l| !l.starts_with("efi-updatevar")));
    }

    #[test]
    fn a_timestamp_that_cannot_be_followed_is_refused_before_any_call() {
        let s = Setup::new();
        let mut e = s.enrollment();
        e.timestamp = u64::MAX;
        let fake = Fake::default();
        assert!(enroll(&fake, &e, &s.root()).is_err());
        assert!(fake.lines().is_empty());
    }

    #[test]
    fn the_messages_say_what_the_firmware_holds() {
        let s = Setup::new();
        let said = |fail: &str| s.enroll(&Fake::failing(&[fail])).unwrap_err().to_string();
        let db = said("efi-updatevar -a -f");
        assert!(
            db.contains("nothing was written, db and KEK are unchanged"),
            "{db}"
        );
        let kek = said("KEK.auth KEK");
        assert!(kek.contains("db was restored"), "{kek}");
        let pk = said("PK.auth PK");
        assert!(pk.contains("db and KEK were restored"), "{pk}");
    }
}
