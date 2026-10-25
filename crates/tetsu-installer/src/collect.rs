//! Collecting the answers when an answer file may hold some of them: the
//! file answers what it can, the frontend asks the rest, and the summary
//! is skipped only when nothing had to be asked and the file says so.

use std::fs;
use std::path::{Path, PathBuf};

use tetsu_installer_core::agent::{self, AgentBackend};
use tetsu_installer_core::answers::Answers;
use tetsu_installer_core::disk::{self, Disk, DiskChoice};
use tetsu_installer_core::driver;
use tetsu_installer_core::env::Env;
use tetsu_installer_core::error::InstallError;
use tetsu_installer_core::frontend::{Frontend, ask_valid};
use tetsu_installer_core::keyboard::{self, Locale};
use tetsu_installer_core::preflight::{Preflight, SecureBoot};
use tetsu_installer_core::secret::Secret;
use tetsu_installer_core::secureboot::foreign;
use tetsu_installer_core::summary;
use tetsu_installer_core::unlock::{self, Unlock, recovery};
use tetsu_installer_core::user::{self, Password, UserAccount};

use crate::answer_file::{AgentAnswer, AnswerFile, KeyboardAnswer};

fn fail(msg: impl Into<String>) -> InstallError {
    InstallError::Failed(msg.into())
}

/// What the question flow produced.
#[derive(Debug)]
pub struct Collected {
    pub answers: Answers,
    /// A certificate to trust in the db after the keys, when the user
    /// imported one for another system on the machine.
    pub trust_cert: Option<PathBuf>,
}

/// Run preflight, answer from `file` (when one is in use), ask the rest,
/// and return the confirmed answers. Nothing is written anywhere.
pub fn collect(
    fe: &mut dyn Frontend,
    env: &Env<'_>,
    file: Option<&AnswerFile>,
) -> Result<Collected, InstallError> {
    let scripted = file.is_some();
    let default = AnswerFile::default();
    let file = file.unwrap_or(&default);
    fe.progress("preflight", "checking the machine");
    let pf = Preflight::detect(env)?;
    pf.check()?;
    driver::report(fe, &pf);

    let mut asked = false;
    let locale = locale(fe, file.keyboard.as_ref(), &mut asked)?;
    let disk = disk_choice(fe, env, &pf, file.disk_by_id.as_deref(), &mut asked)?;
    let unlock = unlock(fe, env, &pf, file, &mut asked)?;
    let user = account(fe, file, &unlock.passphrase, &mut asked)?;
    let agent = agent_backend(fe, file.agent.as_ref(), &mut asked)?;
    let answers = Answers {
        locale,
        disk,
        unlock,
        user,
        hostname: user::hostname(env.entropy)?,
        agent,
    };
    let answers = if file.confirm && !asked {
        fe.message(&answers.summary_lines().join("\n"));
        answers
    } else {
        if file.confirm {
            fe.message("install.confirm is ignored: some answers are not in the file.");
        }
        summary::run(fe, env, &pf, answers)?
    };
    // Only a coming enrollment makes the empty db matter; the disk is final.
    let trust_cert = if pf.secure_boot == SecureBoot::SetupMode {
        let others = foreign::other_systems(&pf.disks, &answers.disk);
        foreign::offer(fe, &env.root, &others, scripted)?
    } else {
        None
    };
    Ok(Collected {
        answers,
        trust_cert,
    })
}

fn locale(
    fe: &mut dyn Frontend,
    given: Option<&KeyboardAnswer>,
    asked: &mut bool,
) -> Result<Locale, InstallError> {
    let Some(k) = given else {
        *asked = true;
        return keyboard::run(fe);
    };
    let timezone = match &k.timezone {
        Some(tz) => tz.clone(),
        None => match keyboard::zones_for(&k.layout) {
            [only] => (*only).to_owned(),
            _ => {
                *asked = true;
                keyboard::ask_timezone(fe, &k.layout)?
            }
        },
    };
    Ok(Locale {
        language: k
            .locale
            .clone()
            .unwrap_or_else(|| keyboard::locale_for(&k.layout).to_owned()),
        keyboard: k.layout.clone(),
        timezone,
        rtc_utc: true,
    })
}

/// Whether the `/dev/disk/by-id` link `id` points at `disk`.
fn names_disk(root: &Path, id: &str, disk: &Disk) -> bool {
    let link = root.join("dev/disk/by-id").join(id);
    let by_link = fs::read_link(link)
        .ok()
        .and_then(|t| t.file_name().map(|n| n == disk.name.as_str()))
        .unwrap_or(false);
    by_link
        || disk
            .by_id
            .as_ref()
            .and_then(|p| p.file_name())
            .is_some_and(|n| n == id)
}

fn disk_choice(
    fe: &mut dyn Frontend,
    env: &Env<'_>,
    pf: &Preflight,
    by_id: Option<&str>,
    asked: &mut bool,
) -> Result<DiskChoice, InstallError> {
    let Some(id) = by_id else {
        *asked = true;
        return disk::run(fe, &pf.disks);
    };
    pf.disks
        .iter()
        .find(|d| names_disk(&env.root, id, d))
        .map(DiskChoice::from)
        .ok_or_else(|| {
            fail(format!(
                "disk.confirm_by_id: no installable disk has the by-id name {id} \
                 (the installer's own boot disk is never offered)"
            ))
        })
}

fn unlock(
    fe: &mut dyn Frontend,
    env: &Env<'_>,
    pf: &Preflight,
    file: &AnswerFile,
    asked: &mut bool,
) -> Result<Unlock, InstallError> {
    let level = match file.level {
        Some(level) if level.needs_tpm() && !pf.tpm2 => {
            return Err(fail(format!(
                "unlock.level = {}: this machine has no TPM 2.0",
                level.id()
            )));
        }
        Some(level) => level,
        None => {
            *asked = true;
            unlock::ask_level(fe, pf.tpm2)?
        }
    };
    let passphrase = match &file.passphrase {
        Some(p) => p.clone(),
        None => {
            *asked = true;
            unlock::ask_passphrase(fe)?
        }
    };
    let recovery_key = recovery::generate(env.entropy)?;
    // Nobody is there to retype the key when the file answered everything.
    if file.level.is_some() && file.passphrase.is_some() {
        unlock::show_key(fe, &recovery_key)?;
    } else {
        unlock::show_and_confirm_key(fe, &recovery_key)?;
    }
    Ok(Unlock {
        level,
        passphrase,
        recovery_key,
    })
}

fn account(
    fe: &mut dyn Frontend,
    file: &AnswerFile,
    passphrase: &Secret,
    asked: &mut bool,
) -> Result<UserAccount, InstallError> {
    let (username, full_name) = match &file.username {
        Some(name) => (name.clone(), file.full_name.clone()),
        None => {
            *asked = true;
            let name = ask_valid(fe, "User name", &user::validate_username)?;
            let full = match &file.full_name {
                Some(full) => full.clone(),
                None => ask_valid(fe, "Full name (optional)", &user::validate_full_name)?,
            };
            (name, Some(full))
        }
    };
    let password = match &file.password {
        Some(hash) => hash.clone(),
        None => {
            *asked = true;
            Password::Plain(user::ask_password(fe, &username, passphrase)?)
        }
    };
    Ok(UserAccount {
        username,
        full_name: full_name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty()),
        password,
    })
}

fn agent_backend(
    fe: &mut dyn Frontend,
    given: Option<&AgentAnswer>,
    asked: &mut bool,
) -> Result<Option<AgentBackend>, InstallError> {
    Ok(match given {
        None => {
            *asked = true;
            agent::run(fe)?
        }
        Some(AgentAnswer::None) => None,
        Some(AgentAnswer::Cli(tool)) => Some(AgentBackend::Cli(*tool)),
        Some(AgentAnswer::Api { provider, key }) => {
            let key = match key {
                Some(key) => key.clone(),
                None => {
                    *asked = true;
                    agent::ask_key(fe, *provider)?
                }
            };
            Some(AgentBackend::Api {
                provider: *provider,
                key,
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::fs::symlink;

    use tetsu_installer_core::disk::os::NullProbe;
    use tetsu_installer_core::entropy::SystemEntropy;
    use tetsu_installer_core::unlock::UnlockLevel;
    use tetsu_tx::boot::BootError;
    use tetsu_tx::boot::tool::Runner;

    use super::*;
    use crate::text::TextFrontend;

    const LSBLK: &str = r#"{"blockdevices":[
      {"name":"sda","path":"/dev/sda","size":512110190592,"model":"Virtual","type":"disk","rm":false},
      {"name":"sdb","path":"/dev/sdb","size":15609618432,"model":"USB","type":"disk","rm":true}]}"#;

    /// Same disks, with an NTFS partition on the stick-like second disk.
    const LSBLK_WITH_WINDOWS: &str = r#"{"blockdevices":[
      {"name":"sda","path":"/dev/sda","size":512110190592,"model":"Virtual","type":"disk","rm":false},
      {"name":"sdb","path":"/dev/sdb","size":15609618432,"model":"USB","type":"disk","rm":true,
       "children":[{"name":"sdb1","path":"/dev/sdb1","size":15609618432,"type":"part","fstype":"ntfs"}]}]}"#;

    struct LsblkOnly(&'static str);

    impl Runner for LsblkOnly {
        fn run(&self, program: &str, _: &[OsString]) -> Result<(), BootError> {
            panic!("collecting answers must not run {program}");
        }

        fn output(&self, program: &str, _: &[OsString]) -> Result<String, BootError> {
            assert_eq!(program, "lsblk");
            Ok(self.0.to_owned())
        }
    }

    /// A UEFI machine with a TPM and two disks linked in by-id.
    fn machine(tpm: bool) -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("sys/firmware/efi")).unwrap();
        if tpm {
            let dir = t.path().join("sys/class/tpm/tpm0");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("tpm_version_major"), "2\n").unwrap();
        }
        let ids = t.path().join("dev/disk/by-id");
        fs::create_dir_all(&ids).unwrap();
        symlink("../../sda", ids.join("virtio-tetsu0")).unwrap();
        symlink("../../sda", ids.join("wwn-0x5000")).unwrap();
        symlink("../../sdb", ids.join("usb-Stick_1")).unwrap();
        t
    }

    fn run(
        t: &tempfile::TempDir,
        file: &str,
        input: &str,
    ) -> (Result<Answers, InstallError>, String) {
        let (result, out) = run_on(t, LSBLK, file, input);
        (result.map(|c| c.answers), out)
    }

    fn run_on(
        t: &tempfile::TempDir,
        lsblk: &'static str,
        file: &str,
        input: &str,
    ) -> (Result<Collected, InstallError>, String) {
        let file = AnswerFile::parse(file).unwrap();
        let env = Env {
            root: t.path().to_owned(),
            runner: &LsblkOnly(lsblk),
            probe: &NullProbe,
            entropy: &SystemEntropy,
            boot_partition: None,
        };
        let mut out = Vec::new();
        let result = {
            let mut fe = TextFrontend::new(input.as_bytes(), &mut out);
            collect(&mut fe, &env, Some(&file))
        };
        (result, String::from_utf8(out).unwrap())
    }

    const COMPLETE: &str = r#"
[keyboard]
layout = "it"
[disk]
confirm_by_id = "virtio-tetsu0"
[unlock]
level = "comfort"
passphrase = "Correct-Horse-9-Battery"
[user]
name = "marco"
password_hash = "$6$salt$abcdef"
[agent]
backend = "none"
[install]
confirm = true
"#;

    /// `SetupMode` set, and a `db` of the given bytes when there are any.
    fn setup_mode(t: &tempfile::TempDir, db: Option<&[u8]>) {
        let dir = t.path().join("sys/firmware/efi/efivars");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SetupMode-8be4df61-93ca-11d2-aa0d-00e098032b8c"),
            [6, 0, 0, 0, 1],
        )
        .unwrap();
        if let Some(bytes) = db {
            fs::write(dir.join("db-d719b2cb-3d3a-4596-a3bc-dad00e67656f"), bytes).unwrap();
        }
    }

    #[test]
    fn another_system_and_an_empty_db_warn_without_asking_when_a_file_is_used() {
        let t = machine(true);
        setup_mode(&t, None);
        let (c, out) = run_on(&t, LSBLK_WITH_WINDOWS, COMPLETE, "");
        let c = c.unwrap();
        assert_eq!(c.trust_cert, None);
        assert!(
            out.contains("other systems were found: Windows (NTFS) (sdb)"),
            "{out}"
        );
        assert!(out.contains("tetsu secureboot trust <file>"), "{out}");
        assert!(!out.contains("Certificate to trust"), "{out}");
    }

    #[test]
    fn the_warning_needs_setup_mode_another_system_and_an_empty_db() {
        // Not in Setup Mode: nothing will be enrolled.
        let t = machine(true);
        let (_, out) = run_on(&t, LSBLK_WITH_WINDOWS, COMPLETE, "");
        assert!(!out.contains("other systems"), "{out}");
        // A db with entries.
        let t = machine(true);
        setup_mode(&t, Some(&[7, 0, 0, 0, 1, 2, 3]));
        let (_, out) = run_on(&t, LSBLK_WITH_WINDOWS, COMPLETE, "");
        assert!(!out.contains("other systems"), "{out}");
        // No other system.
        let t = machine(true);
        setup_mode(&t, None);
        let (_, out) = run_on(&t, LSBLK, COMPLETE, "");
        assert!(!out.contains("other systems"), "{out}");
    }

    #[test]
    fn a_complete_file_installs_without_a_single_question() {
        let t = machine(true);
        let (a, out) = run(&t, COMPLETE, "");
        let a = a.unwrap();
        assert_eq!(a.locale.timezone, "Europe/Rome");
        assert_eq!(a.disk.name, "sda");
        assert_eq!(a.unlock.level, UnlockLevel::Comfort);
        assert_eq!(a.user.password, Password::Hash("$6$salt$abcdef".into()));
        assert_eq!(a.agent, None);
        // The recovery key is on the console, generated here, and the
        // summary was printed rather than asked.
        assert!(out.contains(a.unlock.recovery_key.expose()));
        assert!(out.contains("Hostname:  tetsu-"));
        assert!(!out.contains("Looks right"));
        assert!(!out.contains("Correct-Horse") && !out.contains("$6$salt"));
    }

    #[test]
    fn any_other_link_of_the_disk_confirms_it_too() {
        let t = machine(true);
        let file = COMPLETE.replace("virtio-tetsu0", "wwn-0x5000");
        assert_eq!(run(&t, &file, "").0.unwrap().disk.name, "sda");
    }

    #[test]
    fn a_by_id_that_matches_no_candidate_stops_the_install() {
        let t = machine(true);
        for id in ["virtio-nope", "usb-Stick_1x"] {
            let file = COMPLETE.replace("virtio-tetsu0", id);
            let (a, _) = run(&t, &file, "");
            assert!(a.unwrap_err().to_string().contains("disk.confirm_by_id"));
        }
    }

    #[test]
    fn missing_answers_are_asked_and_the_summary_comes_back() {
        let t = machine(true);
        let file = COMPLETE.replace("name = \"marco\"\n", "");
        // User name, full name, then the summary: install and confirm.
        let (a, out) = run(&t, &file, "anna\n\n1\ny\n");
        assert_eq!(a.unwrap().user.username, "anna");
        assert!(out.contains("install.confirm is ignored"));
        assert!(out.contains("Wipe sda and install?"));
    }

    #[test]
    fn without_confirm_the_summary_is_asked_even_when_complete() {
        let t = machine(true);
        let file = COMPLETE.replace("confirm = true", "confirm = false");
        let (a, out) = run(&t, &file, "1\nn\n1\ny\n");
        a.unwrap();
        assert_eq!(out.matches("Wipe sda and install?").count(), 2);
    }

    #[test]
    fn a_tpm_level_without_a_tpm_fails() {
        let t = machine(false);
        let (a, _) = run(&t, COMPLETE, "");
        assert!(a.unwrap_err().to_string().contains("no TPM"));
        let paranoid = COMPLETE.replace("comfort", "paranoid");
        assert!(run(&t, &paranoid, "").0.is_ok());
    }

    #[test]
    fn a_multi_zone_layout_asks_only_the_timezone() {
        let t = machine(true);
        let file = COMPLETE.replace("\"it\"", "\"us\"");
        let (a, out) = run(&t, &file, "1\n1\ny\n");
        assert_eq!(a.unwrap().locale.timezone, "America/New_York");
        assert!(out.contains("Timezone:"));
    }

    #[test]
    fn an_api_key_missing_from_the_file_is_asked_hidden() {
        let t = machine(true);
        let file = COMPLETE.replace(
            "backend = \"none\"",
            "backend = \"api\"\nprovider = \"openai\"",
        );
        let (a, out) = run(&t, &file, "sk-from-the-user\n1\ny\n");
        let a = a.unwrap();
        assert!(matches!(
            a.agent,
            Some(AgentBackend::Api { key, .. }) if key.expose() == "sk-from-the-user"
        ));
        assert!(out.contains("OpenAI API key (not shown)"));
    }
}
