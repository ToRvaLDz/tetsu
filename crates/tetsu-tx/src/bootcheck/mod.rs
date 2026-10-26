//! The boot check (decision #18): after a boot reaches `multi-user.target`
//! the system is healthy or it is not. Healthy records the booted snapshot
//! as good and lets `boot-complete.target` be reached, so
//! `systemd-bless-boot` renames the ESP entry; unhealthy leaves the entry
//! on trial, and boot counting falls back once the tries are gone.
//!
//! The checks: `/` and `/var` are mounted, no `tetsu-*` system unit is in
//! the `failed` state, and `greetd.service` is active when it is installed.

pub mod facts;

use std::fmt;

use crate::boot::entry::BootState;
use crate::boot::esp::EspState;
use crate::store::StoreError;
use crate::store::record::{BootVerdict, Records};
use facts::{Facts, Greetd};

/// A check that failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    NotMounted(String),
    UnitFailed(String),
    GreetdNotActive(String),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMounted(p) => write!(f, "{p} is not mounted"),
            Self::UnitFailed(u) => write!(f, "{u} has failed"),
            Self::GreetdNotActive(s) => write!(f, "greetd.service is {s}"),
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::facts::Greetd;
    use super::*;
    use crate::boot::entry::{FileKind, parse_file_name};
    use crate::boot::esp::EspEntry;
    use crate::store::record::Record;

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
}
