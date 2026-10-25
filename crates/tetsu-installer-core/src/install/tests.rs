//! Executor tests on the fake machine.

use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::agent::{AgentBackend, ApiProvider};
use crate::error::InstallError;
use crate::preflight::SecureBoot;
use crate::secret::Secret;
use crate::unlock::UnlockLevel;

use super::fake::{LUKS_UUID, Outcome, answers, run_plan, run_with, target};
use super::plan::{InstallPlan, Stage};

const NEVER: &dyn Fn() -> bool = &|| false;

fn disk_plan(plan: &InstallPlan) -> InstallPlan {
    InstallPlan::from_actions(plan.stage(Stage::Disk).collect())
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

struct Run {
    out: Outcome,
    root: tempfile::TempDir,
}

fn stage_one(level: UnlockLevel, tweak: impl FnOnce(&mut crate::answers::Answers)) -> Run {
    let root = tempfile::tempdir().unwrap();
    let mut a = answers(level);
    tweak(&mut a);
    let t = target(root.path(), SecureBoot::SetupMode);
    let plan = disk_plan(&InstallPlan::build(&a, &t));
    let out = run_plan(&a, &t, &plan, NEVER);
    Run { out, root }
}

impl Run {
    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.root.path().join("work").join(rel)
    }

    fn run_dir(&self) -> std::path::PathBuf {
        self.root.path().join("run")
    }
}

#[test]
fn stage_one_runs_the_commands_in_order() {
    let run = stage_one(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let w = run.root.path().join("work");
    let r = run.run_dir();
    let (w, r) = (w.display(), r.display());
    let lines = run.out.machine.lines();
    // The btrfs and container lines are checked on their own below.
    let plain: Vec<&String> = lines
        .iter()
        .filter(|l| !l.starts_with("btrfs") && !l.starts_with("systemd-nspawn"))
        .collect();
    let want = [
        "wipefs -a /dev/sda".to_owned(),
        format!("sh -c exec sfdisk --wipe always \"$0\" < \"$1\" /dev/sda {r}/sfdisk.script"),
        "udevadm settle".to_owned(),
        "mkfs.vfat -F 32 -n TETSU-ESP /dev/sda1".to_owned(),
        format!("mount /dev/sda1 {w}/esp"),
        format!(
            "cryptsetup luksFormat --type luks2 --batch-mode --key-file {r}/luks-passphrase /dev/sda2"
        ),
        "cryptsetup luksUUID /dev/sda2".to_owned(),
        format!(
            "cryptsetup luksAddKey --batch-mode --key-file {r}/luks-passphrase /dev/sda2 {r}/luks-recovery"
        ),
        format!("cryptsetup open --key-file {r}/luks-passphrase /dev/sda2 root"),
        "mkfs.btrfs -L tetsu /dev/mapper/root".to_owned(),
        format!("mount -o subvolid=5 /dev/mapper/root {w}/top"),
        format!(
            "tar --numeric-owner --xattrs --acls -xpf /medium/payload.tar -C {w}/top/@snapshots/1/root"
        ),
        format!("openssl passwd -6 -in {r}/session-password"),
        format!("chown -R 1000:1000 {w}/top/@home/marco"),
    ];
    assert_eq!(plain, want.iter().collect::<Vec<_>>());

    let btrfs: Vec<&String> = lines.iter().filter(|l| l.starts_with("btrfs")).collect();
    assert_eq!(btrfs.len(), 7, "{btrfs:#?}");
    assert!(btrfs[0].ends_with("@snapshots"));
    let nspawn = lines
        .iter()
        .find(|l| l.starts_with("systemd-nspawn"))
        .unwrap();
    assert!(nspawn.contains(&format!("-D {w}/top/@snapshots/1/root")));
    assert!(nspawn.ends_with("mkinitcpio -k 6.12.1-arch1-1 -g /usr/lib/tetsu/initrd.img"));
}

#[test]
fn secrets_travel_in_private_files_that_are_gone_afterwards() {
    let run = stage_one(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let seen = run.out.machine.seen.borrow();
    let find = |name: &str| seen.iter().filter(|s| s.name == name).collect::<Vec<_>>();
    assert!(
        find("luks-passphrase")
            .iter()
            .all(|s| s.content == "correct horse battery staple" && s.mode == 0o600)
    );
    assert_eq!(find("luks-passphrase").len(), 3);
    let rec = find("luks-recovery");
    assert_eq!(
        (rec[0].content.as_str(), rec[0].mode),
        ("cccccccc-dddddddd-eeeeeeee-ffffffff", 0o600)
    );
    let pw = find("session-password");
    assert_eq!(
        (pw[0].content.as_str(), pw[0].mode),
        ("hunter2-hunter2\n", 0o600)
    );
    assert_eq!(find("sfdisk.script")[0].mode, 0o600);
    // Only directories are left in the run directory.
    let left: Vec<_> = fs::read_dir(run.run_dir())
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_file())
        .collect();
    assert!(left.is_empty(), "{left:?}");
    assert_eq!(mode(&run.run_dir()), 0o700);
}

#[test]
fn the_user_lands_in_passwd_shadow_group_and_home() {
    let run = stage_one(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let etc = run.path("top/@snapshots/1/root/etc");
    assert_eq!(
        read(&etc.join("passwd")),
        "root:x:0:0::/root:/bin/bash\nmarco:x:1000:1000:Marco:/home/marco:/bin/bash\n"
    );
    let shadow = read(&etc.join("shadow"));
    let lines: Vec<&str> = shadow.lines().collect();
    assert_eq!(lines[0], "root:!:19000:0:99999:7:::");
    assert!(
        lines[1].starts_with("marco:$6$salt$hashedhashedhashed:"),
        "{shadow}"
    );
    assert!(lines[1].ends_with(":0:99999:7:::"));
    assert_eq!(mode(&etc.join("shadow")), 0o600);
    assert_eq!(
        read(&etc.join("group")),
        "root:x:0:\nwheel:x:998:marco\nmarco:x:1000:\n"
    );
    assert_eq!(mode(&run.path("top/@home/marco")), 0o700);
}

#[test]
fn a_given_hash_is_written_without_running_openssl() {
    let run = stage_one(UnlockLevel::Paranoid, |a| {
        a.user.password = crate::user::Password::from_hash("$6$given$hash").unwrap();
    });
    run.out.result.as_ref().unwrap();
    assert!(
        !run.out
            .machine
            .lines()
            .iter()
            .any(|l| l.starts_with("openssl"))
    );
    let shadow = read(&run.path("top/@snapshots/1/root/etc/shadow"));
    assert!(shadow.contains("marco:$6$given$hash:"));
}

#[test]
fn locale_hostname_cmdline_fstab_and_levels_are_written() {
    let run = stage_one(UnlockLevel::Balanced, |_| {});
    run.out.result.as_ref().unwrap();
    let root = run.path("top/@snapshots/1/root");
    let etc = root.join("etc");
    assert_eq!(read(&etc.join("vconsole.conf")), "KEYMAP=it\n");
    assert_eq!(read(&etc.join("locale.conf")), "LANG=it_IT.UTF-8\n");
    assert_eq!(read(&etc.join("adjtime")), "0.0 0 0\n0\nUTC\n");
    assert_eq!(read(&etc.join("hostname")), "tetsu-ab12\n");
    assert_eq!(
        fs::read_link(etc.join("localtime")).unwrap(),
        Path::new("/usr/share/zoneinfo/Europe/Rome")
    );
    assert_eq!(
        read(&etc.join("kernel/cmdline")),
        format!("rd.luks.name={LUKS_UUID}=root root=/dev/mapper/root rw\n")
    );
    assert!(read(&etc.join("fstab")).contains("subvol=@var"));
    assert!(root.join("efi").is_dir());
    assert_eq!(
        read(&etc.join("tetsu/unlock-level.toml")),
        "level = \"balanced\"\nstate = \"pending\"\n"
    );
    assert!(!etc.join("tetsu/agent-backend.toml").exists());
    assert!(root.join("usr/lib/tetsu/initrd.img").exists());
}

#[test]
fn paranoid_is_written_as_sealed() {
    let run = stage_one(UnlockLevel::Paranoid, |_| {});
    run.out.result.as_ref().unwrap();
    let toml = read(&run.path("top/@snapshots/1/root/etc/tetsu/unlock-level.toml"));
    assert_eq!(toml, "level = \"paranoid\"\nstate = \"sealed\"\n");
}

#[test]
fn the_api_key_is_staged_in_the_home_with_mode_0600() {
    let run = stage_one(UnlockLevel::Paranoid, |a| {
        a.agent = Some(AgentBackend::Api {
            provider: ApiProvider::Anthropic,
            key: Secret::new("sk-ant-0123456789"),
        });
    });
    run.out.result.as_ref().unwrap();
    let key = run.path("top/@home/marco/.local/share/tetsu/staged-api-key");
    assert_eq!(
        read(&key),
        "provider = \"anthropic\"\nkey = \"sk-ant-0123456789\"\n"
    );
    assert_eq!(mode(&key), 0o600);
    assert_eq!(mode(key.parent().unwrap()), 0o700);
    let backend = read(&run.path("top/@snapshots/1/root/etc/tetsu/agent-backend.toml"));
    assert_eq!(backend, "backend = \"api\"\nprovider = \"anthropic\"\n");
    let chowns = run
        .out
        .machine
        .lines()
        .into_iter()
        .filter(|l| l.starts_with("chown"))
        .count();
    assert_eq!(chowns, 2);
}

#[test]
fn every_action_reports_progress_and_the_log_reaches_the_esp() {
    let run = stage_one(UnlockLevel::Comfort, |_| {});
    run.out.result.as_ref().unwrap();
    let plan_len = run.out.script.progress.len();
    assert_eq!(run.out.log.len(), plan_len, "progress and log agree");
    assert!(
        run.out
            .script
            .progress
            .iter()
            .all(|(step, _)| step == "disk" || step == "firmware")
    );
    let on_esp = read(&run.path("esp/tetsu-install.log"));
    // Lines from before the ESP was mounted were flushed too.
    assert!(on_esp.starts_with("[disk] wipe the old signatures from the disk\n"));
    assert_eq!(on_esp.lines().count(), run.out.log.len());
}

#[test]
fn a_failure_stops_the_install_and_is_logged() {
    let root = tempfile::tempdir().unwrap();
    let a = answers(UnlockLevel::Comfort);
    let t = target(root.path(), SecureBoot::Off);
    let plan = disk_plan(&InstallPlan::build(&a, &t));
    let out = run_with(&a, &t, &plan, NEVER, |m| m.fail_on("mkfs.btrfs"));
    let (machine, result, log) = (out.machine, out.result, out.log);
    match result.unwrap_err() {
        InstallError::ActionFailed { action, .. } => assert_eq!(action, "format the root as Btrfs"),
        other => panic!("{other:?}"),
    }
    let lines = machine.lines();
    assert_eq!(
        lines.last().unwrap(),
        "mkfs.btrfs -L tetsu /dev/mapper/root"
    );
    assert!(log.last().unwrap().starts_with("[disk] failed: "));
    // The passphrase file of the earlier steps is gone.
    assert!(
        fs::read_dir(&t.run_dir)
            .unwrap()
            .all(|e| !e.unwrap().file_type().unwrap().is_file())
    );
}

#[test]
fn cancellation_is_checked_between_actions() {
    let root = tempfile::tempdir().unwrap();
    let a = answers(UnlockLevel::Comfort);
    let t = target(root.path(), SecureBoot::Off);
    let plan = disk_plan(&InstallPlan::build(&a, &t));
    let polls = Cell::new(0);
    let cancel = || {
        polls.set(polls.get() + 1);
        polls.get() > 3
    };
    let out = run_plan(&a, &t, &plan, &cancel);
    assert!(matches!(out.result, Err(InstallError::Cancelled)));
    // wipefs, sfdisk and settle ran; nothing after the third poll.
    assert_eq!(out.machine.lines().len(), 3);
    assert!(out.log.last().unwrap().contains("cancelled"));
}
