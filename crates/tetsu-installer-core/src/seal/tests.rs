use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::tool::Runner;

use super::*;
use crate::efi::tests_support::{esl, write_pk};

const GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-e39b6740b8c4";
const PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIB\nAQID\n-----END CERTIFICATE-----\n";
const DER: [u8; 6] = [0x30, 0x82, 0x01, 0x01, 0x02, 0x03];
const CMDLINE: &str = "rd.luks.name=ab-12=root root=/dev/mapper/root rw\n";

#[derive(Debug, Clone)]
struct Call {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// What the enrollment tool saw of the key file when it ran.
#[derive(Debug)]
struct KeyFileSeen {
    content: String,
    mode: u32,
}

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<Call>>,
    answers: RefCell<VecDeque<String>>,
    seen: RefCell<Option<KeyFileSeen>>,
    /// Make `systemd-cryptenroll` fail.
    enroll_fails: bool,
    /// Put the key files in the mount point, as a real mount would.
    keys: Option<(&'static str, &'static str)>,
}

impl Fake {
    fn with_answers(answers: &[&str]) -> Self {
        Self {
            answers: RefCell::new(answers.iter().map(|a| (*a).to_owned()).collect()),
            keys: Some((PEM, "PUBLIC KEY")),
            ..Self::default()
        }
    }

    fn record(&self, program: &str, args: &[OsString], env: &[(&str, &str)]) -> Vec<String> {
        let args: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        self.calls.borrow_mut().push(Call {
            program: program.to_owned(),
            args: args.clone(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        });
        args
    }

    fn programs(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|c| c.program.clone())
            .collect()
    }

    fn call(&self, program: &str) -> Call {
        self.calls
            .borrow()
            .iter()
            .find(|c| c.program == program)
            .cloned()
            .unwrap_or_else(|| panic!("{program} was not run"))
    }

    fn execute(
        &self,
        program: &str,
        args: &[OsString],
        env: &[(&str, &str)],
    ) -> Result<(), BootError> {
        let args = self.record(program, args, env);
        match program {
            "mount" => {
                if let Some((pem, public)) = self.keys {
                    let dir = std::path::Path::new(args.last().unwrap()).join("lib/tetsu/keys");
                    fs::create_dir_all(&dir).unwrap();
                    fs::write(dir.join("PK.crt"), pem).unwrap();
                    fs::write(dir.join("pcr.pub"), public).unwrap();
                }
            }
            "systemd-cryptenroll" => {
                let key = args
                    .iter()
                    .find_map(|a| a.strip_prefix("--unlock-key-file="))
                    .unwrap();
                *self.seen.borrow_mut() = Some(KeyFileSeen {
                    content: fs::read_to_string(key).unwrap(),
                    mode: fs::metadata(key).unwrap().permissions().mode() & 0o777,
                });
                if self.enroll_fails {
                    return Err(BootError::Tool {
                        tool: program.to_owned(),
                        status: "exit status: 1".into(),
                        stderr: "no TPM".into(),
                    });
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl Runner for Fake {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.execute(program, args, &[])
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        assert_eq!(program, "systemd-ask-password");
        self.record(program, args, &[]);
        let answer = self
            .answers
            .borrow_mut()
            .pop_front()
            .expect("unexpected prompt");
        Ok(format!("{answer}\n"))
    }
}

impl SealRunner for Fake {
    fn run_env(
        &self,
        program: &str,
        args: &[OsString],
        env: &[(&str, &str)],
    ) -> Result<(), BootError> {
        self.execute(program, args, env)
    }
}

struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    /// A Comfort system, pending, Secure Boot on with our PK, TPM present.
    fn new(level: &str) -> Self {
        let m = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        let root = m.dir.path().join("root");
        fs::create_dir_all(root.join("proc")).unwrap();
        fs::write(root.join("proc/cmdline"), CMDLINE).unwrap();
        fs::create_dir_all(root.join("sys/class/tpm/tpm0")).unwrap();
        fs::write(root.join("sys/class/tpm/tpm0/tpm_version_major"), "2\n").unwrap();
        m.firmware(true, false);
        write_pk(&root, &esl(&[&DER]));
        fs::create_dir_all(m.dir.path().join("sysroot/etc/tetsu")).unwrap();
        m.level(&format!("level = \"{level}\"\nstate = \"pending\"\n"));
        m
    }

    fn firmware(&self, secure_boot: bool, setup: bool) {
        let dir = self.dir.path().join("root/sys/firmware/efi/efivars");
        fs::create_dir_all(&dir).unwrap();
        for (name, on) in [("SecureBoot", secure_boot), ("SetupMode", setup)] {
            fs::write(
                dir.join(format!("{name}-{GLOBAL}")),
                [6, 0, 0, 0, u8::from(on)],
            )
            .unwrap();
        }
    }

    fn level(&self, text: &str) {
        fs::write(
            self.dir.path().join("sysroot/etc/tetsu/unlock-level.toml"),
            text,
        )
        .unwrap();
    }

    fn level_text(&self) -> String {
        fs::read_to_string(self.dir.path().join("sysroot/etc/tetsu/unlock-level.toml")).unwrap()
    }

    fn paths(&self) -> SealPaths {
        SealPaths {
            root: self.dir.path().join("root"),
            sysroot: self.dir.path().join("sysroot"),
            work_dir: self.dir.path().join("work"),
        }
    }

    fn work_exists(&self) -> bool {
        self.dir.path().join("work").exists()
    }
}

#[test]
fn comfort_asks_the_passphrase_only_and_seals() {
    let m = Machine::new("comfort");
    let f = Fake::with_answers(&["correct horse"]);
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed);
    assert_eq!(
        f.programs(),
        [
            "mount",
            "systemd-ask-password",
            "systemd-cryptenroll",
            "umount"
        ]
    );
    let enroll = f.call("systemd-cryptenroll");
    let key_file = enroll.args[0]
        .strip_prefix("--unlock-key-file=")
        .unwrap()
        .to_owned();
    assert!(key_file.starts_with(m.dir.path().join("work").to_str().unwrap()));
    let public = m.dir.path().join("work/var/lib/tetsu/keys/pcr.pub");
    assert_eq!(
        enroll.args[1..],
        [
            "--tpm2-device=auto".to_owned(),
            format!("--tpm2-public-key={}", public.display()),
            "--tpm2-public-key-pcrs=11".to_owned(),
            "--tpm2-pcrs=7".to_owned(),
            "--wipe-slot=tpm2".to_owned(),
            "/dev/disk/by-uuid/ab-12".to_owned(),
        ]
    );
    assert!(enroll.env.is_empty());
    let seen = f.seen.borrow();
    let seen = seen.as_ref().unwrap();
    assert_eq!(
        seen.content, "correct horse",
        "no newline, exact passphrase"
    );
    assert_eq!(seen.mode, 0o600);
    assert_eq!(m.level_text(), "level = \"comfort\"\nstate = \"sealed\"\n");
    assert!(
        !m.work_exists(),
        "the key file and the mount point are gone"
    );
    let mount = f.call("mount");
    assert_eq!(
        mount.args[..4],
        ["-t", "btrfs", "-o", "subvol=@var,noatime"]
    );
    assert_eq!(mount.args[4], "/dev/mapper/root");
}

#[test]
fn balanced_asks_the_pin_twice_and_passes_it_in_the_environment() {
    let m = Machine::new("balanced");
    let f = Fake::with_answers(&["correct horse", "2468", "2468"]);
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed);
    let enroll = f.call("systemd-cryptenroll");
    assert!(enroll.args.contains(&"--tpm2-with-pin=yes".to_owned()));
    assert_eq!(enroll.env, [("NEWPIN".to_owned(), "2468".to_owned())]);
    for call in f.calls.borrow().iter() {
        for arg in &call.args {
            assert!(
                !arg.contains("2468") && !arg.contains("correct horse"),
                "{call:?}"
            );
        }
    }
    let asks: Vec<Call> = f
        .calls
        .borrow()
        .iter()
        .filter(|c| c.program == "systemd-ask-password")
        .cloned()
        .collect();
    assert_eq!(asks.len(), 3);
    assert!(asks[1].args.contains(&"--id=tetsu-seal:pin".to_owned()));
    assert!(m.level_text().contains("state = \"sealed\""));
}

#[test]
fn a_pin_mismatch_asks_again_and_gives_up_after_three() {
    let m = Machine::new("balanced");
    let f = Fake::with_answers(&["pw", "1", "2", "3", "4", "5", "6"]);
    assert_eq!(
        run(&m.paths(), &f).unwrap_err().to_string(),
        "the PINs did not match"
    );
    assert!(!f.programs().contains(&"systemd-cryptenroll".to_owned()));
    assert!(m.level_text().contains("pending"));
    assert!(!m.work_exists());
    assert_eq!(f.programs().last().unwrap(), "umount");

    let m = Machine::new("balanced");
    let f = Fake::with_answers(&["pw", "1", "2", "7", "7"]);
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed);
    assert_eq!(f.call("systemd-cryptenroll").env[0].1, "7");
}

#[test]
fn nothing_happens_for_paranoid_a_sealed_state_or_no_file() {
    let f = Fake::with_answers(&[]);
    let m = Machine::new("paranoid");
    m.level("level = \"paranoid\"\nstate = \"pending\"\n");
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::NotNeeded);
    m.level("level = \"comfort\"\nstate = \"sealed\"\n");
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::NotNeeded);
    fs::remove_file(m.dir.path().join("sysroot/etc/tetsu/unlock-level.toml")).unwrap();
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::NotNeeded);
    assert!(f.calls.borrow().is_empty());
}

fn waits_for(m: &Machine, why: &str) {
    let f = Fake::with_answers(&[]);
    let Outcome::Waiting(got) = run(&m.paths(), &f).unwrap() else {
        panic!("expected a pending level");
    };
    assert!(got.contains(why), "{got}");
    assert!(!f.programs().contains(&"systemd-cryptenroll".to_owned()));
    assert!(m.level_text().contains("pending"));
}

#[test]
fn it_waits_without_secure_boot_a_tpm_or_our_keys() {
    let m = Machine::new("comfort");
    m.firmware(false, false);
    waits_for(&m, "Secure Boot is off");
    m.firmware(false, true);
    waits_for(&m, "Setup Mode");
    m.firmware(true, false);
    write_pk(&m.dir.path().join("root"), &esl(&[&[1, 2, 3]]));
    waits_for(&m, "keys are not enrolled");
    write_pk(&m.dir.path().join("root"), &esl(&[&DER]));
    fs::write(
        m.dir
            .path()
            .join("root/sys/class/tpm/tpm0/tpm_version_major"),
        "1\n",
    )
    .unwrap();
    waits_for(&m, "no TPM");
}

#[test]
fn a_failed_enrollment_is_an_error_and_leaves_the_level_pending() {
    let m = Machine::new("comfort");
    let f = Fake {
        enroll_fails: true,
        ..Fake::with_answers(&["pw"])
    };
    let err = run(&m.paths(), &f).unwrap_err().to_string();
    assert!(err.contains("no TPM"), "{err}");
    assert!(m.level_text().contains("pending"));
    assert!(
        !m.work_exists(),
        "the key file does not outlive the failure"
    );
    assert_eq!(f.programs().last().unwrap(), "umount");
}

#[test]
fn a_missing_key_directory_keeps_the_level_pending() {
    let m = Machine::new("comfort");
    let f = Fake {
        keys: None,
        ..Fake::with_answers(&[])
    };
    assert!(matches!(run(&m.paths(), &f).unwrap(), Outcome::Waiting(_)));
    let f = Fake {
        keys: Some((PEM, "")),
        ..Fake::with_answers(&["pw"])
    };
    // pcr.pub exists but is empty: still handed to the tool, which judges it.
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed);
}

#[test]
fn a_broken_level_file_or_command_line_is_an_error() {
    let m = Machine::new("comfort");
    m.level("garbage = [");
    let f = Fake::with_answers(&[]);
    assert!(run(&m.paths(), &f).is_err());
    m.level("level = \"comfort\"\nstate = \"pending\"\n");
    fs::write(m.dir.path().join("root/proc/cmdline"), "quiet\n").unwrap();
    assert!(run(&m.paths(), &f).is_err());
    assert!(f.calls.borrow().is_empty());
}

#[test]
fn an_empty_answer_is_refused() {
    let m = Machine::new("comfort");
    let f = Fake::with_answers(&[""]);
    assert!(run(&m.paths(), &f).is_err());
    assert!(!f.programs().contains(&"systemd-cryptenroll".to_owned()));
}
