//! The whole plan on the fake machine: stage 3 and the cross-cutting
//! guarantees (ordering, no secret anywhere).

use std::fs;
use std::path::{Path, PathBuf};

use crate::agent::{AgentBackend, ApiProvider};
use crate::answers::Answers;
use crate::error::InstallError;
use crate::preflight::SecureBoot;
use crate::secret::Secret;
use crate::secureboot::MicrosoftCerts;
use crate::unlock::UnlockLevel;

use super::fake::{ESP_PARTUUID, Outcome, answers, run_plan, run_with, target};
use super::plan::InstallPlan;

const NEVER: &dyn Fn() -> bool = &|| false;

struct Run {
    out: Outcome,
    root: tempfile::TempDir,
}

impl Run {
    fn work(&self, rel: &str) -> PathBuf {
        self.root.path().join("work").join(rel)
    }

    fn lines(&self) -> Vec<String> {
        self.out.machine.lines()
    }
}

fn full_run(
    level: UnlockLevel,
    sb: SecureBoot,
    tweak: impl FnOnce(&mut Answers, &mut super::InstallTarget),
    setup: impl FnOnce(&super::fake::Machine),
) -> Run {
    let root = tempfile::tempdir().unwrap();
    let mut a = answers(level);
    let mut t = target(root.path(), sb);
    tweak(&mut a, &mut t);
    let plan = InstallPlan::build(&a, &t);
    let out = run_with(&a, &t, &plan, NEVER, setup);
    Run { out, root }
}

fn position(lines: &[String], prefix: &str) -> usize {
    lines
        .iter()
        .position(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix:?} in {lines:#?}"))
}

fn firmware_calls(lines: &[String]) -> Vec<&String> {
    let tools = [
        "efibootmgr",
        "efi-updatevar",
        "sign-efi-sig-list",
        "cert-to-efi-sig-list",
    ];
    lines
        .iter()
        .filter(|l| tools.iter().any(|t| l.starts_with(t)))
        .collect()
}

#[test]
fn enrollment_writes_db_then_kek_then_pk_with_one_owner_guid() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |_| {},
    );
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let writes: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("efi-updatevar"))
        .collect();
    assert_eq!(writes.len(), 3);
    assert!(
        writes[0].ends_with(" db") && writes[1].ends_with(" KEK") && writes[2].ends_with(" PK")
    );
    let guids: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("cert-to-efi-sig-list"))
        .map(|l| l.split(' ').nth(2).unwrap())
        .collect();
    assert_eq!(guids.len(), 3);
    assert!(guids.iter().all(|g| *g == guids[0]));
    let g = guids[0];
    assert_eq!(g.len(), 36);
    assert_eq!(&g[14..15], "4");
    assert!("89ab".contains(&g[19..20]));
    // The scratch files are gone.
    assert!(!run.root.path().join("run/secureboot").exists());
}

#[test]
fn microsoft_certificates_are_enrolled_when_given() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, t| {
            t.microsoft_certs = MicrosoftCerts {
                kek: vec!["/ms/kek.der".into()],
                db: vec!["/ms/uefi.der".into()],
            };
        },
        |_| {},
    );
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let run_dir = run.root.path().join("run").display().to_string();
    assert!(lines.contains(&format!(
        "openssl x509 -inform DER -in /ms/kek.der -out {run_dir}/secureboot/ms-kek-0.pem"
    )));
    assert!(lines.iter().any(|l| l.contains("ms-db-0.esl")));
}

#[test]
fn without_the_microsoft_certificates_the_enrollment_is_refused_and_nothing_is_written() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, t| t.without_microsoft = false,
        |_| {},
    );
    // The off path: the install completes, the firmware is not written.
    run.out.result.as_ref().unwrap();
    assert!(!run.out.keys_enrolled);
    assert!(
        run.lines()
            .iter()
            .all(|l| !l.starts_with("efi-updatevar") && !l.contains("efi-sig-list"))
    );
    assert!(
        warnings(&run)
            .iter()
            .any(|w| w.contains("Microsoft Secure Boot certificates are missing")),
        "{:?}",
        warnings(&run)
    );
}

#[test]
fn an_enrollment_failure_takes_the_off_path_and_the_install_goes_on() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |m| m.fail_on("efi-updatevar -f"),
    );
    run.out.result.as_ref().unwrap();
    let warnings: Vec<&String> = run
        .out
        .script
        .messages
        .iter()
        .filter(|m| m.starts_with("Warning:"))
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    // The boot entry is still created and the level stays pending.
    let lines = run.lines();
    assert!(lines.iter().any(|l| l.starts_with("efibootmgr --create")));
    let level =
        fs::read_to_string(run.work("top/@snapshots/1/root/etc/tetsu/unlock-level.toml")).unwrap();
    assert!(level.contains("state = \"pending\""));
    // Keys are kept.
    assert!(run.work("top/@var/lib/tetsu/keys/db.key").is_file());
    assert!(run.out.log.iter().any(|l| l.starts_with("[warning]")));
}

#[test]
fn without_setup_mode_nothing_is_enrolled() {
    for sb in [SecureBoot::Off, SecureBoot::Enforcing] {
        let run = full_run(UnlockLevel::Balanced, sb, |_, _| {}, |_| {});
        run.out.result.as_ref().unwrap();
        let lines = run.lines();
        assert!(lines.iter().all(|l| !l.starts_with("efi-updatevar")));
        assert!(lines.iter().all(|l| !l.contains("efi-sig-list")));
        assert!(
            run.out
                .script
                .messages
                .iter()
                .any(|m| m.contains("not in Setup Mode"))
        );
        assert!(run.work("top/@var/lib/tetsu/keys/db.crt").is_file());
        assert!(lines.iter().any(|l| l.starts_with("efibootmgr --create")));
    }
}

#[test]
fn the_boot_entry_is_created_and_put_first_preserving_the_others() {
    let run = full_run(UnlockLevel::Comfort, SecureBoot::Off, |_, _| {}, |_| {});
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let efi: Vec<&String> = lines
        .iter()
        .filter(|l| l.starts_with("efibootmgr"))
        .collect();
    assert_eq!(
        efi,
        [
            "efibootmgr",
            r"efibootmgr --create --disk /dev/sda --part 1 --label Tetsu --loader \EFI\systemd\systemd-bootx64.efi",
            "efibootmgr --bootorder 0002,0001,0000",
        ]
    );
}

#[test]
fn firmware_is_touched_last_and_unmounting_ends_the_install() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |_| {},
    );
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let first_firmware = position(&lines, "cert-to-efi-sig-list");
    for stage_two in ["sbverify", "sbsign", "ukify", "btrfs property set"] {
        let last = lines
            .iter()
            .rposition(|l| l.starts_with(stage_two))
            .unwrap();
        assert!(last < first_firmware, "{stage_two}");
    }
    let tail = &lines[lines.len() - 5..];
    let w = run.root.path().join("work").display().to_string();
    assert_eq!(
        tail,
        [
            "sync".to_owned(),
            format!("umount {w}/esp"),
            format!("umount {w}/top"),
            "cryptsetup close root".to_owned(),
            "sync".to_owned()
        ]
    );
    // The log on the ESP was closed with the last line before the unmount.
    let on_esp = fs::read_to_string(run.work("esp/tetsu-install.log")).unwrap();
    assert!(on_esp.contains("[firmware] flush, unmount and close the encrypted volume"));
}

#[test]
fn a_failure_before_stage_three_never_touches_firmware() {
    for failing in ["sbverify", "ukify", "sbsign", "cryptsetup open", "tar"] {
        let run = full_run(
            UnlockLevel::Comfort,
            SecureBoot::SetupMode,
            |_, _| {},
            |m| m.fail_on(failing),
        );
        assert!(
            matches!(run.out.result, Err(InstallError::ActionFailed { .. })),
            "{failing}"
        );
        assert!(firmware_calls(&run.lines()).is_empty(), "{failing}");
    }
}

fn warnings(run: &Run) -> Vec<&String> {
    run.out
        .script
        .messages
        .iter()
        .filter(|m| m.starts_with("Warning:"))
        .collect()
}

/// The install is complete: unmounted and closed, with the fallback named.
fn assert_fallback_warning(run: &Run) {
    run.out.result.as_ref().unwrap();
    assert!(run.lines().iter().any(|l| l == "cryptsetup close root"));
    assert!(
        warnings(run)
            .iter()
            .any(|w| w.contains(r"\EFI\BOOT\BOOTX64.EFI")),
        "{:?}",
        warnings(run)
    );
}

#[test]
fn a_failed_boot_entry_warns_and_the_install_completes() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        |m| m.fail_on("efibootmgr --create"),
    );
    assert_fallback_warning(&run);
    // Without an entry there is no order to set.
    assert!(
        run.lines()
            .iter()
            .all(|l| !l.starts_with("efibootmgr --bootorder"))
    );
}

#[test]
fn a_failed_boot_order_warns_and_the_install_completes() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        |m| m.fail_on("efibootmgr --bootorder"),
    );
    assert_fallback_warning(&run);
}

#[test]
fn an_ambiguous_boot_listing_creates_no_order_and_warns() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        |m| {
            *m.boot_output.borrow_mut() =
                (String::new(), "BootOrder: 0000\nBoot0000* Windows\n".into())
        },
    );
    assert_fallback_warning(&run);
    assert!(
        run.lines()
            .iter()
            .all(|l| !l.starts_with("efibootmgr --bootorder"))
    );
}

#[test]
fn an_imported_certificate_is_trusted_after_the_keys_with_the_kek() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, t| t.trust_cert = Some("/usb/vendor.crt".into()),
        |_| {},
    );
    run.out.result.as_ref().unwrap();
    let lines = run.lines();
    let keys = run.work("top/@var/lib/tetsu/keys").display().to_string();
    let pk = position(&lines, "efi-updatevar -f ");
    let sign = lines
        .iter()
        .position(|l| l.contains("trust.esl") && l.starts_with("sign-efi-sig-list"))
        .unwrap();
    assert!(pk < sign);
    assert!(lines[sign].contains(&format!("-k {keys}/KEK.key -c {keys}/KEK.crt db ")));
    assert!(lines[sign - 1].starts_with("cert-to-efi-sig-list -g "));
    assert!(lines[sign - 1].contains(" /usb/vendor.crt "));
    assert!(lines[sign + 1].starts_with("efi-updatevar -a -f "));
    assert!(lines[sign + 1].ends_with(" db"));
    // The scratch files are gone and the boot entry follows.
    assert!(!run.root.path().join("run/trust").exists());
    assert!(position(&lines, "efibootmgr --create") > sign);
}

#[test]
fn no_imported_certificate_or_no_enrollment_plans_no_trust_step() {
    let none = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |_| {},
    );
    assert!(none.lines().iter().all(|l| !l.contains("trust.esl")));
    // Without Setup Mode nothing is enrolled, so nothing is trusted either.
    let off = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, t| t.trust_cert = Some("/usb/vendor.crt".into()),
        |_| {},
    );
    assert!(off.lines().iter().all(|l| !l.contains("efi-sig-list")));
}

#[test]
fn a_failed_trust_step_warns_and_the_install_completes_with_the_keys_kept() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, t| t.trust_cert = Some("/usb/vendor.crt".into()),
        |m| m.fail_containing("trust.auth"),
    );
    run.out.result.as_ref().unwrap();
    assert!(run.out.keys_enrolled);
    assert!(
        warnings(&run)
            .iter()
            .any(|w| w.contains("tetsu secureboot trust")),
        "{:?}",
        warnings(&run)
    );
    assert!(run.lines().iter().any(|l| l == "cryptsetup close root"));
}

#[test]
fn the_plan_lists_every_program_the_run_calls() {
    for level in UnlockLevel::ALL {
        for sb in [SecureBoot::Off, SecureBoot::SetupMode] {
            for api in [false, true] {
                let root = tempfile::tempdir().unwrap();
                let mut a = answers(level);
                let mut t = target(root.path(), sb);
                if api {
                    with_api_key(&mut a, &mut t);
                    t.trust_cert = Some("/usb/vendor.der".into());
                }
                let plan = InstallPlan::build(&a, &t);
                let out = run_plan(&a, &t, &plan, NEVER);
                out.result.as_ref().unwrap();
                let listed = plan.programs();
                for call in out.machine.calls.borrow().iter() {
                    assert!(
                        listed.contains(&call[0].as_str()),
                        "{} is not listed ({level:?}, {sb:?}): {listed:?}",
                        call[0]
                    );
                }
            }
        }
    }
}

#[test]
fn a_missing_tool_stops_the_install_before_anything_is_touched() {
    let root = tempfile::tempdir().unwrap();
    let a = answers(UnlockLevel::Comfort);
    let t = target(root.path(), SecureBoot::SetupMode);
    let plan = InstallPlan::build(&a, &t);
    let out = run_with(&a, &t, &plan, NEVER, |m| {
        *m.missing.borrow_mut() = vec!["efibootmgr".into(), "sbverify".into()];
    });
    match &out.result {
        Err(InstallError::Preflight(msg)) => {
            assert!(
                msg.contains("efibootmgr") && msg.contains("sbverify"),
                "{msg}"
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(out.machine.calls.borrow().is_empty());
    assert!(!out.touched);
}

#[test]
fn a_failure_after_the_enrollment_is_reported_with_the_keys_marked_enrolled() {
    for failing in ["umount", "cryptsetup close", "sync"] {
        let run = full_run(
            UnlockLevel::Comfort,
            SecureBoot::SetupMode,
            |_, _| {},
            |m| m.fail_on(failing),
        );
        assert!(
            matches!(run.out.result, Err(InstallError::ActionFailed { .. })),
            "{failing}"
        );
        assert!(run.out.keys_enrolled, "{failing}");
    }
}

#[test]
fn a_failure_before_the_enrollment_leaves_the_keys_unenrolled() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |m| m.fail_on("ukify"),
    );
    assert!(run.out.result.is_err());
    assert!(run.out.touched && !run.out.keys_enrolled);
    // A failed enrollment is the off path: not enrolled either.
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |m| m.fail_on("efi-updatevar -f"),
    );
    assert!(!run.out.keys_enrolled);
}

// The secrets of the test answers.
const SECRETS: [&str; 5] = [
    "correct horse battery staple",
    "cccccccc-dddddddd-eeeeeeee-ffffffff",
    "hunter2-hunter2",
    "sk-ant-0123456789",
    "$6$salt$hashedhashedhashed",
];

fn with_api_key(a: &mut Answers, _: &mut super::InstallTarget) {
    a.agent = Some(AgentBackend::Api {
        provider: ApiProvider::Anthropic,
        key: Secret::new("sk-ant-0123456789"),
    });
}

#[test]
fn no_secret_appears_in_any_argv_log_progress_or_message() {
    for level in UnlockLevel::ALL {
        for sb in [SecureBoot::SetupMode, SecureBoot::Off] {
            let run = full_run(level, sb, with_api_key, |_| {});
            run.out.result.as_ref().unwrap();
            let haystacks: Vec<String> = run
                .lines()
                .into_iter()
                .chain(run.out.log.iter().cloned())
                .chain(run.out.script.messages.iter().cloned())
                .chain(
                    run.out
                        .script
                        .progress
                        .iter()
                        .map(|(s, m)| format!("{s} {m}")),
                )
                .chain([fs::read_to_string(run.work("esp/tetsu-install.log")).unwrap_or_default()])
                .collect();
            for secret in SECRETS {
                assert!(
                    haystacks.iter().all(|h| !h.contains(secret)),
                    "{secret} leaked at {level:?}/{sb:?}"
                );
            }
        }
    }
}

#[test]
fn the_esp_log_is_complete_for_a_finished_install() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |_| {},
    );
    run.out.result.as_ref().unwrap();
    // The log file was closed at Finish: it misses only the final line.
    let on_esp = fs::read_to_string(run.work("esp/tetsu-install.log")).unwrap();
    assert_eq!(on_esp.lines().count() + 1, run.out.log.len());
    assert_eq!(
        run.out.log.last().unwrap(),
        "[firmware] installation complete"
    );
}

#[test]
fn private_key_material_stays_out_of_the_esp() {
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::SetupMode,
        |_, _| {},
        |_| {},
    );
    run.out.result.as_ref().unwrap();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(&run.work("esp"), &mut files);
    assert!(
        files
            .iter()
            .all(|f| f.extension().is_none_or(|e| e != "key")),
        "{files:?}"
    );
}

#[test]
fn run_plan_with_a_cancelled_flag_stops_in_stage_three() {
    let root = tempfile::tempdir().unwrap();
    let a = answers(UnlockLevel::Comfort);
    let t = target(root.path(), SecureBoot::Off);
    let plan = InstallPlan::build(&a, &t);
    let total = plan.actions().len();
    let seen = std::cell::Cell::new(0);
    let cancel = || {
        seen.set(seen.get() + 1);
        seen.get() == total - 1 // before SetBootOrder... the second to last
    };
    let out = run_plan(&a, &t, &plan, &cancel);
    assert!(matches!(out.result, Err(InstallError::Cancelled)));
    assert!(
        out.machine
            .lines()
            .iter()
            .all(|l| !l.starts_with("efibootmgr --bootorder"))
    );
}

// ---- stale and reused boot entries (#50)

const OTHER_DISK: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const DEAD_DISK: &str = "deadbeef-0000-4000-8000-000000000000";

/// A `efibootmgr -v` line the way the tool prints it.
fn entry(num: &str, label: &str, guid: &str) -> String {
    format!(
        "Boot{num}* {label}\tHD(1,GPT,{guid},0x800,0x400000)/File(\\EFI\\systemd\\systemd-bootx64.efi)\n"
    )
}

/// A firmware with these listings around `--create`, and these partitions.
fn firmware(before: String, after: String, present: &[&str]) -> impl FnOnce(&super::fake::Machine) {
    let present: Vec<String> = present.iter().map(|p| (*p).to_owned()).collect();
    move |m| {
        *m.boot_output.borrow_mut() = (before, after);
        *m.present_partuuids.borrow_mut() = present;
    }
}

fn efibootmgr_lines(run: &Run) -> Vec<String> {
    run.lines()
        .into_iter()
        .filter(|l| l.starts_with("efibootmgr"))
        .collect()
}

#[test]
fn a_dead_tetsu_entry_is_removed_before_the_new_one_is_created() {
    let before = format!(
        "BootOrder: 0003,0001\n{}{}",
        entry("0003", "Tetsu", DEAD_DISK),
        entry("0001", "Fedora", OTHER_DISK)
    );
    // The firmware hands the freed number out again.
    let after = format!(
        "BootOrder: 0003,0001\n{}{}",
        entry("0003", "Tetsu", ESP_PARTUUID),
        entry("0001", "Fedora", OTHER_DISK)
    );
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        firmware(before, after, &[ESP_PARTUUID, OTHER_DISK]),
    );
    run.out.result.as_ref().unwrap();
    let efi = efibootmgr_lines(&run);
    assert_eq!(efi.len(), 4, "{efi:?}");
    assert_eq!(efi[1], "efibootmgr -B -b 0003");
    assert!(efi[2].starts_with("efibootmgr --create"));
    assert_eq!(efi[3], "efibootmgr --bootorder 0003,0001");
    assert!(warnings(&run).iter().all(|w| !w.contains("stale")));
}

#[test]
fn an_entry_for_the_new_esp_is_reused() {
    let before = format!(
        "BootOrder: 0001,0002\n{}{}",
        entry("0001", "Fedora", OTHER_DISK),
        entry("0002", "Tetsu", &ESP_PARTUUID.to_uppercase())
    );
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        firmware(before, String::new(), &[ESP_PARTUUID, OTHER_DISK]),
    );
    run.out.result.as_ref().unwrap();
    let efi = efibootmgr_lines(&run);
    assert_eq!(efi.len(), 2, "{efi:?}");
    assert_eq!(efi[1], "efibootmgr --bootorder 0002,0001");
}

#[test]
fn a_tetsu_entry_on_another_present_disk_is_left_alone() {
    let before = format!(
        "BootOrder: 0002,0001\n{}{}",
        entry("0002", "Tetsu", OTHER_DISK),
        entry("0001", "Fedora", OTHER_DISK)
    );
    let after = format!(
        "BootOrder: 0003,0002,0001\n{}{}{}",
        entry("0002", "Tetsu", OTHER_DISK),
        entry("0001", "Fedora", OTHER_DISK),
        entry("0003", "Tetsu", ESP_PARTUUID)
    );
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        firmware(before, after, &[ESP_PARTUUID, OTHER_DISK]),
    );
    run.out.result.as_ref().unwrap();
    let efi = efibootmgr_lines(&run);
    assert!(
        efi.iter().all(|l| !l.starts_with("efibootmgr -B")),
        "{efi:?}"
    );
    assert_eq!(efi.last().unwrap(), "efibootmgr --bootorder 0003,0002,0001");
}

#[test]
fn a_failed_removal_warns_and_the_entry_is_created_anyway() {
    let before = format!(
        "BootOrder: 0003,0001\n{}{}",
        entry("0003", "Tetsu", DEAD_DISK),
        entry("0001", "Fedora", OTHER_DISK)
    );
    let after = format!(
        "BootOrder: 0004,0003,0001\n{}{}{}",
        entry("0003", "Tetsu", DEAD_DISK),
        entry("0001", "Fedora", OTHER_DISK),
        entry("0004", "Tetsu", ESP_PARTUUID)
    );
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        |m| {
            firmware(before, after, &[ESP_PARTUUID, OTHER_DISK])(m);
            m.fail_on("efibootmgr -B");
        },
    );
    run.out.result.as_ref().unwrap();
    assert!(
        warnings(&run)
            .iter()
            .any(|w| w.contains("stale") && w.contains("0003")),
        "{:?}",
        warnings(&run)
    );
    assert_eq!(
        efibootmgr_lines(&run).last().unwrap(),
        "efibootmgr --bootorder 0004,0003,0001"
    );
}

#[test]
fn nothing_is_removed_when_the_partition_list_cannot_vouch_for_the_new_esp() {
    let before = format!("BootOrder: 0003\n{}", entry("0003", "Tetsu", DEAD_DISK));
    let after = format!(
        "BootOrder: 0004,0003\n{}{}",
        entry("0003", "Tetsu", DEAD_DISK),
        entry("0004", "Tetsu", ESP_PARTUUID)
    );
    // No partition listed at all: every entry would look dead.
    let run = full_run(
        UnlockLevel::Comfort,
        SecureBoot::Off,
        |_, _| {},
        firmware(before, after, &[]),
    );
    run.out.result.as_ref().unwrap();
    assert!(
        efibootmgr_lines(&run)
            .iter()
            .all(|l| !l.starts_with("efibootmgr -B"))
    );
}
