//! The initrd files in `initrd/` carry what the code assumes.

const UNIT: &str = include_str!("../../initrd/tetsu-etc-sync.service");
const HOOK: &str = include_str!("../../initrd/tetsu.install");

#[test]
fn the_unit_runs_between_the_root_mount_and_the_switch() {
    for line in [
        "DefaultDependencies=no",
        "ConditionPathExists=/etc/initrd-release",
        "Requires=sysroot.mount",
        "After=sysroot.mount",
        "Before=initrd.target initrd-switch-root.target tetsu-unlock-seal.service",
        "Type=oneshot",
        "StandardOutput=journal+console",
        "ExecStart=/usr/bin/tetsu-etc-sync",
        "WantedBy=initrd.target",
    ] {
        assert!(UNIT.lines().any(|l| l == line), "{line}");
    }
}

#[test]
fn the_hook_adds_everything_the_sync_runs() {
    for needle in [
        "add_binary /usr/bin/tetsu-etc-sync",
        "add_binary mount",
        "add_binary umount",
        "add_module 'btrfs?'",
        "add_systemd_unit tetsu-etc-sync.service",
        "initrd.target.wants/tetsu-etc-sync.service",
    ] {
        assert!(HOOK.contains(needle), "{needle}");
    }
}

#[test]
fn the_hook_is_valid_shell() {
    let out = std::process::Command::new("bash")
        .args(["-n", "-c", HOOK])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
