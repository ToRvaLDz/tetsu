use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use tetsu_tx::boot::BootError;
use tetsu_tx::boot::tool::Runner;

use super::*;
use crate::efi::tests_support::{esl, write_pk};

const GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
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
    /// What `systemd-cryptenroll <device>` lists; `None` makes it fail.
    listing: Option<&'static str>,
    /// What `cryptsetup luksDump --dump-json-metadata` prints; `None` fails.
    dump: Option<&'static str>,
}

const PASSWORDS_ONLY: &str = "SLOT TYPE\n   0 password\n   1 password\n";
const WITH_TPM2: &str = "SLOT TYPE\n   0 password\n   1 password\n   2 tpm2\n";
/// Tetsu's Balanced policy: signed PCR public key and a PIN.
const TOKEN_PIN: &str = r#"{"tokens":{"0":{"type":"systemd-tpm2","keyslots":["2"],
    "tpm2-pcrs":[7],"tpm2-pin":true,"tpm2_pubkey_pcrs":[11],"tpm2_pubkey":"QUJD"}}}"#;
/// Tetsu's Comfort policy: the same without the PIN; systemd then writes
/// no `tpm2-pin` at all.
const TOKEN_NO_PIN: &str = r#"{"tokens":{"0":{"type":"systemd-tpm2","keyslots":["2"],
    "tpm2-pcrs":[7],"tpm2_pubkey_pcrs":[11],"tpm2_pubkey":"QUJD"}}}"#;
/// A hand-added token: PCR 0 only, no signed policy.
const TOKEN_NO_PUBKEY: &str = r#"{"tokens":{"0":{"type":"systemd-tpm2","keyslots":["2"],
    "tpm2-pcrs":[0],"tpm2-pin":true}}}"#;

impl Fake {
    fn with_answers(answers: &[&str]) -> Self {
        Self {
            answers: RefCell::new(answers.iter().map(|a| (*a).to_owned()).collect()),
            keys: Some((PEM, "PUBLIC KEY")),
            listing: Some(PASSWORDS_ONLY),
            dump: None,
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

    fn enrolled(&self) -> bool {
        self.calls.borrow().iter().any(|c| {
            c.program == "systemd-cryptenroll"
                && c.args.iter().any(|a| a.starts_with("--unlock-key-file="))
        })
    }

    fn programs(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|c| c.program.clone())
            .collect()
    }

    /// The enrollment itself, not the slot listing before it.
    fn enroll_call(&self) -> Call {
        self.calls
            .borrow()
            .iter()
            .find(|c| {
                c.program == "systemd-cryptenroll"
                    && c.args.iter().any(|a| a.starts_with("--unlock-key-file="))
            })
            .cloned()
            .expect("no enrollment ran")
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
        self.record(program, args, &[]);
        if program == "systemd-cryptenroll" {
            return self.listing.map(str::to_owned).ok_or(BootError::Tool {
                tool: program.to_owned(),
                status: "exit status: 1".into(),
                stderr: "cannot read the header".into(),
            });
        }
        if program == "cryptsetup" {
            return self.dump.map(str::to_owned).ok_or(BootError::Tool {
                tool: program.to_owned(),
                status: "exit status: 1".into(),
                stderr: "cannot dump".into(),
            });
        }
        assert_eq!(program, "systemd-ask-password");
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
            "systemd-cryptenroll",
            "mount",
            "systemd-ask-password",
            "systemd-cryptenroll",
            "umount"
        ]
    );
    let enroll = f.enroll_call();
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
    let enroll = f.enroll_call();
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
    assert!(!f.enrolled());
    assert!(m.level_text().contains("pending"));
    assert!(!m.work_exists());
    assert_eq!(f.programs().last().unwrap(), "umount");

    let m = Machine::new("balanced");
    let f = Fake::with_answers(&["pw", "1", "2", "7", "7"]);
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed);
    assert_eq!(f.enroll_call().env[0].1, "7");
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
    assert!(!f.enrolled());
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
    assert!(!f.enrolled());
}

#[test]
fn a_tpm2_slot_with_tetsus_policy_means_sealed_whatever_the_file_says() {
    let m = Machine::new("balanced");
    let f = Fake {
        listing: Some(WITH_TPM2),
        dump: Some(TOKEN_PIN),
        ..Fake::with_answers(&[])
    };
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::AlreadySealed);
    assert_eq!(f.programs(), ["systemd-cryptenroll", "cryptsetup"]);
    let list = f.call("systemd-cryptenroll");
    assert_eq!(list.args, ["/dev/disk/by-uuid/ab-12"]);
    let dump = f.call("cryptsetup");
    assert_eq!(
        dump.args,
        [
            "luksDump",
            "--dump-json-metadata",
            "/dev/disk/by-uuid/ab-12"
        ]
    );
    assert_eq!(
        m.level_text(),
        "level = \"balanced\"\nstate = \"sealed\"\n",
        "the level is kept"
    );
    assert!(!m.work_exists());

    let m = Machine::new("comfort");
    let f = Fake {
        listing: Some(WITH_TPM2),
        dump: Some(TOKEN_NO_PIN),
        ..Fake::with_answers(&[])
    };
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::AlreadySealed);
}

/// A tpm2 slot that does not carry the chosen level's policy is replaced by
/// the normal seal.
fn seals_again(level: &str, answers: &[&str], dump: Option<&'static str>) {
    let m = Machine::new(level);
    let f = Fake {
        listing: Some(WITH_TPM2),
        dump,
        ..Fake::with_answers(answers)
    };
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed, "{dump:?}");
    assert!(f.enrolled());
    assert!(
        f.enroll_call()
            .args
            .contains(&"--wipe-slot=tpm2".to_owned())
    );
}

#[test]
fn a_tpm2_slot_with_another_policy_is_sealed_again() {
    // Balanced needs a PIN.
    seals_again("balanced", &["pw", "1", "1"], Some(TOKEN_NO_PIN));
    // Comfort must not have one.
    seals_again("comfort", &["pw"], Some(TOKEN_PIN));
    // No signed-PCR public key.
    seals_again("balanced", &["pw", "1", "1"], Some(TOKEN_NO_PUBKEY));
    seals_again("comfort", &["pw"], Some(TOKEN_NO_PUBKEY));
    // The dump fails or is not JSON.
    seals_again("comfort", &["pw"], None);
    seals_again("comfort", &["pw"], Some("not json"));
}

#[test]
fn a_failing_listing_falls_through_to_the_normal_seal() {
    let m = Machine::new("comfort");
    let f = Fake {
        listing: None,
        ..Fake::with_answers(&["pw"])
    };
    assert_eq!(run(&m.paths(), &f).unwrap(), Outcome::Sealed);
    assert!(f.enrolled());
}

#[test]
fn has_tpm2_slot_reads_the_type_column() {
    for (listing, expect) in [
        ("", false),
        ("SLOT TYPE\n", false),
        (PASSWORDS_ONLY, false),
        (WITH_TPM2, true),
        ("SLOT TYPE\n   0 tpm2\n", true),
        (
            "SLOT TYPE\n   0 password\n   1 recovery\n   3 fido2\n",
            false,
        ),
        ("  1 tpm2  \n", true),
        ("tpm2\n", false),
        ("SLOT TYPE\n   0 tpm2-like\n", false),
    ] {
        assert_eq!(has_tpm2_slot(listing), expect, "{listing:?}");
    }
}

#[test]
fn has_tetsu_token_wants_the_pubkey_and_the_level_s_pin() {
    use UnlockLevel::{Balanced, Comfort};
    let other = r#"{"tokens":{"0":{"type":"systemd-fido2"},"1":{"type":"systemd-tpm2","tpm2_pubkey_pcrs":[11],"tpm2_pubkey":"QUJD"}}}"#;
    let empty_key = r#"{"tokens":{"0":{"type":"systemd-tpm2","tpm2_pubkey_pcrs":[11],"tpm2_pubkey":"","tpm2-pin":true}}}"#;
    let wrong_type = r#"{"tokens":{"0":{"type":"other","tpm2_pubkey_pcrs":[11],"tpm2_pubkey":"QUJD","tpm2-pin":true}}}"#;
    // systemd never wrote the key with a hyphen.
    let hyphen =
        r#"{"tokens":{"0":{"type":"systemd-tpm2","tpm2_pubkey_pcrs":[11],"tpm2-pubkey":"QUJD"}}}"#;
    // The public key must sign PCR 11, the one the UKI measures.
    let no_pcrs = r#"{"tokens":{"0":{"type":"systemd-tpm2","tpm2_pubkey":"QUJD"}}}"#;
    let other_pcr =
        r#"{"tokens":{"0":{"type":"systemd-tpm2","tpm2_pubkey_pcrs":[7],"tpm2_pubkey":"QUJD"}}}"#;
    for (json, level, expect) in [
        (TOKEN_PIN, Balanced, true),
        (TOKEN_PIN, Comfort, false),
        (TOKEN_NO_PIN, Comfort, true),
        (TOKEN_NO_PIN, Balanced, false),
        (other, Comfort, true),
        (other, Balanced, false),
        (TOKEN_NO_PUBKEY, Balanced, false),
        (empty_key, Balanced, false),
        (wrong_type, Balanced, false),
        (hyphen, Comfort, false),
        (no_pcrs, Comfort, false),
        (other_pcr, Comfort, false),
        (r#"{"tokens":{}}"#, Comfort, false),
        (r#"{"keyslots":{}}"#, Comfort, false),
        ("", Comfort, false),
        ("[]", Comfort, false),
    ] {
        assert_eq!(has_tetsu_token(json, level), expect, "{json} {level:?}");
    }
}

/// `cryptsetup luksDump --dump-json-metadata` of the root device of a
/// Comfort test VM (systemd 262.1-1, installed from 689a589 and sealed by
/// `tetsu-unlock-seal` on its first boot), captured byte for byte over the
/// VM's guest agent before the #115 rollback check. Only the string values
/// longer than 64 characters (`tpm2-blob`, `tpm2_pubkey`, `tpm2_srk`) are
/// cut to their first 48; keys, their order and every other value are as
/// systemd and cryptsetup wrote them.
const SYSTEMD_COMFORT: &str = include_str!("testdata/systemd-262-comfort-metadata.json");
/// The same capture from a Balanced test VM: systemd adds `tpm2-pin`,
/// `tpm2_salt` and the `tpm2_argon2id_*` keys.
const SYSTEMD_BALANCED: &str = include_str!("testdata/systemd-262-balanced-metadata.json");

#[test]
fn a_token_written_by_systemd_counts_as_sealed() {
    use UnlockLevel::{Balanced, Comfort};
    assert!(has_tetsu_token(SYSTEMD_COMFORT, Comfort));
    assert!(!has_tetsu_token(SYSTEMD_COMFORT, Balanced));
    assert!(has_tetsu_token(SYSTEMD_BALANCED, Balanced));
    assert!(!has_tetsu_token(SYSTEMD_BALANCED, Comfort));

    for (level, dump) in [("comfort", SYSTEMD_COMFORT), ("balanced", SYSTEMD_BALANCED)] {
        let m = Machine::new(level);
        let f = Fake {
            listing: Some(WITH_TPM2),
            dump: Some(dump),
            ..Fake::with_answers(&[])
        };
        assert_eq!(
            run(&m.paths(), &f).unwrap(),
            Outcome::AlreadySealed,
            "{level}"
        );
        assert_eq!(
            f.programs(),
            ["systemd-cryptenroll", "cryptsetup"],
            "{level}"
        );
    }
}
