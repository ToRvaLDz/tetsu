//! The initrd files in `initrd/` carry what the code assumes.

const UNIT: &str = include_str!("../../initrd/tetsu-unlock-seal.service");
const HOOK: &str = include_str!("../../initrd/tetsu-seal.install");

#[test]
fn the_unit_runs_between_the_root_mount_and_the_switch() {
    for line in [
        "DefaultDependencies=no",
        "After=sysroot.mount",
        "Before=initrd-switch-root.target",
        "ConditionPathExists=/etc/initrd-release",
        "WantedBy=initrd.target",
        "ExecStart=/usr/bin/tetsu-unlock-seal",
    ] {
        assert!(UNIT.lines().any(|l| l == line), "{line}");
    }
}

#[test]
fn the_hook_adds_everything_the_seal_runs() {
    for needle in [
        "add_binary /usr/bin/tetsu-unlock-seal",
        "add_binary /usr/bin/systemd-cryptenroll",
        "add_binary /usr/bin/systemd-ask-password",
        "add_binary mount",
        "add_binary umount",
        "add_systemd_unit tetsu-unlock-seal.service",
        "initrd.target.wants/tetsu-unlock-seal.service",
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
