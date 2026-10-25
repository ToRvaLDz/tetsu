//! `tetsu secureboot` against a tempdir root and a recording runner.

use std::cell::RefCell;
use std::ffi::OsString;
use std::fs;
use std::path::Path;

use tetsu_cli::secureboot::{Context, Paths, enroll, trust};
use tetsu_installer_core::entropy::Entropy;
use tetsu_installer_core::error::InstallError;
use tetsu_tx::boot::BootError;
use tetsu_tx::boot::tool::Runner;

const GLOBAL: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";
const X509_GUID: [u8; 16] = [
    0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b, 0xf0, 0x72,
];
const PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIB\nAQID\n-----END CERTIFICATE-----\n";
const DER: [u8; 6] = [0x30, 0x82, 0x01, 0x01, 0x02, 0x03];

#[derive(Default)]
struct Recorder {
    lines: RefCell<Vec<String>>,
    fail_on: Option<&'static str>,
}

impl Runner for Recorder {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        let args: Vec<String> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        self.lines
            .borrow_mut()
            .push(format!("{program} {}", args.join(" ")));
        if self.fail_on == Some(program) {
            return Err(BootError::ToolMissing {
                tool: program.to_owned(),
            });
        }
        // The signature list a real tool would write.
        if program == "cert-to-efi-sig-list" {
            fs::write(args.last().unwrap(), b"ESL").unwrap();
        }
        Ok(())
    }

    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        self.run(program, args).map(|()| String::new())
    }
}

struct Fixed;

impl Entropy for Fixed {
    fn fill(&self, buf: &mut [u8]) -> Result<(), InstallError> {
        buf.fill(0x11);
        Ok(())
    }
}

struct Machine {
    dir: tempfile::TempDir,
}

impl Machine {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let m = Self { dir };
        fs::create_dir_all(m.path("sys/firmware/efi/efivars")).unwrap();
        fs::create_dir_all(m.path("keys")).unwrap();
        for name in ["PK", "KEK", "db"] {
            fs::write(m.path(&format!("keys/{name}.key")), "k").unwrap();
            fs::write(m.path(&format!("keys/{name}.crt")), PEM).unwrap();
        }
        m
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }

    fn var(&self, name: &str, bytes: &[u8]) {
        let mut data = vec![6, 0, 0, 0];
        data.extend(bytes);
        let file = self.path(&format!("sys/firmware/efi/efivars/{name}-{GLOBAL}"));
        fs::write(file, data).unwrap();
    }

    fn mode(&self, setup: bool, secure_boot: bool) {
        self.var("SetupMode", &[u8::from(setup)]);
        self.var("SecureBoot", &[u8::from(secure_boot)]);
    }

    fn pk(&self, cert: &[u8]) {
        let sig = u32::try_from(16 + cert.len()).unwrap();
        let mut esl = X509_GUID.to_vec();
        esl.extend((28 + sig).to_le_bytes());
        esl.extend(0u32.to_le_bytes());
        esl.extend(sig.to_le_bytes());
        esl.extend([7u8; 16]);
        esl.extend(cert);
        self.var("PK", &esl);
    }

    fn level(&self, text: &str) {
        fs::create_dir_all(self.path("etc/tetsu")).unwrap();
        fs::write(self.path("etc/tetsu/unlock-level.toml"), text).unwrap();
    }

    fn paths(&self) -> Paths {
        Paths {
            root: self.path(""),
            keys_dir: self.path("keys"),
            level_file: self.path("etc/tetsu/unlock-level.toml"),
            microsoft_dir: self.path("ms"),
            work_dir: self.path("work"),
        }
    }

    fn level_text(&self) -> String {
        fs::read_to_string(self.path("etc/tetsu/unlock-level.toml")).unwrap()
    }
}

fn ctx<'a>(m: &Machine, runner: &'a Recorder, is_root: bool) -> Context<'a> {
    Context {
        paths: m.paths(),
        is_root,
        runner,
        entropy: &Fixed,
    }
}

const GUID: &str = "11111111-1111-4111-9111-111111111111";

#[test]
fn enroll_runs_the_installer_sequence_and_marks_the_level() {
    let m = Machine::new();
    m.mode(true, false);
    m.level("level = \"balanced\"\nstate = \"pending\"\n");
    fs::create_dir_all(m.path("ms/kek")).unwrap();
    fs::write(m.path("ms/kek/ms.der"), "").unwrap();
    let r = Recorder::default();
    let out = enroll(&ctx(&m, &r, true)).unwrap();
    assert!(out.contains("next boot"), "{out}");
    let lines = r.lines.borrow();
    let work = m.path("work");
    assert_eq!(
        lines[0],
        format!(
            "cert-to-efi-sig-list -g {GUID} {} {}/PK.own.esl",
            m.path("keys/PK.crt").display(),
            work.display()
        )
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("openssl x509 -inform DER"))
    );
    assert!(lines.last().unwrap().starts_with("efi-updatevar -f "));
    assert!(lines.last().unwrap().ends_with(" PK"));
    assert_eq!(
        m.level_text(),
        "level = \"balanced\"\nstate = \"pending\"\nsecure_boot = \"enrolled\"\n"
    );
    assert!(!work.exists(), "the scratch directory is removed");
}

#[test]
fn enroll_leaves_a_paranoid_level_alone() {
    let m = Machine::new();
    m.mode(true, false);
    m.level("level = \"paranoid\"\nstate = \"sealed\"\n");
    let r = Recorder::default();
    enroll(&ctx(&m, &r, true)).unwrap();
    assert_eq!(m.level_text(), "level = \"paranoid\"\nstate = \"sealed\"\n");
}

#[test]
fn enroll_without_a_level_file_still_succeeds_with_a_note() {
    let m = Machine::new();
    m.mode(true, false);
    let r = Recorder::default();
    let out = enroll(&ctx(&m, &r, true)).unwrap();
    assert!(out.contains("not updated"), "{out}");
}

#[test]
fn enroll_refuses_outside_setup_mode_and_changes_nothing() {
    let m = Machine::new();
    m.mode(false, true);
    let r = Recorder::default();
    let err = enroll(&ctx(&m, &r, true)).unwrap_err();
    assert!(err.contains("put the firmware in Setup Mode"), "{err}");
    assert!(err.contains("clear the Secure Boot keys"), "{err}");
    assert!(r.lines.borrow().is_empty());
}

#[test]
fn enroll_needs_root_uefi_and_the_keys() {
    let m = Machine::new();
    m.mode(true, false);
    let r = Recorder::default();
    assert!(enroll(&ctx(&m, &r, false)).unwrap_err().contains("root"));
    fs::remove_file(m.path("keys/KEK.key")).unwrap();
    assert!(enroll(&ctx(&m, &r, true)).unwrap_err().contains("KEK.key"));
    fs::remove_dir_all(m.path("sys")).unwrap();
    assert!(enroll(&ctx(&m, &r, true)).unwrap_err().contains("UEFI"));
    assert!(r.lines.borrow().is_empty());
}

#[test]
fn a_failed_tool_fails_the_enrollment_and_cleans_up() {
    let m = Machine::new();
    m.mode(true, false);
    m.level("level = \"comfort\"\nstate = \"pending\"\n");
    let r = Recorder {
        fail_on: Some("efi-updatevar"),
        ..Recorder::default()
    };
    assert!(enroll(&ctx(&m, &r, true)).is_err());
    assert!(!m.path("work").exists());
    assert!(!m.level_text().contains("enrolled"));
}

fn cert_file(m: &Machine, name: &str) -> std::path::PathBuf {
    let p = m.path(name);
    fs::write(&p, "x").unwrap();
    p
}

#[test]
fn trust_signs_with_the_kek_when_the_pk_is_ours() {
    let m = Machine::new();
    m.mode(false, true);
    m.pk(&DER);
    let r = Recorder::default();
    let cert = cert_file(&m, "vendor.crt");
    let out = trust(&ctx(&m, &r, true), &cert).unwrap();
    assert!(out.contains("vendor.crt"));
    let lines = r.lines.borrow();
    let work = m.path("work");
    assert_eq!(lines.len(), 3);
    assert!(lines[0].starts_with(&format!("cert-to-efi-sig-list -g {GUID} ")));
    assert!(lines[1].starts_with("sign-efi-sig-list -a -k "));
    assert!(lines[1].contains("/keys/KEK.key -c "));
    assert_eq!(
        lines[2],
        format!("efi-updatevar -a -f {}/trust.auth db", work.display())
    );
    assert!(!work.exists());
}

#[test]
fn trust_appends_an_auth_file_as_it_is() {
    let m = Machine::new();
    m.mode(true, false);
    let r = Recorder::default();
    let auth = cert_file(&m, "db.auth");
    trust(&ctx(&m, &r, true), &auth).unwrap();
    assert_eq!(
        *r.lines.borrow(),
        [format!("efi-updatevar -a -f {} db", auth.display())]
    );
}

#[test]
fn trust_refuses_a_foreign_pk_and_missing_files() {
    let m = Machine::new();
    m.mode(false, true);
    m.pk(&[1, 2, 3]);
    let r = Recorder::default();
    let cert = cert_file(&m, "vendor.der");
    let err = trust(&ctx(&m, &r, true), &cert).unwrap_err();
    assert!(err.contains("not enrolled under Tetsu's PK"), "{err}");
    let err = trust(&ctx(&m, &r, true), Path::new("/no/such.der")).unwrap_err();
    assert!(err.contains("no such file"), "{err}");
    assert!(
        trust(&ctx(&m, &r, false), &cert)
            .unwrap_err()
            .contains("root")
    );
    m.pk(&DER);
    assert!(trust(&ctx(&m, &r, true), &cert_file(&m, "v.txt")).is_err());
    assert!(r.lines.borrow().is_empty());
}
