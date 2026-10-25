//! Helpers of the privileged and end-to-end tests: a loopback Btrfs, the
//! real boot tools found through a distro-first `PATH`, throwaway keys.

#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use tetsu_tx::boot::BootError;
use tetsu_tx::boot::keys::SigningKeys;
use tetsu_tx::boot::tool::Runner;

/// Where the systemd EFI stubs live.
pub const STUB_DIR: &str = "/usr/lib/systemd/boot/efi";

const IMAGE_BYTES: u64 = 256 * 1024 * 1024;

/// Tool and the Arch package that ships it.
const TOOLS: [(&str, &str); 7] = [
    ("ukify", "systemd-ukify"),
    ("sbsign", "sbsigntools"),
    ("sbverify", "sbsigntools"),
    ("openssl", "openssl"),
    ("objcopy", "binutils"),
    ("btrfs", "btrfs-progs"),
    ("mkfs.btrfs", "btrfs-progs"),
];

/// Search path with the system directories first: a version manager shim
/// (pyenv, for one) can hide the distro python modules `ukify` imports.
pub fn tool_path() -> OsString {
    let mut path = OsString::from("/usr/bin:/bin:");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    path
}

fn find_tool(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&tool_path())
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// Fail, naming the package, when a tool or a stub is missing.
pub fn require_tools() {
    for (tool, package) in TOOLS {
        assert!(
            find_tool(tool).is_some(),
            "{tool} not found: install the `{package}` package to run the privileged tests"
        );
    }
    let stub = Path::new(STUB_DIR).join("linuxx64.efi.stub");
    assert!(
        stub.is_file(),
        "{} missing: install the `systemd` package",
        stub.display()
    );
}

/// Runs the real programs found through [`tool_path`].
pub struct ToolRunner;

impl Runner for ToolRunner {
    fn output(&self, program: &str, args: &[OsString]) -> Result<String, BootError> {
        let out = Command::new(program)
            .args(args)
            .env("PATH", tool_path())
            .output()
            .map_err(|_| BootError::ToolMissing {
                tool: program.to_owned(),
            })?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(BootError::Tool {
                tool: program.to_owned(),
                status: out.status.to_string(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }

    fn run(&self, program: &str, args: &[OsString]) -> Result<(), BootError> {
        self.output(program, args).map(drop)
    }
}

/// The effective uid owns `/proc/self`.
pub fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

pub fn run(program: &str, args: &[&OsStr]) {
    let out = Command::new(program)
        .args(args)
        .env("PATH", tool_path())
        .output()
        .unwrap_or_else(|e| panic!("{program}: {e}"));
    assert!(
        out.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A loop-mounted Btrfs top level, unmounted on drop.
pub struct LoopBtrfs {
    pub dir: TempDir,
    pub mnt: PathBuf,
}

impl LoopBtrfs {
    pub fn new() -> Self {
        assert!(
            is_root(),
            "the loopback Btrfs tests must run as root (sudo)"
        );
        require_tools();
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("btrfs.img");
        fs::File::create(&image)
            .unwrap()
            .set_len(IMAGE_BYTES)
            .unwrap();
        run("mkfs.btrfs", &["-q".as_ref(), image.as_os_str()]);
        let mnt = dir.path().join("top");
        fs::create_dir(&mnt).unwrap();
        run(
            "mount",
            &[
                "-o".as_ref(),
                "loop,subvolid=5".as_ref(),
                image.as_os_str(),
                mnt.as_os_str(),
            ],
        );
        Self { dir, mnt }
    }

    /// A directory beside the mount, on the host filesystem.
    pub fn work(&self, name: &str) -> PathBuf {
        let p = self.dir.path().join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }
}

impl Drop for LoopBtrfs {
    fn drop(&mut self) {
        let _ = Command::new("umount").arg(&self.mnt).status();
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

/// A throwaway Secure Boot db key and certificate in `dir`.
pub fn make_keys(dir: &Path) -> SigningKeys {
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
    SigningKeys::in_dir(dir, false)
}
