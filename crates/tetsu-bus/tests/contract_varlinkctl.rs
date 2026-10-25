//! Every shipped IDL file, current and baseline, validates with systemd's own
//! `varlinkctl validate-idl`, so the contract is not only valid for our parser.
//!
//! Skipped with a note when `varlinkctl` is not installed, unless
//! `TETSU_REQUIRE_VARLINKCTL=1`, which makes a missing tool a failure (CI).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn idl_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("idl")
}

fn required() -> bool {
    std::env::var("TETSU_REQUIRE_VARLINKCTL").is_ok_and(|v| v == "1")
}

/// Whether `varlinkctl` can be run; panics when it is required but missing.
fn varlinkctl_available() -> bool {
    let found = Command::new("varlinkctl")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !found {
        assert!(
            !required(),
            "varlinkctl is missing and TETSU_REQUIRE_VARLINKCTL=1"
        );
        eprintln!("note: varlinkctl not found, skipping IDL validation");
    }
    found
}

fn validate_file(path: &Path) -> Result<(), String> {
    let out = Command::new("varlinkctl")
        .arg("validate-idl")
        .arg(path)
        .output()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn varlink_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("idl dir readable")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "varlink"))
        .collect();
    files.sort();
    files
}

#[test]
fn current_idl_validates_with_varlinkctl() {
    if !varlinkctl_available() {
        return;
    }
    let files = varlink_files(&idl_dir());
    assert_eq!(files.len(), 9, "{files:?}");
    let errors: Vec<String> = files
        .iter()
        .filter_map(|f| validate_file(f).err())
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn baseline_idl_validates_with_varlinkctl() {
    if !varlinkctl_available() {
        return;
    }
    let files = varlink_files(&idl_dir().join("baseline"));
    assert_eq!(files.len(), 9, "{files:?}");
    let errors: Vec<String> = files
        .iter()
        .filter_map(|f| validate_file(f).err())
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn varlinkctl_rejects_an_unresolved_type() {
    if !varlinkctl_available() {
        return;
    }
    let mut child = Command::new("varlinkctl")
        .arg("validate-idl")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("varlinkctl starts");
    child
        .stdin
        .take()
        .expect("stdin piped")
        .write_all(b"interface org.x.y\nmethod A() -> (a: Nope)\n")
        .expect("stdin written");
    let status = child.wait().expect("varlinkctl exits");
    assert!(!status.success(), "validate-idl accepted a bad IDL");
}
