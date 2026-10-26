//! The boot check (decision #18): after a boot reaches `multi-user.target`
//! the system is healthy or it is not. Healthy records the booted snapshot
//! as good and lets `boot-complete.target` be reached, so
//! `systemd-bless-boot` renames the ESP entry; unhealthy leaves the entry
//! on trial, and boot counting falls back once the tries are gone.
//!
//! The checks: `/` and `/var` are mounted, no `tetsu-*` system unit is in
//! the `failed` state, and `greetd.service` is active when it is installed.

#[cfg(test)]
mod dist;
pub mod facts;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use crate::boot::entry::BootState;
use crate::boot::esp::{Esp, EspState};
use crate::boot::tool::Runner;
use crate::cli::DEFAULT_ESP;
use crate::store::StoreError;
use crate::store::layout::booted_counter;
use crate::store::lock::TxLock;
use crate::store::record::{BootVerdict, RECORDS_DIR, Records};
use crate::tx::engine::DEFAULT_LOCK;
use facts::{Facts, Greetd};

/// A check that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    NotMounted(String),
    UnitFailed(String),
    GreetdNotActive(String),
    /// A fact could not be gathered; an unreadable system is not healthy.
    CheckFailed(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMounted(p) => write!(f, "{p} is not mounted"),
            Self::UnitFailed(u) => write!(f, "{u} has failed"),
            Self::GreetdNotActive(s) => write!(f, "greetd.service is {s}"),
            Self::CheckFailed(why) => write!(f, "cannot check: {why}"),
        }
    }
}

/// Run the checks on `facts`; every failure, not only the first.
pub fn evaluate(facts: &Facts) -> Result<(), Vec<Failure>> {
    let mut failures = Vec::new();
    for point in ["/", "/var"] {
        if !facts.mount_points.contains(point) {
            failures.push(Failure::NotMounted(point.to_owned()));
        }
    }
    failures.extend(
        facts
            .failed_units
            .iter()
            .filter(|u| u.starts_with("tetsu-"))
            .cloned()
            .map(Failure::UnitFailed),
    );
    if let Greetd::NotActive(state) = &facts.greetd {
        failures.push(Failure::GreetdNotActive(state.clone()));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

/// Write the verdicts this boot calls for; the counters and verdicts
/// written, in order. A record missing for an ESP entry is rebuilt first.
/// When it passed, the booted snapshot is `Good`. Either way, an ESP entry
/// that ran out of tries and has no verdict gets `Bad`.
pub fn record(
    records: &Records,
    esp: &EspState,
    booted: u64,
    passed: bool,
) -> Result<Vec<(u64, BootVerdict)>, StoreError> {
    let all = records.load_or_reconstruct(esp)?;
    let mut written = Vec::new();
    for entry in &esp.entries {
        let counter = entry.name.counter;
        let exhausted = entry.state() == BootState::Bad;
        // A boot that passes on its last try is good, not bad.
        let this_boot = passed && counter == booted;
        if exhausted && !this_boot && all.get(&counter).is_some_and(|r| r.boot.is_none()) {
            records.set_boot(counter, BootVerdict::Bad)?;
            written.push((counter, BootVerdict::Bad));
        }
    }
    if passed
        && all
            .get(&booted)
            .is_none_or(|r| r.boot != Some(BootVerdict::Good))
    {
        records.set_boot(booted, BootVerdict::Good)?;
        written.push((booted, BootVerdict::Good));
    }
    Ok(written)
}

/// Where the check looks.
#[derive(Clone, Debug)]
pub struct Paths {
    /// `/proc`.
    pub proc: PathBuf,
    /// The ESP mount point.
    pub esp: PathBuf,
    /// The transaction records.
    pub records: PathBuf,
    /// The transaction lock, held while the records are written.
    pub lock: PathBuf,
}

impl Paths {
    pub fn system() -> Self {
        Self {
            proc: PathBuf::from("/proc"),
            esp: PathBuf::from(DEFAULT_ESP),
            records: PathBuf::from(RECORDS_DIR),
            lock: PathBuf::from(DEFAULT_LOCK),
        }
    }
}

/// What a run came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Empty when the system is healthy.
    pub failures: Vec<Failure>,
    /// The verdicts written.
    pub written: Vec<(u64, BootVerdict)>,
    /// Problems that did not change the health verdict.
    pub warnings: Vec<String>,
}

/// Check the running system and record the verdicts. A problem with the
/// records or the ESP is a warning: it does not make a healthy boot fail.
pub fn run(paths: &Paths, runner: &dyn Runner) -> Report {
    let mut report = Report::default();
    let facts = match gather(paths, runner) {
        Ok(facts) => facts,
        Err(why) => {
            report.failures.push(Failure::CheckFailed(why));
            return report;
        }
    };
    let passed = match evaluate(&facts) {
        Ok(()) => true,
        Err(failures) => {
            report.failures = failures;
            false
        }
    };
    let cmdline = fs::read_to_string(paths.proc.join("cmdline")).unwrap_or_default();
    let esp = Esp::open(&paths.esp).scan().unwrap_or_else(|e| {
        report.warnings.push(format!("the ESP cannot be read: {e}"));
        EspState::default()
    });
    let Some(booted) = booted_counter(&cmdline) else {
        report
            .warnings
            .push("not booted from a Tetsu snapshot, no verdict recorded".to_owned());
        return report;
    };
    // Records are written under the transaction lock, never waited for: a
    // running transaction may be writing one, and the next boot catches up.
    let _lock = match TxLock::acquire(&paths.lock) {
        Ok(lock) => lock,
        Err(StoreError::Busy { .. }) => {
            report
                .warnings
                .push("a transaction is running, no verdict recorded this time".to_owned());
            return report;
        }
        Err(e) => {
            report
                .warnings
                .push(format!("the verdict was not recorded: {e}"));
            return report;
        }
    };
    match record(&Records::new(&paths.records), &esp, booted, passed) {
        Ok(written) => report.written = written,
        Err(e) => report
            .warnings
            .push(format!("the verdict was not recorded: {e}")),
    }
    report
}

/// What to print for a report.
pub fn lines(report: &Report) -> Vec<String> {
    const NAME: &str = "tetsu-boot-check";
    let mut out = Vec::new();
    if report.failures.is_empty() {
        out.push(format!("{NAME}: the boot is healthy"));
    }
    out.extend(report.failures.iter().map(|f| format!("{NAME}: {f}")));
    for (counter, verdict) in &report.written {
        out.push(format!("{NAME}: snapshot {counter} recorded {verdict:?}"));
    }
    out.extend(report.warnings.iter().map(|w| format!("{NAME}: {w}")));
    out
}

fn gather(paths: &Paths, runner: &dyn Runner) -> Result<Facts, String> {
    let mountinfo_path = paths.proc.join("self/mountinfo");
    let mountinfo = fs::read_to_string(&mountinfo_path)
        .map_err(|e| format!("{}: {e}", mountinfo_path.display()))?;
    let arg = |s: &str| OsString::from(s);
    let listing = runner
        .output(
            "systemctl",
            &[
                arg("list-units"),
                arg("--state=failed"),
                arg("--plain"),
                arg("--no-legend"),
                arg("tetsu-*"),
            ],
        )
        .map_err(|e| e.to_string())?;
    let show = runner
        .output(
            "systemctl",
            &[
                arg("show"),
                arg("-p"),
                arg("LoadState,ActiveState"),
                arg("greetd.service"),
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(Facts {
        mount_points: facts::mount_points(&mountinfo),
        failed_units: facts::failed_units(&listing),
        greetd: facts::greetd(&show),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::facts::Greetd;
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::EspEntry;
    use crate::store::record::Record;
    use crate::tx::fake::{Reply, ScriptedRunner};

    fn facts() -> Facts {
        Facts {
            mount_points: ["/", "/var", "/home"].map(String::from).into(),
            failed_units: Vec::new(),
            greetd: Greetd::NotInstalled,
        }
    }

    #[test]
    fn a_healthy_system_passes() {
        assert_eq!(evaluate(&facts()), Ok(()));
        let mut f = facts();
        f.greetd = Greetd::Active;
        f.failed_units = vec!["systemd-timesyncd.service".into(), "foo.service".into()];
        assert_eq!(evaluate(&f), Ok(()));
    }

    #[test]
    fn every_failure_is_listed() {
        let mut f = facts();
        f.mount_points.remove("/var");
        f.failed_units = vec!["tetsu-keyring-init.service".into(), "other.service".into()];
        f.greetd = Greetd::NotActive("failed".into());
        assert_eq!(
            evaluate(&f),
            Err(vec![
                Failure::NotMounted("/var".into()),
                Failure::UnitFailed("tetsu-keyring-init.service".into()),
                Failure::GreetdNotActive("failed".into()),
            ])
        );
    }

    #[test]
    fn a_missing_root_mount_fails() {
        let mut f = facts();
        f.mount_points.clear();
        assert_eq!(
            evaluate(&f),
            Err(vec![
                Failure::NotMounted("/".into()),
                Failure::NotMounted("/var".into())
            ])
        );
    }

    #[test]
    fn failures_read_as_sentences() {
        assert_eq!(
            Failure::NotMounted("/var".into()).to_string(),
            "/var is not mounted"
        );
        assert_eq!(
            Failure::UnitFailed("tetsu-x.service".into()).to_string(),
            "tetsu-x.service has failed"
        );
        assert_eq!(
            Failure::GreetdNotActive("failed".into()).to_string(),
            "greetd.service is failed"
        );
    }

    fn records() -> (tempfile::TempDir, Records) {
        let dir = tempfile::tempdir().unwrap();
        let r = Records::new(&dir.path().join("transactions"));
        (dir, r)
    }

    fn esp(names: &[&str]) -> EspState {
        let mut st = EspState::default();
        for n in names {
            let Some(FileKind::Entry(name)) = parse_file_name(n) else {
                panic!("{n}")
            };
            st.entries.push(EspEntry {
                name,
                path: PathBuf::from(n),
                size: 1,
            });
        }
        st
    }

    fn rec(counter: u64, boot: Option<BootVerdict>) -> Record {
        Record {
            counter,
            boot,
            ..Record::default()
        }
    }

    fn verdict(r: &Records, n: u64) -> Option<BootVerdict> {
        r.load().unwrap()[&n].boot
    }

    #[test]
    fn a_pass_records_the_booted_snapshot_good() {
        let (_d, r) = records();
        r.write(&rec(2, None)).unwrap();
        let st = esp(&["tetsu-2+2-1.efi"]);
        let written = record(&r, &st, 2, true).unwrap();
        assert_eq!(written, [(2, BootVerdict::Good)]);
        assert_eq!(verdict(&r, 2), Some(BootVerdict::Good));
    }

    #[test]
    fn a_pass_rebuilds_a_missing_record_first() {
        let (_d, r) = records();
        let st = esp(&["tetsu-1.efi", "tetsu-2+2-1.efi"]);
        record(&r, &st, 2, true).unwrap();
        let all = r.load().unwrap();
        assert!(all[&2].reconstructed);
        assert_eq!(all[&2].boot, Some(BootVerdict::Good));
        assert!(all[&1].reconstructed);
    }

    #[test]
    fn a_record_that_is_already_good_is_not_rewritten() {
        let (_d, r) = records();
        r.write(&rec(2, Some(BootVerdict::Good))).unwrap();
        let before = fs::metadata(r.path(2)).unwrap().modified().unwrap();
        let written = record(&r, &esp(&["tetsu-2.efi"]), 2, true).unwrap();
        assert!(written.is_empty());
        assert_eq!(fs::metadata(r.path(2)).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn a_pass_with_no_entry_and_no_record_is_not_found() {
        let (_d, r) = records();
        assert!(matches!(
            record(&r, &esp(&[]), 2, true),
            Err(StoreError::NotFound(2))
        ));
    }

    #[test]
    fn a_failure_on_the_last_try_records_bad() {
        let (_d, r) = records();
        r.write(&rec(2, None)).unwrap();
        let st = esp(&["tetsu-2+0-3.efi"]);
        let written = record(&r, &st, 2, false).unwrap();
        assert_eq!(written, [(2, BootVerdict::Bad)]);
        assert_eq!(verdict(&r, 2), Some(BootVerdict::Bad));
    }

    #[test]
    fn a_failure_with_tries_left_records_nothing() {
        let (_d, r) = records();
        r.write(&rec(2, None)).unwrap();
        let written = record(&r, &esp(&["tetsu-2+1-2.efi"]), 2, false).unwrap();
        assert!(written.is_empty());
        assert_eq!(verdict(&r, 2), None);
    }

    #[test]
    fn a_blessed_entry_with_no_verdict_is_caught_up_as_good() {
        // Its boot check found the lock busy: blessed on the ESP, no verdict.
        let (_d, r) = records();
        r.write(&rec(2, None)).unwrap();
        r.write(&rec(3, None)).unwrap();
        let st = esp(&["tetsu-2.efi", "tetsu-3+2-1.efi"]);
        let written = record(&r, &st, 3, false).unwrap();
        assert_eq!(written, [(2, BootVerdict::Good)]);
        assert_eq!(verdict(&r, 2), Some(BootVerdict::Good));
        assert_eq!(verdict(&r, 3), None);
    }

    #[test]
    fn every_run_catches_up_on_exhausted_entries() {
        let (_d, r) = records();
        r.write(&rec(2, None)).unwrap();
        r.write(&rec(3, None)).unwrap();
        r.write(&rec(4, Some(BootVerdict::Good))).unwrap();
        let st = esp(&[
            "tetsu-2+0-3.efi",
            "tetsu-3+1-2.efi",
            "tetsu-4.efi",
            "tetsu-5+2-1.efi",
        ]);
        let written = record(&r, &st, 5, true).unwrap();
        assert_eq!(written, [(2, BootVerdict::Bad), (5, BootVerdict::Good)]);
        assert_eq!(verdict(&r, 3), None);
        assert_eq!(verdict(&r, 4), Some(BootVerdict::Good));
    }

    #[test]
    fn a_passing_boot_on_its_last_try_is_good_not_bad() {
        let (_d, r) = records();
        r.write(&rec(2, None)).unwrap();
        record(&r, &esp(&["tetsu-2+0-3.efi"]), 2, true).unwrap();
        assert_eq!(verdict(&r, 2), Some(BootVerdict::Good));
    }

    struct Sys {
        _dir: tempfile::TempDir,
        paths: Paths,
    }

    /// A system with `/` and `/var` mounted, booted from snapshot 2 on its
    /// last try, with its record and its ESP entry.
    fn sys(tries: &str) -> Sys {
        let dir = tempfile::tempdir().unwrap();
        let proc = dir.path().join("proc");
        fs::create_dir_all(proc.join("self")).unwrap();
        fs::write(
            proc.join("cmdline"),
            "rw rootflags=subvol=@snapshots/2/root\n",
        )
        .unwrap();
        fs::write(
            proc.join("self/mountinfo"),
            "30 27 0:31 / / rw - btrfs /dev/vda2 rw\n31 30 0:31 / /var rw - btrfs /dev/vda2 rw\n",
        )
        .unwrap();
        let esp = dir.path().join("efi");
        fs::create_dir_all(esp.join("EFI/Linux")).unwrap();
        fs::write(esp.join(format!("EFI/Linux/tetsu-2{tries}.efi")), "uki").unwrap();
        let paths = Paths {
            proc,
            esp,
            records: dir.path().join("transactions"),
            lock: dir.path().join("run/tetsu/tx.lock"),
        };
        Records::new(&paths.records).write(&rec(2, None)).unwrap();
        Sys { _dir: dir, paths }
    }

    #[test]
    fn a_healthy_boot_is_recorded_good() {
        let s = sys("+2-1");
        let runner = ScriptedRunner::new();
        runner.on(
            &["systemctl", "show"],
            Reply::Out("LoadState=not-found\n".into()),
        );
        let report = run(&s.paths, &runner);
        assert!(
            report.failures.is_empty() && report.warnings.is_empty(),
            "{report:?}"
        );
        assert_eq!(report.written, [(2, BootVerdict::Good)]);
        assert_eq!(
            lines(&report),
            [
                "tetsu-boot-check: the boot is healthy",
                "tetsu-boot-check: snapshot 2 recorded Good"
            ]
        );
        assert!(
            runner
                .lines()
                .iter()
                .any(|l| l == "systemctl list-units --state=failed --plain --no-legend tetsu-*")
        );
        assert!(
            runner
                .lines()
                .iter()
                .any(|l| l == "systemctl show -p LoadState,ActiveState greetd.service")
        );
    }

    #[test]
    fn a_failed_tetsu_unit_fails_the_boot_and_the_last_try_records_bad() {
        let s = sys("+0-3");
        let runner = ScriptedRunner::new();
        runner.on(
            &["systemctl", "list-units"],
            Reply::Out("tetsu-keyring-init.service loaded failed failed x\n".into()),
        );
        let report = run(&s.paths, &runner);
        assert_eq!(
            report.failures,
            [Failure::UnitFailed("tetsu-keyring-init.service".into())]
        );
        assert_eq!(report.written, [(2, BootVerdict::Bad)]);
    }

    #[test]
    fn a_failed_run_with_tries_left_records_nothing() {
        let s = sys("+1-2");
        let runner = ScriptedRunner::new();
        runner.on(
            &["systemctl", "list-units"],
            Reply::Out("tetsu-x.service loaded failed failed x\n".into()),
        );
        let report = run(&s.paths, &runner);
        assert_eq!(report.failures.len(), 1);
        assert!(report.written.is_empty());
    }

    #[test]
    fn a_system_that_cannot_be_read_is_not_healthy() {
        let s = sys("+2-1");
        let runner = ScriptedRunner::new();
        runner.on(&["systemctl"], Reply::Fail("no bus".into()));
        let report = run(&s.paths, &runner);
        assert!(matches!(report.failures[..], [Failure::CheckFailed(_)]));
        assert!(report.written.is_empty());
    }

    #[test]
    fn a_broken_record_store_is_a_warning_not_a_failure() {
        let s = sys("+2-1");
        fs::write(Records::new(&s.paths.records).path(9), "{").unwrap();
        let report = run(&s.paths, &ScriptedRunner::new());
        assert!(report.failures.is_empty());
        assert_eq!(report.warnings.len(), 1, "{report:?}");
    }

    #[test]
    fn another_root_records_nothing() {
        let s = sys("+2-1");
        fs::write(s.paths.proc.join("cmdline"), "root=/dev/sda1 rw\n").unwrap();
        let report = run(&s.paths, &ScriptedRunner::new());
        assert!(report.failures.is_empty() && report.written.is_empty());
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn a_running_transaction_defers_the_verdict_not_the_health() {
        let s = sys("+2-1");
        let _held = crate::store::lock::TxLock::acquire(&s.paths.lock).unwrap();
        let runner = ScriptedRunner::new();
        runner.on(
            &["systemctl", "show"],
            Reply::Out("LoadState=not-found\n".into()),
        );
        let report = run(&s.paths, &runner);
        assert!(report.failures.is_empty(), "{report:?}");
        assert!(report.written.is_empty());
        assert!(report.warnings[0].contains("transaction"), "{report:?}");
        let rec = Records::new(&s.paths.records).load().unwrap();
        assert_eq!(rec[&2].boot, None);
    }
}
