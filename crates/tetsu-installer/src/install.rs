//! Running the install plan with the cancel and failure handling of the
//! binary: Ctrl+C is honoured between two actions, after asking again; a
//! failure or a confirmed abort cleans up and leaves the log in the run
//! directory.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};

use tetsu_installer_core::answers::Answers;
use tetsu_installer_core::entropy::Entropy;
use tetsu_installer_core::error::InstallError;
use tetsu_installer_core::frontend::{Frontend, Validate};
use tetsu_installer_core::install::{Deps, Executor, InstallPlan, InstallTarget};
use tetsu_tx::boot::space::SpaceProbe;
use tetsu_tx::boot::tool::Runner;

use crate::cleanup::cleanup;

/// Asked when the user cancels after the disk was touched.
pub const ABORT_PROMPT: &str = "Abort the install? The disk is already wiped";

/// What the machine offers the install.
pub struct Machine<'a> {
    pub runner: &'a dyn Runner,
    pub space: &'a dyn SpaceProbe,
    pub entropy: &'a dyn Entropy,
    /// Set by the SIGINT handler.
    pub cancel: &'a AtomicBool,
}

/// How the install ended.
#[derive(Debug)]
pub struct Report {
    pub result: Result<(), InstallError>,
    /// The install log, without secrets.
    pub log: Vec<String>,
    /// Cleanup steps that failed; `None` when no cleanup ran.
    pub cleanup_problems: Option<Vec<String>>,
    /// The install failed after the keys were enrolled: the disk holds the
    /// keys the firmware trusts, so it was left as it is.
    pub disk_kept: bool,
}

/// Whether a failed install is cleaned up. Once the keys are enrolled the
/// disk is kept: wiping it would leave the firmware trusting a Platform
/// Key whose private half is gone.
fn should_clean(failed: bool, keys_enrolled: bool) -> bool {
    failed && !keys_enrolled
}

/// The executor owns the frontend for the whole run; this lets the binary
/// ask its abort question in between.
struct Shared<'a>(&'a RefCell<dyn Frontend + 'a>);

impl Frontend for Shared<'_> {
    fn ask(&mut self, prompt: &str, validate: Validate<'_>) -> Result<String, InstallError> {
        self.0.borrow_mut().ask(prompt, validate)
    }

    fn ask_secret(&mut self, prompt: &str) -> Result<String, InstallError> {
        self.0.borrow_mut().ask_secret(prompt)
    }

    fn choose(&mut self, prompt: &str, options: &[String]) -> Result<usize, InstallError> {
        self.0.borrow_mut().choose(prompt, options)
    }

    fn confirm(&mut self, prompt: &str) -> Result<bool, InstallError> {
        self.0.borrow_mut().confirm(prompt)
    }

    fn progress(&mut self, step: &str, msg: &str) {
        self.0.borrow_mut().progress(step, msg);
    }

    fn message(&mut self, text: &str) {
        self.0.borrow_mut().message(text);
    }

    fn show_qr(&mut self, payload: &str) -> Result<(), InstallError> {
        self.0.borrow_mut().show_qr(payload)
    }
}

/// Run the plan for `answers`. Any error after the first action cleans up.
pub fn install(
    fe: &RefCell<dyn Frontend + '_>,
    machine: &Machine<'_>,
    answers: &Answers,
    target: &InstallTarget,
) -> Report {
    let plan = InstallPlan::build(answers, target);
    let cancelled = || machine.cancel.load(Ordering::SeqCst);
    let deps = Deps {
        runner: machine.runner,
        probe: machine.space,
        entropy: machine.entropy,
        cancelled: &cancelled,
    };
    let mut shared = Shared(fe);
    let mut exec = match Executor::new(deps, answers, target, &mut shared) {
        Ok(exec) => exec,
        // Nothing ran yet: there is nothing to clean up.
        Err(e) => {
            return Report {
                result: Err(e),
                log: Vec::new(),
                cleanup_problems: None,
                disk_kept: false,
            };
        }
    };
    let result = loop {
        match exec.run(&plan) {
            Err(InstallError::Cancelled) => {
                machine.cancel.store(false, Ordering::SeqCst);
                // A closed console counts as a yes: nobody can answer.
                let abort = fe.borrow_mut().confirm(ABORT_PROMPT).unwrap_or(true);
                if abort {
                    break Err(InstallError::Cancelled);
                }
            }
            other => break other,
        }
    };
    // The executor holds the log file open on the ESP; release it first.
    let log = exec.log().lines().to_vec();
    let keys_enrolled = exec.keys_enrolled();
    drop(exec);
    let cleanup_problems =
        should_clean(result.is_err(), keys_enrolled).then(|| cleanup(machine.runner, target));
    Report {
        disk_kept: result.is_err() && keys_enrolled,
        result,
        log,
        cleanup_problems,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use tetsu_installer_core::agent::AgentBackend;
    use tetsu_installer_core::disk::DiskChoice;
    use tetsu_installer_core::keyboard::Locale;
    use tetsu_installer_core::preflight::SecureBoot;
    use tetsu_installer_core::secret::Secret;
    use tetsu_installer_core::unlock::{Unlock, UnlockLevel};
    use tetsu_installer_core::user::{Password, UserAccount};
    use tetsu_tx::boot::BootError;

    use crate::text::TextFrontend;

    use super::*;

    /// Every tool succeeds and prints nothing, so the install stops at the
    /// first thing it needs to read back: the LUKS UUID. `raise` sets the
    /// cancel flag when a call line starts with it.
    struct Tools<'a> {
        calls: RefCell<Vec<String>>,
        raise: Option<(&'static str, &'a AtomicBool)>,
    }

    impl Runner for Tools<'_> {
        fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
            self.output(program, args).map(drop)
        }

        fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
            let a: Vec<String> = args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let line = format!("{program} {}", a.join(" "));
            if let Some((prefix, flag)) = self.raise
                && line.starts_with(prefix)
            {
                flag.store(true, Ordering::SeqCst);
            }
            self.calls.borrow_mut().push(line);
            Ok(String::new())
        }
    }

    struct Roomy;

    impl SpaceProbe for Roomy {
        fn available_bytes(&self, _: &Path) -> Result<u64, BootError> {
            Ok(1 << 40)
        }
    }

    struct Fixed;

    impl Entropy for Fixed {
        fn fill(&self, buf: &mut [u8]) -> Result<(), InstallError> {
            buf.fill(7);
            Ok(())
        }
    }

    fn answers() -> Answers {
        Answers {
            locale: Locale {
                keyboard: "it".into(),
                language: "it_IT.UTF-8".into(),
                timezone: "Europe/Rome".into(),
                rtc_utc: true,
            },
            disk: DiskChoice {
                name: "sda".into(),
                path: "/dev/sda".into(),
                by_id: None,
                size: 1 << 30,
                model: None,
                os: None,
            },
            unlock: Unlock {
                level: UnlockLevel::Paranoid,
                passphrase: Secret::new("Correct-Horse-9-Battery"),
                recovery_key: Secret::new("cccccccc-dddddddd-eeeeeeee-ffffffff"),
            },
            user: UserAccount {
                username: "marco".into(),
                full_name: None,
                password: Password::Hash("$6$salt$hash".into()),
            },
            hostname: "tetsu-ab12".into(),
            agent: None::<AgentBackend>,
        }
    }

    struct Outcome {
        report: Report,
        calls: Vec<String>,
        console: String,
    }

    /// Run on a tempdir machine; `input` answers the abort question.
    fn run(input: &str, raise: Option<&'static str>) -> Outcome {
        let t = tempfile::tempdir().unwrap();
        let disk = t.path().join("sda");
        for node in ["sda", "sda1", "sda2"] {
            fs::write(t.path().join(node), "").unwrap();
        }
        let mut target = InstallTarget::new(
            &disk,
            Path::new("/medium/payload.tar"),
            &t.path().join("work"),
            SecureBoot::Off,
        );
        target.run_dir = t.path().join("run");
        target.cache_dir = t.path().join("cache");
        let cancel = AtomicBool::new(false);
        let tools = Tools {
            calls: RefCell::default(),
            raise: raise.map(|p| (p, &cancel)),
        };
        let fe = RefCell::new(TextFrontend::new(input.as_bytes(), Vec::new()));
        let report = install(
            &fe,
            &Machine {
                runner: &tools,
                space: &Roomy,
                entropy: &Fixed,
                cancel: &cancel,
            },
            &answers(),
            &target,
        );
        let console = String::from_utf8(fe.into_inner().into_parts().1).unwrap();
        let root = t.path().display().to_string();
        let calls = tools
            .calls
            .into_inner()
            .into_iter()
            .map(|c| c.replace(&root, "<t>"))
            .collect();
        Outcome {
            report,
            calls,
            console,
        }
    }

    fn position(calls: &[String], prefix: &str) -> usize {
        calls
            .iter()
            .position(|c| c.starts_with(prefix))
            .unwrap_or_else(|| panic!("no call starting with {prefix:?} in {calls:#?}"))
    }

    #[test]
    fn cleanup_runs_on_failure_unless_the_keys_are_enrolled() {
        assert!(should_clean(true, false));
        assert!(!should_clean(true, true));
        assert!(!should_clean(false, false));
        assert!(!should_clean(false, true));
    }

    #[test]
    fn a_failed_action_cleans_up_and_keeps_the_log() {
        let o = run("", None);
        assert!(matches!(
            o.report.result,
            Err(InstallError::ActionFailed { .. })
        ));
        let problems = o.report.cleanup_problems.unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        // The install got as far as the volume, then everything was released
        // and the signatures wiped.
        let failed = position(&o.calls, "cryptsetup luksUUID");
        let umount = position(&o.calls[failed..], "umount <t>/work/esp") + failed;
        assert!(position(&o.calls[umount..], "wipefs -a <t>/sda2") > 0);
        assert!(o.calls.last().unwrap().starts_with("wipefs -a <t>/sda"));
        assert!(o.report.log.iter().any(|l| l.contains("failed")));
        assert!(!o.console.contains(ABORT_PROMPT));
        let all = o.report.log.join("\n") + &o.console;
        assert!(!all.contains("Correct-Horse"));
    }

    #[test]
    fn a_confirmed_abort_stops_where_the_run_was_and_cleans_up() {
        let o = run("y\n", Some("udevadm"));
        assert!(matches!(o.report.result, Err(InstallError::Cancelled)));
        assert!(o.console.contains(ABORT_PROMPT));
        // The action after the signal never ran.
        assert!(!o.calls.iter().any(|c| c.starts_with("mkfs.vfat")));
        assert!(o.calls.iter().any(|c| c.starts_with("wipefs -a <t>/sda1")));
        assert!(o.report.cleanup_problems.is_some());
    }

    #[test]
    fn declining_the_abort_carries_on_with_the_plan() {
        let o = run("n\n", Some("udevadm"));
        assert_eq!(o.console.matches(ABORT_PROMPT).count(), 1);
        // Back to work: formatting ran, and only the missing UUID stops it.
        assert!(o.calls.iter().any(|c| c.starts_with("mkfs.vfat")));
        assert!(matches!(
            o.report.result,
            Err(InstallError::ActionFailed { .. })
        ));
        // Each action ran once: no restart from the top.
        assert_eq!(
            o.calls.iter().filter(|c| c.starts_with("udevadm")).count(),
            1
        );
    }

    #[test]
    fn a_closed_console_counts_as_an_abort() {
        let o = run("", Some("udevadm"));
        assert!(matches!(o.report.result, Err(InstallError::Cancelled)));
    }
}
