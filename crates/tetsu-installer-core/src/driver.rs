//! The step driver: preflight, the six steps in order, then the summary.

use crate::answers::{Answers, Step};
use crate::env::Env;
use crate::error::InstallError;
use crate::frontend::Frontend;
use crate::preflight::{Preflight, SecureBoot};
use crate::{agent, disk, keyboard, summary, unlock, user};

/// Run the whole question flow and return the confirmed answers. Nothing
/// is written anywhere.
pub fn run_steps(fe: &mut dyn Frontend, env: &Env<'_>) -> Result<Answers, InstallError> {
    fe.progress("preflight", "checking the machine");
    let pf = Preflight::detect(env)?;
    pf.check()?;
    report(fe, &pf);

    let locale = keyboard::run(fe)?;
    let disk = disk::run(fe, &pf.disks)?;
    let unlock = unlock::run(fe, pf.tpm2, env.entropy)?;
    let user = user::run(fe, &unlock.passphrase)?;
    let agent = agent::run(fe)?;
    let answers = Answers {
        locale,
        disk,
        unlock,
        user,
        hostname: user::hostname(env.entropy)?,
        agent,
    };
    summary::run(fe, env, &pf, answers)
}

fn report(fe: &mut dyn Frontend, pf: &Preflight) {
    let sb = match pf.secure_boot {
        SecureBoot::Off => "off",
        SecureBoot::SetupMode => "Setup Mode",
        SecureBoot::Enforcing => "enforcing",
    };
    let tpm = if pf.tpm2 { "present" } else { "absent" };
    fe.message(&format!("UEFI: yes. TPM 2.0: {tpm}. Secure Boot: {sb}."));
}

/// Redo one step with the other answers kept.
pub fn redo(
    step: Step,
    fe: &mut dyn Frontend,
    env: &Env<'_>,
    pf: &Preflight,
    current: Answers,
) -> Result<Answers, InstallError> {
    Ok(match step {
        Step::Keyboard => Answers {
            locale: keyboard::run(fe)?,
            ..current
        },
        Step::Regional => Answers {
            locale: keyboard::edit_regional(fe, &current.locale)?,
            ..current
        },
        Step::Disk => Answers {
            disk: disk::run(fe, &pf.disks)?,
            ..current
        },
        Step::Unlock => Answers {
            unlock: unlock::run(fe, pf.tpm2, env.entropy)?,
            ..current
        },
        Step::User => Answers {
            user: user::run(fe, &current.unlock.passphrase)?,
            ..current
        },
        Step::Agent => Answers {
            agent: agent::run(fe)?,
            ..current
        },
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;

    use tetsu_tx::boot::BootError;
    use tetsu_tx::boot::tool::Runner;

    use super::*;
    use crate::disk::lsblk::tests::SAMPLE;
    use crate::disk::os::NullProbe;
    use crate::entropy::fake::Counter;
    use crate::testing::Script;
    use crate::unlock::{UnlockLevel, recovery};
    use crate::user::Password;

    /// Serves the lsblk sample and fails on anything that would write.
    struct ReadOnlyRunner;

    impl Runner for ReadOnlyRunner {
        fn run(&self, program: &str, _: &[OsString]) -> Result<(), BootError> {
            panic!("the question flow must not run {program}");
        }

        fn output(&self, program: &str, _: &[OsString]) -> Result<String, BootError> {
            assert_eq!(program, "lsblk");
            Ok(SAMPLE.to_owned())
        }
    }

    fn machine(root: &Path, uefi: bool, tpm: bool, secure_boot: u8) {
        if uefi {
            let vars = root.join("sys/firmware/efi/efivars");
            fs::create_dir_all(&vars).unwrap();
            let guid = "8be4df61-93ca-11d2-aa0d-e39b6740b8c4";
            // SetupMode = 0 and SecureBoot as given.
            fs::write(vars.join(format!("SetupMode-{guid}")), [6, 0, 0, 0, 0]).unwrap();
            fs::write(
                vars.join(format!("SecureBoot-{guid}")),
                [6, 0, 0, 0, secure_boot],
            )
            .unwrap();
        }
        if tpm {
            let dir = root.join("sys/class/tpm/tpm0");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("tpm_version_major"), "2\n").unwrap();
        }
    }

    fn run_with(root: &Path, script: &mut Script) -> Result<Answers, InstallError> {
        let counter = Counter::default();
        let env = Env {
            root: root.to_owned(),
            runner: &ReadOnlyRunner,
            probe: &NullProbe,
            entropy: &counter,
            boot_partition: None,
        };
        run_steps(script, &env)
    }

    fn key() -> String {
        recovery::generate(&Counter::default())
            .unwrap()
            .expose()
            .to_owned()
    }

    /// Keyboard, disk, unlock, user and agent answers up to the summary.
    fn happy_path() -> Script {
        Script::new()
            .pick(0) // it
            .pick(1) // sdb
            .yes(true)
            .pick(0) // Paranoid
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text(&key())
            .text("marco")
            .text("Marco M")
            .secret("session-pw-1")
            .secret("session-pw-1")
            .pick(6) // skip the agent backend
    }

    #[test]
    fn full_flow_collects_every_answer() {
        let t = tempfile::tempdir().unwrap();
        machine(t.path(), true, true, 0);
        let mut script = happy_path().pick(0).yes(true);
        let a = run_with(t.path(), &mut script).unwrap();
        assert_eq!(a.locale.language, "it_IT.UTF-8");
        assert_eq!(a.locale.timezone, "Europe/Rome");
        assert_eq!(a.disk.name, "sdb");
        assert_eq!(a.unlock.level, UnlockLevel::Paranoid);
        assert_eq!(a.user.username, "marco");
        assert_eq!(
            a.user.password,
            Password::Plain(crate::secret::Secret::new("session-pw-1"))
        );
        assert!(a.hostname.starts_with("tetsu-") && a.hostname.len() == 10);
        assert_eq!(a.agent, None);
        script.assert_done();
        let summary = script
            .messages
            .iter()
            .find(|m| m.contains("Hostname:"))
            .unwrap();
        assert!(!summary.contains("Correct-Horse") && !summary.contains("session-pw"));
    }

    #[test]
    fn the_summary_can_redo_a_step_and_decline_the_install() {
        let t = tempfile::tempdir().unwrap();
        machine(t.path(), true, true, 0);
        let mut script = happy_path()
            .pick(2) // Change: language and timezone
            .text("en_GB.UTF-8")
            .text("Europe/London")
            .pick(0)
            .yes(false) // not yet
            .pick(5) // Change: user
            .text("anna")
            .text("")
            .secret("another-pw-1")
            .secret("another-pw-1")
            .pick(0)
            .yes(true);
        let a = run_with(t.path(), &mut script).unwrap();
        assert_eq!(a.locale.keyboard, "it");
        assert_eq!(a.locale.language, "en_GB.UTF-8");
        assert_eq!(a.locale.timezone, "Europe/London");
        assert_eq!(a.user.username, "anna");
        assert_eq!(a.disk.name, "sdb");
        script.assert_done();
    }

    #[test]
    fn without_tpm_the_level_question_is_skipped() {
        let t = tempfile::tempdir().unwrap();
        machine(t.path(), true, false, 0);
        // happy_path() answers the level; drop that pick by building it here.
        let mut script = Script::new()
            .pick(0)
            .pick(1)
            .yes(true)
            .secret("Correct-Horse-9")
            .secret("Correct-Horse-9")
            .text(&key())
            .text("marco")
            .text("")
            .secret("session-pw-1")
            .secret("session-pw-1")
            .pick(6)
            .pick(0)
            .yes(true);
        let a = run_with(t.path(), &mut script).unwrap();
        assert_eq!(a.unlock.level, UnlockLevel::Paranoid);
        script.assert_done();
    }

    #[test]
    fn preflight_failures_stop_before_any_question() {
        let t = tempfile::tempdir().unwrap();
        let mut script = Script::new();
        assert!(matches!(
            run_with(t.path(), &mut script),
            Err(InstallError::Preflight(_))
        ));
        let enforcing = tempfile::tempdir().unwrap();
        machine(enforcing.path(), true, true, 1);
        assert!(matches!(
            run_with(enforcing.path(), &mut script),
            Err(InstallError::Preflight(_))
        ));
        assert!(script.prompts.is_empty());
    }
}
