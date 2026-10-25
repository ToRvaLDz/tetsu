//! End-to-end boot tests with the real ukify, sbsign, sbverify and openssl.
//!
//! No root is needed. A missing tool fails the test and names the package
//! that provides it; nothing is skipped silently.
//!
//! The kernel is a stand-in: ukify insists on a PE image for `--linux`, so
//! the systemd EFI stub plays the part of `vmlinuz`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use tetsu_tx::boot::BootError;
use tetsu_tx::boot::entry::{BootState, EntryName, FileKind, parse_file_name};
use tetsu_tx::boot::esp::Esp;
use tetsu_tx::boot::install::write_uki;
use tetsu_tx::boot::keys::SigningKeys;
use tetsu_tx::boot::space::StatvfsProbe;
use tetsu_tx::boot::tool::Runner;
use tetsu_tx::boot::uki::{UkiSpec, sign_efi, verify_signature};

const STUB_DIR: &str = "/usr/lib/systemd/boot/efi";
const ESTIMATE: u64 = 1024 * 1024;

/// Tool and the Arch package that ships it.
const TOOLS: [(&str, &str); 5] = [
    ("ukify", "systemd-ukify"),
    ("sbsign", "sbsigntools"),
    ("sbverify", "sbsigntools"),
    ("openssl", "openssl"),
    ("objcopy", "binutils"),
];

/// Search path with the system directories first: a version manager shim
/// (pyenv, for one) can hide the distro python modules `ukify` imports.
fn tool_path() -> OsString {
    let mut path = OsString::from("/usr/bin:/bin:");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    path
}

fn find_tool(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&tool_path())
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn require_tools() {
    for (tool, package) in TOOLS {
        assert!(
            find_tool(tool).is_some(),
            "{tool} not found: install the `{package}` package to run the end-to-end boot tests"
        );
    }
    for file in ["linuxx64.efi.stub", "systemd-bootx64.efi"] {
        let p = Path::new(STUB_DIR).join(file);
        assert!(
            p.is_file(),
            "{} missing: install the `systemd` package",
            p.display()
        );
    }
}

/// Runner that uses [`tool_path`].
struct TestRunner;

impl Runner for TestRunner {
    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        let out = Command::new(program)
            .args(args)
            .env("PATH", tool_path())
            .output()
            .map_err(|_| BootError::ToolMissing {
                tool: program.to_owned(),
            })?;
        if out.status.success() {
            Ok(())
        } else {
            Err(BootError::Tool {
                tool: program.to_owned(),
                status: out.status.to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }
}

fn openssl(args: &[&str]) {
    let out = Command::new("openssl")
        .args(args)
        .env("PATH", tool_path())
        .output()
        .expect("run openssl");
    assert!(
        out.status.success(),
        "openssl {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Throwaway db key/cert and PCR key pair.
fn make_keys(dir: &Path, with_pcr: bool) -> SigningKeys {
    let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        &p("db.key"),
        "-out",
        &p("db.crt"),
        "-subj",
        "/CN=tetsu-test",
        "-days",
        "2",
    ]);
    if with_pcr {
        openssl(&["genrsa", "-out", &p("pcr.key"), "2048"]);
        openssl(&[
            "rsa",
            "-in",
            &p("pcr.key"),
            "-pubout",
            "-out",
            &p("pcr.pub"),
        ]);
    }
    SigningKeys::in_dir(dir, with_pcr)
}

/// Fake snapshot tree holding the three inputs.
fn make_snapshot(dir: &Path) -> PathBuf {
    let root = dir.join("snapshot");
    let lib = root.join("usr/lib");
    fs::create_dir_all(lib.join("modules/6.1.0")).unwrap();
    fs::create_dir_all(lib.join("tetsu")).unwrap();
    fs::copy(
        Path::new(STUB_DIR).join("linuxx64.efi.stub"),
        lib.join("modules/6.1.0/vmlinuz"),
    )
    .unwrap();
    fs::write(lib.join("tetsu/initrd.img"), b"initrd").unwrap();
    fs::write(
        lib.join("os-release"),
        "NAME=Tetsu\nID=tetsu\nIMAGE_VERSION=old\n",
    )
    .unwrap();
    root
}

struct Fixture {
    _tmp: TempDir,
    esp: Esp,
    snapshot: PathBuf,
    keys: SigningKeys,
}

fn fixture(with_pcr: bool) -> Fixture {
    require_tools();
    let tmp = tempfile::tempdir().unwrap();
    let keys = make_keys(tmp.path(), with_pcr);
    let snapshot = make_snapshot(tmp.path());
    let esp_root = tmp.path().join("esp");
    fs::create_dir_all(&esp_root).unwrap();
    Fixture {
        esp: Esp::open(&esp_root),
        _tmp: tmp,
        snapshot,
        keys,
    }
}

impl Fixture {
    fn spec(&self, counter: u64) -> UkiSpec {
        UkiSpec {
            counter,
            snapshot_root: self.snapshot.clone(),
            kernel_version: "6.1.0".into(),
            base_cmdline: "quiet rw".into(),
            keys: self.keys.clone(),
        }
    }

    fn write(&self, counter: u64, booted: Option<u64>) -> PathBuf {
        write_uki(
            &self.esp,
            &StatvfsProbe,
            &TestRunner,
            &self.spec(counter),
            booted,
            ESTIMATE,
        )
        .unwrap_or_else(|e| panic!("write_uki({counter}): {e}"))
        .path
    }

    /// Simulate `systemd-bless-boot`: drop the counting suffix.
    fn bless(&self, counter: u64) {
        let state = self.esp.scan().unwrap();
        let e = state
            .entries
            .iter()
            .find(|e| e.name.counter == counter)
            .expect("entry to bless");
        let good = self.esp.dir().join(
            EntryName {
                counter,
                tries: None,
            }
            .file_name(),
        );
        fs::rename(&e.path, good).unwrap();
    }

    fn listing(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.esp.dir())
            .unwrap()
            .map(|d| d.unwrap().file_name().into_string().unwrap())
            .collect();
        v.sort();
        v
    }
}

/// Contents of a PE section, `None` when the section is absent.
fn section(efi: &Path, name: &str) -> Option<Vec<u8>> {
    let dump = tempfile::NamedTempFile::new().unwrap();
    let out = Command::new("objcopy")
        .arg(format!("--dump-section={name}={}", dump.path().display()))
        .arg(efi)
        .arg(dump.path().with_extension("discard"))
        .env("PATH", tool_path())
        .output()
        .expect("run objcopy");
    let _ = fs::remove_file(dump.path().with_extension("discard"));
    out.status
        .success()
        .then(|| fs::read(dump.path()).unwrap())
        .filter(|b| !b.is_empty())
}

#[test]
fn builds_signs_and_commits_a_uki() {
    let fx = fixture(true);
    let path = fx.write(7, None);

    assert_eq!(path, fx.esp.dir().join("tetsu-7+3.efi"));
    let Some(FileKind::Entry(name)) = parse_file_name(path.file_name().unwrap().to_str().unwrap())
    else {
        panic!("committed name does not parse")
    };
    assert_eq!(name.state(), BootState::Untried);
    assert_eq!(name.tries.unwrap().left, 3);
    assert_eq!(fx.listing(), vec!["tetsu-7+3.efi"]);

    verify_signature(&TestRunner, &path, &fx.keys.db_cert).expect("sbverify --cert");

    let cmdline = String::from_utf8(section(&path, ".cmdline").expect(".cmdline")).unwrap();
    assert!(
        cmdline.contains("rootflags=subvol=@snapshots/7/root"),
        "{cmdline:?}"
    );
    assert!(cmdline.contains("quiet rw"), "{cmdline:?}");
    let osrel = String::from_utf8(section(&path, ".osrel").expect(".osrel")).unwrap();
    assert!(osrel.contains("IMAGE_VERSION=7"), "{osrel:?}");
    assert!(!osrel.contains("IMAGE_VERSION=old"), "{osrel:?}");
    assert!(osrel.contains("NAME=Tetsu"), "{osrel:?}");
    assert!(
        section(&path, ".pcrsig").is_some(),
        "no .pcrsig with a PCR key"
    );
    assert!(section(&path, ".pcrpkey").is_some());
}

#[test]
fn no_pcr_key_means_no_pcrsig() {
    let fx = fixture(false);
    let path = fx.write(1, None);
    verify_signature(&TestRunner, &path, &fx.keys.db_cert).expect("sbverify --cert");
    assert!(section(&path, ".pcrsig").is_none());
    assert!(section(&path, ".cmdline").is_some());
}

#[test]
fn signature_fails_against_the_wrong_cert() {
    let fx = fixture(false);
    let path = fx.write(1, None);
    let other = tempfile::tempdir().unwrap();
    let wrong = make_keys(other.path(), false);
    assert!(verify_signature(&TestRunner, &path, &wrong.db_cert).is_err());
}

#[test]
fn signs_systemd_boot() {
    let fx = fixture(false);
    let efi_dir = fx.esp.dir().parent().unwrap();
    fs::create_dir_all(efi_dir).unwrap();
    let signed = efi_dir.join("systemd-bootx64.efi");
    sign_efi(
        &TestRunner,
        &Path::new(STUB_DIR).join("systemd-bootx64.efi"),
        &signed,
        &fx.keys,
    )
    .expect("sbsign");
    verify_signature(&TestRunner, &signed, &fx.keys.db_cert).expect("sbverify systemd-boot");
}

#[test]
fn retention_over_several_commits() {
    let fx = fixture(false);

    // 1..3 committed and blessed, each booted in turn.
    fx.write(1, None);
    fx.bless(1);
    fx.write(2, Some(1));
    fx.bless(2);
    fx.write(3, Some(2));
    fx.bless(3);

    // Litter: a Bad entry and an interrupted write from the past.
    fs::write(fx.esp.dir().join("tetsu-0+0-3.efi"), b"bad").unwrap();
    fs::write(fx.esp.dir().join("tetsu-99.efi.tmp"), b"half").unwrap();
    fs::write(fx.esp.dir().join("foreign.efi"), b"not ours").unwrap();

    // Booted 3: keeps 3, 2, 1; the incoming 4 makes four. Bad and tmp go.
    fx.write(4, Some(3));
    assert_eq!(
        fx.listing(),
        vec![
            "foreign.efi",
            "tetsu-1.efi",
            "tetsu-2.efi",
            "tetsu-3.efi",
            "tetsu-4+3.efi"
        ]
    );

    // 4 is blessed and booted: 1 is now the fourth-oldest and goes.
    fx.bless(4);
    fx.write(5, Some(4));
    assert_eq!(
        fx.listing(),
        vec![
            "foreign.efi",
            "tetsu-2.efi",
            "tetsu-3.efi",
            "tetsu-4.efi",
            "tetsu-5+3.efi"
        ]
    );

    // 5 never booted, 6 supersedes it while 4 is still the booted entry.
    fx.write(6, Some(4));
    assert_eq!(
        fx.listing(),
        vec![
            "foreign.efi",
            "tetsu-2.efi",
            "tetsu-3.efi",
            "tetsu-4.efi",
            "tetsu-6+3.efi"
        ]
    );

    let state = fx.esp.scan().unwrap();
    assert_eq!(state.default_entry().unwrap().name.counter, 6);
    assert!(state.orphan_tmps.is_empty());
}
