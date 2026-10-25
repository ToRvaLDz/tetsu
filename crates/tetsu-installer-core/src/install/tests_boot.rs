//! Stage 2 on the fake machine.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::error::InstallError;
use crate::preflight::SecureBoot;
use crate::unlock::UnlockLevel;

use super::fake::{KERNEL, LUKS_UUID, Outcome, answers, run_plan, run_with, target};
use super::plan::{InstallPlan, Stage};

const NEVER: &dyn Fn() -> bool = &|| false;

struct Run {
    out: Outcome,
    root: tempfile::TempDir,
}

impl Run {
    fn work(&self, rel: &str) -> PathBuf {
        self.root.path().join("work").join(rel)
    }

    fn keys(&self) -> PathBuf {
        self.work("top/@var/lib/tetsu/keys")
    }

    fn lines(&self) -> Vec<String> {
        self.out.machine.lines()
    }

    fn snapshot_root(&self) -> PathBuf {
        self.work("top/@snapshots/1/root")
    }
}

/// Stages 1 and 2 of the full plan.
fn run_two_stages(level: UnlockLevel, setup: impl FnOnce(&super::fake::Machine)) -> Run {
    let root = tempfile::tempdir().unwrap();
    let a = answers(level);
    let t = target(root.path(), SecureBoot::SetupMode);
    let full = InstallPlan::build(&a, &t);
    let plan = InstallPlan::from_actions(
        full.actions()
            .iter()
            .copied()
            .filter(|a| a.stage() != Stage::Firmware)
            .collect(),
    );
    let out = run_with(&a, &t, &plan, NEVER, setup);
    Run { out, root }
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn position(lines: &[String], prefix: &str) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} in {lines:#?}"))
}

#[test]
fn keys_are_generated_private_in_the_var_subvolume() {
    let run = run_two_stages(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let k = run.keys();
    assert_eq!(mode(&k), 0o700);
    for name in ["PK", "KEK", "db", "pcr"] {
        assert_eq!(mode(&k.join(format!("{name}.key"))), 0o600, "{name}");
    }
    for f in ["PK.crt", "PK.der", "KEK.crt", "db.crt", "db.der", "pcr.pub"] {
        assert!(k.join(f).is_file(), "{f}");
    }
    let lines = run.lines();
    let kd = k.display();
    let want = format!(
        "openssl req -new -x509 -newkey rsa:2048 -nodes -days 3650 -subj /CN=Tetsu db tetsu-ab12/ -keyout {kd}/db.key -out {kd}/db.crt"
    );
    assert!(lines.contains(&want), "{lines:#?}");
    assert!(lines.contains(&format!(
        "openssl x509 -outform DER -in {kd}/db.crt -out {kd}/db.der"
    )));
    assert!(lines.contains(&format!(
        "openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out {kd}/pcr.key"
    )));
    assert!(lines.contains(&format!(
        "openssl rsa -pubout -in {kd}/pcr.key -out {kd}/pcr.pub"
    )));
}

#[test]
fn paranoid_has_no_pcr_key_and_no_pcr_signature() {
    let run = run_two_stages(UnlockLevel::Paranoid, |_| {});
    run.out.result.as_ref().unwrap();
    assert!(!run.keys().join("pcr.key").exists());
    let ukify = &run.lines()[position(&run.lines(), "ukify")];
    assert!(!ukify.contains("--pcr-private-key"));
}

#[test]
fn the_uki_is_built_from_snapshot_one_with_the_signing_keys() {
    let run = run_two_stages(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let ukify = &lines[position(&lines, "ukify")];
    let root = run.snapshot_root();
    let kd = run.keys();
    for part in [
        format!(
            "--linux={}/usr/lib/modules/{KERNEL}/vmlinuz",
            root.display()
        ),
        format!("--initrd={}/usr/lib/tetsu/initrd.img", root.display()),
        format!(
            "--cmdline=rd.luks.name={LUKS_UUID}=root root=/dev/mapper/root rw rootflags=subvol=@snapshots/1/root"
        ),
        format!("--secureboot-private-key={}/db.key", kd.display()),
        format!("--secureboot-certificate={}/db.crt", kd.display()),
        format!("--pcr-private-key={}/pcr.key", kd.display()),
        format!("--pcr-public-key={}/pcr.pub", kd.display()),
    ] {
        assert!(ukify.contains(&part), "{part}\n{ukify}");
    }
    let uki = run.work("esp/EFI/Linux/tetsu-1+3.efi");
    assert!(
        uki.is_file(),
        "{:?}",
        fs::read_dir(uki.parent().unwrap())
            .unwrap()
            .collect::<Vec<_>>()
    );
}

#[test]
fn systemd_boot_is_signed_placed_and_copied_to_the_fallback_path() {
    let run = run_two_stages(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let kd = run.keys();
    let esp = run.work("esp");
    let want = format!(
        "sbsign --key={kd}/db.key --cert={kd}/db.crt --output={esp}/EFI/systemd/systemd-bootx64.efi -- {root}/usr/lib/systemd/boot/efi/systemd-bootx64.efi",
        kd = kd.display(),
        esp = esp.display(),
        root = run.snapshot_root().display()
    );
    assert!(lines.contains(&want), "{lines:#?}");
    assert_eq!(
        fs::read(esp.join("EFI/BOOT/BOOTX64.EFI")).unwrap(),
        b"signed"
    );
    assert_eq!(
        fs::read_to_string(esp.join("loader/loader.conf")).unwrap(),
        "timeout 3\neditor no\nconsole-mode keep\n"
    );
}

#[test]
fn every_signature_is_verified_after_the_files_exist() {
    let run = run_two_stages(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let cert = format!("--cert={}/db.crt", run.keys().display());
    let esp = run.work("esp").display().to_string();
    let verifies: Vec<&String> = lines.iter().filter(|l| l.starts_with("sbverify")).collect();
    assert_eq!(
        verifies,
        [
            &format!("sbverify {cert} -- {esp}/EFI/Linux/tetsu-1+3.efi"),
            &format!("sbverify {cert} -- {esp}/EFI/systemd/systemd-bootx64.efi"),
            &format!("sbverify {cert} -- {esp}/EFI/BOOT/BOOTX64.EFI"),
        ]
    );
    assert!(position(&lines, "sbverify") > position(&lines, "sbsign"));
    assert!(position(&lines, "sbverify") > position(&lines, "ukify"));
}

#[test]
fn the_snapshot_is_sealed_before_the_uki_and_recorded_after() {
    let run = run_two_stages(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let top = run.work("top").display().to_string();
    let seal = position(&lines, "btrfs property set");
    assert!(
        lines[seal].ends_with("@snapshots/1/root ro true"),
        "{}",
        lines[seal]
    );
    assert_eq!(lines[seal + 1], format!("btrfs filesystem sync {top}"));
    assert!(seal < position(&lines, "ukify"));
    assert!(run.work("top/@snapshots/1/committed").is_file());
    let record = fs::read_to_string(run.work("top/@var/lib/tetsu/transactions/1.json")).unwrap();
    assert!(record.contains("\"counter\": 1"), "{record}");
}

#[test]
fn a_failed_signature_check_stops_before_the_record() {
    let run = run_two_stages(UnlockLevel::Comfort, |m| m.fail_on("sbverify"));
    match run.out.result.as_ref().unwrap_err() {
        InstallError::ActionFailed { action, .. } => assert_eq!(action, "verify every signature"),
        other => panic!("{other:?}"),
    }
    assert!(!run.work("top/@snapshots/1/committed").exists());
}

#[test]
fn a_missing_kernel_command_line_is_an_error_not_a_guess() {
    let root = tempfile::tempdir().unwrap();
    let a = answers(UnlockLevel::Comfort);
    let t = target(root.path(), SecureBoot::Off);
    let plan = InstallPlan::from_actions(vec![super::plan::Action::BuildUki]);
    let out = run_plan(&a, &t, &plan, NEVER);
    assert!(matches!(out.result, Err(InstallError::ActionFailed { .. })));
    assert!(out.machine.lines().is_empty());
}
